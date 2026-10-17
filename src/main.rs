mod app;
mod config;
mod wg;

use eframe::egui;

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Wirus")
            .with_inner_size([980.0, 660.0])
            .with_min_inner_size([720.0, 440.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Wirus",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
