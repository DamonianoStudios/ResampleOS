//! Floating windows: Key/BPM harmonizer, choke group matrix, shortcut help.

use super::theme;
use crate::app::{App, ConformMode};
use crate::engine::Cmd;
use crate::music::{Key, NOTE_NAMES, Scale, bank_name, note_name, pad_label};
use eframe::egui::{self, Sense, Stroke, StrokeKind, vec2};

pub fn draw(app: &mut App, ctx: &egui::Context) {
    harmonizer(app, ctx);
    choke_matrix(app, ctx);
    help(app, ctx);
}

fn harmonizer(app: &mut App, ctx: &egui::Context) {
    if !app.harm.open {
        return;
    }
    let sel = app.selected;
    let mut open = true;
    egui::Window::new(format!("Key / BPM Harmonizer — {}", pad_label(sel)))
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            let Some(a) = app.pads[sel].analysis.clone() else {
                ui.label("No analysis for this pad.");
                return;
            };
            ui.label(egui::RichText::new("DETECTED").small().color(theme::DIM));
            egui::Grid::new("harm_detected").num_columns(2).show(ui, |ui| {
                ui.label("Tempo");
                if a.bpm > 0.0 {
                    ui.label(format!("{:.1} BPM   confidence {:.0}%", a.bpm, a.bpm_conf * 100.0));
                } else {
                    ui.label("none (one-shot or too sparse)");
                }
                ui.end_row();
                ui.label("Key");
                ui.label(format!(
                    "{} {}   confidence {:.0}%",
                    NOTE_NAMES[a.key_root as usize],
                    if a.key_minor { "minor" } else { "major" },
                    a.key_conf * 100.0
                ));
                ui.end_row();
                ui.label("Pitch");
                match a.pitch {
                    Some(p) => {
                        let cents = (p - p.round()) * 100.0;
                        ui.label(format!("{} {cents:+.0} ct (monophonic)", note_name(p.round() as i32)))
                    }
                    None => ui.label("polyphonic / unpitched"),
                };
                ui.end_row();
            });
            // The pitch class profile the key was read from.
            let (rect, _) = ui.allocate_exact_size(vec2(300.0, 46.0), Sense::hover());
            let p = ui.painter_at(rect);
            let max = a.pcp.iter().fold(1e-6f32, |m, v| m.max(*v));
            for pc in 0..12 {
                let x = rect.left() + pc as f32 * 25.0;
                let h = a.pcp[pc] / max * 30.0;
                let in_key = app.key.contains(pc as i32 + app.conform_semis(sel).round() as i32);
                let bar = egui::Rect::from_min_max(egui::pos2(x + 2.0, rect.top() + 32.0 - h), egui::pos2(x + 22.0, rect.top() + 32.0));
                p.rect_filled(bar, 1.0, if in_key { theme::CYAN } else { theme::LINE });
                p.text(egui::pos2(x + 12.0, rect.bottom()), egui::Align2::CENTER_BOTTOM, NOTE_NAMES[pc], egui::FontId::proportional(9.0), theme::DIM);
            }
            ui.label(egui::RichText::new("Highlighted: notes that land in the project scale after the shift.").small().color(theme::DIM));
            ui.separator();

            ui.label(egui::RichText::new("SOURCE (edit if the detection is off)").small().color(theme::DIM));
            ui.horizontal(|ui| {
                ui.checkbox(&mut app.harm.tempo, "Tempo");
                ui.add_enabled(app.harm.tempo, egui::DragValue::new(&mut app.harm.bpm).speed(0.1).range(30.0..=300.0).fixed_decimals(1).suffix(" BPM"));
                if ui.small_button("÷2").clicked() {
                    app.harm.bpm = (app.harm.bpm / 2.0).max(30.0);
                }
                if ui.small_button("×2").clicked() {
                    app.harm.bpm = (app.harm.bpm * 2.0).min(300.0);
                }
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut app.harm.key, "Key");
                egui::ComboBox::from_id_salt("harm_root")
                    .width(44.0)
                    .selected_text(NOTE_NAMES[app.harm.root as usize])
                    .show_ui(ui, |ui| {
                        for (i, n) in NOTE_NAMES.iter().enumerate() {
                            ui.selectable_value(&mut app.harm.root, i as u8, *n);
                        }
                    });
                ui.selectable_value(&mut app.harm.minor, false, "major");
                ui.selectable_value(&mut app.harm.minor, true, "minor");
            });
            ui.horizontal(|ui| {
                ui.selectable_value(&mut app.conform_mode, ConformMode::Strict, "Strict Chromatic");
                ui.selectable_value(&mut app.conform_mode, ConformMode::Smart, "Smart Conform");
                ui.checkbox(&mut app.preserve_transients, "Lock transients");
            });
            ui.separator();

            let ratio = app.conform_ratio();
            let semis = app.conform_semis(sel);
            ui.label(format!("Project: {:.1} BPM, {}", app.bpm, app.key.name()));
            ui.label(
                egui::RichText::new(format!(
                    "Result: length ×{ratio:.3} ({}), pitch {semis:+.0} st",
                    if ratio > 1.0 { "slower" } else if ratio < 1.0 { "faster" } else { "unchanged" }
                ))
                .color(theme::AMBER)
                .strong(),
            );
            ui.horizontal(|ui| {
                if ui.button("Conform to project (bake)  Enter").clicked() {
                    app.conform(sel);
                    app.harm.open = false;
                }
                if ui.button("Set project to this sample").clicked() {
                    if app.harm.tempo {
                        app.bpm = app.harm.bpm;
                        app.send(Cmd::SetBpm(app.bpm));
                    }
                    if app.harm.key {
                        let scale = if app.harm.minor { Scale::Minor } else { Scale::Major };
                        app.key = Key { root: app.harm.root, scale };
                    }
                    app.harm.open = false;
                }
            });
        });
    if !open {
        app.harm.open = false;
    }
}

fn choke_matrix(app: &mut App, ctx: &egui::Context) {
    if !app.show_choke {
        return;
    }
    let mut open = true;
    egui::Window::new(format!("Choke Group Matrix — Bank {}", bank_name(app.bank)))
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new("Pads in the same group cut each other off. Click a cell to assign; click again to clear.")
                    .small()
                    .color(theme::DIM),
            );
            let cell = 15.0;
            ui.horizontal(|ui| {
                ui.add_space(86.0);
                for g in 1..=32 {
                    let (r, _) = ui.allocate_exact_size(vec2(cell, 12.0), Sense::hover());
                    if g == 1 || g % 4 == 0 {
                        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, g.to_string(), egui::FontId::proportional(9.0), theme::DIM);
                    }
                }
            });
            ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
            for i in (0..16).rev() {
                let idx = app.bank * 16 + i;
                ui.horizontal(|ui| {
                    let name: String = app.pads[idx].name.chars().take(8).collect();
                    ui.add_sized([80.0, cell], egui::Label::new(egui::RichText::new(format!("{} {name}", pad_label(idx))).small()));
                    for g in 1..=32u8 {
                        let (r, resp) = ui.allocate_exact_size(vec2(cell, cell), Sense::click());
                        let on = app.pads[idx].cfg.choke == g;
                        let p = ui.painter();
                        p.rect_filled(r.shrink(1.0), 2.0, if on { theme::PURPLE } else { theme::RAISED });
                        if resp.hovered() {
                            p.rect_stroke(r.shrink(1.0), 2.0, Stroke::new(1.0, theme::TEXT), StrokeKind::Inside);
                        }
                        if resp.clicked() {
                            app.pads[idx].cfg.choke = if on { 0 } else { g };
                            app.push_pad(idx);
                        }
                    }
                });
            }
        });
    app.show_choke = open;
}

fn help(app: &mut App, ctx: &egui::Context) {
    if !app.show_help {
        return;
    }
    let mut open = true;
    egui::Window::new("Shortcuts").open(&mut open).resizable(false).show(ctx, |ui| {
        let rows: &[(&str, &str)] = &[
            ("Space", "Start / stop transport"),
            ("R", "Resample: arm, hit target pad, perform, R to bake"),
            ("S", "Skip-back: last 16 bars to the selected pad (next empty pad if it is in use)"),
            ("Tab", "Waveform <-> Piano Roll"),
            ("1-4, 5", "Focus Bus 1-4, Master"),
            ("M + pad", "Mute pad"),
            ("Q + pad", "Stop that pad's playback"),
            ("Backspace", "Stop the selected pad's playback"),
            ("G", "Step sequencer for the current bank"),
            ("Shift + pad", "Select without triggering"),
            ("Ctrl + K", "Key / BPM harmonizer"),
            ("F2", "VST3 browser: load an instrument on the selected pad"),
            ("Ctrl + ,", "Preferences: audio device, sample rate, buffer, scale, plugin folders"),
            ("C", "Transient slice to the lowest empty bank"),
            ("[  ]", "Octave down / up"),
            ("X", "Chromatic: spread selected pad over the 16 pads"),
            ("T", "Roll (note repeat)"),
            ("Left / Right", "Previous / next bank"),
            ("Ctrl + Z / Ctrl + Y", "Undo / redo any project edit"),
            ("Ctrl + Shift + Z / Y", "Step this pad's take stack back / forward"),
            ("Ctrl + C / V", "Copy / paste pad"),
            ("Ctrl + S / O", "Save / open project"),
            ("Delete", "Clear pad (or pattern, in the piano roll)"),
            ("Esc", "Cancel selection / arm, silence all voices"),
        ];
        egui::Grid::new("help_grid").num_columns(2).striped(true).show(ui, |ui| {
            for (k, v) in rows {
                ui.label(egui::RichText::new(*k).monospace().color(theme::AMBER));
                ui.label(*v);
                ui.end_row();
            }
        });
        ui.separator();
        ui.label("Pads on the computer keyboard (bottom row = pads 1-4):");
        ui.monospace("  6 7 8 9\n  Y U I O\n  H J K L\n  N , . /");
        ui.separator();
        ui.label("MIDI: notes 36-51 = pads · other notes play the selected pad melodically");
        ui.label("CC 70/71/72 = focused FX slot knobs · CC 20/21 bank -/+ · 22 resample · 23 skip-back · 24 roll · 25 chromatic");
        if !app.midi_ports.is_empty() {
            ui.label(egui::RichText::new(format!("Connected: {}", app.midi_ports.join(", "))).color(theme::CYAN));
        } else {
            ui.label(egui::RichText::new("No MIDI inputs found at startup.").color(theme::DIM));
        }
    });
    app.show_help = open;
}
