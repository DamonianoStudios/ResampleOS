//! Flyout piano roll for MIDI pads: scale-locked rows, velocity lane, free microtiming.

use super::{humanize, knob, theme, toggle};
use crate::app::{App, PluginTarget, RollDrag, SNAPS};
use crate::engine::{Cmd, MidiTarget, Note, PadKind, PlayMode};
use crate::music::{PPQ, TICKS_PER_BAR, is_black_key, note_name, pad_label};
use crate::synth::SynthEngine;
use eframe::egui::{self, Align2, Color32, FontId, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, vec2};

const KEY_W: f32 = 46.0;
const ROW_H: f32 = 13.0;
const VEL_H: f32 = 54.0;
const HEAD_H: f32 = 14.0;
const AUDITION_KEY: u32 = 0x3000;

pub fn draw(app: &mut App, ui: &mut Ui) {
    let sel = app.selected;
    if app.pads[sel].cfg.kind != PadKind::Midi {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("This is an audio pad").size(16.0).color(theme::DIM));
            ui.label("A piano roll needs a MIDI pad. It can drive the internal synth, or play another pad's sample chromatically.");
            if app.pads[sel].is_empty() {
                if ui.button("Make this a MIDI pad").clicked() {
                    app.set_kind(sel, PadKind::Midi);
                }
            } else {
                ui.label(egui::RichText::new("Pick an empty pad, then press Tab.").color(theme::DIM));
            }
        });
        return;
    }
    toolbar(app, ui);
    grid(app, ui);
}

fn toolbar(app: &mut App, ui: &mut Ui) {
    let sel = app.selected;
    let before = app.pads[sel].cfg;
    let mut cfg = before;
    let mut pattern_changed = false;

    ui.horizontal_wrapped(|ui| {
        ui.label("Engine");
        let mut browse = false;
        let label = match (&app.pads[sel].plugin, cfg.target) {
            (Some(p), MidiTarget::Plugin) => format!("VST3: {}", p.name),
            _ => app.midi_target_label(cfg.target),
        };
        egui::ComboBox::from_id_salt("roll_target").width(150.0).selected_text(label).show_ui(ui, |ui| {
            ui.selectable_value(&mut cfg.target, MidiTarget::Synth, "Internal synth");
            if ui.selectable_label(cfg.target == MidiTarget::Plugin, "VST3 instrument…").clicked() {
                browse = true;
            }
            for i in 0..app.pads.len() {
                if i != sel && app.pads[i].cfg.kind == PadKind::Audio && app.pads[i].sample().is_some() {
                    let text = format!("Pad {} {}", pad_label(i), app.pads[i].name);
                    ui.selectable_value(&mut cfg.target, MidiTarget::Pad(i as u16), text);
                }
            }
        });
        if browse {
            app.open_browser(PluginTarget::Pad(sel));
        }
        if cfg.target == MidiTarget::Plugin {
            if app.pads[sel].plugin.is_some() {
                if ui.button("Open editor").clicked() {
                    app.open_editor(PluginTarget::Pad(sel));
                }
                if ui.button("Change…").on_hover_text("F2").clicked() {
                    app.open_browser(PluginTarget::Pad(sel));
                }
                if ui.button("Remove").clicked() {
                    app.remove_plugin(PluginTarget::Pad(sel));
                    cfg.target = MidiTarget::Synth;
                }
            } else {
                ui.label(egui::RichText::new("plugin not loaded").color(theme::RED));
                if ui.button("Choose…").clicked() {
                    app.open_browser(PluginTarget::Pad(sel));
                }
            }
        }
        if cfg.target == MidiTarget::Synth {
            egui::ComboBox::from_id_salt("roll_engine")
                .width(96.0)
                .selected_text(cfg.synth.engine.name())
                .show_ui(ui, |ui| {
                    for e in SynthEngine::ALL {
                        if ui.selectable_value(&mut cfg.synth.engine, e, e.name()).clicked() {
                            cfg.synth.octave = e.default_octave();
                        }
                    }
                });
            knob(ui, &mut cfg.synth.decay, "Decay", 0.5, theme::CYAN);
            knob(ui, &mut cfg.synth.tone, "Tone", 0.5, theme::CYAN);
            knob(ui, &mut cfg.synth.drive, "Drive", 0.3, theme::CYAN);
            ui.label("Oct");
            ui.add(egui::DragValue::new(&mut cfg.synth.octave).range(-4..=4).speed(0.05));
        }
        ui.separator();
        ui.label("Bars");
        let mut bars = app.pads[sel].pattern.len_ticks / TICKS_PER_BAR;
        egui::ComboBox::from_id_salt("roll_bars").width(40.0).selected_text(bars.to_string()).show_ui(ui, |ui| {
            for b in [1, 2, 4, 8, 16] {
                ui.selectable_value(&mut bars, b, b.to_string());
            }
        });
        if bars * TICKS_PER_BAR != app.pads[sel].pattern.len_ticks {
            let pat = &mut app.pads[sel].pattern;
            pat.len_ticks = bars * TICKS_PER_BAR;
            pat.notes.retain(|n| n.start < pat.len_ticks);
            pattern_changed = true;
        }
        ui.label("Snap");
        egui::ComboBox::from_id_salt("roll_snap")
            .width(56.0)
            .selected_text(SNAPS[app.roll.snap].0)
            .show_ui(ui, |ui| {
                for (i, (name, _)) in SNAPS.iter().enumerate() {
                    ui.selectable_value(&mut app.roll.snap, i, *name);
                }
            })
            .response
            .on_hover_text("Off = free microtiming. Hold Alt while dragging to bypass snap.");
        ui.checkbox(&mut app.roll.scale_lock, "Scale lock").on_hover_text(format!("Grey out and block notes outside {}", app.key.name()));
        ui.checkbox(&mut app.roll.ghosts, "Ghosts");
        humanize(ui, &mut cfg.human_time, &mut cfg.human_vel);
        if toggle(ui, app.roll.rec, "Rec", theme::RED)
            .on_hover_text("While playing, pad and keyboard notes are written into this pattern")
            .clicked()
        {
            app.roll.rec ^= true;
        }
        if ui.button("Quantize").clicked() {
            let snap = SNAPS[app.roll.snap].1.max(1);
            let pat = &mut app.pads[sel].pattern;
            for n in pat.notes.iter_mut() {
                n.start = ((n.start + snap / 2) / snap * snap) % pat.len_ticks.max(1);
            }
            pattern_changed = true;
        }
        if ui.button("Clear").clicked() {
            app.pads[sel].pattern.notes.clear();
            pattern_changed = true;
        }
    });
    if cfg != before {
        // Leaving the plugin target by hand unloads the instrument.
        if before.target == MidiTarget::Plugin && cfg.target != MidiTarget::Plugin && app.pads[sel].plugin.is_some() {
            app.remove_plugin(PluginTarget::Pad(sel));
        }
        app.pads[sel].cfg = cfg;
        app.push_pad(sel);
    }
    if pattern_changed {
        app.push_pattern(sel);
    }
}

fn grid(app: &mut App, ui: &mut Ui) {
    let sel = app.selected;
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    if rect.width() < KEY_W + 40.0 || rect.height() < VEL_H + HEAD_H + 30.0 {
        return;
    }
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, theme::BG);
    let grid = Rect::from_min_max(rect.min + vec2(KEY_W, HEAD_H), rect.max - vec2(0.0, VEL_H));
    let lane = Rect::from_min_max(Pos2::new(grid.left(), grid.bottom()), rect.max);

    if resp.hovered() {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        app.roll.top_f = (app.roll.top_f + dy / ROW_H).clamp(12.0, 127.0);
    }
    let top = app.roll.top_f.round() as i32;
    let rows = (grid.height() / ROW_H).ceil() as i32;
    let len = app.pads[sel].pattern.len_ticks.max(1);
    let x_of = |t: f32| grid.left() + t / len as f32 * grid.width();
    let y_of = |pitch: i32| grid.top() + (top - pitch) as f32 * ROW_H;
    let tick_at = |x: f32| ((x - grid.left()) / grid.width() * len as f32) as i64;
    let pitch_at = |y: f32| top - ((y - grid.top()) / ROW_H).floor() as i32;
    let key = app.key;
    let lock = app.roll.scale_lock;

    // Rows and the keyboard strip. Out-of-scale rows are greyed out when the lock is on.
    for r in 0..rows {
        let pitch = top - r;
        let y = grid.top() + r as f32 * ROW_H;
        let row = Rect::from_min_max(Pos2::new(grid.left(), y), Pos2::new(grid.right(), (y + ROW_H).min(grid.bottom())));
        let in_scale = key.contains(pitch);
        let is_root = (pitch - key.root as i32).rem_euclid(12) == 0;
        let fill = if lock && !in_scale {
            Color32::from_rgb(11, 12, 15)
        } else if is_root {
            Color32::from_rgb(38, 36, 30)
        } else if is_black_key(pitch) {
            Color32::from_rgb(24, 27, 33)
        } else {
            Color32::from_rgb(30, 34, 41)
        };
        p.rect_filled(row, 0.0, fill);
        p.hline(grid.x_range(), y, Stroke::new(1.0, theme::BG));

        let keyr = Rect::from_min_max(Pos2::new(rect.left(), y), Pos2::new(grid.left() - 1.0, (y + ROW_H - 1.0).min(grid.bottom())));
        let kcol = if is_black_key(pitch) { Color32::from_rgb(20, 20, 24) } else { Color32::from_rgb(206, 208, 214) };
        p.rect_filled(keyr, 1.0, if lock && !in_scale { kcol.gamma_multiply(0.35) } else { kcol });
        if pitch.rem_euclid(12) == 0 || is_root {
            let tcol = if is_black_key(pitch) { theme::TEXT } else { theme::BG };
            p.text(keyr.right_center() - vec2(3.0, 0.0), Align2::RIGHT_CENTER, note_name(pitch), FontId::proportional(9.0), tcol);
        }
    }

    // Time grid.
    let snap = SNAPS[app.roll.snap].1;
    let px_per_tick = grid.width() / len as f32;
    if snap > 1 && snap as f32 * px_per_tick >= 5.0 {
        let mut t = 0;
        while t < len {
            p.vline(x_of(t as f32), grid.y_range(), Stroke::new(1.0, Color32::from_rgb(20, 22, 27)));
            t += snap;
        }
    }
    let mut t = 0;
    while t <= len {
        let bar = t % TICKS_PER_BAR == 0;
        let col = if bar { theme::DIM } else { theme::LINE };
        p.vline(x_of(t as f32), grid.top() - if bar { HEAD_H } else { 4.0 }..=rect.bottom(), Stroke::new(1.0, col));
        if bar && t < len {
            p.text(
                Pos2::new(x_of(t as f32) + 3.0, rect.top() + 1.0),
                Align2::LEFT_TOP,
                (t / TICKS_PER_BAR + 1).to_string(),
                FontId::proportional(9.0),
                theme::DIM,
            );
        }
        t += PPQ;
    }

    // Ghosts: other MIDI pads' notes, and the transients of looping audio pads in this bank.
    if app.roll.ghosts {
        let bank = sel / 16;
        let tps = app.bpm / 60.0 * PPQ as f32 / app.sr();
        for i in bank * 16..bank * 16 + 16 {
            let other = &app.pads[i];
            if i == sel {
                continue;
            }
            if other.cfg.kind == PadKind::Midi && other.pattern.len_ticks > 0 {
                let olen = other.pattern.len_ticks;
                let mut base = 0;
                while base < len {
                    for n in &other.pattern.notes {
                        let (s, e) = ((base + n.start) as f32, (base + n.start + n.len) as f32);
                        let y = y_of(n.pitch as i32);
                        if s >= len as f32 || y < grid.top() || y + ROW_H > grid.bottom() {
                            continue;
                        }
                        let r = Rect::from_min_max(Pos2::new(x_of(s), y + 2.0), Pos2::new(x_of(e.min(len as f32)), y + ROW_H - 2.0));
                        p.rect_filled(r, 1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 26));
                    }
                    base += olen;
                }
            } else if other.cfg.mode == PlayMode::Loop
                && let Some(s) = other.sample() {
                    let olen = (s.len() as f32 * tps).max(1.0);
                    let mut base = 0.0;
                    while base < len as f32 {
                        for &tr in &other.transients {
                            let t = base + tr as f32 * tps;
                            if t < len as f32 {
                                let x = x_of(t);
                                p.vline(x, rect.top()..=grid.top(), Stroke::new(1.5, theme::AMBER));
                                p.vline(x, grid.y_range(), Stroke::new(1.0, Color32::from_rgba_unmultiplied(244, 168, 40, 22)));
                            }
                        }
                        base += olen;
                    }
                }
        }
    }

    // ---- Editing.
    let note_rect = |n: &Note| {
        let y = y_of(n.pitch as i32);
        Rect::from_min_max(
            Pos2::new(x_of(n.start as f32), y + 1.0),
            Pos2::new(x_of((n.start + n.len) as f32).max(x_of(n.start as f32) + 3.0), y + ROW_H - 1.0),
        )
    };
    let alt = ui.input(|i| i.modifiers.alt);
    let quant = |t: i64| -> i64 {
        if alt || snap <= 1 { t } else { (t as f64 / snap as f64).round() as i64 * snap as i64 }
    };
    let floor_q = |t: i64| -> i64 { if alt || snap <= 1 { t } else { t.div_euclid(snap as i64) * snap as i64 } };
    let fit_pitch = |pitch: i32| -> u8 {
        let pitch = if lock { key.snap(pitch) } else { pitch };
        pitch.clamp(0, 127) as u8
    };
    let hit = |pos: Pos2, notes: &[Note]| notes.iter().rposition(|n| note_rect(n).expand(1.0).contains(pos));
    let mut changed = false;
    let mut audition: Option<u8> = None;
    let origin = ui.input(|i| i.pointer.press_origin());
    let pointer = resp.interact_pointer_pos();

    let add_note = |app: &mut App, pos: Pos2| -> usize {
        let pat = &mut app.pads[sel].pattern;
        let start = floor_q(tick_at(pos.x)).clamp(0, len as i64 - 1) as u32;
        let note = Note { start, len: app.roll.last_len.max(30), pitch: fit_pitch(pitch_at(pos.y)), vel: 100 };
        pat.notes.push(note);
        pat.notes.len() - 1
    };

    if resp.drag_started_by(PointerButton::Primary)
        && let Some(o) = origin.filter(|o| grid.contains(*o)) {
            let found = hit(o, &app.pads[sel].pattern.notes);
            let (idx, resize) = match found {
                Some(i) => (i, o.x > note_rect(&app.pads[sel].pattern.notes[i]).right() - 6.0),
                None => {
                    changed = true;
                    (add_note(app, o), false)
                }
            };
            let orig = app.pads[sel].pattern.notes[idx];
            audition = Some(orig.pitch);
            app.roll.drag = Some(RollDrag { idx, resize, grab_tick: tick_at(o.x) - orig.start as i64 });
        }
    if resp.dragged_by(PointerButton::Primary) {
        if let (Some(d), Some(pos)) = (app.roll.drag, pointer)
            && let Some(n) = app.pads[sel].pattern.notes.get_mut(d.idx) {
                let was = *n;
                if d.resize {
                    let end = quant(tick_at(pos.x)).max(n.start as i64 + 30);
                    n.len = (end - n.start as i64).min(len as i64) as u32;
                } else {
                    n.start = quant(tick_at(pos.x) - d.grab_tick).clamp(0, len as i64 - 1) as u32;
                    n.pitch = fit_pitch(pitch_at(pos.y));
                    if n.pitch != was.pitch {
                        audition = Some(n.pitch);
                    }
                }
                changed |= *n != was;
            }
        // Dragging in the velocity lane paints velocities.
        if let (None, Some(pos)) = (app.roll.drag, pointer)
            && origin.is_some_and(|o| lane.contains(o)) {
                changed |= set_velocity(app, sel, pos, &x_of, lane);
            }
    }
    if resp.drag_stopped()
        && let Some(d) = app.roll.drag.take() {
            if let Some(n) = app.pads[sel].pattern.notes.get(d.idx) {
                app.roll.last_len = n.len;
            }
            changed = true;
        }
    if resp.clicked()
        && let Some(pos) = pointer {
            if grid.contains(pos) {
                match hit(pos, &app.pads[sel].pattern.notes) {
                    Some(i) => audition = Some(app.pads[sel].pattern.notes[i].pitch),
                    None => {
                        let i = add_note(app, pos);
                        audition = Some(app.pads[sel].pattern.notes[i].pitch);
                        changed = true;
                    }
                }
            } else if lane.contains(pos) {
                changed |= set_velocity(app, sel, pos, &x_of, lane);
            } else if pos.x < grid.left() && pos.y > grid.top() {
                audition = Some(pitch_at(pos.y).clamp(0, 127) as u8);
            }
        }
    if resp.secondary_clicked()
        && let Some(pos) = pointer
            && let Some(i) = hit(pos, &app.pads[sel].pattern.notes) {
                app.pads[sel].pattern.notes.remove(i);
                app.roll.drag = None;
                changed = true;
            }
    if let Some(pitch) = audition {
        // Sounds for as long as the mouse button is down.
        if let Some((pad, k)) = app.mouse_held.take() {
            app.send(Cmd::PadOff { pad, key: k });
        }
        app.send(Cmd::PadOn { pad: sel as u16, vel: 0.8, semis: pitch as f32 - 60.0, key: AUDITION_KEY });
        if ui.input(|i| i.pointer.primary_down()) {
            app.mouse_held = Some((sel as u16, AUDITION_KEY));
        } else {
            app.send(Cmd::PadOff { pad: sel as u16, key: AUDITION_KEY });
        }
    }
    if changed {
        app.push_pattern(sel);
    }

    // ---- Notes, velocities, playhead.
    let active = app.roll.drag.map(|d| d.idx);
    for (i, n) in app.pads[sel].pattern.notes.iter().enumerate() {
        let r = note_rect(n).intersect(grid);
        if r.height() >= 2.0 && r.width() > 0.0 {
            let v = n.vel as f32 / 127.0;
            p.rect_filled(r, 2.0, theme::CYAN.gamma_multiply(0.45 + 0.55 * v));
            let edge = if active == Some(i) { theme::TEXT } else { theme::CYAN_DIM };
            p.rect_stroke(r, 2.0, Stroke::new(1.0, edge), StrokeKind::Inside);
        }
        let x = x_of(n.start as f32);
        let h = (lane.height() - 6.0) * n.vel as f32 / 127.0;
        p.vline(x + 1.0, lane.bottom() - h..=lane.bottom(), Stroke::new(3.0, theme::CYAN.gamma_multiply(0.5 + 0.5 * n.vel as f32 / 127.0)));
    }
    p.hline(rect.x_range(), lane.top(), Stroke::new(1.0, theme::LINE));
    p.text(Pos2::new(rect.left() + 4.0, lane.center().y), Align2::LEFT_CENTER, "VEL", FontId::proportional(9.0), theme::DIM);
    if app.playing() {
        let t = app.shared.tick().rem_euclid(len as f64) as f32;
        p.vline(x_of(t), rect.y_range(), Stroke::new(1.5, theme::GREEN));
    }
}

fn set_velocity(app: &mut App, sel: usize, pos: Pos2, x_of: &dyn Fn(f32) -> f32, lane: Rect) -> bool {
    let vel = (((lane.bottom() - pos.y) / (lane.height() - 6.0)).clamp(0.01, 1.0) * 127.0) as u8;
    let notes = &mut app.pads[sel].pattern.notes;
    let nearest = notes
        .iter_mut()
        .map(|n| ((x_of(n.start as f32) - pos.x).abs(), n))
        .filter(|(d, _)| *d < 8.0)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    match nearest {
        Some((_, n)) if n.vel != vel.max(1) => {
            n.vel = vel.max(1);
            true
        }
        _ => false,
    }
}
