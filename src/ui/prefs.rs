//! Preferences window: audio device, capture length, interface scale and plugin folders.

use super::theme;
use crate::app::App;
use eframe::egui;

const BUFFERS: [u32; 6] = [64, 128, 256, 512, 1024, 2048];

pub fn draw(app: &mut App, ctx: &egui::Context) {
    if !app.show_prefs {
        return;
    }
    let mut open = true;
    let mut restart = false;
    egui::Window::new("Preferences").open(&mut open).resizable(false).collapsible(false).default_width(460.0).show(ctx, |ui| {
        let e = &mut app.prefs_edit;

        theme::title(ui, "AUDIO", theme::AMBER);
        egui::Grid::new("prefs_audio").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
            ui.label("Output device");
            let shown = e.output_device.clone().unwrap_or_else(|| "System default".into());
            egui::ComboBox::from_id_salt("pref_device").width(300.0).selected_text(shown).show_ui(ui, |ui| {
                ui.selectable_value(&mut e.output_device, None, "System default");
                for d in &app.outputs {
                    ui.selectable_value(&mut e.output_device, Some(d.name.clone()), &d.name);
                }
            });
            ui.end_row();

            ui.label("Sample rate");
            // Offer what the chosen device supports; with no device picked, the common rates.
            let rates: Vec<u32> = e
                .output_device
                .as_ref()
                .and_then(|n| app.outputs.iter().find(|d| &d.name == n))
                .map(|d| d.rates.clone())
                .filter(|r| !r.is_empty())
                .unwrap_or_else(|| vec![44100, 48000, 88200, 96000]);
            let shown = e.sample_rate.map_or("Device default".to_string(), |r| format!("{r} Hz"));
            egui::ComboBox::from_id_salt("pref_rate").width(160.0).selected_text(shown).show_ui(ui, |ui| {
                ui.selectable_value(&mut e.sample_rate, None, "Device default");
                for r in rates {
                    ui.selectable_value(&mut e.sample_rate, Some(r), format!("{r} Hz"));
                }
            });
            ui.end_row();

            ui.label("Buffer size");
            let shown = e.buffer_frames.map_or("Driver default".to_string(), |b| format!("{b} frames"));
            egui::ComboBox::from_id_salt("pref_buffer").width(160.0).selected_text(shown).show_ui(ui, |ui| {
                ui.selectable_value(&mut e.buffer_frames, None, "Driver default");
                for b in BUFFERS {
                    ui.selectable_value(&mut e.buffer_frames, Some(b), format!("{b} frames"));
                }
            });
            ui.end_row();

            ui.label("Skip-back length");
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut e.skipback_minutes, 1..=15).suffix(" min"));
                let rate = e.sample_rate.unwrap_or(app.shared.sample_rate) as f32;
                // Two 32-bit channels per frame.
                let mb = e.skipback_minutes as f32 * 60.0 * rate * 8.0 / 1.0e6;
                ui.label(egui::RichText::new(format!("about {mb:.0} MB")).small().color(theme::DIM));
            });
            ui.end_row();
        });
        ui.label(
            egui::RichText::new(format!("Running now: {} at {} Hz", app.device_name, app.shared.sample_rate))
                .small()
                .color(theme::DIM),
        );
        ui.add_space(8.0);

        theme::title(ui, "INTERFACE", theme::AMBER);
        ui.horizontal(|ui| {
            ui.label("Scale");
            ui.add(egui::Slider::new(&mut e.ui_scale, 0.7..=1.6).step_by(0.05).fixed_decimals(2));
            if ui.button("Reset").clicked() {
                e.ui_scale = 1.0;
            }
        });
        ui.add_space(8.0);

        theme::title(ui, "VST3 FOLDERS", theme::AMBER);
        ui.label(egui::RichText::new("The standard VST3 folders are always scanned. Add others here.").small().color(theme::DIM));
        let mut remove = None;
        for (i, dir) in e.vst3_dirs.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
                ui.label(dir.display().to_string());
            });
        }
        if let Some(i) = remove {
            e.vst3_dirs.remove(i);
        }
        if ui.button("Add folder…").clicked()
            && let Some(dir) = rfd::FileDialog::new().set_title("Folder containing VST3 plugins").pick_folder()
            && !e.vst3_dirs.contains(&dir)
        {
            e.vst3_dirs.push(dir);
        }
        ui.add_space(10.0);
        ui.separator();

        let needs_restart = app.prefs_edit.needs_restart(&app.prefs);
        if needs_restart {
            ui.label(
                egui::RichText::new("Audio device, sample rate, buffer and skip-back length take effect after a restart.")
                    .small()
                    .color(theme::AMBER),
            );
        }
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                app.apply_prefs(ui.ctx());
            }
            if needs_restart
                && ui
                    .button("Save and restart")
                    .on_hover_text("Closes and reopens ResampleOS. Save your project first: unsaved work is lost.")
                    .clicked()
            {
                app.apply_prefs(ui.ctx());
                restart = true;
            }
            if ui.button("Cancel").clicked() {
                app.prefs_edit = app.prefs.clone();
                app.show_prefs = false;
            }
        });
    });
    if !open {
        app.prefs_edit = app.prefs.clone();
        app.show_prefs = false;
    }
    if restart {
        match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).spawn()) {
            Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(e) => app.status = format!("Could not restart: {e}"),
        }
    }
}
