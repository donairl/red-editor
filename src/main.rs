mod app;
mod file_browser;
mod modals;

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([600.0, 400.0]), // Ukuran window awal
        ..Default::default()
    };
    
    eframe::run_native(
        "Simple Text Editor",
        options,
        Box::new(|_cc| Ok(Box::new(TextEditorApp::default()))),
    )
}

struct TextEditorApp {
    text_content: String,
}

impl Default for TextEditorApp {
    fn default() -> Self {
        Self {
            text_content: String::from("Ketik sesuatu di sini..."),
        }
    }
}

impl eframe::App for TextEditorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Panel atas untuk Menu / Toolbar
        egui::Panel::top("top_panel").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Clear Text").clicked() {
                        self.text_content.clear();
                        ui.close();
                    }
                });
                
                // Indikator jumlah karakter di sebelah kanan toolbar
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("Karakter: {}", self.text_content.len()));
                });
            });
        });

        // Panel utama untuk area Text Editor
        egui::CentralPanel::default().show(ui, |ui| {
            // Mengatur font agar text editor menggunakan font monospace (cocok untuk coding/menulis)
            let text_style = egui::TextStyle::Monospace;
            
            // Membuat TextEdit multi-baris yang memenuhi seluruh area panel
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_sized(
                    ui.available_size(), // Mengisi sisa ruang yang ada
                    egui::TextEdit::multiline(&mut self.text_content)
                        .font(text_style)
                        .lock_focus(true) // Tetap fokus meskipun klik di luar teks
                        .desired_width(f32::INFINITY)
                );
            });
        });
    }
}

