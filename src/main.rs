mod app;
mod config;
mod helper;
mod tray;
mod wg;

use eframe::egui;

fn main() -> Result<(), eframe::Error> {
    // The same binary runs as the root helper daemon (see helper.rs); the
    // install/uninstall flags are for troubleshooting from a terminal.
    match std::env::args().nth(1).as_deref() {
        Some("--helper") => helper::run_daemon(),
        Some("--install-helper") => {
            let tools = wg::find_tools().expect("WireGuard tools not found; run ./run.sh");
            helper::install(&tools).unwrap_or_else(|e| panic!("install failed: {e}"));
            println!("Helper installed");
            return Ok(());
        }
        Some("--uninstall-helper") => {
            helper::uninstall().unwrap_or_else(|e| panic!("uninstall failed: {e}"));
            println!("Helper removed");
            return Ok(());
        }
        _ => {}
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Wirest")
            .with_inner_size([980.0, 660.0])
            .with_min_inner_size([720.0, 440.0])
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png"))
                    .expect("bundled icon is a valid PNG"),
            ),
        ..Default::default()
    };
    eframe::run_native(
        "Wirest",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
