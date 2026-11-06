// Release builds on Windows run as a GUI program, so no console window opens
// beside the editor. Debug builds keep the console for their output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod about;
mod activity_bar;
mod app;
mod auto_edit;
mod blame;
mod brackets;
mod bookmarks;
mod console_panel;
mod diff_view;
mod editor_view;
mod embedded_sql;
mod file_browser;
mod file_ops;
mod git;
mod git_graph;
mod git_log;
mod git_worker;
mod find;
mod folding;
mod gutter;
mod highlighter;
mod history;
mod i18n;
mod layout_cache;
mod line_diff;
mod line_ops;
mod markdown_preview;
mod menus;
mod modals;
mod palette;
mod prettify;
mod process;
mod search;
mod session;
mod shortcuts;
mod status_bar;
mod source_control;
mod symbols;
mod tab_bar;
mod terminal;
mod theme;
mod welcome;
mod zoom;

use app::TextEditorApp;
use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 600.0])
            .with_title("Red Text Editor"),
        ..Default::default()
    };

    eframe::run_native(
        "Red Text Editor",
        options,
        Box::new(|cc| Ok(Box::new(TextEditorApp::new(cc)))),
    )
}
