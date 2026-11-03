//! The status bar: file info, caret position, git branch, bookmarks.

use eframe::egui;

use crate::app::TextEditorApp;
use crate::find;
use crate::git_worker::Op;
use crate::theme;

impl TextEditorApp {
    /// The document name and the latest status message, and the caret's
    /// line and column at the right end, after a badge counting the
    /// bookmarks that opens their list.
    pub fn show_status_bar(&mut self, ui: &mut egui::Ui) {
        self.status_position = self.caret_position(ui.ctx());
        self.active_tab_mut().sync_bookmarks();
        self.status_bookmarks = self.active_tab().bookmarks.len();
        ui.horizontal(|ui| {
            if self.git.status.is_some() {
                self.show_branch_menu(ui);
                ui.separator();
            }
            let modified = if self.is_dirty() {
                self.i18n.t("status.modified")
            } else {
                ""
            };
            let name = self.file_display_name();
            ui.label(self.i18n.tf(
                "status.document",
                &[("name", &name), ("modified", modified)],
            ));

            if let Some((msg, is_error)) = &self.status_message {
                ui.separator();
                let color = if *is_error {
                    egui::Color32::from_rgb(220, 60, 60)
                } else {
                    egui::Color32::from_rgb(60, 180, 60)
                };
                ui.colored_label(color, msg);
            }

            if self.status_position.is_some() || self.status_bookmarks > 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some((line, col)) = self.status_position {
                        let (line, col) = (line.to_string(), col.to_string());
                        ui.label(
                            self.i18n
                                .tf("status.position", &[("line", &line), ("col", &col)]),
                        );
                    }
                    if self.status_bookmarks > 0 {
                        let count = self.status_bookmarks.to_string();
                        let color = theme::active(ui.ctx()).gutter.bookmark;
                        let label = egui::RichText::new(
                            self.i18n.tf("status.bookmarks", &[("count", &count)]),
                        )
                        .small()
                        .color(color);
                        let badge = egui::Button::new(label)
                            .stroke(egui::Stroke::new(1.0, color))
                            .corner_radius(8)
                            .selected(self.show_bookmarks);
                        if ui
                            .add(badge)
                            .on_hover_text(self.i18n.t("bookmark.show_list"))
                            .clicked()
                        {
                            self.show_bookmarks = !self.show_bookmarks;
                        }
                    }
                });
            }
        });
    }

    /// The status bar's branch button, `main* ↓1 ↑2`, and the menu it opens:
    /// switch to or create a branch, fetch, pull and push.
    fn show_branch_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(status) = &self.git.status else {
            return;
        };
        let mut label = status.head_label();
        if !status.files.is_empty() {
            label.push('*');
        }
        if status.behind > 0 {
            label.push_str(&format!(" ↓{}", status.behind));
        }
        if status.ahead > 0 {
            label.push_str(&format!(" ↑{}", status.ahead));
        }
        let busy = self.git.busy().is_some();
        if busy {
            ui.spinner();
        }
        let mut op = None;
        ui.menu_button(label, |ui| {
            ui.set_min_width(200.0);
            ui.label(
                egui::RichText::new(self.i18n.t("git.branches"))
                    .small()
                    .weak(),
            );
            for branch in &self.git.branches {
                if ui
                    .add_enabled(
                        !branch.current,
                        egui::Button::selectable(branch.current, &branch.name),
                    )
                    .clicked()
                {
                    op = Some(Op::Switch(branch.name.clone()));
                    ui.close();
                }
            }
            ui.separator();
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.new_branch_name)
                    .hint_text(self.i18n.t("git.new_branch_hint")),
            );
            let name = self.new_branch_name.trim().to_owned();
            if field.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                && !name.is_empty()
            {
                op = Some(Op::CreateBranch(name));
                self.new_branch_name.clear();
                ui.close();
            }
            ui.separator();
            for (key, action) in [
                ("git.fetch", Op::Fetch),
                ("git.pull", Op::Pull),
                ("git.push", Op::Push),
            ] {
                if ui
                    .add_enabled(!busy, egui::Button::new(self.i18n.t(key)))
                    .clicked()
                {
                    op = Some(action);
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text(self.i18n.t("git.branch_tooltip"));
        if let Some(op) = op {
            self.run_git(&ctx, op);
        }
    }

    /// 1-based line and column of the editor's caret, once it has one.
    pub(crate) fn caret_position(&self, ctx: &egui::Context) -> Option<(usize, usize)> {
        let state = egui::TextEdit::load_state(ctx, self.editor_id())?;
        let caret = state.cursor.char_range()?.primary.index.0;
        Some(find::line_col(&self.active_tab().text_content, caret))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::EditorRequest;
    use crate::app::test_support::*;
    use crate::bookmarks::Bookmarks;
    use crate::layout_cache::LayoutCache;
    use crate::line_diff::LineMarks;
    use std::path::PathBuf;

    #[test]
    fn test_status_bar_shows_the_git_branch() {
        let mut app = TextEditorApp::default();
        let ctx = egui::Context::default();
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_status_bar(ui));
        assert!(
            painted_text(&shapes, "main").is_none(),
            "no repository, no branch"
        );

        app.git.status = Some(crate::git::RepoStatus {
            branch: Some("main".into()),
            ahead: 2,
            behind: 1,
            files: vec![crate::git::FileChange {
                path: PathBuf::from("/r/a"),
                orig_path: None,
                staged: None,
                unstaged: Some(crate::git::Kind::Modified),
            }],
            ..Default::default()
        });
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_status_bar(ui));
        assert!(painted_text(&shapes, "main* ↓1 ↑2").is_some());
    }

    /// The caret's line and column sit at the right end of the status bar,
    /// once the editor has a caret; the document name stays on the left.
    #[test]
    fn test_status_bar_shows_the_caret_position_on_the_right() {
        let mut app = TextEditorApp::default();
        app.active_tab_mut().text_content = "ab\ncde\n".to_string();
        let ctx = egui::Context::default();
        let status = |app: &mut TextEditorApp| {
            run_frame_with_events(&ctx, Vec::new(), |ui| app.show_status_bar(ui))
        };
        // Where the first text starting with `prefix` was painted.
        let starting = |shapes: &[egui::epaint::ClippedShape], prefix: &str| {
            shapes.iter().find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(shape) if shape.galley.text().starts_with(prefix) => {
                    Some(egui::Rect::from_min_size(shape.pos, shape.galley.size()))
                }
                _ => None,
            })
        };

        let before = status(&mut app);
        assert_eq!(
            starting(&before, "Baris"),
            None,
            "no caret yet, no position"
        );
        let document = starting(&before, "Dokumen:").expect("document label");

        let mut state = egui::text_edit::TextEditState::default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(5),
            )));
        state.store(&ctx, app.editor_id());
        let after = status(&mut app);
        let (position, _) = painted_text(&after, "Baris 2, Kolom 3").expect("caret position");
        assert_eq!(
            starting(&after, "Dokumen:"),
            Some(document),
            "the left side does not move"
        );
        assert!(position.left() > document.right());
        assert!(
            position.right() > 780.0,
            "right-aligned in the 800 pt window"
        );

        assert!(app.i18n.select("en"));
        assert!(painted_text(&status(&mut app), "Ln 2, Col 3").is_some());
    }

    /// Typing and the arrow keys move the caret; the status bar follows on
    /// the next frame, as it is drawn before the editor.
    #[test]
    fn test_status_bar_position_follows_typing_and_arrows() {
        let mut app = TextEditorApp::default();
        app.active_tab_mut().text_content = "ab\ncde".to_string();
        let ctx = egui::Context::default();
        let id = app.editor_id();
        // The status bar, then the editor: every frame shows the editor, or
        // egui drops its focus.
        let frame = |app: &mut TextEditorApp, events, request: EditorRequest| {
            let mut request = Some(request);
            run_frame_with_events(&ctx, events, |ui| {
                app.show_status_bar(ui);
                let text = &mut app.active_tab_mut().text_content;
                let request = request.take().unwrap_or_default();
                crate::app::render_editor_with(
                    ui,
                    text,
                    None,
                    false,
                    false,
                    "",
                    Some(id),
                    request,
                    None,
                    &mut Bookmarks::default(),
                    &LineMarks::default(),
                    None,
                    &mut LayoutCache::default(),
                );
            })
        };
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let step = |app: &mut TextEditorApp, events, request| {
            frame(app, events, request);
            frame(app, Vec::new(), EditorRequest::default())
        };

        let shapes = step(
            &mut app,
            Vec::new(),
            EditorRequest {
                focus: true,
                select: Some((0, 0)),
                ..Default::default()
            },
        );
        assert!(painted_text(&shapes, "Baris 1, Kolom 1").is_some());
        let shapes = step(
            &mut app,
            vec![egui::Event::Text("xy".into())],
            EditorRequest::default(),
        );
        assert_eq!(app.active_tab().text_content, "xyab\ncde");
        assert!(
            painted_text(&shapes, "Baris 1, Kolom 3").is_some(),
            "typing moves the caret"
        );
        let shapes = step(
            &mut app,
            vec![key(egui::Key::ArrowDown)],
            EditorRequest::default(),
        );
        assert!(
            painted_text(&shapes, "Baris 2, Kolom 3").is_some(),
            "down arrow"
        );
        let shapes = step(
            &mut app,
            vec![key(egui::Key::ArrowRight)],
            EditorRequest::default(),
        );
        assert!(
            painted_text(&shapes, "Baris 2, Kolom 4").is_some(),
            "right arrow"
        );
    }

    /// With bookmarks, the status bar shows their count left of the caret
    /// position; a click on that badge opens their list, a second closes it.
    #[test]
    fn test_status_bar_badge_counts_bookmarks_and_opens_the_list() {
        let mut app = TextEditorApp::default();
        app.active_tab_mut().text_content = "a\nb\nc".into();
        let ctx = egui::Context::default();
        let status = |app: &mut TextEditorApp, events| {
            run_frame_with_events(&ctx, events, |ui| app.show_status_bar(ui))
        };
        let badge_shown = |shapes: &[egui::epaint::ClippedShape]| {
            shapes.iter().any(|clipped| {
                matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text().starts_with("Di-bookmark"))
            })
        };
        assert!(
            !badge_shown(&status(&mut app, Vec::new())),
            "no badge without bookmarks"
        );

        app.toggle_bookmark(1);
        app.toggle_bookmark(3);
        let mut state = egui::text_edit::TextEditState::default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(2),
            )));
        state.store(&ctx, app.editor_id());
        let shapes = status(&mut app, Vec::new());
        let (badge, _) = painted_text(&shapes, "Di-bookmark: 2").expect("badge");
        let (position, _) = painted_text(&shapes, "Baris 2, Kolom 1").expect("caret position");
        assert!(
            badge.right() < position.left(),
            "the caret position stays rightmost"
        );

        assert!(!app.show_bookmarks);
        status(&mut app, click_events(badge.center()));
        assert!(app.show_bookmarks, "the badge opens the list");
        status(&mut app, click_events(badge.center()));
        assert!(!app.show_bookmarks, "and closes it");

        assert!(app.i18n.select("en"));
        assert!(painted_text(&status(&mut app, Vec::new()), "Bookmarked: 2").is_some());
    }
}
