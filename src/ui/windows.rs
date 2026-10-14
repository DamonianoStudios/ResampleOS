//! Floating windows: Key/BPM harmonizer, choke group matrix, shortcut help.

use super::theme;
use crate::app::{App, ConformMode};
use crate::engine::Cmd;
use crate::music::{Key, NOTE_NAMES, Scale, bank_name, note_name, pad_label};
use eframe::egui::{self, Sense, Stroke, StrokeKind, vec2};

pub fn draw(app: &mut App, ctx: &egui::Context) {
    confirm_new(app, ctx);
    super::buses::analyzers_window(app, ctx);
    harmonizer(app, ctx);
    choke_matrix(app, ctx);
    help(app, ctx);
}

/// Asks before File > New throws away unsaved changes.
fn confirm_new(app: &mut App, ctx: &egui::Context) {
    if !app.confirm_new {
        return;
    }
    let modal = egui::Modal::new(egui::Id::new("confirm_new")).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.heading("Start a new project?");
        ui.add_space(4.0);
        ui.label("This project has changes that have not been saved.");
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Save first").clicked() {
                crate::project::save_dialog(app);
                // A cancelled or failed save leaves the project as it was.
                if !app.has_unsaved_changes() {
                    crate::project::new_project(app);
                }
                app.confirm_new = false;
            }
            if ui.button("Discard changes").clicked() {
                crate::project::new_project(app);
                app.confirm_new = false;
            }
            if ui.button("Cancel").clicked() {
                app.confirm_new = false;
            }
        });
    });
    if modal.should_close() {
        app.confirm_new = false;
    }
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
            ui.horizontal(|ui| {
                ui.label("Method");
                for mode in crate::stretch::WarpMode::ALL {
                    ui.selectable_value(&mut app.warp_mode, mode, mode.name()).on_hover_text(mode.hint());
                }
                if app.warp_mode == crate::stretch::WarpMode::Elastic {
                    ui.checkbox(&mut app.warp_formants, "Keep formants");
                }
            });
            ui.separator();

            let (ratio, semis) = app.conform_plan(sel);
            ui.label(format!("Project: {:.1} BPM, {}", app.bpm, app.key.name()));
            ui.label(
                egui::RichText::new(format!(
                    "Result: length ×{ratio:.3} ({}), pitch {semis:+.1} st",
                    if ratio > 1.0 { "slower" } else if ratio < 1.0 { "faster" } else { "unchanged" }
                ))
                .color(theme::AMBER)
                .strong(),
            );
            if app.warp_mode == crate::stretch::WarpMode::Varispeed && app.harm.tempo && app.harm.key {
                ui.label(
                    egui::RichText::new("Varispeed ties pitch to tempo: the tempo is matched and the pitch follows, so the key is not conformed.")
                        .small()
                        .color(theme::DIM),
                );
            }
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
                egui::RichText::new("Pads in the same group cut each other off. Click or drag across cells to assign; start on a lit cell to clear.")
                    .small()
                    .color(theme::DIM),
            );
            let cell = 15.0;
            let (pressed, down, pointer) =
                ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.interact_pos()));
            if !down {
                app.choke_paint = None;
            }
            ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
            ui.horizontal(|ui| {
                ui.add_space(86.0);
                for g in 1..=32 {
                    let (r, _) = ui.allocate_exact_size(vec2(cell, 12.0), Sense::hover());
                    if g == 1 || g % 4 == 0 {
                        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, g.to_string(), egui::FontId::proportional(9.0), theme::DIM);
                    }
                }
            });
            for i in (0..16).rev() {
                let idx = app.bank * 16 + i;
                ui.horizontal(|ui| {
                    let name: String = app.pads[idx].name.chars().take(8).collect();
                    ui.add_sized([80.0, cell], egui::Label::new(egui::RichText::new(format!("{} {name}", pad_label(idx))).small()));
                    for g in 1..=32u8 {
                        // Sensing drags keeps a paint stroke from moving the window instead.
                        let (r, resp) = ui.allocate_exact_size(vec2(cell, cell), Sense::click_and_drag());
                        let mut on = app.pads[idx].cfg.choke == g;
                        // Cover the gaps between cells, or a stroke would flicker across them.
                        let under = pointer.is_some_and(|pos| r.expand2(vec2(3.0, 9.0)).contains(pos));
                        if pressed && resp.contains_pointer() {
                            app.choke_paint = Some(!on);
                        }
                        if let (true, Some(paint)) = (under, app.choke_paint)
                            && paint != on
                        {
                            app.pads[idx].cfg.choke = if paint { g } else { 0 };
                            app.push_pad(idx);
                            on = paint;
                        }
                        let p = ui.painter();
                        p.rect_filled(r.shrink(1.0), 2.0, if on { theme::PURPLE } else { theme::RAISED });
                        if resp.hovered() || under && app.choke_paint.is_some() {
                            p.rect_stroke(r.shrink(1.0), 2.0, Stroke::new(1.0, theme::TEXT), StrokeKind::Inside);
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
            ("Shift + Space", "Stop All: transport, every pad and the effect tails"),
            ("R", "Resample: arm, hit target pad, perform, R to bake"),
            ("S", "Skip-back: last 16 bars to the selected pad (next empty pad if it is in use)"),
            ("Tab", "Waveform <-> Piano Roll"),
            ("1-4, 5", "Focus Bus 1-4, Master"),
            ("M + pad", "Mute pad"),
            ("Q + pad", "Stop that pad's playback"),
            ("Backspace", "Stop the selected pad's playback"),
            ("G", "Step sequencer for the current bank"),
            ("P", "Perform mode: pad keys and controller pads launch loops"),
            ("Ctrl + B / Ctrl + M", "Drum pad: bake the hit to a take / mutate and bake"),
            ("Alt + click a drum step", "Lock knob values to that step"),
            ("Shift + R", "Vocal pad: arm, then Space records a take each time round the loop"),
            ("Vocal tab: C / D / X", "Comp, Move and Cut tools. G steps the snap grid"),
            ("Vocal tab: M, Backspace, Ctrl + D", "Mute, delete or repeat the selected clip"),
            ("Vocal tab: Shift + pad key", "Step the comp to the next take for that 16th of the loop"),
            ("Vocal tab: Ctrl + B", "Bake the comp to a pad (with Shift: phrases to a bank)"),
            ("Shift + click a loop", "Add it to the chain"),
            ("Shift + pad", "Select without triggering"),
            ("Ctrl + K", "Key / BPM harmonizer"),
            ("F2", "Plugin browser (VST3 and CLAP): load an instrument on the selected pad"),
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
            ("Delete", "Clear pad (in the piano roll: the selected notes, or the pattern)"),
            ("Piano roll: Shift + drag", "Box-select notes (Ctrl + A selects all)"),
            ("Piano roll: Ctrl + drag", "Duplicate the selected notes (Ctrl + D repeats them)"),
            ("Piano roll: Shift + Up / Down", "Selected notes up / down an octave"),
            ("Piano roll: arrows", "Selected notes up / down a step, left / right a grid step"),
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
