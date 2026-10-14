//! The arrangement view: a song timeline with a track per pad, a loop lane above it
//! showing which loop each stretch came from, a ruler, and the loop brace.

use super::{theme, toggle};
use crate::app::{ARR_SNAPS, App, ArrDrag, Clip, View};
use crate::engine::{PadKind, Pattern};
use crate::music::{PPQ, TICKS_PER_BAR, pad_label};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, vec2};

const HEAD_W: f32 = 168.0;
const BRACE_H: f32 = 12.0;
const RULER_H: f32 = 20.0;
const LANE_H: f32 = 26.0;
const TRACK_H: f32 = 36.0;
/// How near a clip's edge the pointer must be to trim rather than move, in points.
const EDGE: f32 = 6.0;
const BAR: u64 = TICKS_PER_BAR as u64;

/// A colour for each loop, so clips from one loop read as one.
fn loop_colour(scene: usize) -> Color32 {
    const COLOURS: [Color32; 8] = [theme::AMBER, theme::CYAN, theme::LIME, theme::MAGENTA, theme::PURPLE, theme::GREEN, theme::RED, Color32::from_rgb(120, 160, 255)];
    COLOURS[scene % COLOURS.len()]
}

/// Where ticks fall on screen this frame.
#[derive(Clone, Copy)]
struct View2 {
    grid: Rect,
    zoom: f32,
    scroll: f64,
}

impl View2 {
    fn x_of(&self, tick: f64) -> f32 {
        self.grid.left() + ((tick - self.scroll) / BAR as f64) as f32 * self.zoom
    }

    fn tick_at(&self, x: f32) -> f64 {
        (self.scroll + ((x - self.grid.left()) / self.zoom) as f64 * BAR as f64).max(0.0)
    }
}

/// A pattern of loop `scene` for `pad`, without copying it.
fn pattern_ref(app: &App, scene: usize, pad: usize) -> Option<&Pattern> {
    if scene == app.scene {
        app.pads.get(pad).map(|p| &p.pattern)
    } else {
        app.scenes.get(scene)?.patterns.iter().find(|(p, _)| *p == pad).map(|(_, pat)| pat)
    }
}

/// The pads that get a track: those with clips, and those that play in any loop (so a
/// clip can be added to them).
fn track_pads(app: &App) -> Vec<usize> {
    let mut pads: Vec<usize> = app.arrangement.tracks.iter().map(|(p, _)| *p).collect();
    for (k, s) in app.scenes.iter().enumerate() {
        if k == app.scene {
            pads.extend((0..app.pads.len()).filter(|&i| !app.pads[i].pattern.notes.is_empty()));
        } else {
            pads.extend(s.patterns.iter().filter(|(_, p)| !p.notes.is_empty()).map(|(i, _)| *i));
        }
    }
    pads.sort_unstable();
    pads.dedup();
    pads
}

pub fn draw(app: &mut App, ui: &mut Ui) {
    toolbar(app, ui);
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    timeline(app, ui, rect);
}

fn toolbar(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        theme::title(ui, "ARRANGEMENT", theme::GREEN);
        if ui
            .button("Chain to song")
            .on_hover_text("Writes the chain (or, with no chain, the loop on screen) after the end of the song: a section per loop, a clip per pad")
            .clicked()
        {
            app.chain_to_arrangement();
        }
        ui.menu_button("Put loop", |ui| {
            ui.label(egui::RichText::new("At the insert marker").small().color(theme::DIM));
            for k in 0..app.scenes.len() {
                if ui.button(app.scene_label(k)).clicked() {
                    app.insert_loop(k);
                    ui.close();
                }
            }
        });
        ui.separator();
        ui.label("Snap");
        egui::ComboBox::from_id_salt("arr_snap").width(52.0).selected_text(ARR_SNAPS[app.arr.snap].0).show_ui(ui, |ui| {
            for (i, (name, _)) in ARR_SNAPS.iter().enumerate() {
                ui.selectable_value(&mut app.arr.snap, i, *name);
            }
        });
        let brace = app.arrangement.brace.is_some();
        if toggle(ui, app.arrangement.brace_on && brace, "Loop", theme::AMBER)
            .on_hover_text("Play round the loop brace. Drag in the strip above the ruler to set it.")
            .clicked()
        {
            if brace {
                app.arrangement.brace_on ^= true;
            } else {
                // A first brace: the four bars from the insert marker.
                let s = app.arr.cursor / BAR * BAR;
                app.arrangement.brace = Some((s, s + 4 * BAR));
                app.arrangement.brace_on = true;
            }
        }
        if toggle(ui, app.arr.follow, "Follow", theme::GREEN).on_hover_text("Keep the playhead in view while playing").clicked() {
            app.arr.follow ^= true;
        }
        if ui.small_button("−").on_hover_text("Zoom out (Ctrl + scroll)").clicked() {
            app.arr.zoom = (app.arr.zoom / 1.4).max(12.0);
        }
        if ui.small_button("+").on_hover_text("Zoom in (Ctrl + scroll)").clicked() {
            app.arr.zoom = (app.arr.zoom * 1.4).min(1200.0);
        }
        ui.separator();
        let label = if app.exporting { "Exporting…" } else { "Export song…" };
        if ui
            .add_enabled(!app.exporting, egui::Button::new(label))
            .on_hover_text("Renders the song (or the loop brace, when it is on) to a WAV file, with the tails of the last notes and effects")
            .clicked()
        {
            app.export_song_dialog();
        }
        egui::ComboBox::from_id_salt("export_depth").width(84.0).selected_text(app.export_depth.name()).show_ui(ui, |ui| {
            for d in crate::app::Depth::ALL {
                ui.selectable_value(&mut app.export_depth, d, d.name());
            }
        });
        if app.arr.overridden && app.playing() {
            let back = egui::Button::new(egui::RichText::new("Back to arrangement").color(theme::BG)).fill(theme::AMBER);
            if ui.add(back).on_hover_text("A launched loop is playing; carry on with the song where it has got to").clicked() {
                app.back_to_arrangement();
            }
        }
        ui.label(
            egui::RichText::new("double-click: add a clip / open its loop · drag: move · edges: trim · Ctrl+D duplicate · Ctrl+E split · Del")
                .small()
                .color(theme::DIM),
        );
    });
}

fn timeline(app: &mut App, ui: &mut Ui, rect: Rect) {
    let p = ui.painter_at(rect);
    let grid = Rect::from_min_max(Pos2::new(rect.left() + HEAD_W, rect.top()), rect.max);
    let brace_r = Rect::from_min_size(grid.min, vec2(grid.width(), BRACE_H));
    let ruler = Rect::from_min_size(Pos2::new(grid.left(), brace_r.bottom()), vec2(grid.width(), RULER_H));
    let lane = Rect::from_min_size(Pos2::new(grid.left(), ruler.bottom()), vec2(grid.width(), LANE_H));
    let tracks_r = Rect::from_min_max(Pos2::new(rect.left(), lane.bottom() + 2.0), rect.max);
    let pads = track_pads(app);

    // ---- Scrolling and zoom.
    let hover = ui.input(|i| i.pointer.hover_pos()).filter(|p| rect.contains(*p));
    if let Some(pos) = hover {
        let (scroll, zoom, shift) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers.shift));
        let v = View2 { grid, zoom: app.arr.zoom, scroll: app.arr.scroll };
        if (zoom - 1.0).abs() > 1e-3 {
            // Zoom around the pointer.
            let at = v.tick_at(pos.x);
            app.arr.zoom = (app.arr.zoom * zoom).clamp(12.0, 1200.0);
            app.arr.scroll = (at - ((pos.x - grid.left()) / app.arr.zoom) as f64 * BAR as f64).max(0.0);
        } else if shift || scroll.x.abs() > scroll.y.abs() {
            let d = if scroll.x != 0.0 { scroll.x } else { scroll.y };
            app.arr.scroll = (app.arr.scroll - (d / app.arr.zoom) as f64 * BAR as f64).max(0.0);
        } else if scroll.y != 0.0 {
            let max = (pads.len() as f32 * TRACK_H - tracks_r.height() + 20.0).max(0.0);
            app.arr.vscroll = (app.arr.vscroll - scroll.y).clamp(0.0, max);
        }
    }
    let playing_song = app.playing() && app.song_mode;
    let head = app.song_pos();
    if playing_song && app.arr.follow {
        let v = View2 { grid, zoom: app.arr.zoom, scroll: app.arr.scroll };
        let x = v.x_of(head);
        if x > grid.right() - 40.0 || x < grid.left() {
            app.arr.scroll = (head - BAR as f64 * 0.5).max(0.0);
        }
    }
    let v = View2 { grid, zoom: app.arr.zoom, scroll: app.arr.scroll };
    let snap = ARR_SNAPS[app.arr.snap].1;
    let snapped = |t: f64| ((t / snap as f64).round() as u64) * snap;
    let floor_snap = |t: f64| ((t / snap as f64).floor() as u64) * snap;

    // ---- Background, bar lines and the ruler.
    p.rect_filled(grid, 3.0, theme::WELL);
    let first_bar = (v.scroll / BAR as f64).floor() as u64;
    let last_bar = v.tick_at(grid.right()) as u64 / BAR + 1;
    let every = if v.zoom < 24.0 { 8 } else if v.zoom < 50.0 { 4 } else if v.zoom < 90.0 { 2 } else { 1 };
    for bar in first_bar..=last_bar {
        let x = v.x_of((bar * BAR) as f64);
        if x < grid.left() - 1.0 {
            continue;
        }
        let major = bar.is_multiple_of(every);
        p.vline(x, ruler.top()..=grid.bottom(), Stroke::new(1.0, if bar % 4 == 0 { theme::FAINT.gamma_multiply(0.6) } else { Color32::from_rgb(34, 37, 42) }));
        if major {
            p.text(Pos2::new(x + 3.0, ruler.center().y), Align2::LEFT_CENTER, (bar + 1).to_string(), FontId::proportional(10.0), theme::DIM);
        }
        if v.zoom >= 90.0 {
            for b in 1..4 {
                let bx = v.x_of((bar * BAR + b * PPQ as u64) as f64);
                p.vline(bx, ruler.bottom() - 4.0..=ruler.bottom(), Stroke::new(1.0, theme::FAINT));
            }
        }
    }
    p.hline(grid.x_range(), ruler.bottom(), Stroke::new(1.0, theme::FAINT));

    // ---- The loop brace.
    if let Some((s, e)) = app.arrangement.brace {
        let r = Rect::from_min_max(Pos2::new(v.x_of(s as f64), brace_r.top() + 2.0), Pos2::new(v.x_of(e as f64), brace_r.bottom() - 1.0)).intersect(brace_r);
        let colour = if app.arrangement.brace_on { theme::AMBER } else { theme::FAINT };
        p.rect_filled(r, 2.0, colour.gamma_multiply(0.8));
    }
    let br = ui.interact(brace_r, egui::Id::new("arr_brace"), Sense::click_and_drag());
    if br.drag_started()
        && let Some(o) = ui.input(|i| i.pointer.press_origin())
    {
        app.arr.drag = Some(ArrDrag::Brace { from: snapped(v.tick_at(o.x)) });
    }
    if let (Some(ArrDrag::Brace { from }), Some(pos)) = (app.arr.drag, br.interact_pointer_pos()) {
        let to = snapped(v.tick_at(pos.x));
        if to != from {
            app.arrangement.brace = Some((from.min(to), from.max(to)));
            app.arrangement.brace_on = true;
        }
    }
    if br.clicked() {
        app.arrangement.brace_on ^= true;
    }

    // ---- Ruler: click to put the insert marker (and the playhead) there.
    let rr = ui.interact(ruler, egui::Id::new("arr_ruler"), Sense::click_and_drag());
    if (rr.clicked() || rr.dragged())
        && let Some(pos) = rr.interact_pointer_pos()
    {
        app.locate(snapped(v.tick_at(pos.x)));
    }

    // ---- Loop lane: sections.
    p.rect_filled(lane, 2.0, Color32::from_rgb(18, 20, 24));
    p.text(Pos2::new(rect.left() + 8.0, lane.center().y), Align2::LEFT_CENTER, "LOOPS", theme::bold(10.0), theme::FAINT);
    for (i, s) in app.arrangement.sections.clone().iter().enumerate() {
        let r = Rect::from_min_max(Pos2::new(v.x_of(s.start as f64), lane.top() + 2.0), Pos2::new(v.x_of(s.end() as f64), lane.bottom() - 2.0));
        if r.right() < grid.left() || r.left() > grid.right() {
            continue;
        }
        let colour = loop_colour(s.scene);
        let selected = app.arr.section == Some(i);
        let painter = p.with_clip_rect(r.intersect(lane));
        painter.rect_filled(r, 3.0, colour.gamma_multiply(if selected { 0.9 } else { 0.55 }));
        painter.text(r.left_center() + vec2(5.0, 0.0), Align2::LEFT_CENTER, app.scene_label(s.scene), theme::bold(10.5), theme::BG);
        let resp = ui.interact(r.intersect(lane), egui::Id::new(("arr_section", i)), Sense::click_and_drag());
        if resp.clicked() {
            app.arr.section = Some(i);
            app.arr.selected.clear();
        }
        if resp.double_clicked() {
            app.show_scene(s.scene, true);
            app.view = View::Pads;
        }
        if resp.drag_started()
            && let Some(o) = ui.input(|i| i.pointer.press_origin())
        {
            app.arr.section = Some(i);
            app.arr.selected.clear();
            app.arr.drag = Some(ArrDrag::Section { index: i, grab: v.tick_at(o.x) as u64 });
        }
        if resp.secondary_clicked() {
            app.arr.section = Some(i);
        }
        resp.context_menu(|ui| {
            if ui.button("Open the loop").clicked() {
                app.show_scene(s.scene, true);
                app.view = View::Pads;
                ui.close();
            }
            if ui.button("Duplicate    Ctrl+D").clicked() {
                app.arr.section = Some(i);
                app.duplicate_selection();
                ui.close();
            }
            if ui.button("Delete with its clips    Del").clicked() {
                app.arr.section = Some(i);
                app.delete_selected_clips();
                ui.close();
            }
        });
        resp.on_hover_text(format!("{}: bars {} to {}. Double-click to open the loop.", app.scene_label(s.scene), s.start / BAR + 1, s.end() / BAR));
    }

    // ---- Tracks.
    let painter = p.with_clip_rect(tracks_r);
    let mut clip_hit: Option<(usize, usize, bool, bool)> = None; // pad, index, near start, near end
    let pointer = ui.input(|i| i.pointer.hover_pos());
    for (row, &pad) in pads.iter().enumerate() {
        let top = tracks_r.top() + row as f32 * TRACK_H - app.arr.vscroll;
        if top + TRACK_H < tracks_r.top() || top > tracks_r.bottom() {
            continue;
        }
        let row_r = Rect::from_min_size(Pos2::new(rect.left(), top), vec2(rect.width(), TRACK_H - 2.0));
        let head_r = Rect::from_min_size(row_r.min, vec2(HEAD_W - 6.0, row_r.height()));
        let lane_r = Rect::from_min_max(Pos2::new(grid.left(), row_r.top()), row_r.max);
        track_head(app, ui, &painter, head_r, pad);
        painter.rect_filled(lane_r, 2.0, Color32::from_rgba_unmultiplied(255, 255, 255, if row % 2 == 0 { 6 } else { 3 }));

        // Empty stretches of the track: click for the marker, double-click to add a clip.
        let lr = ui.interact(lane_r.intersect(tracks_r), egui::Id::new(("arr_track", pad)), Sense::click());
        if lr.clicked()
            && let Some(pos) = lr.interact_pointer_pos()
        {
            app.arr.selected.clear();
            app.arr.section = None;
            app.arr.cursor = snapped(v.tick_at(pos.x));
            if !app.playing() {
                app.locate(app.arr.cursor);
            }
        }
        if lr.double_clicked()
            && let Some(pos) = lr.interact_pointer_pos()
        {
            // The loop on screen if this pad plays in it, otherwise the first that it does.
            let scene = std::iter::once(app.scene)
                .chain(0..app.scenes.len())
                .find(|&k| pattern_ref(app, k, pad).is_some_and(|p| !p.notes.is_empty()));
            if let Some(scene) = scene {
                app.add_clip(pad, scene, floor_snap(v.tick_at(pos.x)));
            }
        }

        let clips: Vec<Clip> = app.arrangement.clips(pad).to_vec();
        for (i, c) in clips.iter().enumerate() {
            let r = Rect::from_min_max(Pos2::new(v.x_of(c.start as f64), lane_r.top() + 1.0), Pos2::new(v.x_of(c.end() as f64), lane_r.bottom() - 1.0));
            if r.right() < grid.left() || r.left() > grid.right() {
                continue;
            }
            let selected = app.arr.selected.contains(&(pad, i));
            paint_clip(app, &painter, r.intersect(Rect::from_min_max(Pos2::new(grid.left(), tracks_r.top()), tracks_r.max)), r, pad, c, &v, selected);
            if let Some(pos) = pointer.filter(|p| r.contains(*p) && tracks_r.contains(*p)) {
                let near_start = pos.x - r.left() < EDGE && r.width() > EDGE * 3.0;
                let near_end = r.right() - pos.x < EDGE;
                clip_hit = Some((pad, i, near_start, near_end));
            }
            let id = egui::Id::new(("arr_clip", pad, c.start));
            let resp = ui.interact(r.intersect(tracks_r).intersect(grid), id, Sense::click_and_drag());
            clip_events(app, ui, &resp, pad, i, &v);
        }
    }
    if let Some((_, _, s, e)) = clip_hit
        && (s || e)
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }

    // ---- Drags under way.
    if let (Some(drag), Some(pos)) = (app.arr.drag, ui.input(|i| i.pointer.interact_pos())) {
        let at = v.tick_at(pos.x);
        match drag {
            ArrDrag::Move { grab } => {
                // Shown as outlines until the drop, when they land and cut into what is there.
                let delta = snapped(at) as i64 - floor_snap(grab as f64) as i64;
                let delta = delta.max(-(app.arr.moving.iter().map(|(_, c)| c.start).min().unwrap_or(0) as i64));
                for (pad, c) in &app.arr.moving {
                    let Some(row) = pads.iter().position(|p| p == pad) else { continue };
                    let top = tracks_r.top() + row as f32 * TRACK_H - app.arr.vscroll;
                    let x0 = v.x_of((c.start as i64 + delta) as f64);
                    let r = Rect::from_min_size(Pos2::new(x0, top + 1.0), vec2(c.len as f32 / BAR as f32 * v.zoom, TRACK_H - 4.0));
                    painter.rect_stroke(r, 3.0, Stroke::new(2.0, theme::TEXT), StrokeKind::Inside);
                }
            }
            ArrDrag::Trim { pad, index, end } => {
                if let Some(c) = app.arrangement.clips(pad).get(index).copied() {
                    let t = snapped(at);
                    let x = v.x_of(t as f64);
                    let row = pads.iter().position(|p| *p == pad).unwrap_or(0);
                    let top = tracks_r.top() + row as f32 * TRACK_H - app.arr.vscroll;
                    let (a, b) = if end { (v.x_of(c.start as f64), x) } else { (x, v.x_of(c.end() as f64)) };
                    painter.rect_stroke(Rect::from_min_max(Pos2::new(a.min(b), top + 1.0), Pos2::new(a.max(b), top + TRACK_H - 3.0)), 3.0, Stroke::new(2.0, theme::TEXT), StrokeKind::Inside);
                }
            }
            ArrDrag::Section { index, grab } => {
                if let Some(s) = app.arrangement.sections.get(index) {
                    let to = (s.start as i64 + snapped(at) as i64 - floor_snap(grab as f64) as i64).max(0) as f64;
                    let r = Rect::from_min_max(Pos2::new(v.x_of(to), lane.top() + 1.0), Pos2::new(v.x_of(to + s.len as f64), grid.bottom()));
                    p.rect_stroke(r, 3.0, Stroke::new(2.0, theme::TEXT), StrokeKind::Inside);
                }
            }
            ArrDrag::Brace { .. } => {}
        }
    }
    if ui.input(|i| i.pointer.any_released()) && let Some(drag) = app.arr.drag.take() {
        let at = ui.input(|i| i.pointer.interact_pos()).map(|p| v.tick_at(p.x));
        match (drag, at) {
            (ArrDrag::Move { grab }, Some(at)) => {
                let moving = std::mem::take(&mut app.arr.moving);
                let delta = snapped(at) as i64 - floor_snap(grab as f64) as i64;
                let delta = delta.max(-(moving.iter().map(|(_, c)| c.start).min().unwrap_or(0) as i64));
                if delta != 0 {
                    app.arrangement.take(app.arr.selected.clone());
                    app.drop_moved(&moving, delta);
                }
            }
            (ArrDrag::Trim { pad, index, end }, Some(at)) => app.trim_clip(pad, index, end, snapped(at)),
            (ArrDrag::Section { index, grab }, Some(at)) => {
                if let Some(s) = app.arrangement.sections.get(index) {
                    let to = (s.start as i64 + snapped(at) as i64 - floor_snap(grab as f64) as i64).max(0) as u64;
                    app.move_section(index, to);
                }
            }
            _ => {}
        }
        app.arr.moving.clear();
    }

    // ---- Insert marker and playhead.
    let cx = v.x_of(app.arr.cursor as f64);
    if cx >= grid.left() {
        p.vline(cx, ruler.top()..=grid.bottom(), Stroke::new(1.0, theme::TEXT.gamma_multiply(0.35)));
        let tri = vec![Pos2::new(cx - 5.0, ruler.top()), Pos2::new(cx + 5.0, ruler.top()), Pos2::new(cx, ruler.top() + 7.0)];
        p.add(egui::Shape::convex_polygon(tri, theme::TEXT, Stroke::NONE));
    }
    if playing_song {
        let x = v.x_of(head);
        if x >= grid.left() {
            p.vline(x, ruler.top()..=grid.bottom(), Stroke::new(1.5, theme::GREEN));
        }
    }
    if pads.is_empty() {
        p.text(
            tracks_r.center(),
            Align2::CENTER_CENTER,
            "Nothing to arrange yet. Make loops on the pads, then press Chain to song or Put loop.",
            FontId::proportional(14.0),
            theme::DIM,
        );
    } else if app.arrangement.is_empty() {
        p.text(
            Pos2::new(grid.center().x, tracks_r.top() + pads.len() as f32 * TRACK_H + 30.0).min(tracks_r.center_bottom() - vec2(0.0, 20.0)),
            Align2::CENTER_CENTER,
            "Press Chain to song to lay the chain out here, or double-click a track to add a clip",
            FontId::proportional(13.0),
            theme::DIM,
        );
    }
}

/// A track's name, mute and solo.
fn track_head(app: &mut App, ui: &mut Ui, painter: &egui::Painter, r: Rect, pad: usize) {
    painter.rect_filled(r, 3.0, theme::WELL);
    let selected = app.selected == pad;
    let label = pad_label(pad);
    let name = &app.pads[pad].name;
    let kind = match app.pads[pad].cfg.kind {
        PadKind::Audio => "audio",
        PadKind::Midi => "midi",
        PadKind::Vocal => "vocal",
        PadKind::Drum => "drum",
    };
    painter.text(r.left_top() + vec2(8.0, 5.0), Align2::LEFT_TOP, &label, theme::bold(11.0), if selected { theme::AMBER } else { theme::TEXT });
    painter.with_clip_rect(r.shrink(2.0)).text(r.left_top() + vec2(40.0, 5.0), Align2::LEFT_TOP, name, FontId::proportional(11.0), theme::DIM);
    painter.text(r.left_bottom() + vec2(8.0, -4.0), Align2::LEFT_BOTTOM, kind, FontId::proportional(9.0), theme::FAINT);
    let hit = ui.interact(r, egui::Id::new(("arr_head", pad)), Sense::click());
    if hit.clicked() {
        app.select(pad);
    }
    let buttons = Rect::from_min_size(Pos2::new(r.right() - 52.0, r.top() + 4.0), vec2(50.0, r.height() - 8.0));
    let mut cfg = app.pads[pad].cfg;
    ui.scope_builder(egui::UiBuilder::new().max_rect(buttons).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if toggle(ui, cfg.mute, "M", theme::RED).clicked() {
            cfg.mute ^= true;
        }
        if toggle(ui, cfg.solo, "S", theme::AMBER).clicked() {
            cfg.solo ^= true;
        }
    });
    if cfg != app.pads[pad].cfg {
        app.pads[pad].cfg = cfg;
        app.push_pad(pad);
    }
}

/// A clip: its loop's colour, name, and the notes it plays.
#[allow(clippy::too_many_arguments)]
fn paint_clip(app: &App, p: &egui::Painter, visible: Rect, r: Rect, pad: usize, c: &Clip, v: &View2, selected: bool) {
    let colour = loop_colour(c.scene);
    let clip_p = p.with_clip_rect(visible);
    clip_p.rect_filled(r, 3.0, colour.gamma_multiply(if selected { 0.55 } else { 0.32 }));
    clip_p.rect_stroke(r, 3.0, Stroke::new(if selected { 1.5 } else { 1.0 }, if selected { theme::TEXT } else { colour.gamma_multiply(0.8) }), StrokeKind::Inside);
    clip_p.text(r.left_top() + vec2(4.0, 2.0), Align2::LEFT_TOP, app.scene_label(c.scene), FontId::proportional(9.5), theme::TEXT);
    let Some(pat) = pattern_ref(app, c.scene, pad).filter(|p| !p.notes.is_empty() && p.len_ticks > 0) else { return };
    // The notes, each repeat of the pattern across the clip.
    let (lo, hi) = pat.notes.iter().fold((127u8, 0u8), |(a, b), n| (a.min(n.pitch), b.max(n.pitch)));
    let span = (hi - lo).max(1) as f32;
    let body = Rect::from_min_max(r.left_top() + vec2(0.0, 14.0), r.right_bottom() - vec2(0.0, 3.0));
    let len = pat.len_ticks as u64;
    let first = c.offset / len;
    let mut rep = first;
    loop {
        let base = c.start as i64 + (rep * len) as i64 - c.offset as i64;
        if base >= c.end() as i64 || v.x_of(base as f64) > visible.right() {
            break;
        }
        for n in &pat.notes {
            let t = base + n.start as i64;
            if t < c.start as i64 || t >= c.end() as i64 {
                continue;
            }
            let x0 = v.x_of(t as f64);
            let x1 = v.x_of((t + n.len as i64).min(c.end() as i64) as f64).max(x0 + 1.5);
            let y = body.bottom() - (n.pitch - lo) as f32 / span * (body.height() - 2.0) - 1.0;
            clip_p.line_segment([Pos2::new(x0, y), Pos2::new(x1, y)], Stroke::new(2.0, colour));
        }
        rep += 1;
        if rep - first > 4096 {
            break;
        }
    }
}

/// Clicks and drags on one clip.
fn clip_events(app: &mut App, ui: &Ui, resp: &egui::Response, pad: usize, i: usize, v: &View2) {
    let Some(c) = app.arrangement.clips(pad).get(i).copied() else { return };
    let shift = ui.input(|inp| inp.modifiers.shift || inp.modifiers.command);
    if resp.clicked() {
        app.arr.section = None;
        if shift {
            match app.arr.selected.iter().position(|s| *s == (pad, i)) {
                Some(at) => drop(app.arr.selected.remove(at)),
                None => app.arr.selected.push((pad, i)),
            }
        } else {
            app.arr.selected = vec![(pad, i)];
        }
        app.select(pad);
    }
    if resp.double_clicked() {
        app.edit_clip(pad, i);
        return;
    }
    if resp.drag_started()
        && let Some(o) = ui.input(|inp| inp.pointer.press_origin())
    {
        app.arr.section = None;
        let r = resp.rect;
        let (x_start, x_end) = (v.x_of(c.start as f64), v.x_of(c.end() as f64));
        if (o.x - x_start).abs() < EDGE && x_end - x_start > EDGE * 3.0 {
            app.arr.drag = Some(ArrDrag::Trim { pad, index: i, end: false });
        } else if (x_end - o.x).abs() < EDGE {
            app.arr.drag = Some(ArrDrag::Trim { pad, index: i, end: true });
        } else {
            if !app.arr.selected.contains(&(pad, i)) {
                app.arr.selected = vec![(pad, i)];
            }
            app.arr.moving = app.selected_clips();
            app.arr.drag = Some(ArrDrag::Move { grab: v.tick_at(o.x) as u64 });
        }
        let _ = r;
    }
    resp.context_menu(|ui| {
        ui.label(egui::RichText::new(format!("{} · {}", pad_label(pad), app.scene_label(c.scene))).small().color(theme::DIM));
        if ui.button("Open its loop").clicked() {
            app.edit_clip(pad, i);
            ui.close();
        }
        ui.menu_button("Play another loop", |ui| {
            for k in 0..app.scenes.len() {
                let plays = pattern_ref(app, k, pad).is_some_and(|p| !p.notes.is_empty());
                if ui.add_enabled(plays && k != c.scene, egui::Button::new(app.scene_label(k))).clicked() {
                    let track = app.arrangement.tracks.iter_mut().find(|(p, _)| *p == pad);
                    if let Some(clip) = track.and_then(|(_, t)| t.get_mut(i)) {
                        clip.scene = k;
                    }
                    ui.close();
                }
            }
        });
        if ui.button("Make unique").on_hover_text("Copies the loop, so this clip can be edited without changing the others").clicked() {
            if !app.arr.selected.contains(&(pad, i)) {
                app.arr.selected = vec![(pad, i)];
            }
            app.make_unique();
            ui.close();
        }
        if ui.button("Duplicate    Ctrl+D").clicked() {
            app.arr.selected = vec![(pad, i)];
            app.duplicate_selection();
            ui.close();
        }
        if ui.button("Split at marker    Ctrl+E").clicked() {
            app.arr.selected = vec![(pad, i)];
            app.split_at_cursor();
            ui.close();
        }
        if ui.button("Delete    Del").clicked() {
            app.arr.selected = vec![(pad, i)];
            app.delete_selected_clips();
            ui.close();
        }
    });
}
