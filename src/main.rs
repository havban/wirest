mod app;
mod config;
mod wg;

use eframe::egui;

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Wirus")
            .with_inner_size([980.0, 660.0])
            .with_min_inner_size([720.0, 440.0])
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png"))
                    .expect("bundled icon is a valid PNG"),
            ),
        ..Default::default()
    };
    eframe::run_native(
        "Wirus",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
