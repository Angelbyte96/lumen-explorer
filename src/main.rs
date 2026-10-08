mod app;
mod config;
mod format;
mod fs;
mod input;
mod places;
mod ui;

use eframe::egui;

fn main() -> eframe::Result {
    // LUMEN_LOG=1 prints how long every folder listing takes to stderr.
    let log = std::env::var_os("LUMEN_LOG").is_some_and(|v| v != "0");
    let start = app::start_dir(std::env::args().nth(1));
    let config_home = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(std::path::PathBuf::from);
    let config_path = config::default_path(fs::home_dir().as_deref(), config_home.as_deref());
    let settings = config::open(config_path);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lumen")
            .with_app_id("lumen")
            .with_inner_size(settings.config.window.unwrap_or([960.0, 640.0]))
            .with_min_inner_size(config::MIN_WINDOW),
        ..Default::default()
    };
    eframe::run_native(
        "Lumen",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::LumenApp::with_config(&cc.egui_ctx, start, log, settings)))
        }),
    )
}
