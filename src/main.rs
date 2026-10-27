// Release builds on Windows run as a GUI program, so no console window opens
// beside the editor. Debug builds keep the console for their output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod activity_bar;
mod app;
mod bookmarks;
mod embedded_sql;
mod file_browser;
mod find;
mod gutter;
mod highlighter;
mod history;
mod i18n;
mod layout_cache;
mod markdown_preview;
mod modals;
mod prettify;
mod search;
mod symbols;
mod tab_bar;
mod terminal;
mod theme;

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
