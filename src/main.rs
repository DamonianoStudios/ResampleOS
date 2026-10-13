// Release builds are a windowed app; debug keeps the console for diagnostics.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analysis;
mod app;
mod audio_io;
mod engine;
mod fx;
mod midi;
mod model;
mod music;
mod plugins;
mod prefs;
mod project;
mod sample;
mod shared;
mod stretch;
mod synth;
mod tune;
mod vocal;
mod ui;

use eframe::egui;

/// The window and taskbar icon, baked into the binary. See `assets/make_logo.py`.
fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")).unwrap_or_default()
}

fn main() -> eframe::Result {
    // Scanner mode: probe one plugin and exit. See `plugins.rs`.
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some(plugins::SCAN_FLAG) {
        plugins::scan_child(&args.next().unwrap_or_default());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ResampleOS")
            .with_inner_size([1600.0, 900.0])
            .with_min_inner_size([1180.0, 700.0])
            .with_icon(window_icon())
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "ResampleOS",
        options,
        Box::new(|cc| {
            ui::install_theme(&cc.egui_ctx);
            let prefs = prefs::Prefs::load();
            cc.egui_ctx.set_zoom_factor(prefs.ui_scale);
            Ok(Box::new(app::App::new(prefs)))
        }),
    )
}
