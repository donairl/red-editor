//! The menu bar's menus and the editor's right-click menu.

use eframe::egui;

use crate::about::HOME_URL;
use crate::app::{CONTEXT_CHAR, MIN_RECENT_FOLDER_WIDTH, SidebarView, TextEditorApp};
use crate::find;
use crate::highlighter::Language;
use crate::history::Entry;
use crate::palette::PaletteMode;
use crate::prettify;
use crate::shortcuts;
use crate::shortcuts::ShortcutAction;
use crate::symbols;
use crate::theme::ThemeId;

/// A menu entry with its shortcut on the right; closes the menu when clicked.
fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    let clicked = ui
        .add(egui::Button::new(label).shortcut_text(shortcut))
        .clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// One File > Open Recent row: the entry's name, then its folder dimmed,
/// fitted into egui's menu width. A name too long for it is cut with `…`,
/// as is the folder in the room the name leaves. Cuts fall between whole
/// characters, and only change what is shown, never the entry.
fn recent_row(ui: &egui::Ui, entry: &Entry) -> egui::Button<'static> {
    let fit = |text: egui::RichText, width: f32| {
        egui::WidgetText::from(text).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            width,
            egui::TextStyle::Button,
        )
    };
    let (name, folder) = entry.label();
    let width = ui.spacing().menu_width;
    let name = fit(egui::RichText::new(name), width);
    let room = width - name.size().x - ui.spacing().item_spacing.x;
    let row = egui::Button::new(name);
    if room < MIN_RECENT_FOLDER_WIDTH {
        return row;
    }
    row.shortcut_text(fit(egui::RichText::new(folder).weak(), room))
}

impl TextEditorApp {
    /// Draws the "Bahasa" dropdown used by both the File and Tampilan menus.
    ///
    /// Re-selecting the already active language clears the override and returns
    /// to automatic detection.
    pub fn show_language_menu(&mut self, ui: &mut egui::Ui) {
        let label = self.i18n.t("menu.syntax").to_owned();
        ui.menu_button(label, |ui| {
            let current = self.effective_language();
            for lang in Language::all() {
                let selected = *lang == current;
                if ui
                    .selectable_label(selected, self.language_label(*lang))
                    .clicked()
                {
                    self.active_tab_mut().language_override =
                        if selected { None } else { Some(*lang) };
                    ui.close();
                }
            }
        });
    }

    /// The File > Open Recent submenu.
    pub fn show_recent_menu(&mut self, ui: &mut egui::Ui) {
        let label = self.i18n.t("file.open_recent").to_owned();
        ui.menu_button(label, |ui| self.show_recent_items(ui));
    }

    /// The history, newest first, each entry's folder on its right; a click
    /// reopens the entry. Ends with Clear Recently Opened.
    fn show_recent_items(&mut self, ui: &mut egui::Ui) {
        if self.history.is_empty() {
            ui.weak(self.i18n.t("file.no_recent"));
            return;
        }
        let mut picked = None;
        for entry in self.history.entries() {
            let row = ui
                .add(recent_row(ui, entry))
                .on_hover_text(entry.path.display().to_string());
            if row.clicked() {
                picked = Some(entry.clone());
                ui.close();
            }
        }
        ui.separator();
        if ui.button(self.i18n.t("file.clear_recent")).clicked() {
            self.history.clear();
            ui.close();
        }
        if let Some(entry) = picked {
            self.open_recent(entry);
        }
    }

    pub fn show_view_menu(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.word_wrap, self.i18n.t("view.word_wrap"));
        ui.checkbox(
            &mut self.show_line_numbers,
            self.i18n.t("view.line_numbers"),
        );
        let ctx = ui.ctx().clone();
        // Folding acts on the editor, which the start page and the Git
        // Graph hide.
        let editing = !self.show_welcome && self.git_graph.is_none();
        let fold_items = [
            ("view.fold", shortcuts::label(&ctx, &shortcuts::FOLD), ShortcutAction::Fold),
            ("view.unfold", shortcuts::label(&ctx, &shortcuts::UNFOLD), ShortcutAction::Unfold),
            ("view.fold_all", String::new(), ShortcutAction::FoldAll),
            ("view.unfold_all", String::new(), ShortcutAction::UnfoldAll),
        ];
        ui.separator();
        for (key, shortcut, action) in fold_items {
            let label = self.i18n.t(key);
            if ui.add_enabled_ui(editing, |ui| menu_item(ui, label, &shortcut)).inner {
                self.apply_shortcut(&ctx, action);
            }
        }
        ui.separator();
        let blame = egui::Button::new(self.i18n.t("view.blame"))
            .selected(self.show_blame)
            .shortcut_text(shortcuts::label(&ctx, &shortcuts::BLAME));
        if ui.add(blame).clicked() {
            self.show_blame = !self.show_blame;
            ui.close();
        }
        let graph = egui::Button::new(self.i18n.t("view.git_graph"))
            .selected(self.git_graph.is_some())
            .shortcut_text(shortcuts::label(&ctx, &shortcuts::GIT_GRAPH));
        if ui.add_enabled(self.git.repo().is_some(), graph).clicked() {
            self.toggle_git_graph(&ctx);
            ui.close();
        }
        if menu_item(
            ui,
            self.i18n.t("view.zen_mode"),
            &shortcuts::label(&ctx, &shortcuts::ZEN_MODE),
        ) {
            self.toggle_zen_mode(&ctx);
        }
        let preview_label = self.i18n.t("view.markdown_preview");
        let preview_shortcut = shortcuts::label(&ctx, &shortcuts::MARKDOWN_PREVIEW);
        if ui
            .add_enabled(
                self.preview_available(),
                egui::Button::new(preview_label)
                    .selected(self.show_markdown_preview)
                    .shortcut_text(preview_shortcut),
            )
            .clicked()
        {
            self.toggle_markdown_preview();
            ui.close();
        }
        let scm_label = self.i18n.t("view.source_control");
        let scm_shortcut = shortcuts::label(&ctx, &shortcuts::SOURCE_CONTROL);
        let scm_open = self.show_sidebar && self.sidebar_view == SidebarView::SourceControl;
        if ui
            .add(
                egui::Button::new(scm_label)
                    .selected(scm_open)
                    .shortcut_text(scm_shortcut),
            )
            .clicked()
        {
            self.toggle_source_control();
            ui.close();
        }
        let console_label = self.i18n.t("view.console");
        let console_shortcut = shortcuts::label(&ctx, &shortcuts::TOGGLE_CONSOLE);
        if ui
            .add(
                egui::Button::new(console_label)
                    .selected(self.show_console)
                    .shortcut_text(console_shortcut),
            )
            .clicked()
        {
            self.toggle_console(&ctx);
            ui.close();
        }
        if ui
            .add(egui::Button::new(self.i18n.t("view.git_log")).selected(self.show_git_log))
            .clicked()
        {
            self.show_git_log = !self.show_git_log;
            ui.close();
        }
        if menu_item(
            ui,
            self.i18n.t("view.command_palette"),
            &shortcuts::label(&ctx, &shortcuts::COMMAND_PALETTE),
        ) {
            self.open_palette(PaletteMode::Commands);
        }
        self.show_theme_menu(ui);
        ui.separator();
        self.show_language_menu(ui);
        self.show_ui_language_menu(ui);
    }

    /// Lists every color theme; picking one switches to it at once.
    pub fn show_theme_menu(&mut self, ui: &mut egui::Ui) {
        let label = self.i18n.t("menu.theme").to_owned();
        ui.menu_button(label, |ui| {
            for id in ThemeId::ALL {
                let name = id.name().unwrap_or_else(|| self.i18n.t("theme.system"));
                if ui.selectable_label(self.theme == id, name).clicked() {
                    self.theme = id;
                    ui.close();
                }
            }
        });
    }

    /// Lists every interface language; picking one switches to it.
    pub fn show_ui_language_menu(&mut self, ui: &mut egui::Ui) {
        let label = self.i18n.t("menu.ui_language").to_owned();
        ui.menu_button(label, |ui| {
            let current = self.i18n.code().to_owned();
            let locales: Vec<(String, String)> = self
                .i18n
                .locales()
                .map(|(code, name)| (code.to_owned(), name.to_owned()))
                .collect();
            for (code, name) in locales {
                if ui.selectable_label(code == current, name).clicked() {
                    self.i18n.select(&code);
                    ui.close();
                }
            }
        });
    }

    pub fn show_help_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button(self.i18n.t("help.home")).clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(HOME_URL));
            ui.close();
        }
        if ui.button(self.i18n.t("help.about")).clicked() {
            self.show_about_modal = true;
            ui.close();
        }
    }

    pub fn show_edit_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let item = |ui: &mut egui::Ui, label: &str, shortcut: &egui::KeyboardShortcut| {
            menu_item(ui, label, &shortcuts::label(ctx, shortcut))
        };
        if item(ui, self.i18n.t("edit.undo"), &shortcuts::UNDO) {
            self.send_editor_key(egui::Key::Z);
        }
        if item(ui, self.i18n.t("edit.redo"), &shortcuts::REDO) {
            self.send_editor_key(egui::Key::Y);
        }
        ui.separator();
        self.show_clipboard_items(ui, ctx);
        ui.separator();
        let editing = !self.show_welcome;
        if ui
            .add_enabled_ui(editing, |ui| {
                item(ui, self.i18n.t("edit.find"), &shortcuts::FIND)
            })
            .inner
        {
            self.open_find(ctx, false);
        }
        if ui
            .add_enabled_ui(editing, |ui| {
                item(ui, self.i18n.t("edit.replace"), &shortcuts::REPLACE)
            })
            .inner
        {
            self.open_find(ctx, true);
        }
        if item(
            ui,
            self.i18n.t("edit.find_in_files"),
            &shortcuts::FIND_IN_FILES,
        ) {
            self.open_search(ctx);
        }
        if ui
            .add_enabled_ui(editing, |ui| {
                item(ui, self.i18n.t("edit.goto_line"), &shortcuts::GOTO_LINE)
            })
            .inner
        {
            self.open_goto();
        }
        if ui
            .add_enabled_ui(editing, |ui| {
                item(ui, self.i18n.t("edit.go_to_bracket"), &shortcuts::GO_TO_BRACKET)
            })
            .inner
        {
            self.apply_shortcut(ctx, ShortcutAction::GoToBracket);
        }
        ui.separator();
        let line_items = [
            (
                "edit.move_line_up",
                &shortcuts::MOVE_LINE_UP,
                ShortcutAction::MoveLineUp,
            ),
            (
                "edit.move_line_down",
                &shortcuts::MOVE_LINE_DOWN,
                ShortcutAction::MoveLineDown,
            ),
            (
                "edit.copy_line_up",
                &shortcuts::COPY_LINE_UP,
                ShortcutAction::CopyLineUp,
            ),
            (
                "edit.copy_line_down",
                &shortcuts::COPY_LINE_DOWN,
                ShortcutAction::CopyLineDown,
            ),
            (
                "edit.delete_line",
                &shortcuts::DELETE_LINE,
                ShortcutAction::DeleteLine,
            ),
            (
                "edit.toggle_comment",
                &shortcuts::TOGGLE_COMMENT,
                ShortcutAction::ToggleComment,
            ),
            (
                "edit.select_next_match",
                &shortcuts::SELECT_NEXT_MATCH,
                ShortcutAction::SelectNextMatch,
            ),
        ];
        for (key, shortcut, action) in line_items {
            if ui
                .add_enabled_ui(editing, |ui| item(ui, self.i18n.t(key), shortcut))
                .inner
            {
                self.apply_shortcut(ctx, action);
            }
        }
        ui.separator();
        let navigable = symbols::supports(self.effective_language());
        let nav_item = |ui: &mut egui::Ui, label: &str, shortcut: &egui::KeyboardShortcut| {
            let button = egui::Button::new(label).shortcut_text(shortcuts::label(ctx, shortcut));
            let clicked = ui.add_enabled(navigable, button).clicked();
            if clicked {
                ui.close();
            }
            clicked
        };
        if nav_item(ui, self.i18n.t("edit.goto_symbol"), &shortcuts::GOTO_SYMBOL) {
            self.open_symbol_picker();
        }
        if nav_item(
            ui,
            self.i18n.t("edit.goto_definition"),
            &shortcuts::GOTO_DEFINITION,
        ) {
            self.goto_definition(ctx, None);
        }
        if nav_item(
            ui,
            self.i18n.t("edit.find_references"),
            &shortcuts::FIND_REFERENCES,
        ) {
            self.find_references(ctx, None);
        }
        ui.separator();
        let prettify_label = self.i18n.t("edit.prettify");
        let prettify_shortcut = shortcuts::label(ctx, &shortcuts::PRETTIFY);
        if ui
            .add_enabled(
                prettify::supports(self.effective_language()),
                egui::Button::new(prettify_label).shortcut_text(prettify_shortcut),
            )
            .clicked()
        {
            self.prettify_document();
            ui.close();
        }
    }

    /// Cut, Copy and Paste, shared by the Edit menu and the editor's
    /// right-click menu.
    fn show_clipboard_items(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let item = |ui: &mut egui::Ui, label: &str, shortcut: &egui::KeyboardShortcut| {
            menu_item(ui, label, &shortcuts::label(ctx, shortcut))
        };
        if item(ui, self.i18n.t("edit.cut"), &shortcuts::CUT) {
            self.cut_selection(ctx);
        }
        if item(ui, self.i18n.t("edit.copy"), &shortcuts::COPY) {
            self.copy_selection(ctx);
        }
        if item(ui, self.i18n.t("edit.paste"), &shortcuts::PASTE) {
            self.paste_clipboard(ctx);
        }
    }

    /// The editor's right-click menu. Opening it takes the focus like a click
    /// on the menu bar: the selection stays painted, and the editor gets both
    /// back when the menu closes.
    pub(crate) fn show_editor_context_menu(
        &mut self,
        ctx: &egui::Context,
        editor: &egui::Response,
    ) {
        if editor.secondary_clicked() && ctx.memory(|m| m.has_focus(editor.id)) {
            self.menu_focus = Some(editor.id);
            ctx.memory_mut(|m| m.surrender_focus(editor.id));
        }
        editor.context_menu(|ui| {
            self.show_clipboard_items(ui, ctx);
            if symbols::supports(self.effective_language()) {
                ui.separator();
                let at = ctx.data(|d| d.get_temp::<usize>(self.editor_id().with(CONTEXT_CHAR)));
                let item = |ui: &mut egui::Ui, label: &str, shortcut: &egui::KeyboardShortcut| {
                    menu_item(ui, label, &shortcuts::label(ctx, shortcut))
                };
                if item(
                    ui,
                    self.i18n.t("edit.goto_definition"),
                    &shortcuts::GOTO_DEFINITION,
                ) {
                    self.goto_definition(ctx, at);
                }
                if item(
                    ui,
                    self.i18n.t("edit.find_references"),
                    &shortcuts::FIND_REFERENCES,
                ) {
                    self.find_references(ctx, at);
                }
            }
            if let Some(line) = self.context_line(ctx) {
                ui.separator();
                let key = if self.active_tab().bookmarks.contains(line) {
                    "bookmark.remove"
                } else {
                    "bookmark.add"
                };
                if ui.button(self.i18n.t(key)).clicked() {
                    self.toggle_bookmark(line);
                    ui.close();
                }
            }
        });
    }

    /// The line the editor was last right-clicked on, else the caret's.
    fn context_line(&self, ctx: &egui::Context) -> Option<usize> {
        ctx.data(|d| d.get_temp::<usize>(self.editor_id().with(CONTEXT_CHAR)))
            .map(|at| find::line_col(&self.active_tab().text_content, at).0)
            .or_else(|| self.caret_position(ctx).map(|(line, _)| line))
    }

    pub fn show_file_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let item = |ui: &mut egui::Ui, label: &str, shortcut: &egui::KeyboardShortcut| {
            menu_item(ui, label, &shortcuts::label(ctx, shortcut))
        };
        if item(ui, self.i18n.t("file.new_tab"), &shortcuts::NEW_TAB) {
            self.open_new_tab();
        }
        if item(ui, self.i18n.t("file.close_tab"), &shortcuts::CLOSE_TAB) {
            self.trigger_close_tab(self.active_tab_index);
        }
        if item(ui, self.i18n.t("file.new_window"), &shortcuts::NEW_WINDOW) {
            self.open_new_window();
        }
        ui.separator();
        if item(ui, self.i18n.t("file.open_file"), &shortcuts::OPEN_FILE) {
            self.trigger_open_file();
        }
        if item(ui, self.i18n.t("file.open_folder"), &shortcuts::OPEN_FOLDER) {
            self.trigger_open_folder();
        }
        if item(ui, self.i18n.t("file.quick_open"), &shortcuts::QUICK_OPEN) {
            self.open_palette(PaletteMode::Files);
        }
        self.show_recent_menu(ui);
        ui.separator();
        let editing = !self.show_welcome;
        if ui
            .add_enabled_ui(editing, |ui| {
                item(ui, self.i18n.t("file.save"), &shortcuts::SAVE)
            })
            .inner
        {
            self.save();
        }
        if ui
            .add_enabled_ui(editing, |ui| {
                item(ui, self.i18n.t("file.save_as"), &shortcuts::SAVE_AS)
            })
            .inner
        {
            self.save_as();
        }
        ui.separator();
        self.show_language_menu(ui);
        ui.separator();
        if item(ui, self.i18n.t("file.exit"), &shortcuts::EXIT) {
            self.trigger_exit(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::*;
    use crate::history::EntryKind;
    use std::fs;
    use std::path::PathBuf;

    /// The painted text shape whose full text is `text`.
    fn painted_galley(
        shapes: &[egui::epaint::ClippedShape],
        text: &str,
    ) -> Option<std::sync::Arc<egui::Galley>> {
        shapes.iter().find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(shape) if shape.galley.text() == text => Some(shape.galley.clone()),
            _ => None,
        })
    }

    /// The characters a galley actually shows, the `…` of a cut included.
    fn shown_chars(galley: &egui::Galley) -> String {
        galley
            .rows
            .iter()
            .flat_map(|row| row.glyphs.iter().map(|glyph| glyph.chr))
            .collect()
    }

    /// The language picker sits in the Tampilan menu, not in the status bar.
    #[test]
    fn test_language_menu_lives_in_view_menu_not_status_bar() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_status_bar(ui));
        assert!(
            painted_text(&shapes, "Bahasa").is_none(),
            "status bar must not offer Bahasa"
        );
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_view_menu(ui));
        assert!(
            painted_text(&shapes, "Bahasa").is_some(),
            "Tampilan menu must offer Bahasa"
        );
    }

    /// Help > Home opens the project page in the browser.
    #[test]
    fn test_help_home_opens_the_project_page() {
        let mut app = TextEditorApp::default();
        let ctx = egui::Context::default();
        let frame = |app: &mut TextEditorApp, events| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.show_help_menu(ui));
            output.textures_delta.clear();
            output
        };
        let home = frame(&mut app, Vec::new())
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "Beranda" => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                }
                _ => None,
            })
            .expect("Home item");
        let output = frame(&mut app, click_events(home.center()));
        let opened: Vec<&str> = output
            .platform_output
            .commands
            .iter()
            .filter_map(|command| match command {
                egui::OutputCommand::OpenUrl(open) => Some(open.url.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(opened, ["https://github.com/donairl/red-editor/"]);
        assert!(!app.show_about_modal, "Home is not About");
    }

    #[test]
    fn test_open_recent_menu_lists_entries_and_reopens_the_clicked_one() {
        let dir = history_folder("menu");
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.history.record(a.clone(), EntryKind::File);
        app.history.record(b, EntryKind::File);

        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_recent_items(ui));
        let (b_rect, _) = painted_text(&shapes, "b.txt").expect("b.txt is listed");
        let (a_rect, _) = painted_text(&shapes, "a.txt").expect("a.txt is listed");
        assert!(b_rect.top() < a_rect.top(), "the newest is listed first");
        assert!(
            painted_text(&shapes, &dir.display().to_string()).is_some(),
            "each entry shows its folder"
        );

        run_frame_with_events(&ctx, click_events(a_rect.center()), |ui| {
            app.show_recent_items(ui)
        });
        assert_eq!(
            app.active_tab().current_path.as_ref(),
            Some(&a),
            "one click reopens the entry"
        );
        assert_eq!(app.history.entries()[0].path, a);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_open_recent_menu_cuts_long_names_with_an_ellipsis() {
        let dir = history_folder("long_name");
        // Two-, three- and one-byte characters, 220 bytes in all.
        let name = format!("{}.txt", "café_ñandú_€_".repeat(12));
        let long = dir.join(&name);
        fs::write(&long, "long").unwrap();
        let short = dir.join("a.txt");
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.history.record(short.clone(), EntryKind::File);
        app.history.record(long.clone(), EntryKind::File);

        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_recent_items(ui));
        let menu_width = ctx.global_style().spacing.menu_width;
        let cut = painted_galley(&shapes, &name).expect("the long name is painted");
        assert!(cut.elided, "the long name is cut");
        assert!(cut.size().x <= menu_width, "and fits the menu width");
        let shown = shown_chars(&cut);
        let kept = shown.strip_suffix('…').expect("the cut ends in …");
        assert!(
            !kept.is_empty() && name.starts_with(kept),
            "{shown:?} is whole characters of {name:?}"
        );

        let whole = painted_galley(&shapes, "a.txt").expect("the short name is painted");
        assert!(!whole.elided, "a short name is shown as is");
        assert_eq!(shown_chars(&whole), "a.txt");

        let (cut_rect, _) = painted_text(&shapes, &name).unwrap();
        run_frame_with_events(&ctx, click_events(cut_rect.center()), |ui| {
            app.show_recent_items(ui)
        });
        assert_eq!(
            app.active_tab().current_path.as_ref(),
            Some(&long),
            "the full path is opened"
        );
        assert_eq!(app.active_tab().text_content, "long");
        assert_eq!(
            app.history.entries()[0].path,
            long,
            "the history keeps the full path"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_open_recent_menu_cuts_a_long_folder_to_the_room_left() {
        let folder: PathBuf = std::iter::once(std::path::MAIN_SEPARATOR.to_string())
            .chain((0..30).map(|i| format!("folder_{i}")))
            .collect();
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.history.record(folder.join("x.txt"), EntryKind::File);

        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_recent_items(ui));
        let menu_width = ctx.global_style().spacing.menu_width;
        let name = painted_galley(&shapes, "x.txt").expect("the name is painted");
        assert!(!name.elided, "the name keeps its room");
        let folder =
            painted_galley(&shapes, &folder.display().to_string()).expect("the folder is painted");
        assert!(folder.elided, "the long folder is cut");
        assert!(name.size().x + folder.size().x <= menu_width);
    }

    #[test]
    fn test_open_recent_menu_when_empty_and_after_clearing() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        let empty = app.i18n.t("file.no_recent").to_owned();
        let clear = app.i18n.t("file.clear_recent").to_owned();
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_recent_items(ui));
        assert!(painted_text(&shapes, &empty).is_some());
        assert!(painted_text(&shapes, &clear).is_none(), "nothing to clear");

        app.history.record(PathBuf::from("/x.txt"), EntryKind::File);
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_recent_items(ui));
        let (clear_rect, _) = painted_text(&shapes, &clear).expect("Clear is offered");
        run_frame_with_events(&ctx, click_events(clear_rect.center()), |ui| {
            app.show_recent_items(ui)
        });
        assert!(app.history.is_empty());
    }

    #[test]
    fn test_file_menu_tab_actions() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp {
            show_welcome: false,
            ..Default::default()
        };
        assert_eq!(app.tabs.len(), 1);

        // Warm up frame so egui lays out the buttons
        let mut tab_baru_center = egui::pos2(0.0, 0.0);
        let mut tutup_tab_center = egui::pos2(0.0, 0.0);
        run_frame_with_events(&ctx, Vec::new(), |ui| {
            let r1 = ui.button("Tab Baru").rect;
            let r2 = ui.button("Tutup Tab").rect;
            tab_baru_center = r1.center();
            tutup_tab_center = r2.center();
        });

        // Click "Tab Baru" button in show_file_menu
        let click_tab_baru = vec![
            egui::Event::PointerMoved(tab_baru_center),
            egui::Event::PointerButton {
                pos: tab_baru_center,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: tab_baru_center,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        run_frame_with_events(&ctx, click_tab_baru, |ui| {
            app.show_file_menu(ui, &ctx);
        });
        assert_eq!(app.tabs.len(), 2, "Menu 'Tab Baru' should create a new tab");
        assert_eq!(app.active_tab_index, 1);

        // Warm up frame before next click
        run_frame_with_events(&ctx, Vec::new(), |ui| {
            app.show_file_menu(ui, &ctx);
        });

        // Click "Tutup Tab" button in show_file_menu
        let click_tutup_tab = vec![
            egui::Event::PointerMoved(tutup_tab_center),
            egui::Event::PointerButton {
                pos: tutup_tab_center,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: tutup_tab_center,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        run_frame_with_events(&ctx, click_tutup_tab, |ui| {
            app.show_file_menu(ui, &ctx);
        });
        assert_eq!(
            app.tabs.len(),
            1,
            "Menu 'Tutup Tab' should close active tab"
        );
        assert_eq!(app.active_tab_index, 0);
    }

    #[test]
    fn test_file_menu_offers_new_window_after_close_tab() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| {
            app.show_file_menu(ui, &ctx);
        });
        let (close_tab, _) = painted_text(&shapes, app.i18n.t("file.close_tab")).unwrap();
        let (new_window, _) =
            painted_text(&shapes, app.i18n.t("file.new_window")).expect("New Window is listed");
        assert!(new_window.top() > close_tab.top(), "below Close Tab");
        let shortcut = shortcuts::label(&ctx, &shortcuts::NEW_WINDOW);
        let (shortcut, _) = painted_text(&shapes, &shortcut).expect("its shortcut is shown");
        assert!((shortcut.center().y - new_window.center().y).abs() < 1.0);
    }
}
