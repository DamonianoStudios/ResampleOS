//! The Vocal tab: recording controls, the comp, and the take lanes under it.

use super::{db, pill, theme, toggle};
use crate::app::{App, RecState, VOCAL_SNAPS, VocalDrag, VocalTool};
use crate::engine::{Cmd, PadKind};
use crate::music::{TICKS_PER_BAR, pad_label};
use crate::shared::{PEAK_HOP, f32_load};
use crate::vocal::{Clip, LaneKind, VocalStack};
use eframe::egui::{self, Align2, Color32, FontId, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, vec2};

/// Width of the column of lane names and settings.
const HEAD_W: f32 = 180.0;
const RULER_H: f32 = 18.0;
const COMP_H: f32 = 46.0;

/// One colour per lane, so a stretch of the comp can be traced back to its take.
const LANE_COLORS: [Color32; 6] = [theme::MAGENTA, theme::CYAN, theme::AMBER, theme::GREEN, theme::PURPLE, Color32::from_rgb(255, 132, 92)];

fn lane_color(lane: usize) -> Color32 {
    LANE_COLORS[lane % LANE_COLORS.len()]
}

pub fn draw(app: &mut App, ui: &mut Ui) {
    let sel = app.selected;
    if app.pads[sel].cfg.kind != PadKind::Vocal {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("This is not a vocal pad").size(16.0).color(theme::DIM));
            ui.label("A vocal pad records takes over the playing loop, lets you comp the best of each, and plays the result.");
            if app.pads[sel].is_empty() {
                if ui.button("Make this a vocal pad").clicked() {
                    app.make_vocal(sel);
                }
            } else {
                ui.label(egui::RichText::new("Pick an empty pad first.").color(theme::DIM));
            }
        });
        return;
    }
    toolbar(app, ui);
    ui.add_space(2.0);
    lanes(app, ui);
}

fn toolbar(app: &mut App, ui: &mut Ui) {
    let sel = app.selected;
    let state = app.rec_state(sel);
    let blink = ui.input(|i| i.time * 3.0).fract() < 0.5;
    // Two rows, each with one job: getting a take in, then working on what is there.
    ui.horizontal(|ui| {
        // ---- Record.
        let (label, color) = match state {
            RecState::Recording => ("● REC", if blink { theme::RED } else { theme::MAGENTA }),
            RecState::Waiting => ("● ARMED", if blink { theme::MAGENTA } else { theme::MAGENTA_DIM }),
            RecState::Idle => ("● ARM", theme::RED),
        };
        if toggle(ui, state != RecState::Idle, label, color)
            .on_hover_text("Shift+R. Arm, then play: every time round the loop becomes a new take. Stop to finish.")
            .clicked()
        {
            app.vocal_arm(sel);
        }
        ui.spacing_mut().item_spacing.x = 3.0;
        if pill(ui, !app.vocal.free, "Loop").on_hover_text("A new take each time the loop comes round").clicked() {
            app.vocal.free = false;
        }
        if pill(ui, app.vocal.free, "Free").on_hover_text("One continuous take while you launch loops, marked where each loop changes").clicked() {
            app.vocal.free = true;
        }
        ui.spacing_mut().item_spacing.x = 6.0;
        let punch = app.vocal.range.is_some();
        if ui
            .add_enabled(punch, egui::Button::new("Punch").selected(app.vocal.punch && punch))
            .on_hover_text("Record only into the range dragged on the ruler; the rest of the comp is left alone")
            .on_disabled_hover_text("Drag a range on the ruler first")
            .clicked()
        {
            app.vocal.punch ^= true;
        }
        if ui.button("Import…").on_hover_text("Bring an audio file in as a take").clicked() {
            app.vocal_import(sel);
        }
        ui.separator();

        // ---- Input.
        theme::caption(ui, "MIC");
        let current = app.input_sel.and_then(|i| app.input_names.get(i).cloned()).unwrap_or_else(|| "Off".into());
        let short: String = current.chars().take(16).collect();
        let mut pick: Option<Option<usize>> = None;
        egui::ComboBox::from_id_salt("vocal_input").width(118.0).selected_text(short).show_ui(ui, |ui| {
            if ui.selectable_label(app.input_sel.is_none(), "Off").clicked() {
                pick = Some(None);
            }
            for (i, name) in app.input_names.iter().enumerate() {
                if ui.selectable_label(app.input_sel == Some(i), name).clicked() {
                    pick = Some(Some(i));
                }
            }
        });
        if let Some(choice) = pick {
            app.set_input(choice);
        }
        let mut gain = app.input_gain;
        ui.add(egui::DragValue::new(&mut gain).speed(0.01).range(0.0..=4.0).fixed_decimals(2).prefix("x")).on_hover_text("Input gain");
        if gain != app.input_gain {
            app.input_gain = gain;
            app.send(Cmd::InputCfg { dest: app.input_dest, gain });
        }
        input_meter(app, ui);
        if toggle(ui, app.vocal.monitor, "Mon", theme::GREEN)
            .on_hover_text("Hear yourself through the vocal bus while armed. Turn off if you monitor from your interface.")
            .clicked()
        {
            app.vocal.monitor ^= true;
        }
        let mut late = app.prefs.input_latency_ms;
        let changed = ui
            .add(egui::DragValue::new(&mut late).speed(0.25).range(0.0..=250.0).fixed_decimals(0).suffix(" ms"))
            .on_hover_text("Input latency. Takes are moved earlier by this much. If takes sit behind the beat, raise it.");
        if late != app.prefs.input_latency_ms {
            app.prefs.input_latency_ms = late;
        }
        if changed.drag_stopped() || changed.lost_focus() {
            let _ = app.prefs.save();
        }
        let measuring = app.latency_probe.is_some();
        if ui
            .add_enabled(!measuring, egui::Button::new(if measuring { "Measuring…" } else { "Measure" }))
            .on_hover_text(
                "Measures the input latency for you. Connect an output of your interface to the input you record from \
                 (or hold the mic near a speaker), then click: a few short chirps play and the time they take to come back is set above.",
            )
            .clicked()
        {
            app.measure_latency();
        }
    });
    ui.horizontal(|ui| {
        // ---- Tools.
        theme::caption(ui, "TOOL");
        ui.spacing_mut().item_spacing.x = 3.0;
        for (tool, label, hint) in [
            (VocalTool::Comp, "Comp", "C. Swipe across a take to put that stretch of it in the comp"),
            (VocalTool::Move, "Move", "D. Drag clips in time or to another lane. Alt+drag slips the audio inside a clip"),
            (VocalTool::Cut, "Cut", "X. Click a clip to cut it in two"),
        ] {
            if pill(ui, app.vocal.tool == tool, label).on_hover_text(hint).clicked() {
                app.vocal.tool = tool;
            }
        }
        ui.spacing_mut().item_spacing.x = 6.0;
        egui::ComboBox::from_id_salt("vocal_snap")
            .width(52.0)
            .selected_text(VOCAL_SNAPS[app.vocal.snap].0)
            .show_ui(ui, |ui| {
                for (i, (name, _)) in VOCAL_SNAPS.iter().enumerate() {
                    ui.selectable_value(&mut app.vocal.snap, i, *name);
                }
            })
            .response
            .on_hover_text("G. Grid that cuts, moves and comp edges snap to");
        ui.separator();

        // ---- Bake.
        theme::caption(ui, "BAKE");
        ui.checkbox(&mut app.vocal.bake_fx, "FX").on_hover_text("Print the vocal bus effects into the result");
        if ui.button("To pad").on_hover_text("Ctrl+B: the comp and its doubles as one sample on the next empty pad").clicked() {
            app.vocal_bake_to_pad(sel);
        }
        if ui.button("To bank").on_hover_text("Ctrl+Shift+B: each phrase on its own pad of an empty bank, ready to play").clicked() {
            app.vocal_bake_to_bank(sel);
        }
    });
}

/// Input level with a clip light that holds for a moment.
fn input_meter(app: &App, ui: &mut Ui) {
    let peak = f32_load(&app.shared.input_peak);
    let (rect, resp) = ui.allocate_exact_size(vec2(64.0, 16.0), Sense::hover());
    let p = ui.painter();
    let bar = Rect::from_min_max(rect.min + vec2(0.0, 4.0), rect.max - vec2(12.0, 4.0));
    p.rect_filled(bar, 2.0, theme::WELL);
    // -48 dB to full scale across the bar.
    let level = ((db(peak).max(-48.0) + 48.0) / 48.0).clamp(0.0, 1.0);
    let color = if peak >= 0.5 { theme::AMBER } else { theme::GREEN };
    p.rect_filled(Rect::from_min_max(bar.min, Pos2::new(bar.left() + bar.width() * level, bar.bottom())), 2.0, color);
    let clip = Pos2::new(rect.right() - 5.0, rect.center().y);
    p.circle_filled(clip, 4.0, if peak >= 0.99 { theme::RED } else { theme::RAISED });
    resp.on_hover_text(format!("Input {:.1} dB. The light is red while the input clips.", db(peak).max(-99.0)));
}

/// The geometry of the lane area: where frames fall across it and where each row sits.
#[derive(Clone, Copy)]
struct View {
    /// The part of the area that shows time.
    time: Rect,
    len: i64,
    row_h: f32,
    /// Top of the first lane row, which is above `rows_clip` when the lanes are scrolled.
    rows_top: f32,
    /// The part of the area the lane rows show through.
    rows_clip: Rect,
}

impl View {
    fn x_of(&self, frame: i64) -> f32 {
        self.time.left() + frame as f32 / self.len.max(1) as f32 * self.time.width()
    }

    fn frame_at(&self, x: f32) -> i64 {
        (((x - self.time.left()) / self.time.width()).clamp(0.0, 1.0) as f64 * self.len as f64) as i64
    }

    fn row(&self, lane: usize) -> Rect {
        let top = self.rows_top + lane as f32 * self.row_h;
        Rect::from_min_max(Pos2::new(self.time.left(), top), Pos2::new(self.time.right(), top + self.row_h - 2.0))
    }

    fn lane_at(&self, y: f32, lanes: usize) -> Option<usize> {
        let i = ((y - self.rows_top) / self.row_h).floor();
        (y >= self.rows_clip.top() && i >= 0.0 && (i as usize) < lanes).then_some(i as usize)
    }

    fn clip_rect(&self, lane: usize, clip: &Clip) -> Rect {
        let row = self.row(lane);
        Rect::from_min_max(Pos2::new(self.x_of(clip.start), row.top()), Pos2::new(self.x_of(clip.end()).max(self.x_of(clip.start) + 2.0), row.bottom()))
    }

    fn clip_at(&self, stack: &VocalStack, pos: Pos2) -> Option<(usize, usize)> {
        let lane = self.lane_at(pos.y, stack.lanes.len())?;
        let clip = stack.lanes[lane].clips.iter().rposition(|c| self.clip_rect(lane, c).contains(pos))?;
        Some((lane, clip))
    }
}

fn lanes(app: &mut App, ui: &mut Ui) {
    let sel = app.selected;
    let full = ui.available_rect_before_wrap();
    let resp = ui.interact(full, ui.id().with("vocal_lanes"), Sense::click_and_drag());
    let p = ui.painter_at(full);
    p.rect_filled(full, 6.0, theme::WELL);

    let loop_len = (app.scenes[app.scene].bars as f64 * app.frames_per_bar()).round() as i64;
    let stack = app.pads[sel].vocal.clone().unwrap_or_default();
    let recording = app.vocal_recording_from();
    // One more row while recording, for the take that is coming in.
    let rows = stack.lanes.len() + recording.is_some() as usize;
    let top = full.top() + RULER_H + COMP_H + 4.0;
    // Rows keep a workable height; when there are more than fit, the wheel scrolls them
    // under the comp, which stays put.
    let room = Rect::from_min_max(Pos2::new(full.left(), top), full.max);
    let row_h = ((room.height() - 2.0) / rows.max(1) as f32).clamp(38.0, 64.0);
    let overflow = (rows as f32 * row_h - room.height()).max(0.0);
    if resp.hovered() {
        app.vocal.scroll -= ui.input(|i| i.smooth_scroll_delta.y);
    }
    // The pass being recorded is the bottom row: keep it in sight.
    app.vocal.scroll = if recording.is_some() { overflow } else { app.vocal.scroll.clamp(0.0, overflow) };
    let v = View {
        time: Rect::from_min_max(Pos2::new(full.left() + HEAD_W, full.top()), full.max - vec2(4.0, 0.0)),
        len: stack.len.max(loop_len).max(1),
        row_h,
        rows_top: top - app.vocal.scroll,
        rows_clip: room,
    };

    ruler(app, &p, &v, &stack);
    comp_row(&p, &v, &stack, full);
    let rows_painter = p.with_clip_rect(room);
    for (i, lane) in stack.lanes.iter().enumerate() {
        lane_row(app, &rows_painter, &v, &stack, i, lane);
    }
    if let Some(from) = recording {
        incoming_row(app, &rows_painter, &v, stack.lanes.len(), from, full);
    }
    lane_heads(app, ui, &v, &stack, full);
    if overflow > 0.0 {
        // A thin bar down the right edge shows where the view is among the lanes.
        let track = Rect::from_min_max(Pos2::new(full.right() - 3.0, room.top()), full.max);
        let shown = room.height() / (rows as f32 * row_h);
        let at = track.top() + app.vocal.scroll / (rows as f32 * row_h) * track.height();
        p.rect_filled(Rect::from_min_max(Pos2::new(track.left(), at), Pos2::new(track.right(), at + track.height() * shown)), 1.5, theme::LINE);
    }

    if let Some((lane, clip)) = app.vocal.selected
        && stack.lanes.get(lane).is_some_and(|l| clip < l.clips.len())
    {
        let hint = "M mute · Backspace delete · Ctrl+D repeat · right-click for more";
        p.text(full.left_bottom() + vec2(8.0, -4.0), Align2::LEFT_BOTTOM, hint, FontId::proportional(9.5), theme::FAINT);
    }
    if stack.lanes.is_empty() && recording.is_none() {
        let hint = "No takes yet. Arm (Shift+R), press Space, and sing: each pass of the loop lands here as a take.";
        p.text(Pos2::new(v.time.center().x, top + 30.0), Align2::CENTER_CENTER, hint, FontId::proportional(13.0), theme::DIM);
    }

    edit(app, ui, &resp, &v, &stack);

    // In-progress gestures, drawn over the lanes.
    if let (Some((lane, from)), Some(pos)) = (app.vocal.swipe, resp.interact_pointer_pos()) {
        let (a, b) = (app.vocal_snap(from), app.vocal_snap(v.frame_at(pos.x)));
        let row = v.row(lane);
        let band = Rect::from_min_max(Pos2::new(v.x_of(a.min(b)), row.top()), Pos2::new(v.x_of(a.max(b)), row.bottom()));
        p.rect_filled(band, 2.0, Color32::from_white_alpha(40));
        p.rect_stroke(band, 2.0, Stroke::new(1.0, theme::TEXT), StrokeKind::Inside);
    }
    if app.playing() {
        let frame = (app.shared.tick() / TICKS_PER_BAR as f64 * app.frames_per_bar()) as i64;
        if frame < v.len {
            p.vline(v.x_of(frame), full.y_range(), Stroke::new(1.5, theme::GREEN));
        }
    }
}

/// Bars and beats, the punch range, and where loops changed during a free pass.
fn ruler(app: &mut App, p: &egui::Painter, v: &View, stack: &VocalStack) {
    let bar = app.frames_per_bar();
    let strip = Rect::from_min_max(v.time.min, Pos2::new(v.time.right(), v.time.top() + RULER_H));
    p.rect_filled(strip, 0.0, theme::BG);
    let bars = (v.len as f64 / bar).ceil() as usize;
    let px_per_beat = v.time.width() / (v.len as f32 / (bar as f32 / 4.0));
    for beat in 0..=bars * 4 {
        let x = v.x_of((beat as f64 * bar / 4.0) as i64);
        if x > v.time.right() + 0.5 {
            break;
        }
        if beat % 4 == 0 {
            p.vline(x, v.time.y_range(), Stroke::new(1.0, theme::LINE));
            if beat / 4 < bars {
                p.text(Pos2::new(x + 3.0, strip.top() + 2.0), Align2::LEFT_TOP, (beat / 4 + 1).to_string(), FontId::proportional(9.5), theme::DIM);
            }
        } else if px_per_beat > 9.0 {
            p.vline(x, strip.bottom()..=v.time.bottom(), Stroke::new(1.0, theme::LINE.gamma_multiply(0.45)));
        }
    }
    for (frame, scene) in &stack.markers {
        let x = v.x_of(*frame);
        p.vline(x, v.time.y_range(), Stroke::new(1.0, theme::AMBER.gamma_multiply(0.7)));
        p.text(Pos2::new(x + 2.0, strip.bottom() - 1.0), Align2::LEFT_BOTTOM, format!("loop {scene}"), FontId::proportional(8.5), theme::AMBER);
    }
    if let Some((a, b)) = app.vocal.range {
        let band = Rect::from_min_max(Pos2::new(v.x_of(a), strip.top()), Pos2::new(v.x_of(b), v.time.bottom()));
        let color = if app.vocal.punch { theme::RED } else { theme::TEXT };
        p.rect_filled(band, 0.0, color.gamma_multiply(0.07));
        p.rect_filled(Rect::from_min_max(band.min, Pos2::new(band.right(), strip.bottom())), 0.0, color.gamma_multiply(0.45));
    }
}

/// Draws the part of a clip between two frames as a waveform in `row`.
fn wave(p: &egui::Painter, v: &View, row: Rect, clip: &Clip, from: i64, to: i64, color: Color32) {
    let (x0, x1) = (v.x_of(from.max(clip.start)).ceil() as i32, v.x_of(to.min(clip.end())).floor() as i32);
    let mid = row.center().y;
    let half = row.height() * 0.46;
    let per_px = v.len as f32 / v.time.width();
    for x in x0..x1 {
        let frame = v.frame_at(x as f32 + 0.5);
        let at = clip.offset + (frame - clip.start);
        let (lo, hi) = clip.audio.extent(at, at + per_px.ceil() as i64);
        // Scaled up a little: vocals rarely fill the range, and shape matters more here.
        let (lo, hi) = ((lo * 1.6).max(-1.0), (hi * 1.6).min(1.0));
        p.vline(x as f32 + 0.5, mid - hi * half..=mid - lo * half + 0.6, Stroke::new(1.0, color));
    }
}

/// The comp: each stretch drawn from the take it comes from, in that take's colour.
fn comp_row(p: &egui::Painter, v: &View, stack: &VocalStack, full: Rect) {
    let row = Rect::from_min_max(Pos2::new(v.time.left(), full.top() + RULER_H + 1.0), Pos2::new(v.time.right(), full.top() + RULER_H + COMP_H));
    p.rect_filled(row, 3.0, Color32::from_rgb(26, 22, 30));
    let head = Rect::from_min_max(Pos2::new(full.left(), row.top()), Pos2::new(v.time.left() - 4.0, row.bottom()));
    p.text(head.left_center() + vec2(8.0, -7.0), Align2::LEFT_CENTER, "COMP", theme::bold(11.5), theme::MAGENTA);
    p.text(head.left_center() + vec2(8.0, 8.0), Align2::LEFT_CENTER, "what the pad plays", FontId::proportional(9.5), theme::DIM);
    for (from, to, lane) in stack.comp_runs() {
        let Some(lane) = lane.filter(|l| *l < stack.lanes.len()) else { continue };
        let color = lane_color(lane);
        let band = Rect::from_min_max(Pos2::new(v.x_of(from), row.top()), Pos2::new(v.x_of(to), row.bottom()));
        p.rect_filled(band.shrink2(vec2(0.5, 0.0)), 2.0, color.gamma_multiply(0.14));
        if stack.lanes[lane].mute {
            continue;
        }
        for clip in stack.lanes[lane].clips.iter().filter(|c| !c.muted) {
            wave(p, v, row, clip, from, to, color);
        }
        p.vline(band.left(), row.y_range(), Stroke::new(1.0, theme::BG));
    }
}

fn lane_row(app: &App, p: &egui::Painter, v: &View, stack: &VocalStack, i: usize, lane: &crate::vocal::Lane) {
    let row = v.row(i);
    let color = lane_color(i);
    p.rect_filled(row, 3.0, theme::PANEL);
    let thru = lane.kind == LaneKind::Thru;
    // Where the comp is listening to this take, the row is lit.
    if !thru {
        for (from, to, _) in stack.comp_runs().into_iter().filter(|r| r.2 == Some(i)) {
            let band = Rect::from_min_max(Pos2::new(v.x_of(from), row.top()), Pos2::new(v.x_of(to), row.bottom()));
            p.rect_filled(band, 0.0, color.gamma_multiply(0.10));
        }
    }
    for (c, clip) in lane.clips.iter().enumerate() {
        let r = v.clip_rect(i, clip);
        let dead = clip.muted || lane.mute;
        p.rect_filled(r, 3.0, if dead { theme::RAISED.gamma_multiply(0.6) } else { color.gamma_multiply(0.20) });
        if dead {
            wave(p, v, row, clip, clip.start, clip.end(), theme::FAINT);
        } else if thru {
            wave(p, v, row, clip, clip.start, clip.end(), color);
        } else {
            // Bright inside the comp, quiet outside it.
            wave(p, v, row, clip, clip.start, clip.end(), color.gamma_multiply(0.38));
            for (from, to, _) in stack.comp_runs().into_iter().filter(|r| r.2 == Some(i)) {
                wave(p, v, row, clip, from, to, color);
            }
        }
        let selected = app.vocal.selected == Some((i, c));
        let edge = if selected { theme::TEXT } else { color.gamma_multiply(0.55) };
        p.rect_stroke(r, 3.0, Stroke::new(if selected { 1.5 } else { 1.0 }, edge), StrokeKind::Inside);
        if clip.muted && r.width() > 34.0 {
            p.text(r.left_top() + vec2(4.0, 2.0), Align2::LEFT_TOP, "muted", FontId::proportional(9.0), theme::DIM);
        }
    }
}

/// The pass being recorded, drawn from the input as it arrives.
fn incoming_row(app: &App, p: &egui::Painter, v: &View, lane: usize, from: u64, full: Rect) {
    let row = v.row(lane);
    p.rect_filled(row, 3.0, theme::RED.gamma_multiply(0.10));
    let ring = &app.shared.input_ring;
    let now = ring.written().saturating_sub(from) as i64;
    let mid = row.center().y;
    let end = v.x_of(now.min(v.len)).floor() as i32;
    for x in v.time.left().ceil() as i32..end {
        let frame = from + v.frame_at(x as f32 + 0.5) as u64;
        let peak = (ring.peak(frame / PEAK_HOP as u64) * 1.6).min(1.0);
        p.vline(x as f32 + 0.5, mid - peak * row.height() * 0.46..=mid + peak * row.height() * 0.46 + 0.6, Stroke::new(1.0, theme::RED));
    }
    let head = Rect::from_min_max(Pos2::new(full.left(), row.top()), Pos2::new(v.time.left() - 4.0, row.bottom()));
    p.text(head.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, "● recording", theme::bold(11.5), theme::RED);
}

/// Something a lane's controls asked for, run once the lanes have been drawn.
type Deferred = Box<dyn FnOnce(&mut App)>;

/// Names and settings down the left of the lanes.
fn lane_heads(app: &mut App, ui: &mut Ui, v: &View, stack: &VocalStack, full: Rect) {
    let sel = app.selected;
    let mut act: Option<Deferred> = None;
    for (i, lane) in stack.lanes.iter().enumerate() {
        let row = v.row(i);
        let head = Rect::from_min_max(Pos2::new(full.left() + 4.0, row.top()), Pos2::new(v.time.left() - 4.0, row.bottom()));
        ui.painter().with_clip_rect(v.rows_clip).rect_filled(Rect::from_min_max(head.min, Pos2::new(head.left() + 3.0, head.bottom())), 1.0, lane_color(i));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(head.shrink2(vec2(7.0, 1.0))).layout(egui::Layout::top_down(egui::Align::Min)));
        if !head.intersects(v.rows_clip) {
            continue;
        }
        child.set_clip_rect(head.intersect(v.rows_clip));
        child.spacing_mut().item_spacing = vec2(3.0, 1.0);
        child.spacing_mut().button_padding = vec2(4.0, 0.0);
        child.spacing_mut().interact_size.y = 16.0;
        let thru = lane.kind == LaneKind::Thru;
        let mut edited = lane.clone();
        child.horizontal(|ui| {
            let name: String = lane.name.chars().take(9).collect();
            ui.label(egui::RichText::new(name).font(theme::bold(11.5)).color(if lane.mute { theme::DIM } else { theme::TEXT }));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("×").on_hover_text("Remove this lane").clicked() {
                    act = Some(Box::new(move |app| app.vocal_remove_lane(sel, i)));
                }
                if ui.small_button("2x").on_hover_text("Double: copy this lane to one that always plays, for stacks and harmonies").clicked() {
                    act = Some(Box::new(move |app| app.vocal_double(sel, i)));
                }
                if ui.add(egui::Button::new("M").small().selected(lane.mute)).on_hover_text("Mute this lane").clicked() {
                    edited.mute ^= true;
                }
            });
        });
        if row.height() >= 36.0 {
            child.horizontal(|ui| {
                let label = if thru { "THRU" } else { "TAKE" };
                let hint = if thru { "Always plays, alongside the comp. Click to make it a take." } else { "Plays only where the comp selects it. Click to make it play through." };
                if ui.add(egui::Button::new(egui::RichText::new(label).size(9.5)).small()).on_hover_text(hint).clicked() {
                    edited.kind = if thru { LaneKind::Take } else { LaneKind::Thru };
                }
                let mut level = edited.gain * 100.0;
                ui.add(egui::DragValue::new(&mut level).speed(0.5).range(0.0..=200.0).fixed_decimals(0).suffix("%")).on_hover_text("Level");
                edited.gain = level / 100.0;
                if thru {
                    let mut pan = edited.pan * 100.0;
                    ui.add(egui::DragValue::new(&mut pan).speed(0.5).range(-100.0..=100.0).fixed_decimals(0).custom_formatter(|v, _| {
                        if v.abs() < 0.5 { "C".into() } else if v < 0.0 { format!("{:.0}L", -v) } else { format!("{v:.0}R") }
                    }))
                    .on_hover_text("Pan");
                    edited.pan = pan / 100.0;
                    let mut cents = edited.semis * 100.0;
                    ui.add(egui::DragValue::new(&mut cents).speed(1.0).range(-1200.0..=1200.0).fixed_decimals(0).suffix("ct"))
                        .on_hover_text("Pitch in cents: a few for a double, 1200 for an octave. Formants move with the pitch.");
                    edited.semis = cents / 100.0;
                }
            });
        }
        if edited != *lane {
            act = Some(Box::new(move |app| {
                app.vocal_edit(sel, |s| {
                    if let Some(l) = s.lanes.get_mut(i) {
                        // A play-through lane is not part of the comp, and the reverse.
                        if l.kind != edited.kind {
                            for seg in s.comp.iter_mut().filter(|seg| seg.lane == Some(i)) {
                                seg.lane = None;
                            }
                        }
                        (l.kind, l.gain, l.pan, l.semis, l.mute) = (edited.kind, edited.gain, edited.pan, edited.semis, edited.mute);
                    }
                })
            }));
        }
    }
    if let Some(act) = act {
        act(app);
    }
}

/// Mouse input over the lanes: the ruler's range, and whatever the current tool does.
fn edit(app: &mut App, ui: &Ui, resp: &egui::Response, v: &View, stack: &VocalStack) {
    let sel = app.selected;
    let Some(pos) = resp.interact_pointer_pos() else {
        if !ui.input(|i| i.pointer.any_down()) {
            (app.vocal.drag, app.vocal.swipe) = (None, None);
        }
        return;
    };
    let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
    let in_ruler = origin.y < v.time.top() + RULER_H && origin.x >= v.time.left();
    let frame = v.frame_at(pos.x);
    let alt = ui.input(|i| i.modifiers.alt);

    if in_ruler {
        // Dragging on the ruler marks a range; a click clears it.
        if resp.dragged_by(PointerButton::Primary) {
            let (a, b) = (app.vocal_snap(v.frame_at(origin.x)), app.vocal_snap(frame));
            app.vocal.range = (a != b).then_some((a.min(b), a.max(b)));
        } else if resp.clicked() {
            app.vocal.range = None;
            app.vocal.punch = false;
        }
        return;
    }
    if origin.x < v.time.left() {
        return;
    }

    let under = v.clip_at(stack, origin);
    if resp.drag_started_by(PointerButton::Primary) {
        match app.vocal.tool {
            VocalTool::Comp => {
                if let Some(lane) = v.lane_at(origin.y, stack.lanes.len()).filter(|l| stack.lanes[*l].kind == LaneKind::Take) {
                    app.vocal.swipe = Some((lane, v.frame_at(origin.x)));
                }
            }
            VocalTool::Move => {
                if let Some((lane, clip)) = under {
                    let c = &stack.lanes[lane].clips[clip];
                    app.vocal.selected = under;
                    app.vocal.drag = Some(VocalDrag { lane, clip, grab: v.frame_at(origin.x), start: c.start, offset: c.offset, slip: alt });
                }
            }
            VocalTool::Cut => {}
        }
    }
    if resp.dragged_by(PointerButton::Primary)
        && let Some(d) = app.vocal.drag
    {
        let by = frame - d.grab;
        if d.slip {
            // Dragging right moves the sound later, so the clip reads from earlier.
            app.vocal_edit(sel, |s| s.slip(d.lane, d.clip, d.offset - by));
        } else {
            let start = app.vocal_snap(d.start + by);
            let to_lane = v.lane_at(pos.y, stack.lanes.len()).unwrap_or(d.lane);
            let mut landed = None;
            app.vocal_edit(sel, |s| landed = s.move_clip(d.lane, d.clip, start, to_lane));
            if let Some((lane, clip)) = landed {
                app.vocal.drag = Some(VocalDrag { lane, clip, ..d });
                app.vocal.selected = Some((lane, clip));
            }
        }
    }
    if resp.drag_stopped() {
        if let Some((lane, from)) = app.vocal.swipe.take() {
            app.vocal_comp(sel, from, frame, lane);
        }
        app.vocal.drag = None;
    }
    if resp.clicked() {
        app.vocal.selected = under;
        if let (VocalTool::Cut, Some((lane, clip))) = (app.vocal.tool, under) {
            app.vocal_cut(sel, lane, clip, frame);
        }
    }
    if resp.secondary_clicked() {
        app.vocal.selected = under;
        app.vocal.menu_frame = frame;
    }
    // Right-click acts on the clip under the pointer, whatever the tool.
    resp.context_menu(|ui| {
        let Some((lane, clip)) = app.vocal.selected else {
            ui.label(egui::RichText::new("Right-click a clip").color(theme::DIM));
            return;
        };
        ui.label(egui::RichText::new(format!("{} · {}", pad_label(sel), stack.lanes.get(lane).map_or("", |l| l.name.as_str()))).strong());
        if ui.button("Cut here").clicked() {
            app.vocal_cut(sel, lane, clip, app.vocal.menu_frame);
            ui.close();
        }
        if stack.lanes.get(lane).is_some_and(|l| l.kind == LaneKind::Take) && ui.button("Use this clip in the comp").clicked() {
            if let Some(c) = stack.lanes[lane].clips.get(clip) {
                let (a, b) = (c.start, c.end());
                app.vocal_edit(sel, |s| {
                    s.set_comp(a, b, Some(lane));
                    s.hand_comped = true;
                });
            }
            ui.close();
        }
        if ui.button("Mute or unmute").clicked() {
            app.vocal_mute_selected(sel);
            ui.close();
        }
        if ui.button("Repeat after itself").clicked() {
            app.vocal_duplicate_selected(sel);
            ui.close();
        }
        if ui.button("Delete").clicked() {
            app.vocal_delete_selected(sel);
            ui.close();
        }
    });
}
