mod app;
mod file_browser;
mod gutter;
mod highlighter;
mod modals;
mod tab_bar;

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
        Box::new(|_cc| Ok(Box::new(TextEditorApp::default()))),
    )
}
