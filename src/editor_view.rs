//! Drawing the text editor: the text area, gutter, selection and blame column.

use eframe::egui;

use crate::app::{BlameColumn, CONTEXT_CHAR, EditorRequest};
use crate::auto_edit;
use crate::blame;
use crate::bookmarks::Bookmarks;
use crate::brackets;
use crate::find;
use crate::gutter;
use crate::highlighter::{HighlighterLayouter, Language};
use crate::layout_cache::{self, LayoutCache};
use crate::line_diff::LineMarks;
use crate::markdown_preview::ScrollPos;

/// Documents above this size skip syntax highlighting and render plain.
///
/// Highlighting of anything past a few KB runs on a background thread (see
/// `HighlighterLayouter::frame_job`), so this only bounds how long that pass can
/// take, not how long the UI can freeze.
const HIGHLIGHT_SIZE_LIMIT: usize = 1024 * 1024;

/// True when a document is too large to highlight cheaply.
pub(crate) fn text_content_exceeds_highlight_limit(content: &str) -> bool {
    content.len() > HIGHLIGHT_SIZE_LIMIT
}

/// Renders the multiline editor, optionally through a syntax highlighter.
///
/// When `layouter` is `None` the text renders plain, which is what oversized
/// documents get. Lines are joined back together with `\n` separators so the
/// buffer round-trips exactly, keeping the caret and selection aligned.
///
/// With `word_wrap` on, long lines wrap to the window width and only vertical
/// scrolling is needed. With it off, lines never wrap and the editor scrolls
/// sideways as well. Wrap off needs a layouter on the plain path too: a stock
/// multiline `TextEdit` always shrinks to the available width, so it wraps
/// whatever `desired_width` says.
///
/// The editor is wrapped in a `ScrollArea`: `TextEdit` has no scrolling
/// of its own for multiline input (`clip_text` is `false`, so its `text_offset`
/// handling never runs) and its only `ui.scroll_to_rect` call writes to
/// `PassState::scroll_target`, which only `ScrollArea::show` drains. Without this
/// wrapper a document taller than the viewport cannot be scrolled and, with the
/// default `cursor_at_end`, the caret lands off-screen and stays unreachable.
///
/// With `line_numbers` on, the editor's left margin widens to hold the
/// gutter, which is painted over it once the text is laid out. `hint` shows
/// in grey while the document is empty.
#[cfg(test)]
pub(crate) fn render_editor(
    ui: &mut egui::Ui,
    text: &mut String,
    layouter: Option<&mut HighlighterLayouter<'_>>,
    word_wrap: bool,
    line_numbers: bool,
    hint: &str,
) {
    render_editor_with(
        ui,
        text,
        layouter,
        word_wrap,
        line_numbers,
        hint,
        None,
        EditorRequest::default(),
        None,
        &mut Bookmarks::default(),
        &LineMarks::default(),
        None,
        &mut LayoutCache::default(),
    );
}

/// What the user did to the folds in a frame, for the app to apply to the
/// document's folds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FoldOutput {
    /// Fold or unfold the block with this header (0-based line): its
    /// chevron or its `⋯` marker was clicked.
    pub toggle: Option<usize>,
}

/// Paints a small rounded `⋯` box after the end of each folded header line
/// in view; returns the header (0-based line) whose box was clicked.
fn paint_fold_markers(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    hidden: &[(usize, usize)],
) -> Option<usize> {
    if hidden.is_empty() {
        return None;
    }
    let clip = ui.clip_rect();
    let color = ui
        .visuals()
        .override_text_color
        .unwrap_or_else(|| ui.visuals().widgets.inactive.text_color());
    let mut clicked = None;
    let mut chars = 0;
    let mut line = 0;
    for row in &output.galley.rows {
        let start = chars;
        chars += row.char_count_including_newline().0;
        let top = output.galley_pos.y + row.pos.y;
        if top > clip.bottom() {
            break;
        }
        if !row.ends_with_newline {
            continue;
        }
        let newline = chars - 1;
        if top + row.height() >= clip.top()
            && hidden.binary_search_by_key(&newline, |&(a, _)| a).is_ok()
        {
            let end = output.galley_pos + row.pos.to_vec2()
                + egui::vec2(row.x_offset(egui::text::CharIndex(newline - start)), row.height() * 0.5);
            let size = egui::vec2(row.height() * 1.4, row.height() * 0.7);
            let rect = egui::Rect::from_min_size(
                egui::pos2(end.x + 4.0, end.y - size.y * 0.5),
                size,
            );
            let id = output.response.response.id.with(("fold_marker", line));
            let response = ui
                .interact(rect, id, egui::Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() {
                clicked = Some(line);
            }
            let fill = color.gamma_multiply(if response.hovered() { 0.3 } else { 0.15 });
            let painter = ui.painter();
            painter.rect_filled(rect, egui::CornerRadius::same(3), fill);
            for dot in [-1.0, 0.0, 1.0] {
                let at = rect.center() + egui::vec2(dot * size.x * 0.25, 0.0);
                painter.circle_filled(at, 1.2, color.gamma_multiply(0.8));
            }
        }
        line += 1;
    }
    clicked
}

/// Rectangles covering the `char` range `(start, end)` of `galley`, one per
/// row, relative to the galley. Drawn the way egui draws a focused editor's
/// selection, including the half-height nub for a selected newline.
fn selection_rects(galley: &egui::Galley, (start, end): find::Range) -> Vec<egui::Rect> {
    if start >= end {
        return Vec::new();
    }
    let min = galley.layout_from_cursor(egui::text::CCursor::new(start));
    let max = galley.layout_from_cursor(egui::text::CCursor::new(end));
    (min.row..=max.row)
        .map(|index| {
            let row = &galley.rows[index];
            let left = if index == min.row {
                row.x_offset(min.column)
            } else {
                0.0
            };
            let right = if index == max.row {
                row.x_offset(max.column)
            } else if row.ends_with_newline {
                row.size.x + row.height() * 0.5
            } else {
                row.size.x
            };
            egui::Rect::from_min_max(egui::pos2(left, 0.0), egui::pos2(right, row.size.y))
                .translate(row.pos.to_vec2())
        })
        .collect()
}

/// [`render_editor`] with a fixed widget `id` (so the app can find the
/// editor's state) and a one-shot `request` to act on. `bookmarks` follow
/// the edits and are marked in the gutter. The text is laid out through
/// `layout_cache`, which keeps the layout while nothing changes. Also
/// returns the editor's response, for its right-click menu, and what the
/// user did to the folds.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_editor_with(
    ui: &mut egui::Ui,
    text: &mut String,
    layouter: Option<&mut HighlighterLayouter<'_>>,
    word_wrap: bool,
    line_numbers: bool,
    hint: &str,
    id: Option<egui::Id>,
    request: EditorRequest,
    scroll_to: Option<f32>,
    bookmarks: &mut Bookmarks,
    changes: &LineMarks,
    blame: Option<BlameColumn<'_>>,
    layout_cache: &mut LayoutCache,
) -> (ScrollPos, egui::Response, FoldOutput) {
    // The blame column is a strip left of the scroll area, painted after
    // it, once the text's rows are known.
    let Some(column) = blame else {
        return render_editor_inner(
            ui,
            text,
            layouter,
            word_wrap,
            line_numbers,
            hint,
            id,
            request,
            scroll_to,
            bookmarks,
            changes,
            layout_cache,
            None,
        );
    };
    let small = egui::FontId::monospace(11.0);
    let width = column
        .blame
        .map_or(BLAME_NOTE_WIDTH, |b| b.width(ui, &small));
    // The strip lies outside the scroll area, so it is painted through this
    // clip rect, not the scroll area's, which stops at the strip's edge.
    let clip = ui.clip_rect();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let (strip, _) = ui.allocate_exact_size(
            egui::vec2(width, ui.available_height()),
            egui::Sense::hover(),
        );
        render_editor_inner(
            ui,
            text,
            layouter,
            word_wrap,
            line_numbers,
            hint,
            id,
            request,
            scroll_to,
            bookmarks,
            changes,
            layout_cache,
            Some((strip, clip, &column, small)),
        )
    })
    .inner
}

/// Width of the column while it only carries a note.
const BLAME_NOTE_WIDTH: f32 = 140.0;

/// Auto-indent and auto-close: runs this frame's typing through
/// [`auto_edit::apply_events`] before the `TextEdit` sees it. Only while
/// the editor `id` has focus and a caret; the events it handles are taken
/// out of the frame, so egui does not type them a second time.
fn apply_auto_edit(ui: &egui::Ui, id: egui::Id, text: &mut String, language: Language) {
    if !ui.memory(|m| m.has_focus(id)) {
        return;
    }
    let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) else {
        return;
    };
    let Some(range) = state.cursor.char_range() else {
        return;
    };
    let (a, b) = (range.primary.index.0, range.secondary.index.0);
    let n = text.chars().count();
    let sel = (a.min(b).min(n), a.max(b).min(n));
    let Some((edit, handled)) =
        ui.input(|i| auto_edit::apply_events(text, sel, &i.events, language))
    else {
        return;
    };
    ui.ctx().input_mut(|i| {
        let mut index = 0;
        i.events.retain(|_| {
            index += 1;
            handled.binary_search(&(index - 1)).is_err()
        });
    });
    *text = edit.text;
    let (a, b) = edit.selection;
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(a),
            egui::text::CCursor::new(b),
        )));
    state.store(ui.ctx(), id);
}

#[allow(clippy::too_many_arguments)]
fn render_editor_inner(
    ui: &mut egui::Ui,
    text: &mut String,
    mut layouter: Option<&mut HighlighterLayouter<'_>>,
    word_wrap: bool,
    line_numbers: bool,
    hint: &str,
    id: Option<egui::Id>,
    request: EditorRequest,
    scroll_to: Option<f32>,
    bookmarks: &mut Bookmarks,
    changes: &LineMarks,
    layout_cache: &mut LayoutCache,
    blame: Option<(egui::Rect, egui::Rect, &BlameColumn<'_>, egui::FontId)>,
) -> (ScrollPos, egui::Response, FoldOutput) {
    let scroll_area = if word_wrap {
        egui::ScrollArea::vertical()
    } else {
        egui::ScrollArea::both()
    };
    let scroll_area = match scroll_to {
        Some(offset) => scroll_area.vertical_scroll_offset(offset),
        None => scroll_area,
    };
    let output = scroll_area.show(ui, |ui| {
        let font_id = egui::TextStyle::Monospace.resolve(ui.style());
        let gutter_width =
            line_numbers.then(|| gutter::width(ui, &font_id, gutter::line_count(text)));
        let margin = egui::Margin {
            // `gutter::width` is whole points and fits an `i8`.
            left: gutter_width.map_or(8, |w| w as i8),
            ..egui::Margin::symmetric(8, 6)
        };

        // Fill the viewport through `min_size`, not `ui.add_sized`: `add_sized`
        // centres a widget larger than the space it is given, so a line wider
        // than the window pushed the start of every line off the left edge,
        // beyond the reach of the scroll bar. A plain `show` grows the
        // editor right and down from the top-left corner instead.
        // Clamped here: the text is lent to the editor below, and may have
        // shrunk (Prettify) since the selection was taken.
        let select = request.select.map(|(a, b)| {
            let n = text.chars().count();
            (a.min(n), b.min(n))
        });
        if let Some(id) = id {
            if request.focus {
                ui.memory_mut(|m| m.request_focus(id));
            }
            // Before `show`, so a paste or undo of this frame acts on it.
            if let Some((a, b)) = select {
                let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(a),
                        egui::text::CCursor::new(b),
                    )));
                state.store(ui.ctx(), id);
            }
            if let Some(key) = request.key {
                ui.ctx().input_mut(|i| {
                    i.events.push(egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::COMMAND,
                    });
                });
            }
            // After `select`, so it types at the selection the app asked for.
            if let Some(language) = request.auto_edit {
                apply_auto_edit(ui, id, text, language);
            }
        }

        let text_edit = egui::TextEdit::multiline(text)
            .font(egui::TextStyle::Monospace)
            .lock_focus(true)
            .desired_width(f32::INFINITY)
            .min_size(ui.available_size())
            .hint_text(hint.to_owned())
            // No outline or rounded fill: the editor sits on the central
            // panel's background, the same colour as the active tab.
            .frame(egui::Frame::NONE.inner_margin(margin));

        let text_edit = match id {
            Some(id) => text_edit.id(id),
            None => text_edit,
        };

        // egui moves the caret to any press, a right one included; a
        // right-click inside the selection must keep it for the menu.
        let right_press = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary));
        let before_press = id
            .filter(|_| right_press)
            .and_then(|id| egui::TextEdit::load_state(ui.ctx(), id))
            .and_then(|state| state.cursor.char_range())
            .filter(|range| !range.is_empty());

        // Reserved before `show` so the selection is painted under the text.
        let inactive_shape = request
            .inactive_selection
            .map(|_| ui.painter().add(egui::Shape::Noop));

        let output = match layouter.as_mut() {
            Some(highlighter) => {
                // `TextEdit` calls this every frame. `frame_job` reuses the
                // cached result, and highlights large documents on a background
                // thread (plain text meanwhile) so the window never freezes.
                // Wrapping is a layout concern, so it is applied by the layout
                // cache and never reaches the highlighter's cache.
                let mut lay = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                    let job = highlighter.frame_job(buf.as_str(), ui.ctx());
                    let wrap_width = if word_wrap { wrap_width } else { f32::INFINITY };
                    layout_cache.galley(
                        ui,
                        buf.as_str(),
                        layout_cache::Style::Highlighted(job),
                        wrap_width,
                        &request.hidden,
                    )
                };
                text_edit.layouter(&mut lay).show(ui)
            }
            None => {
                // Mirror egui's stock `TextEdit` layouter, so plain text looks
                // the same with or without wrapping: the colour is the
                // `inactive` widget colour (a text edit does not brighten on
                // hover), unless the theme overrides it, and every section gets
                // the same line height. Wrap off needs a layouter of its own: a
                // stock multiline `TextEdit` always wraps.
                let color = ui
                    .visuals()
                    .override_text_color
                    .unwrap_or_else(|| ui.visuals().widgets.inactive.text_color());
                let line_height =
                    ui.fonts_mut(|f| f.row_height(&font_id)) + ui.spacing().extra_text_line_spacing;
                let style = layout_cache::Style::Plain {
                    font_id: font_id.clone(),
                    color,
                    line_height,
                };
                let mut lay = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                    let wrap_width = if word_wrap { wrap_width } else { f32::INFINITY };
                    layout_cache.galley(ui, buf.as_str(), style.clone(), wrap_width, &request.hidden)
                };
                text_edit.layouter(&mut lay).show(ui)
            }
        };

        bookmarks.sync(text);
        let mut folds = FoldOutput::default();
        if let Some(width) = gutter_width {
            let folding = gutter::Folding {
                hidden: &request.hidden,
                foldable: &request.foldable,
            };
            folds.toggle = gutter::paint(ui, &output, &font_id, width, bookmarks, changes, &folding);
        }
        folds.toggle = folds.toggle.or(paint_fold_markers(ui, &output, &request.hidden));
        if let Some((strip, clip, column, small)) = &blame {
            match column.blame {
                Some(blame) => blame::paint_column(
                    ui,
                    *strip,
                    *clip,
                    blame,
                    &output.galley,
                    output.galley_pos,
                    small,
                    column.now,
                ),
                None => blame::paint_note(ui, *strip, *clip, column.note, small),
            }
        }

        // Where the right-click menu was opened, for its bookmark item: the
        // click that opens it, a long touch included, records the spot.
        if let (true, Some(id), Some(pointer)) = (
            output.response.secondary_clicked(),
            id,
            ui.input(|i| i.pointer.interact_pos()),
        ) {
            let at = output
                .galley
                .cursor_from_pos(pointer - output.galley_pos)
                .index
                .0;
            ui.ctx()
                .data_mut(|d| d.insert_temp(id.with(CONTEXT_CHAR), at));
        }

        let mut cursor_range = output.cursor_range;
        if let (Some(before), Some(id), Some(pointer)) =
            (before_press, id, ui.input(|i| i.pointer.interact_pos()))
        {
            let at = output
                .galley
                .cursor_from_pos(pointer - output.galley_pos)
                .index
                .0;
            let (a, b) = (before.primary.index.0, before.secondary.index.0);
            if output.response.contains_pointer() && (a.min(b)..=a.max(b)).contains(&at) {
                let mut state = output.state.clone();
                state.cursor.set_char_range(Some(before));
                state.store(ui.ctx(), id);
                cursor_range = Some(before);
                ui.ctx().request_repaint();
            }
        }

        // Remembered while focused: a click on a menu takes the focus, and
        // from the next frame egui keeps only the caret.
        if let (Some(id), true) = (id, output.response.has_focus()) {
            let saved: Option<find::Range> = cursor_range
                .map(|r| {
                    let (a, b) = (r.primary.index.0, r.secondary.index.0);
                    (a.min(b), a.max(b))
                })
                .filter(|(a, b)| a < b);
            ui.ctx().data_mut(|d| {
                let key = id.with("saved_selection");
                match saved {
                    Some(range) => {
                        d.insert_temp(key, range);
                    }
                    None => d.remove::<find::Range>(key),
                }
            });
        }

        // egui paints a selection only while it is in the editor's state.
        let egui_selection = output.cursor_range.is_some_and(|r| !r.is_empty());
        if let (Some(shape), Some(range), false) =
            (inactive_shape, request.inactive_selection, egui_selection)
        {
            let fill = ui.visuals().selection.bg_fill;
            let offset = output.galley_pos.to_vec2();
            let rects = selection_rects(&output.galley, range)
                .into_iter()
                .map(|rect| egui::Shape::rect_filled(rect.translate(offset), 0.0, fill));
            ui.painter().set(shape, egui::Shape::Vec(rects.collect()));
        }

        if let (Some(index), Some(id)) = (request.goto, id) {
            let mut state = output.state.clone();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(index),
                )));
            state.store(ui.ctx(), id);
            ui.ctx().request_repaint();
        }

        // Rectangle of a `char` range, when it sits on one row.
        let span = |(start, end): find::Range| {
            let a = output
                .galley
                .pos_from_cursor(egui::text::CCursor::new(start));
            let b = output.galley.pos_from_cursor(egui::text::CCursor::new(end));
            (a.center().y == b.center().y).then(|| {
                egui::Rect::from_min_max(a.min, b.max).translate(output.galley_pos.to_vec2())
            })
        };
        if let Some(rect) = request.highlight.and_then(span) {
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(2),
                egui::Color32::from_rgba_unmultiplied(255, 200, 0, 90),
            );
        }
        // The bracket at this frame's caret and its partner, boxed as in VS
        // Code. Only for a caret in a focused editor, and not in a document
        // too large to highlight, so the scan never slows a frame.
        let caret = cursor_range
            .filter(|r| r.is_empty())
            .map(|r| r.primary.index.0);
        if let (true, Some(caret), true) =
            (request.match_brackets, caret, output.response.has_focus())
            && !text_content_exceeds_highlight_limit(text)
            && let Some((a, b)) = brackets::matching_pair(text, caret)
        {
            let color = ui
                .visuals()
                .override_text_color
                .unwrap_or_else(|| ui.visuals().widgets.inactive.text_color())
                .gamma_multiply(0.4);
            for at in [a, b] {
                if let Some(rect) = span((at, at + 1)) {
                    ui.painter().rect_stroke(
                        rect,
                        egui::CornerRadius::same(2),
                        egui::Stroke::new(1.0, color),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
        if let Some(rect) = request.reveal.and_then(span) {
            ui.scroll_to_rect(rect, None);
        }
        (output.response.response.clone(), folds)
    });
    let pos = ScrollPos::of(&output);
    let (response, folds) = output.inner;
    (pos, response, folds)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::*;
    use crate::highlighter::{Language, SyntaxHighlighter};

    #[test]
    fn test_selection_rects_cover_each_selected_row() {
        let ctx = egui::Context::default();
        let mut galley = None;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let font_id = egui::TextStyle::Monospace.resolve(ui.style());
            galley = Some(ui.fonts_mut(|f| {
                f.layout_no_wrap("ab\ncd".to_owned(), font_id, egui::Color32::WHITE)
            }));
        });
        out.textures_delta.clear();
        let galley = galley.unwrap();

        assert!(super::selection_rects(&galley, (2, 2)).is_empty());
        // "b\nc": the end of the first row, its newline, and the start of the second.
        let rects = super::selection_rects(&galley, (1, 4));
        assert_eq!(rects.len(), 2);
        assert!(rects[0].min.x > 0.0, "starts after 'a'");
        assert!(rects[0].max.x > galley.rows[0].size.x, "covers the newline");
        assert_eq!(rects[1].min.x, 0.0);
        assert!(rects[1].max.x < galley.rows[1].size.x, "stops before 'd'");
        assert!(rects[1].min.y >= rects[0].center().y, "on the second row");
    }

    #[test]
    fn test_highlight_size_limit() {
        assert!(!super::text_content_exceeds_highlight_limit("small"));
        assert!(!super::text_content_exceeds_highlight_limit(
            &"a".repeat(super::HIGHLIGHT_SIZE_LIMIT)
        ));
        assert!(super::text_content_exceeds_highlight_limit(
            &"a".repeat(super::HIGHLIGHT_SIZE_LIMIT + 1)
        ));
    }

    /// A document with more lines than fit on one screen.
    fn long_document(lines: usize) -> String {
        (0..lines)
            .map(|i| format!("let x{i} = {i};"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The single `ScrollArea` the editor created, if any. `render_editor` must
    /// create exactly one, on the plain and on the highlighted path alike:
    /// `TextEdit` does not scroll multiline input by itself, and its
    /// `ui.scroll_to_rect` call is drained only by `ScrollArea::show`.
    fn editor_scroll_area(ctx: &egui::Context) -> Option<egui::containers::scroll_area::State> {
        ctx.data(|data| {
            data.iter_temp_by_type::<egui::containers::scroll_area::State>()
                .next()
                .cloned()
        })
    }

    /// Scrolls the editor with a real wheel event: the pointer hovers over the
    /// editor and the wheel moves the content by `delta` points (negative moves
    /// the content up or left). Runs enough frames for the animated scroll to
    /// settle.
    fn scroll_editor_with_wheel_delta(
        ctx: &egui::Context,
        delta: egui::Vec2,
        mut body: impl FnMut(&mut egui::Ui),
    ) -> egui::containers::scroll_area::State {
        let pointer = egui::Event::PointerMoved(egui::pos2(100.0, 100.0));
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::NONE,
        };
        run_frame_with_events(ctx, vec![pointer, wheel], &mut body);
        for _ in 0..30 {
            run_frame_with_events(ctx, Vec::new(), &mut body);
        }
        editor_scroll_area(ctx).expect("ScrollArea must survive")
    }

    /// Scrolls the editor down with the mouse wheel.
    fn scroll_editor_with_wheel(
        ctx: &egui::Context,
        body: impl FnMut(&mut egui::Ui),
    ) -> egui::containers::scroll_area::State {
        scroll_editor_with_wheel_delta(ctx, egui::vec2(0.0, -200.0), body)
    }

    /// Regression: the editor must live inside a `ScrollArea` on the plain path.
    /// Without it a document taller than the viewport cannot be scrolled, and
    /// because `TextEdit` starts with the cursor at the end, the caret opens
    /// off-screen and stays unreachable.
    #[test]
    fn test_render_editor_plain_path_is_scrollable() {
        let ctx = egui::Context::default();
        let original = long_document(400);
        let mut text = original.clone();

        run_frame_with_events(&ctx, Vec::new(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                super::render_editor(ui, &mut text, None, false, false, "");
            });
        });
        let area = editor_scroll_area(&ctx).expect("editor must render inside a ScrollArea");
        assert_eq!(
            area.clamped_offset().y,
            0.0,
            "a fresh editor starts at the top"
        );

        let area = scroll_editor_with_wheel(&ctx, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                super::render_editor(ui, &mut text, None, false, false, "");
            });
        });
        assert!(
            area.clamped_offset().y > 0.0,
            "the wheel must scroll the editor, offset {:?}",
            area.clamped_offset()
        );
        assert_eq!(text, original, "scrolling must not alter the buffer");
    }

    /// Regression: the same for the highlighted path, which additionally proves
    /// the multi-line layouter keeps the text byte-identical through layout.
    #[test]
    fn test_render_editor_highlighted_path_is_scrollable() {
        let ctx = egui::Context::default();
        let original = long_document(400);
        let mut text = original.clone();

        let area = scroll_editor_with_wheel(&ctx, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let mut highlighter = SyntaxHighlighter::new();
                let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                let mut layouter = highlighter.layouter(Language::Rust, font_id);
                super::render_editor(ui, &mut text, Some(&mut layouter), false, false, "");
            });
        });
        assert!(
            area.clamped_offset().y > 0.0,
            "the highlighted editor must scroll, offset {:?}",
            area.clamped_offset()
        );
        assert_eq!(text, original, "layouter must not alter the buffer");
    }

    /// The editor lays its text out through the cache it is given, on the
    /// highlighted and the plain path alike, so a frame where nothing
    /// changed reuses the last layout.
    #[test]
    fn test_render_editor_lays_out_through_the_cache() {
        let mut highlighter = SyntaxHighlighter::new();
        for highlighted in [true, false] {
            for word_wrap in [false, true] {
                let ctx = egui::Context::default();
                let original = "fn main() {}\nlet x = 1;".to_string();
                let mut text = original.clone();
                let mut cache = LayoutCache::default();
                let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                        let mut layouter = highlighter.layouter(Language::Rust, font_id);
                        super::render_editor_with(
                            ui,
                            &mut text,
                            highlighted.then_some(&mut layouter),
                            word_wrap,
                            false,
                            "",
                            None,
                            EditorRequest::default(),
                            None,
                            &mut Bookmarks::default(),
                            &LineMarks::default(),
                            None,
                            &mut cache,
                        );
                    });
                });
                let painted = shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::Shape::Text(shape) if shape.galley.text() == original => {
                            Some(shape.galley.clone())
                        }
                        _ => None,
                    })
                    .expect("the editor painted the text");
                let cached = cache.last_galley().expect("the editor filled the cache");
                assert!(
                    std::sync::Arc::ptr_eq(&painted, cached),
                    "highlighted {highlighted}, wrap {word_wrap}"
                );
            }
        }
    }

    /// Both branches of `render_editor` keep the buffer byte-identical.
    #[test]
    fn test_render_editor_round_trips_text_on_both_paths() {
        let original = "fn main() {\n    let x = 1;\n}\n// tail\nlast".to_string();
        let ctx = egui::Context::default();

        for word_wrap in [false, true] {
            let mut plain = original.clone();
            run_frame_with_events(&ctx, Vec::new(), |ui| {
                super::render_editor(ui, &mut plain, None, word_wrap, false, "");
            });
            assert_eq!(plain, original);

            let mut highlighted = original.clone();
            run_frame_with_events(&ctx, Vec::new(), |ui| {
                let mut highlighter = SyntaxHighlighter::new();
                let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                let mut layouter = highlighter.layouter(Language::C, font_id);
                super::render_editor(
                    ui,
                    &mut highlighted,
                    Some(&mut layouter),
                    word_wrap,
                    false,
                    "",
                );
            });
            assert_eq!(highlighted, original);
        }
    }

    /// One long line, far wider than the 800 point test viewport.
    fn wide_document() -> String {
        format!("let a = \"{}\";\nshort\nline3", "x".repeat(3000))
    }

    /// Renders `text` through `render_editor` on the plain or highlighted path
    /// and returns the scroll state after a horizontal wheel scroll of 300
    /// points. The buffer is asserted unchanged.
    fn scroll_sideways(highlighted: bool, word_wrap: bool) -> egui::containers::scroll_area::State {
        let ctx = egui::Context::default();
        let original = wide_document();
        let mut text = original.clone();
        let mut highlighter = SyntaxHighlighter::new();

        let area = scroll_editor_with_wheel_delta(&ctx, egui::vec2(-300.0, 0.0), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                if highlighted {
                    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                    let mut layouter = highlighter.layouter(Language::Rust, font_id);
                    super::render_editor(ui, &mut text, Some(&mut layouter), word_wrap, false, "");
                } else {
                    super::render_editor(ui, &mut text, None, word_wrap, false, "");
                }
            });
        });
        assert_eq!(text, original, "scrolling must not alter the buffer");
        area
    }

    /// Renders `text` with wrap off on the plain or highlighted path, scrolled
    /// sideways by `scroll_x` points, and returns where it was painted.
    fn paint_wide_line(highlighted: bool, scroll_x: f32) -> Option<(egui::Rect, egui::Rect)> {
        let ctx = egui::Context::default();
        let original = wide_document();
        let mut text = original.clone();
        let mut highlighter = SyntaxHighlighter::new();
        let mut body = |ui: &mut egui::Ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                if highlighted {
                    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                    let mut layouter = highlighter.layouter(Language::Rust, font_id);
                    super::render_editor(ui, &mut text, Some(&mut layouter), false, false, "");
                } else {
                    super::render_editor(ui, &mut text, None, false, false, "");
                }
            });
        };
        if scroll_x != 0.0 {
            scroll_editor_with_wheel_delta(&ctx, egui::vec2(-scroll_x, 0.0), &mut body);
        }
        let shapes = run_frame_with_events(&ctx, Vec::new(), &mut body);
        painted_text(&shapes, &original)
    }

    /// Regression: with wrap off, one line wider than the window pushed the
    /// start of every line off the left edge, where no scroll offset reaches.
    /// `Ui::add_sized` centres a widget that is larger than the space it is
    /// given, so the editor overflowed to the left by half its excess width.
    #[test]
    fn test_wide_line_keeps_line_starts_visible() {
        for highlighted in [false, true] {
            let (galley, clip) =
                paint_wide_line(highlighted, 0.0).expect("the editor must paint the buffer");
            assert!(
                galley.left() >= clip.left(),
                "line starts painted left of the viewport (highlighted: {highlighted}): \
                 galley {galley:?}, clip {clip:?}"
            );
        }
    }

    /// Regression: scrolled fully right, the end of the long line must fill the
    /// viewport. With the centred overflow the scroll range ran past the text
    /// and the viewport showed empty space.
    #[test]
    fn test_wide_line_end_is_reachable_by_scrolling() {
        for highlighted in [false, true] {
            let painted = paint_wide_line(highlighted, 1_000_000.0);
            assert!(
                painted.is_some_and(|(galley, clip)| galley.right() > clip.center().x),
                "line end not visible when scrolled fully right (highlighted: {highlighted}): \
                 painted (galley, clip) {painted:?}"
            );
        }
    }

    /// Wrap off: a line wider than the window scrolls sideways on the plain path.
    #[test]
    fn test_plain_path_scrolls_horizontally_without_wrap() {
        let area = scroll_sideways(false, false);
        assert!(
            area.clamped_offset().x > 0.0,
            "a long line must scroll sideways, offset {:?}",
            area.clamped_offset()
        );
    }

    /// Wrap off: the same on the highlighted path.
    #[test]
    fn test_highlighted_path_scrolls_horizontally_without_wrap() {
        let area = scroll_sideways(true, false);
        assert!(
            area.clamped_offset().x > 0.0,
            "a long line must scroll sideways, offset {:?}",
            area.clamped_offset()
        );
    }

    /// Wrap on: the text wraps to the window, so there is nothing to scroll
    /// sideways, on the plain path.
    #[test]
    fn test_plain_path_does_not_scroll_horizontally_with_wrap() {
        let area = scroll_sideways(false, true);
        assert_eq!(
            area.clamped_offset().x,
            0.0,
            "wrapped text must fit the window width, offset {:?}",
            area.clamped_offset()
        );
    }

    /// Wrap on: the same on the highlighted path.
    #[test]
    fn test_highlighted_path_does_not_scroll_horizontally_with_wrap() {
        let area = scroll_sideways(true, true);
        assert_eq!(
            area.clamped_offset().x,
            0.0,
            "wrapped text must fit the window width, offset {:?}",
            area.clamped_offset()
        );
    }

    /// Renders `text` through `render_editor` with the gutter on or off.
    fn render_with_gutter(
        ctx: &egui::Context,
        text: &mut String,
        highlighted: bool,
        word_wrap: bool,
        line_numbers: bool,
        events: Vec<egui::Event>,
    ) -> Vec<egui::epaint::ClippedShape> {
        let mut highlighter = SyntaxHighlighter::new();
        run_frame_with_events(ctx, events, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                if highlighted {
                    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                    let mut layouter = highlighter.layouter(Language::Rust, font_id);
                    super::render_editor(
                        ui,
                        text,
                        Some(&mut layouter),
                        word_wrap,
                        line_numbers,
                        "",
                    );
                } else {
                    super::render_editor(ui, text, None, word_wrap, line_numbers, "");
                }
            });
        })
    }

    /// Each line gets its number, right of the window edge, left of the
    /// text and level with the line, on every path.
    #[test]
    fn test_line_numbers_paint_beside_each_line() {
        let doc = "alpha\nbeta\ngamma".to_string();
        for highlighted in [false, true] {
            for word_wrap in [false, true] {
                let ctx = egui::Context::default();
                let mut text = doc.clone();
                let shapes =
                    render_with_gutter(&ctx, &mut text, highlighted, word_wrap, true, Vec::new());
                let (galley, _) = painted_text(&shapes, &doc).expect("text painted");
                let row_h = galley.height() / 3.0;
                for n in 1..=3 {
                    let (rect, _) = painted_number(&shapes, n).unwrap_or_else(|| {
                        panic!(
                            "line {n} not numbered (highlighted {highlighted}, wrap {word_wrap})"
                        )
                    });
                    assert!(rect.left() > 0.0, "number {n} off the left edge: {rect:?}");
                    assert!(rect.right() < galley.left(), "number {n} overlaps the text");
                    let line_center = galley.top() + row_h * (n as f32 - 0.5);
                    assert!(
                        (rect.center().y - line_center).abs() < 1.0,
                        "number {n} at y {} but its line is at {line_center}",
                        rect.center().y
                    );
                }
                assert!(painted_number(&shapes, 4).is_none(), "only 3 lines");
                assert_eq!(text, doc, "the gutter must not alter the buffer");
            }
        }
    }

    #[test]
    fn test_hidden_line_numbers_paint_nothing() {
        let doc = "alpha\nbeta\ngamma".to_string();
        for highlighted in [false, true] {
            let ctx = egui::Context::default();
            let mut text = doc.clone();
            let shapes = render_with_gutter(&ctx, &mut text, highlighted, false, true, Vec::new());
            let (with_gutter, _) = painted_text(&shapes, &doc).expect("text painted");

            let shapes = render_with_gutter(&ctx, &mut text, highlighted, false, false, Vec::new());
            assert!(painted_number(&shapes, 1).is_none());
            let (without, _) = painted_text(&shapes, &doc).expect("text painted");
            assert!(
                without.left() < with_gutter.left(),
                "hiding the gutter must give its room back to the text: \
                 {without:?} vs {with_gutter:?}"
            );
        }
    }

    /// Scrolled sideways, the numbers stay put and the text slides under them.
    #[test]
    fn test_line_numbers_stick_when_scrolled_sideways() {
        let ctx = egui::Context::default();
        let original = wide_document();
        let mut text = original.clone();
        let shapes = render_with_gutter(&ctx, &mut text, false, false, true, Vec::new());
        let (before, _) = painted_number(&shapes, 1).expect("line 1 numbered");

        scroll_editor_with_wheel_delta(&ctx, egui::vec2(-300.0, 0.0), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                super::render_editor(ui, &mut text, None, false, true, "");
            });
        });
        let shapes = render_with_gutter(&ctx, &mut text, false, false, true, Vec::new());
        let (after, _) = painted_number(&shapes, 1).expect("line 1 still numbered");
        let (galley, clip) = painted_text(&shapes, &original).expect("text painted");
        assert!(
            galley.left() < clip.left(),
            "the editor must have scrolled sideways"
        );
        assert_eq!(after, before, "the gutter must not scroll sideways");
    }

    /// The caret's line number is brighter than the rest.
    #[test]
    fn test_caret_line_number_is_highlighted() {
        let ctx = egui::Context::default();
        let doc = "alpha\nbeta\ngamma".to_string();
        let mut text = doc.clone();
        let shapes = render_with_gutter(&ctx, &mut text, false, false, true, Vec::new());
        let (galley, _) = painted_text(&shapes, &doc).expect("text painted");
        let line2 = egui::pos2(galley.left() + 4.0, galley.top() + galley.height() * 0.5);

        render_with_gutter(&ctx, &mut text, false, false, true, click_events(line2));
        let shapes = render_with_gutter(&ctx, &mut text, false, false, true, Vec::new());
        let palette = crate::theme::active(&ctx).gutter;
        assert_eq!(painted_number(&shapes, 2).unwrap().1, palette.number_active);
        assert_eq!(painted_number(&shapes, 1).unwrap().1, palette.number);
        assert_eq!(painted_number(&shapes, 3).unwrap().1, palette.number);
    }

    /// One frame of a focusable editor `ed` over `text`, fed `events`.
    fn auto_edit_frame(
        ctx: &egui::Context,
        text: &mut String,
        events: Vec<egui::Event>,
        request: EditorRequest,
    ) {
        let mut request = Some(request);
        run_frame_with_events(ctx, events, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                super::render_editor_with(
                    ui,
                    text,
                    None,
                    false,
                    false,
                    "",
                    Some(egui::Id::unique("ed")),
                    request.take().unwrap_or_default(),
                    None,
                    &mut Bookmarks::default(),
                    &LineMarks::default(),
                    None,
                    &mut LayoutCache::default(),
                );
            });
        });
    }

    fn editor_caret(ctx: &egui::Context) -> Option<(usize, usize)> {
        let range = egui::TextEdit::load_state(ctx, egui::Id::unique("ed"))?.cursor.char_range()?;
        Some((range.primary.index.0, range.secondary.index.0))
    }

    /// A focused editor with its caret at the start, auto-edit as given.
    fn focused_editor(ctx: &egui::Context, text: &mut String, auto_edit: Option<Language>) {
        let request = EditorRequest { focus: true, select: Some((0, 0)), ..Default::default() };
        auto_edit_frame(ctx, text, Vec::new(), request);
        let request = EditorRequest { auto_edit, ..Default::default() };
        auto_edit_frame(ctx, text, Vec::new(), request);
    }

    #[test]
    fn test_auto_edit_closes_a_bracket_and_indents_inside_it() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        let rust = Some(Language::Rust);
        focused_editor(&ctx, &mut text, rust);
        let request = || EditorRequest { auto_edit: rust, ..Default::default() };

        let typed = vec![egui::Event::Text(String::from("{"))];
        auto_edit_frame(&ctx, &mut text, typed, request());
        assert_eq!(text, "{}");
        assert_eq!(editor_caret(&ctx), Some((1, 1)));

        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        auto_edit_frame(&ctx, &mut text, vec![enter], request());
        assert_eq!(text, "{\n    \n}");
        assert_eq!(editor_caret(&ctx), Some((6, 6)));
    }

    #[test]
    fn test_without_auto_edit_typing_is_left_to_egui() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        focused_editor(&ctx, &mut text, None);

        let typed = vec![egui::Event::Text(String::from("("))];
        auto_edit_frame(&ctx, &mut text, typed, EditorRequest::default());
        assert_eq!(text, "(");
        assert_eq!(editor_caret(&ctx), Some((1, 1)));
    }

    /// An editor without focus does not act on the frame's typing.
    #[test]
    fn test_auto_edit_needs_focus() {
        let ctx = egui::Context::default();
        let mut text = String::from("x");
        let typed = vec![egui::Event::Text(String::from("("))];
        let request = EditorRequest { auto_edit: Some(Language::Rust), ..Default::default() };
        auto_edit_frame(&ctx, &mut text, typed, request);
        assert_eq!(text, "x");
    }

    /// One frame of an editor with line numbers over `text`, folded as
    /// `hidden` says, fed `events`.
    fn fold_frame(
        ctx: &egui::Context,
        text: &mut String,
        events: Vec<egui::Event>,
        hidden: &[(usize, usize)],
        foldable: &[(usize, usize)],
    ) -> (FoldOutput, Vec<egui::epaint::ClippedShape>) {
        let mut folds = FoldOutput::default();
        let shapes = run_frame_with_events(ctx, events, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let request = EditorRequest {
                    hidden: hidden.to_vec(),
                    foldable: foldable.to_vec(),
                    ..Default::default()
                };
                folds = super::render_editor_with(
                    ui,
                    text,
                    None,
                    false,
                    true,
                    "",
                    Some(egui::Id::unique("ed")),
                    request,
                    None,
                    &mut Bookmarks::default(),
                    &LineMarks::default(),
                    None,
                    &mut LayoutCache::default(),
                )
                .2;
            });
        });
        (folds, shapes)
    }

    #[test]
    fn test_a_folded_block_is_drawn_collapsed() {
        let doc = String::from("a {\n  b\n  c\n}\nz");
        let mut text = doc.clone();
        let ctx = egui::Context::default();
        // "a {" ends at char 3, "  c" at char 11.
        let (hidden, foldable) = ([(3, 11)], [(0, 2)]);
        let (_, open) = fold_frame(&ctx, &mut text, Vec::new(), &[], &foldable);
        let (_, shapes) = fold_frame(&ctx, &mut text, Vec::new(), &hidden, &foldable);
        let (open_galley, _) = painted_text(&open, &doc).expect("text painted");
        let (galley, _) = painted_text(&shapes, &doc).expect("text painted");
        let row_h = open_galley.height() / 5.0;
        assert!((open_galley.height() - galley.height() - 2.0 * row_h).abs() < 1.0);
        for (n, shown) in [(1, true), (2, false), (3, false), (4, true), (5, true)] {
            assert_eq!(painted_number(&shapes, n).is_some(), shown, "line {n}");
        }
        // Line 4 sits right below the header.
        let (header, _) = painted_number(&shapes, 1).unwrap();
        let (after, _) = painted_number(&shapes, 4).unwrap();
        assert!((after.center().y - header.center().y - row_h).abs() < 1.0);
        assert_eq!(text, doc);
    }

    #[test]
    fn test_clicking_the_marker_or_the_chevron_toggles_the_fold() {
        let doc = String::from("a {\n  b\n  c\n}\nz");
        let mut text = doc.clone();
        let ctx = egui::Context::default();
        let (hidden, foldable) = ([(3, 11)], [(0, 2)]);
        let (_, shapes) = fold_frame(&ctx, &mut text, Vec::new(), &hidden, &foldable);
        let (galley, _) = painted_text(&shapes, &doc).unwrap();
        let (number, _) = painted_number(&shapes, 1).unwrap();
        let char_width = galley.width() / 3.0;
        // The `⋯` box starts 4 points after the end of "a {".
        let marker = egui::pos2(galley.left() + 3.0 * char_width + 8.0, number.center().y);
        let (folds, _) = fold_frame(&ctx, &mut text, click_events(marker), &hidden, &foldable);
        assert_eq!(folds.toggle, Some(0), "marker");
        fold_frame(&ctx, &mut text, Vec::new(), &hidden, &foldable);
        // The chevron sits between the number and the text.
        let chevron = egui::pos2(galley.left() - 4.0, number.center().y);
        let (folds, _) = fold_frame(&ctx, &mut text, click_events(chevron), &hidden, &foldable);
        assert_eq!(folds.toggle, Some(0), "chevron");
        // A click on the text toggles nothing.
        fold_frame(&ctx, &mut text, Vec::new(), &hidden, &foldable);
        let on_text = egui::pos2(galley.left() + 2.0, number.center().y);
        let (folds, _) = fold_frame(&ctx, &mut text, click_events(on_text), &hidden, &foldable);
        assert_eq!(folds.toggle, None);
    }
}
