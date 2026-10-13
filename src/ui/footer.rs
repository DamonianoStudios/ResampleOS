//! Footer: live routing toggles, choke matrix, conform settings and master metering.

use super::{db, theme, toggle};
use crate::app::{App, ConformMode, ROLL_DIVS};
use crate::engine::Cmd;
use crate::shared::{MASTER, f32_load};
use eframe::egui::{self, Ui};

pub fn draw(app: &mut App, ui: &mut Ui) {
    ui.horizontal_centered(|ui| {
        theme::caption(ui, "CAPTURE");
        let mut src = app.ring_src;
        egui::ComboBox::from_id_salt("ring_src")
            .width(70.0)
            .selected_text(src_name(src))
            .show_ui(ui, |ui| {
                for s in 0..=5u8 {
                    ui.selectable_value(&mut src, s, src_name(s));
                }
            })
            .response
            .on_hover_text("What the skip-back buffer and Resample record");
        if src != app.ring_src {
            app.ring_src = src;
            app.send(Cmd::RingSource(src));
        }
        ui.separator();

        input_controls(app, ui);

        if toggle(ui, app.roll_on, "Roll", theme::AMBER).on_hover_text("T: note repeat while a pad is held").clicked() {
            app.toggle_roll();
        }
        let mut div = app.roll_div;
        egui::ComboBox::from_id_salt("roll_div").width(54.0).selected_text(ROLL_DIVS[div].0).show_ui(ui, |ui| {
            for (i, (name, _)) in ROLL_DIVS.iter().enumerate() {
                ui.selectable_value(&mut div, i, *name);
            }
        });
        if div != app.roll_div {
            app.roll_div = div;
            app.send(Cmd::Roll { on: app.roll_on, ticks: ROLL_DIVS[div].1 });
        }
        if toggle(ui, app.chromatic, "Chromatic", theme::CYAN)
            .on_hover_text("X: spread the selected pad across the 16 pads")
            .clicked()
        {
            app.chromatic ^= true;
        }
        ui.label(format!("Oct {:+}", app.octave)).on_hover_text("[ and ]");
        ui.separator();

        if toggle(ui, app.show_choke, "Choke Matrix", theme::PURPLE).clicked() {
            app.show_choke ^= true;
        }
        ui.separator();

        theme::caption(ui, "CONFORM");
        egui::ComboBox::from_id_salt("conform_mode")
            .width(112.0)
            .selected_text(mode_name(app.conform_mode))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut app.conform_mode, ConformMode::Strict, mode_name(ConformMode::Strict));
                ui.selectable_value(&mut app.conform_mode, ConformMode::Smart, mode_name(ConformMode::Smart));
            })
            .response
            .on_hover_text("Strict: exact semitones. Smart: land on scale degrees (also shapes Chromatic mode).");
        ui.checkbox(&mut app.auto_conform, "Auto on import");

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let s = &app.shared;
            let peak = f32_load(&s.bus_peak[MASTER * 2]).max(f32_load(&s.bus_peak[MASTER * 2 + 1]));
            let fmt = |v: f32| if v < -69.0 { "-inf".to_string() } else { format!("{v:5.1}") };
            let peak_col = if peak >= 1.0 { theme::RED } else { theme::TEXT };
            ui.label(egui::RichText::new(format!("LUFS S {}", fmt(f32_load(&s.lufs_short)))).monospace());
            ui.label(egui::RichText::new(format!("M {}", fmt(f32_load(&s.lufs_momentary)))).monospace());
            ui.label(egui::RichText::new(format!("PEAK {} dB", fmt(db(peak)))).monospace().color(peak_col));
            let load = f32_load(&s.load) * 100.0;
            ui.label(egui::RichText::new(format!("DSP {load:3.0}%")).monospace().color(theme::DIM));
            if app.busy > 0 {
                ui.spinner();
            }
            ui.separator();
            ui.add(egui::Label::new(egui::RichText::new(&app.status).color(theme::DIM)).truncate());
        });
    });
}

fn src_name(s: u8) -> String {
    match s {
        0 => "Master".into(),
        5 => "Input".into(),
        s => format!("Bus {s}"),
    }
}

fn dest_name(d: u8) -> String {
    match d {
        0 => "Muted".into(),
        1 => "Thru".into(),
        d => format!("Bus {}", d - 1),
    }
}

/// External input: device, where it is monitored, gain and a level readout.
fn input_controls(app: &mut App, ui: &mut Ui) {
    theme::caption(ui, "INPUT");
    let current = app.input_sel.and_then(|i| app.input_names.get(i).cloned()).unwrap_or_else(|| "Off".into());
    let short: String = current.chars().take(18).collect();
    let mut pick: Option<Option<usize>> = None;
    egui::ComboBox::from_id_salt("input_dev").width(128.0).selected_text(short).show_ui(ui, |ui| {
        if ui.selectable_label(app.input_sel.is_none(), "Off").clicked() {
            pick = Some(None);
        }
        for (i, name) in app.input_names.iter().enumerate() {
            if ui.selectable_label(app.input_sel == Some(i), name).clicked() {
                pick = Some(Some(i));
            }
        }
    });
    if let Some(sel) = pick {
        app.set_input(sel);
    }
    if app.input_sel.is_some() {
        let (mut dest, mut gain) = (app.input_dest, app.input_gain);
        egui::ComboBox::from_id_salt("input_dest")
            .width(58.0)
            .selected_text(dest_name(dest))
            .show_ui(ui, |ui| {
                for d in 0..=5u8 {
                    ui.selectable_value(&mut dest, d, dest_name(d));
                }
            })
            .response
            .on_hover_text("Where the input is heard. Muted still records when Capture is set to Input.");
        ui.add(egui::DragValue::new(&mut gain).speed(0.01).range(0.0..=4.0).fixed_decimals(2).prefix("x"));
        if dest != app.input_dest || gain != app.input_gain {
            app.input_dest = dest;
            app.input_gain = gain;
            app.send(Cmd::InputCfg { dest, gain });
        }
        let peak = f32_load(&app.shared.input_peak);
        let col = if peak >= 1.0 { theme::RED } else if peak > 0.003 { theme::GREEN } else { theme::DIM };
        ui.label(egui::RichText::new(format!("{:5.1} dB", db(peak).max(-99.0))).monospace().color(col));
    }
    ui.separator();
}

fn mode_name(m: ConformMode) -> &'static str {
    match m {
        ConformMode::Strict => "Strict Chromatic",
        ConformMode::Smart => "Smart Conform",
    }
}
