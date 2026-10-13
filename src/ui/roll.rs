//! Flyout piano roll for MIDI pads: scale-locked rows, velocity lane, free microtiming.

use super::{humanize, knob, theme, toggle};
use crate::app::{App, PluginTarget, RollDrag, SNAPS};
use crate::engine::{Cmd, MidiTarget, Note, PadKind, PlayMode};
use crate::music::{Key, PPQ, TICKS_PER_BAR, is_black_key, note_name, pad_label};
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

/// Where ticks and pitches fall on screen this frame.
#[derive(Clone, Copy)]
struct View {
    rect: Rect,
    /// The note area, between the keyboard strip and the velocity lane.
    grid: Rect,
    lane: Rect,
    /// Pitch of the top row.
    top: i32,
    /// Pattern length in ticks.
    len: u32,
}

impl View {
    fn x_of(&self, tick: f32) -> f32 {
        self.grid.left() + tick / self.len as f32 * self.grid.width()
    }

    fn y_of(&self, pitch: i32) -> f32 {
        self.grid.top() + (self.top - pitch) as f32 * ROW_H
    }

    fn tick_at(&self, x: f32) -> i64 {
        ((x - self.grid.left()) / self.grid.width() * self.len as f32) as i64
    }

    fn pitch_at(&self, y: f32) -> i32 {
        self.top - ((y - self.grid.top()) / ROW_H).floor() as i32
    }

    fn note_rect(&self, n: &Note) -> Rect {
        let y = self.y_of(n.pitch as i32);
        let x = self.x_of(n.start as f32);
        Rect::from_min_max(Pos2::new(x, y + 1.0), Pos2::new(self.x_of((n.start + n.len) as f32).max(x + 3.0), y + ROW_H - 1.0))
    }

    /// The topmost note under `pos`.
    fn hit(&self, pos: Pos2, notes: &[Note]) -> Option<usize> {
        notes.iter().rposition(|n| self.note_rect(n).expand(1.0).contains(pos))
    }
}

/// What an edit is held to: the time grid (unless Alt is down) and the project scale.
#[derive(Clone, Copy)]
struct Snap {
    /// Grid step in ticks; 1 or less means free.
    ticks: u32,
    key: Key,
    lock: bool,
}

impl Snap {
    fn free(&self) -> bool {
        self.ticks <= 1
    }

    fn nearest(&self, t: i64) -> i64 {
        if self.free() { t } else { (t as f64 / self.ticks as f64).round() as i64 * self.ticks as i64 }
    }

    fn floor(&self, t: i64) -> i64 {
        if self.free() { t } else { t.div_euclid(self.ticks as i64) * self.ticks as i64 }
    }

    fn pitch(&self, pitch: i32) -> u8 {
        let pitch = if self.lock { self.key.snap(pitch) } else { pitch };
        pitch.clamp(0, 127) as u8
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
    if resp.hovered() {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        app.roll.top_f = (app.roll.top_f + dy / ROW_H).clamp(12.0, 127.0);
    }
    let grid = Rect::from_min_max(rect.min + vec2(KEY_W, HEAD_H), rect.max - vec2(0.0, VEL_H));
    let v = View {
        rect,
        grid,
        lane: Rect::from_min_max(Pos2::new(grid.left(), grid.bottom()), rect.max),
        top: app.roll.top_f.round() as i32,
        len: app.pads[sel].pattern.len_ticks.max(1),
    };
    let alt = ui.input(|i| i.modifiers.alt);
    let snap = Snap { ticks: if alt { 1 } else { SNAPS[app.roll.snap].1 }, key: app.key, lock: app.roll.scale_lock };

    paint_rows(&p, &v, &snap);
    paint_time_grid(&p, &v, SNAPS[app.roll.snap].1);
    if app.roll.ghosts {
        paint_ghosts(app, &p, &v);
    }

    let (changed, audition) = edit(app, ui, &resp, &v, &snap);
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

    paint_notes(app, &p, &v, resp.interact_pointer_pos());
}

/// Rows and the keyboard strip. Out-of-scale rows are greyed out when the lock is on.
fn paint_rows(p: &egui::Painter, v: &View, snap: &Snap) {
    let (grid, key, lock) = (v.grid, snap.key, snap.lock);
    for r in 0..(grid.height() / ROW_H).ceil() as i32 {
        let pitch = v.top - r;
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

        let keyr = Rect::from_min_max(Pos2::new(v.rect.left(), y), Pos2::new(grid.left() - 1.0, (y + ROW_H - 1.0).min(grid.bottom())));
        let kcol = if is_black_key(pitch) { Color32::from_rgb(20, 20, 24) } else { Color32::from_rgb(206, 208, 214) };
        p.rect_filled(keyr, 1.0, if lock && !in_scale { kcol.gamma_multiply(0.35) } else { kcol });
        if pitch.rem_euclid(12) == 0 || is_root {
            let tcol = if is_black_key(pitch) { theme::TEXT } else { theme::BG };
            p.text(keyr.right_center() - vec2(3.0, 0.0), Align2::RIGHT_CENTER, note_name(pitch), FontId::proportional(9.0), tcol);
        }
    }
}

/// Snap lines where there is room for them, then beats and numbered bars.
fn paint_time_grid(p: &egui::Painter, v: &View, snap_ticks: u32) {
    let px_per_tick = v.grid.width() / v.len as f32;
    if snap_ticks > 1 && snap_ticks as f32 * px_per_tick >= 5.0 {
        for t in (0..v.len).step_by(snap_ticks as usize) {
            p.vline(v.x_of(t as f32), v.grid.y_range(), Stroke::new(1.0, Color32::from_rgb(20, 22, 27)));
        }
    }
    for t in (0..=v.len).step_by(PPQ as usize) {
        let bar = t % TICKS_PER_BAR == 0;
        let x = v.x_of(t as f32);
        let col = if bar { theme::DIM } else { theme::LINE };
        p.vline(x, v.grid.top() - if bar { HEAD_H } else { 4.0 }..=v.rect.bottom(), Stroke::new(1.0, col));
        if bar && t < v.len {
            let number = (t / TICKS_PER_BAR + 1).to_string();
            p.text(Pos2::new(x + 3.0, v.rect.top() + 1.0), Align2::LEFT_TOP, number, FontId::proportional(9.0), theme::DIM);
        }
    }
}

/// Ghosts: other MIDI pads' notes, and the transients of looping audio pads in this bank.
fn paint_ghosts(app: &App, p: &egui::Painter, v: &View) {
    let (sel, len, grid) = (app.selected, v.len, v.grid);
    let bank = sel / 16;
    let tps = app.bpm / 60.0 * PPQ as f32 / app.sr();
    for i in (bank * 16..bank * 16 + 16).filter(|i| *i != sel) {
        let other = &app.pads[i];
        if other.cfg.kind == PadKind::Midi && other.pattern.len_ticks > 0 {
            // Repeated across the roll if the other pattern is shorter.
            for base in (0..len).step_by(other.pattern.len_ticks as usize) {
                for n in &other.pattern.notes {
                    let (s, e) = ((base + n.start) as f32, (base + n.start + n.len) as f32);
                    let y = v.y_of(n.pitch as i32);
                    if s >= len as f32 || y < grid.top() || y + ROW_H > grid.bottom() {
                        continue;
                    }
                    let r = Rect::from_min_max(Pos2::new(v.x_of(s), y + 2.0), Pos2::new(v.x_of(e.min(len as f32)), y + ROW_H - 2.0));
                    p.rect_filled(r, 1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 26));
                }
            }
        } else if other.cfg.mode == PlayMode::Loop
            && let Some(s) = other.sample()
        {
            let olen = (s.len() as f32 * tps).max(1.0);
            let mut base = 0.0;
            while base < len as f32 {
                for &tr in &other.transients {
                    let t = base + tr as f32 * tps;
                    if t < len as f32 {
                        let x = v.x_of(t);
                        p.vline(x, v.rect.top()..=grid.top(), Stroke::new(1.5, theme::AMBER));
                        p.vline(x, grid.y_range(), Stroke::new(1.0, Color32::from_rgba_unmultiplied(244, 168, 40, 22)));
                    }
                }
                base += olen;
            }
        }
    }
}

fn add_note(app: &mut App, v: &View, snap: &Snap, pos: Pos2) -> usize {
    let start = snap.floor(v.tick_at(pos.x)).clamp(0, v.len as i64 - 1) as u32;
    let note = Note { start, len: app.roll.last_len.max(30), pitch: snap.pitch(v.pitch_at(pos.y)), vel: 100 };
    let notes = &mut app.pads[app.selected].pattern.notes;
    notes.push(note);
    notes.len() - 1
}

/// Applies this frame's mouse input to the pattern and the selection. Returns whether the
/// pattern changed, and a pitch to sound if the gesture calls for one.
fn edit(app: &mut App, ui: &Ui, resp: &egui::Response, v: &View, snap: &Snap) -> (bool, Option<u8>) {
    let sel = app.selected;
    let (ctrl, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
    let origin = ui.input(|i| i.pointer.press_origin());
    let pointer = resp.interact_pointer_pos();
    let mut changed = false;
    let mut audition = None;

    if app.roll.sel_pad != sel {
        app.roll.sel.clear();
        app.roll.sel_pad = sel;
    }
    let count = app.pads[sel].pattern.notes.len();
    app.roll.sel.retain(|&i| i < count);

    if resp.drag_started_by(PointerButton::Primary)
        && let Some(o) = origin.filter(|o| v.grid.contains(*o))
    {
        let found = v.hit(o, &app.pads[sel].pattern.notes);
        if found.is_none() && shift {
            app.roll.marquee = Some(o);
        } else {
            let (idx, resize) = match found {
                Some(i) => (i, o.x > v.note_rect(&app.pads[sel].pattern.notes[i]).right() - 6.0),
                None => {
                    changed = true;
                    (add_note(app, v, snap, o), false)
                }
            };
            // Dragging a note outside the selection drags just that note.
            if !app.roll.sel.contains(&idx) {
                if !shift {
                    app.roll.sel.clear();
                }
                app.roll.sel.push(idx);
            }
            let notes = &mut app.pads[sel].pattern.notes;
            if ctrl && !resize && found.is_some() {
                // Duplicate: copies stay where the notes were, and the drag carries the originals.
                for &i in &app.roll.sel {
                    notes.push(notes[i]);
                }
                changed = true;
            }
            let anchor = notes[idx];
            audition = Some(anchor.pitch);
            app.roll.drag = Some(RollDrag {
                idx,
                resize,
                grab_tick: v.tick_at(o.x) - anchor.start as i64,
                grab_pitch: v.pitch_at(o.y),
                orig: app.roll.sel.iter().map(|&i| (i, notes[i])).collect(),
            });
        }
    }

    if resp.dragged_by(PointerButton::Primary) {
        if let (Some(d), Some(pos)) = (app.roll.drag.as_ref(), pointer) {
            let notes = &mut app.pads[sel].pattern.notes;
            let before = notes.get(d.idx).map(|n| n.pitch);
            changed |= drag_notes(notes, d, pos, v, snap);
            // Hear the note under the pointer when it lands on a new pitch.
            audition = audition.or(notes.get(d.idx).map(|n| n.pitch).filter(|p| Some(*p) != before));
        }
        if let (Some(from), Some(pos)) = (app.roll.marquee, pointer) {
            let band = Rect::from_two_pos(from, pos);
            let notes = &app.pads[sel].pattern.notes;
            app.roll.sel = (0..count).filter(|&i| v.note_rect(&notes[i]).intersects(band)).collect();
        }
        // Dragging in the velocity lane paints velocities.
        if let (true, None, Some(pos)) = (app.roll.drag.is_none(), app.roll.marquee, pointer)
            && origin.is_some_and(|o| v.lane.contains(o))
        {
            changed |= set_velocity(&mut app.pads[sel].pattern.notes, pos, v);
        }
    }

    if resp.drag_stopped() {
        app.roll.marquee = None;
        if let Some(d) = app.roll.drag.take() {
            if let Some(n) = app.pads[sel].pattern.notes.get(d.idx) {
                app.roll.last_len = n.len;
            }
            changed = true;
        }
    }

    if resp.clicked()
        && let Some(pos) = pointer
    {
        if v.grid.contains(pos) {
            match v.hit(pos, &app.pads[sel].pattern.notes) {
                Some(i) => {
                    audition = Some(app.pads[sel].pattern.notes[i].pitch);
                    if shift || ctrl {
                        // Add to the selection, or take out of it.
                        match app.roll.sel.iter().position(|&s| s == i) {
                            Some(at) => drop(app.roll.sel.remove(at)),
                            None => app.roll.sel.push(i),
                        }
                    } else {
                        app.roll.sel = vec![i];
                    }
                }
                None if shift || ctrl => app.roll.sel.clear(),
                None => {
                    let i = add_note(app, v, snap, pos);
                    audition = Some(app.pads[sel].pattern.notes[i].pitch);
                    app.roll.sel = vec![i];
                    changed = true;
                }
            }
        } else if v.lane.contains(pos) {
            changed |= set_velocity(&mut app.pads[sel].pattern.notes, pos, v);
        } else if pos.x < v.grid.left() && pos.y > v.grid.top() {
            audition = Some(v.pitch_at(pos.y).clamp(0, 127) as u8);
        }
    }

    if resp.secondary_clicked()
        && let Some(pos) = pointer
        && let Some(i) = v.hit(pos, &app.pads[sel].pattern.notes)
    {
        app.pads[sel].pattern.notes.remove(i);
        app.roll.sel.retain(|&s| s != i);
        for s in app.roll.sel.iter_mut().filter(|s| **s > i) {
            *s -= 1;
        }
        app.roll.drag = None;
        changed = true;
    }
    (changed, audition)
}

/// Moves or resizes every note of a drag to follow the pointer. The notes keep their
/// positions relative to each other and stop together at the edges.
fn drag_notes(notes: &mut [Note], d: &RollDrag, pos: Pos2, v: &View, snap: &Snap) -> bool {
    let Some(&(_, anchor)) = d.orig.iter().find(|(i, _)| *i == d.idx) else { return false };
    let len = v.len as i64;
    let mut changed = false;
    if d.resize {
        let end = snap.nearest(v.tick_at(pos.x)).max(anchor.start as i64 + 30);
        let grow = end - (anchor.start + anchor.len) as i64;
        for &(i, o) in &d.orig {
            if let Some(n) = notes.get_mut(i) {
                let was = n.len;
                n.len = (o.len as i64 + grow).clamp(30, len) as u32;
                changed |= n.len != was;
            }
        }
        return changed;
    }
    let starts = d.orig.iter().map(|(_, o)| o.start as i64);
    let pitches = d.orig.iter().map(|(_, o)| o.pitch as i32);
    let (first, last) = (starts.clone().min().unwrap_or(0), starts.max().unwrap_or(0));
    let (low, high) = (pitches.clone().min().unwrap_or(0), pitches.max().unwrap_or(127));
    let dt = (snap.nearest(v.tick_at(pos.x) - d.grab_tick) - anchor.start as i64).clamp(-first, len - 1 - last);
    let dp = (v.pitch_at(pos.y) - d.grab_pitch).clamp(-low, 127 - high);
    for &(i, o) in &d.orig {
        if let Some(n) = notes.get_mut(i) {
            let was = *n;
            n.start = (o.start as i64 + dt) as u32;
            n.pitch = snap.pitch(o.pitch as i32 + dp);
            changed |= *n != was;
        }
    }
    changed
}

/// Notes, their velocity stems, the selection box and the playhead.
fn paint_notes(app: &App, p: &egui::Painter, v: &View, pointer: Option<Pos2>) {
    let lane = v.lane;
    for (i, n) in app.pads[app.selected].pattern.notes.iter().enumerate() {
        let level = n.vel as f32 / 127.0;
        let r = v.note_rect(n).intersect(v.grid);
        if r.height() >= 2.0 && r.width() > 0.0 {
            p.rect_filled(r, 2.0, theme::CYAN.gamma_multiply(0.45 + 0.55 * level));
            if app.roll.sel.contains(&i) {
                p.rect_filled(r, 2.0, Color32::from_white_alpha(70));
                p.rect_stroke(r, 2.0, Stroke::new(1.5, theme::TEXT), StrokeKind::Inside);
            } else {
                p.rect_stroke(r, 2.0, Stroke::new(1.0, theme::CYAN_DIM), StrokeKind::Inside);
            }
        }
        let stem = (lane.height() - 6.0) * level;
        let x = v.x_of(n.start as f32) + 1.0;
        p.vline(x, lane.bottom() - stem..=lane.bottom(), Stroke::new(3.0, theme::CYAN.gamma_multiply(0.5 + 0.5 * level)));
    }
    if let (Some(from), Some(pos)) = (app.roll.marquee, pointer) {
        let band = Rect::from_two_pos(from, pos).intersect(v.grid);
        p.rect_filled(band, 0.0, Color32::from_white_alpha(18));
        p.rect_stroke(band, 0.0, Stroke::new(1.0, theme::TEXT), StrokeKind::Inside);
    }
    p.hline(v.rect.x_range(), lane.top(), Stroke::new(1.0, theme::LINE));
    p.text(Pos2::new(v.rect.left() + 4.0, lane.center().y), Align2::LEFT_CENTER, "VEL", FontId::proportional(9.0), theme::DIM);
    if app.playing() {
        let t = app.shared.tick().rem_euclid(v.len as f64) as f32;
        p.vline(v.x_of(t), v.rect.y_range(), Stroke::new(1.5, theme::GREEN));
    }
}

/// Sets the velocity of the note whose stem is nearest the pointer in the velocity lane.
fn set_velocity(notes: &mut [Note], pos: Pos2, v: &View) -> bool {
    let vel = ((((v.lane.bottom() - pos.y) / (v.lane.height() - 6.0)).clamp(0.01, 1.0) * 127.0) as u8).max(1);
    let nearest = notes
        .iter_mut()
        .map(|n| ((v.x_of(n.start as f32) - pos.x).abs(), n))
        .filter(|(d, _)| *d < 8.0)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    match nearest {
        Some((_, n)) if n.vel != vel => {
            n.vel = vel;
            true
        }
        _ => false,
    }
}
