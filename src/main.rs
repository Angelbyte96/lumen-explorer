// Release builds on Windows are GUI apps: no console window next to Lumen.
// Debug builds keep it, for LUMEN_LOG and panics.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
#[cfg(not(windows))]
mod apps;
mod open_here;
mod behavior;
mod config;
mod drag;
#[cfg(not(windows))]
mod file_manager1;
mod folder_views;
mod format;
#[cfg(not(windows))]
mod icon_names;
#[cfg(not(windows))]
mod icon_theme;
mod fs;
mod i18n;
mod input;
mod menu;
mod shell_menu;
mod places;
mod recycle_bin;
mod setup;
mod ui;
#[cfg(target_os = "linux")]
mod portal;
#[cfg(target_os = "linux")]
mod wayland_dnd;
#[cfg(windows)]
mod windows_drag;

use eframe::egui;

fn main() -> eframe::Result {
    // LUMEN_LOG=1 prints how long every folder listing takes to stderr.
    let log = std::env::var_os("LUMEN_LOG").is_some_and(|v| v != "0");
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    // Started by D-Bus to show files in their folder: no window of its own.
    #[cfg(target_os = "linux")]
    if args.iter().any(|a| a == file_manager1::FLAG) {
        let served = std::env::current_exe().map_err(|err| err.to_string()).and_then(|exe| file_manager1::serve(&exe));
        if let Err(err) = served {
            eprintln!("[lumen] {err}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let start = app::start(&args);
    let select = app::selected_at_start(&args);
    // `--setup` (Windows' Apps & features "Uninstall") opens Lumen Setup.
    let setup = app::wants_setup(&args);
    let home = fs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
    let config_home = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(std::path::PathBuf::from);
    let appdata = std::env::var_os("APPDATA").filter(|v| !v.is_empty()).map(std::path::PathBuf::from);
    let location = config::location(cfg!(windows), fs::home_dir().as_deref(), config_home.as_deref(), appdata.as_deref());
    let settings = config::open(location.map(config::Location::resolve));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lumen")
            .with_app_id("lumen")
            // Lumen draws its own title bar (the tab row), on Windows and Linux alike.
            .with_decorations(false)
            .with_inner_size(settings.config.window.unwrap_or([960.0, 640.0]))
            .with_maximized(settings.config.maximized)
            .with_min_inner_size(config::MIN_WINDOW)
            .with_icon(window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Lumen",
        options,
        Box::new(move |cc| {
            let mut app = app::LumenApp::start(&cc.egui_ctx, start, &home, log, settings);
            if let Some(name) = select {
                app.select_at_start(name);
            }
            if setup {
                app.open_setup();
            }
            Ok(Box::new(app))
        }),
    )
}

/// The app icon (assets/lumen.png), built into the binary. The title bar and
/// taskbar show it on X11 and Windows; GNOME on Wayland takes the icon from
/// lumen.desktop instead.
fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/lumen.png"))
        .expect("assets/lumen.png is a valid PNG")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_icon_is_the_256_pixel_png() {
        let icon = window_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
        // Corners are transparent (rounded tile), the centre is opaque.
        let alpha = |x: usize, y: usize| icon.rgba[(y * 256 + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(128, 128), 255);
    }

    /// build.rs puts assets/lumen.ico into the Windows .exe, the icon File
    /// Explorer shows for it; it needs every size Explorer's views ask for.
    #[test]
    fn the_exe_icon_holds_every_size_file_explorer_uses() {
        let ico = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/lumen.ico")).expect("assets/lumen.ico");
        assert_eq!(&ico[..4], &[0, 0, 1, 0], "an icon file");
        let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
        // Each 16-byte entry starts with its width; 0 means 256.
        let mut sizes: Vec<u32> =
            (0..count).map(|i| ico[6 + i * 16]).map(|w| if w == 0 { 256 } else { w.into() }).collect();
        sizes.sort();
        assert_eq!(sizes, [16, 24, 32, 48, 64, 128, 256]);
    }

    /// The Menu key reaches Lumen only through vendor/egui-winit (it arrives as
    /// Shift+F10). Cargo ignores a [patch] whose version no longer matches,
    /// with just a warning: after an egui update this fails until it is redone.
    #[test]
    fn the_patched_egui_winit_is_the_one_built() {
        let lock = include_str!("../Cargo.lock");
        let entry = lock.split("[[package]]").find(|p| p.contains("name = \"egui-winit\"")).expect("egui-winit is locked");
        assert!(!entry.contains("source = "), "egui-winit comes from the registry, not vendor/: {entry}");
    }

    /// GNOME matches the window to its menu entry by app_id, so the entry,
    /// its icon and its window class are all "lumen".
    #[test]
    fn the_desktop_entry_is_named_after_the_app_id() {
        let entry = include_str!("../assets/lumen.desktop");
        for line in ["Exec=lumen --restore %f", "Icon=lumen", "StartupWMClass=lumen", "MimeType=inode/directory;"] {
            assert!(entry.lines().any(|l| l == line), "missing {line}");
        }
    }
}
