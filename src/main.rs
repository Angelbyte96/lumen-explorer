mod app;
mod format;
mod fs;
mod input;
mod ui;

use eframe::egui;

fn main() -> eframe::Result {
    // LUMEN_LOG=1 prints how long every folder listing takes to stderr.
    let log = std::env::var_os("LUMEN_LOG").is_some_and(|v| v != "0");
    let start = app::start_dir(std::env::args().nth(1));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lumen")
            .with_app_id("lumen")
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([480.0, 320.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Lumen",
        options,
        Box::new(move |cc| Ok(Box::new(app::LumenApp::new(&cc.egui_ctx, start, log)))),
    )
}
