//! Flyout piano roll for MIDI pads: scale-locked rows, velocity lane, free microtiming.

use super::{humanize, knob, theme, toggle};
use crate::app::{App, PluginTarget, RollDrag, SNAPS};
use crate::engine::{Cmd, MidiTarget, Note, PadKind};
use crate::music::{Key, PPQ, TICKS_PER_BAR, is_black_key, note_name, pad_label};
use crate::synth::SynthEngine;
use eframe::egui::{self, Align2, Color32, FontId, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, vec2};

const KEY_W: f32 = 46.0;
const ROW_H: f32 = 13.0;
const VEL_H: f32 = 54.0;
const HEAD_H: f32 = 16.0;
/// The horizontal scroll bar under the velocity lane.
const SCROLL_H: f32 = 10.0;
const AUDITION_KEY: u32 = 0x3000;

pub fn draw(app: &mut App, ui: &mut Ui) {
    let sel = app.selected;
    if !matches!(app.pads[sel].cfg.kind, PadKind::Midi | PadKind::Drum) {
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

    // One row however narrow the window: it scrolls sideways rather than wrap, so the
    // note grid keeps its height.
    egui::ScrollArea::horizontal().id_salt("roll_toolbar").show(ui, |ui| ui.horizontal(|ui| {
        if cfg.kind == PadKind::Midi {
            ui.label("Engine");
            let mut browse = false;
            let label = match (&app.pads[sel].plugin, cfg.target) {
                (Some(p), MidiTarget::Plugin) => format!("Plugin: {}", p.name),
                _ => app.midi_target_label(cfg.target),
            };
            egui::ComboBox::from_id_salt("roll_target").width(150.0).selected_text(label).show_ui(ui, |ui| {
                ui.selectable_value(&mut cfg.target, MidiTarget::Synth, "Internal synth");
                if ui.selectable_label(cfg.target == MidiTarget::Plugin, "Plugin instrument…").clicked() {
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
        } else {
            ui.label(egui::RichText::new("Drum synth: notes tune the drum, middle C is as set").color(theme::LIME));
            ui.separator();
        }
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
        ui.checkbox(&mut app.roll.ghosts, "Ghosts").on_hover_text("Show the notes of the other MIDI pads in this bank");
        if toggle(ui, app.roll.follow, "Follow", theme::GREEN)
            .on_hover_text("Keep the playhead in view while playing when zoomed in. Ctrl + scroll zooms, Shift + scroll moves along the pattern.")
            .clicked()
        {
            app.roll.follow ^= true;
        }
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
        ui.separator();
        if ui
            .button("Randomize vel")
            .on_hover_text("Moves each velocity a random amount, up to the figure beside it either way: the selected notes, or all of them when none is selected")
            .clicked()
        {
            app.roll_randomize_velocity();
        }
        let mut amount = app.roll.vel_random * 100.0;
        if ui
            .add(egui::DragValue::new(&mut amount).speed(0.5).range(1.0..=100.0).fixed_decimals(0).prefix("±").suffix("%"))
            .on_hover_text("How far Randomize may move a velocity")
            .changed()
        {
            app.roll.vel_random = amount / 100.0;
        }
    }));
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
    /// The ticks in view: from `t0`, `span` of them.
    t0: f32,
    span: f32,
}

impl View {
    fn x_of(&self, tick: f32) -> f32 {
        self.grid.left() + (tick - self.t0) / self.span * self.grid.width()
    }

    fn y_of(&self, pitch: i32) -> f32 {
        self.grid.top() + (self.top - pitch) as f32 * ROW_H
    }

    fn tick_at(&self, x: f32) -> i64 {
        (self.t0 + (x - self.grid.left()) / self.grid.width() * self.span) as i64
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
    if rect.width() < KEY_W + 40.0 || rect.height() < VEL_H + HEAD_H + SCROLL_H + 30.0 {
        return;
    }
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, theme::BG);
    let grid = Rect::from_min_max(rect.min + vec2(KEY_W, HEAD_H), rect.max - vec2(0.0, VEL_H + SCROLL_H));
    let len = app.pads[sel].pattern.len_ticks.max(1) as f32;
    let track = Rect::from_min_max(Pos2::new(grid.left(), rect.bottom() - SCROLL_H + 2.0), rect.max - vec2(0.0, 1.0));
    scroll_and_zoom(app, ui, &resp, grid, track, len);
    let span = len * app.roll.zoom;
    let v = View {
        rect,
        grid,
        lane: Rect::from_min_max(Pos2::new(grid.left(), grid.bottom()), Pos2::new(rect.right(), track.top() - 2.0)),
        top: app.roll.top_f.round() as i32,
        len: len as u32,
        t0: app.roll.scroll,
        span,
    };
    // Everything on the time axis is kept off the keyboard strip when scrolled.
    let pg = p.with_clip_rect(Rect::from_min_max(Pos2::new(grid.left(), rect.top()), Pos2::new(rect.right(), v.lane.bottom())).intersect(p.clip_rect()));
    let alt = ui.input(|i| i.modifiers.alt);
    let snap = Snap { ticks: if alt { 1 } else { SNAPS[app.roll.snap].1 }, key: app.key, lock: app.roll.scale_lock };

    paint_rows(&p, &v, &snap);
    paint_time_grid(&pg, &v, SNAPS[app.roll.snap].1);
    if app.roll.ghosts {
        paint_ghosts(app, &pg, &v);
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

    p.hline(rect.x_range(), v.lane.top(), Stroke::new(1.0, theme::LINE));
    p.text(Pos2::new(rect.left() + 4.0, v.lane.center().y), Align2::LEFT_CENTER, "VEL", FontId::proportional(9.0), theme::DIM);
    paint_notes(app, &pg, &v, resp.interact_pointer_pos());
    paint_scrollbar(app, &p, track, len);
}

/// Mouse wheel and the scroll bar. Plain scroll moves through the pitches, Ctrl + scroll
/// (or a pinch) zooms the time axis around the pointer, Shift + scroll moves along it. While
/// playing with Follow on, the view turns the page whenever the playhead reaches its edge.
fn scroll_and_zoom(app: &mut App, ui: &Ui, resp: &egui::Response, grid: Rect, track: Rect, len: f32) {
    let roll = &mut app.roll;
    // Zoomed all the way in, a beat fills the grid.
    let min_zoom = (PPQ as f32 / len).min(1.0);
    if resp.hovered() {
        let (delta, zoom, hover) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
        roll.top_f = (roll.top_f + delta.y / ROW_H).clamp(12.0, 127.0);
        let span = len * roll.zoom;
        if zoom != 1.0 {
            let at = hover.map_or(0.5, |h| ((h.x - grid.left()) / grid.width()).clamp(0.0, 1.0));
            let pivot = roll.scroll + at * span;
            roll.zoom = (roll.zoom / zoom).clamp(min_zoom, 1.0);
            roll.scroll = pivot - at * len * roll.zoom;
        }
        roll.scroll -= delta.x / grid.width() * span;
    }
    roll.zoom = roll.zoom.clamp(min_zoom, 1.0);

    // The bar: drag the thumb, click the track to jump, double-click to see everything.
    let bar = ui.interact(track, egui::Id::new("roll_scroll"), Sense::click_and_drag());
    let span = len * roll.zoom;
    if bar.dragged() {
        roll.scroll += bar.drag_delta().x / track.width() * len;
    } else if bar.double_clicked() {
        roll.zoom = 1.0;
    } else if bar.clicked()
        && let Some(pos) = bar.interact_pointer_pos()
    {
        roll.scroll = (pos.x - track.left()) / track.width() * len - span * 0.5;
    }
    if roll.follow && app.shared.playing.load(std::sync::atomic::Ordering::Relaxed) && roll.zoom < 1.0 && !resp.dragged() && !bar.dragged() {
        let t = app.shared.tick().rem_euclid(len as f64) as f32;
        if t < roll.scroll || t > roll.scroll + span * 0.95 {
            roll.scroll = t - span * 0.02;
        }
    }
    roll.scroll = roll.scroll.clamp(0.0, (len - span).max(0.0));
}

fn paint_scrollbar(app: &App, p: &egui::Painter, track: Rect, len: f32) {
    p.rect_filled(track, 3.0, theme::WELL);
    let (a, b) = (app.roll.scroll / len, (app.roll.scroll + len * app.roll.zoom) / len);
    let thumb = Rect::from_min_max(Pos2::new(track.left() + a * track.width(), track.top()), Pos2::new(track.left() + b * track.width(), track.bottom()));
    p.rect_filled(thumb, 3.0, if app.roll.zoom < 1.0 { theme::FAINT } else { theme::LINE });
    if app.playing() {
        let t = app.shared.tick().rem_euclid(len as f64) as f32 / len;
        p.vline(track.left() + t * track.width(), track.y_range(), Stroke::new(1.5, theme::GREEN));
    }
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

/// Snap lines where there is room for them, then beats and bars, numbered along the top
/// as bar.beat (1.1, 1.2, 1.3, 1.4, 2.1 ...) when there is room, and by bar when not.
fn paint_time_grid(p: &egui::Painter, v: &View, snap_ticks: u32) {
    let px_per_tick = v.grid.width() / v.span;
    let (first, last) = (v.t0.max(0.0) as u32, ((v.t0 + v.span).ceil() as u32).min(v.len));
    if snap_ticks > 1 && snap_ticks as f32 * px_per_tick >= 5.0 {
        for t in (first / snap_ticks * snap_ticks..last).step_by(snap_ticks as usize) {
            p.vline(v.x_of(t as f32), v.grid.y_range(), Stroke::new(1.0, Color32::from_rgb(20, 22, 27)));
        }
    }
    let beat_px = PPQ as f32 * px_per_tick;
    let beats_per_bar = TICKS_PER_BAR / PPQ;
    // When even bar numbers would collide, label every second or fourth bar.
    let bar_every = [1u32, 2, 4, 8].into_iter().find(|n| beat_px * (beats_per_bar * n) as f32 >= 24.0).unwrap_or(16);
    for t in (first / PPQ * PPQ..=last).step_by(PPQ as usize) {
        let (bar, beat) = (t / TICKS_PER_BAR, t % TICKS_PER_BAR / PPQ);
        let x = v.x_of(t as f32);
        let downbeat = beat == 0;
        let col = if downbeat { theme::DIM } else { theme::LINE };
        p.vline(x, v.grid.top() - if downbeat { HEAD_H } else { 5.0 }..=v.lane.bottom(), Stroke::new(1.0, col));
        if t >= v.len {
            continue;
        }
        let label = if beat_px >= 26.0 {
            Some(format!("{}.{}", bar + 1, beat + 1))
        } else {
            (downbeat && bar.is_multiple_of(bar_every)).then(|| (bar + 1).to_string())
        };
        if let Some(label) = label {
            let font = if downbeat { theme::bold(10.0) } else { FontId::proportional(9.5) };
            p.text(Pos2::new(x + 3.0, v.rect.top() + 1.5), Align2::LEFT_TOP, label, font, if downbeat { theme::TEXT } else { theme::DIM });
        }
    }
}

/// Ghosts: the other MIDI pads' notes in this bank, faint behind this pad's own.
fn paint_ghosts(app: &App, p: &egui::Painter, v: &View) {
    let (sel, len, grid) = (app.selected, v.len, v.grid);
    let bank = sel / 16;
    for i in (bank * 16..bank * 16 + 16).filter(|i| *i != sel) {
        let other = &app.pads[i];
        if other.cfg.kind != PadKind::Midi || other.pattern.len_ticks == 0 {
            continue;
        }
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
    }
}

fn add_note(app: &mut App, v: &View, snap: &Snap, pos: Pos2) -> usize {
    let start = snap.floor(v.tick_at(pos.x)).clamp(0, v.len as i64 - 1) as u32;
    let note = Note::new(start, app.roll.last_len.max(30), snap.pitch(v.pitch_at(pos.y)), 100);
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
        // Dragging in the velocity lane moves the stack under the pointer.
        if let (true, None, Some(pos)) = (app.roll.drag.is_none(), app.roll.marquee, pointer)
            && origin.is_some_and(|o| v.lane.contains(o))
        {
            changed |= drag_velocity(app, sel, pos, v);
        }
    }

    if resp.drag_stopped() {
        app.roll.marquee = None;
        app.roll.vel_drag = None;
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
            changed |= drag_velocity(app, sel, pos, v);
            app.roll.vel_drag = None;
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
    let selected = app.roll_selected();
    let notes = &app.pads[app.selected].pattern.notes;
    for (i, n) in notes.iter().enumerate() {
        let level = n.vel as f32 / 127.0;
        let r = v.note_rect(n).intersect(v.grid);
        if r.height() >= 2.0 && r.width() > 0.0 {
            p.rect_filled(r, 2.0, theme::CYAN.gamma_multiply(0.45 + 0.55 * level));
            if selected.contains(&i) {
                p.rect_filled(r, 2.0, Color32::from_white_alpha(70));
                p.rect_stroke(r, 2.0, Stroke::new(1.5, theme::TEXT), StrokeKind::Inside);
            } else {
                p.rect_stroke(r, 2.0, Stroke::new(1.0, theme::CYAN_DIM), StrokeKind::Inside);
            }
        }
    }
    // Stems, each with a dot at its height so the notes of a chord can be told apart,
    // the selected ones drawn last and brightest.
    let order = (0..notes.len()).filter(|i| !selected.contains(i)).chain(selected.iter().copied().filter(|&i| i < notes.len()));
    for i in order {
        let n = &notes[i];
        let level = n.vel as f32 / 127.0;
        let top = lane.bottom() - (lane.height() - 6.0) * level;
        let x = v.x_of(n.start as f32) + 1.0;
        let picked = selected.contains(&i);
        let colour = if picked { theme::TEXT } else { theme::CYAN.gamma_multiply(0.5 + 0.5 * level) };
        p.vline(x, top..=lane.bottom(), Stroke::new(if picked { 2.0 } else { 3.0 }, colour));
        p.circle_filled(Pos2::new(x, top), if picked { 3.5 } else { 2.5 }, colour);
    }
    if let (Some(from), Some(pos)) = (app.roll.marquee, pointer) {
        let band = Rect::from_two_pos(from, pos).intersect(v.grid);
        p.rect_filled(band, 0.0, Color32::from_white_alpha(18));
        p.rect_stroke(band, 0.0, Stroke::new(1.0, theme::TEXT), StrokeKind::Inside);
    }
    if app.playing() {
        let t = app.shared.tick().rem_euclid(v.len as f64) as f32;
        p.vline(v.x_of(t), v.rect.y_range(), Stroke::new(1.5, theme::GREEN));
    }
}

/// The velocity a pointer height in the lane stands for.
fn lane_velocity(pos: Pos2, v: &View) -> u8 {
    ((((v.lane.bottom() - pos.y) / (v.lane.height() - 6.0)).clamp(0.01, 1.0) * 127.0) as u8).max(1)
}

/// The notes a velocity edit at `pos` reaches: the stack (every note starting at one
/// tick) whose stem is nearest the pointer, or only the selected notes in it if any are.
fn velocity_targets(notes: &[Note], selected: &[usize], pos: Pos2, v: &View) -> Option<(u32, Vec<usize>)> {
    let (_, tick) = notes
        .iter()
        .map(|n| ((v.x_of(n.start as f32) - pos.x).abs(), n.start))
        .filter(|(d, _)| *d < 8.0)
        .min_by(|a, b| a.0.total_cmp(&b.0))?;
    let stack: Vec<usize> = (0..notes.len()).filter(|&i| notes[i].start == tick).collect();
    let chosen: Vec<usize> = stack.iter().copied().filter(|i| selected.contains(i)).collect();
    Some((tick, if chosen.is_empty() { stack } else { chosen }))
}

/// Sets velocities from the pointer in the velocity lane. The loudest note of the stack
/// goes where the pointer is and the others move with it, keeping a chord's voicing;
/// moving to another stack mid-drag carries on there.
fn drag_velocity(app: &mut App, sel: usize, pos: Pos2, v: &View) -> bool {
    let selected = app.roll_selected().to_vec();
    let notes = &mut app.pads[sel].pattern.notes;
    let Some((tick, targets)) = velocity_targets(notes, &selected, pos, v) else { return false };
    // The velocities as the drag found them, so clamping at the ends loses no voicing.
    if app.roll.vel_drag.as_ref().is_none_or(|(t, orig)| *t != tick || orig.iter().map(|o| o.0).ne(targets.iter().copied())) {
        app.roll.vel_drag = Some((tick, targets.iter().map(|&i| (i, notes[i].vel)).collect()));
    }
    let Some((_, orig)) = &app.roll.vel_drag else { return false };
    let top = orig.iter().map(|o| o.1).max().unwrap_or(1) as i32;
    let shift = lane_velocity(pos, v) as i32 - top;
    let mut changed = false;
    for &(i, was) in orig {
        if let Some(n) = notes.get_mut(i) {
            let vel = (was as i32 + shift).clamp(1, 127) as u8;
            changed |= n.vel != vel;
            n.vel = vel;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 400.0));
        View {
            rect,
            grid: Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1000.0, 300.0)),
            lane: Rect::from_min_max(Pos2::new(0.0, 300.0), Pos2::new(1000.0, 400.0)),
            top: 84,
            len: 1920,
            t0: 0.0,
            span: 1920.0,
        }
    }

    #[test]
    fn a_velocity_edit_reaches_the_whole_stack_or_just_the_selected_notes() {
        let v = view();
        // A three-note chord at the downbeat and a single note at beat 3.
        let notes = vec![Note::new(0, 240, 60, 100), Note::new(0, 240, 64, 80), Note::new(0, 240, 67, 60), Note::new(960, 240, 62, 90)];
        let at_chord = Pos2::new(v.x_of(0.0) + 2.0, 350.0);
        assert_eq!(velocity_targets(&notes, &[], at_chord, &v), Some((0, vec![0, 1, 2])));
        // With one of the chord's notes selected, only it moves.
        assert_eq!(velocity_targets(&notes, &[1], at_chord, &v), Some((0, vec![1])));
        // A selection elsewhere doesn't narrow this stack.
        assert_eq!(velocity_targets(&notes, &[3], at_chord, &v), Some((0, vec![0, 1, 2])));
        let at_single = Pos2::new(v.x_of(960.0), 350.0);
        assert_eq!(velocity_targets(&notes, &[], at_single, &v), Some((960, vec![3])));
        // Nowhere near a stem: nothing.
        assert_eq!(velocity_targets(&notes, &[], Pos2::new(v.x_of(480.0), 350.0), &v), None);
    }
}
