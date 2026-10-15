//! The Automation tab: the lanes of the loop on screen, one row each, drawn over the loop's
//! bars and beats. The song's lanes use the same rows, under the song timeline and lined up
//! with it.

use super::{theme, toggle};
use crate::app::{AUTO_SNAPS, App, AutoScope, PluginTarget};
use crate::engine::{AutoPoint, AutoTarget, PadKind, bend};
use crate::fx::FxKind;
use crate::music::{PPQ, TICKS_PER_BAR};
use crate::shared::{MASTER, NUM_BUSES, bus_label};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, vec2};

const HEAD_W: f32 = 190.0;
const LANE_H: f32 = 72.0;
/// How close the pointer must be to a point or a bend handle to pick it up, in points.
const GRAB: f32 = 8.0;
/// Most plugin parameters a menu lists at once; the filter narrows the rest.
const PARAM_LIST_MAX: usize = 200;

/// How a lane's ticks fall across the screen.
pub struct LaneMap {
    /// Width of the lane's header.
    pub head_w: f32,
    /// First tick in view and ticks per point; `None` fits the whole lane in the row.
    pub view: Option<(f64, f64)>,
    /// How long the lane runs, in ticks; points stay inside it.
    pub len: u32,
    /// The playhead, in the lane's ticks.
    pub now: Option<f64>,
}

pub fn draw(app: &mut App, ui: &mut Ui) {
    let scene = &app.scenes[app.scene];
    let len = scene.bars * TICKS_PER_BAR;
    let title = format!("AUTOMATION · LOOP {}{}", app.scene + 1, if scene.name.is_empty() { String::new() } else { format!(" {}", scene.name.to_uppercase()) });
    ui.horizontal_wrapped(|ui| {
        theme::title(ui, &title, theme::AMBER);
        if toggle(ui, app.auto.rec, "Rec", theme::RED)
            .on_hover_text("While playing, turning an automatable control (here or in a plugin's own window) writes it into this loop's lanes")
            .clicked()
        {
            app.auto.rec ^= true;
        }
        ui.label("Snap").on_hover_text("Hold Alt to place points off the grid");
        egui::ComboBox::from_id_salt("auto_snap").width(52.0).selected_text(AUTO_SNAPS[app.auto.snap].0).show_ui(ui, |ui| {
            for (i, (name, _)) in AUTO_SNAPS.iter().enumerate() {
                ui.selectable_value(&mut app.auto.snap, i, *name);
            }
        });
        add_lane_menu(app, ui);
        if let Some(target) = app.auto.touched {
            let name = app.target_name(target);
            if ui
                .button(format!("Automate {name}"))
                .on_hover_text("The plugin control you moved last, in the plugin's own window")
                .clicked()
            {
                app.automate(target);
            }
        }
        ui.label(
            egui::RichText::new("click: add · drag: move · ring: bend · right-click: delete")
                .small()
                .color(theme::DIM),
        );
    });
    ui.add_space(4.0);

    if app.lanes().is_empty() {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("Nothing is automated in this loop").size(16.0).color(theme::DIM));
            ui.label("Right-click a bus knob, a bus fader or pan, a pad's volume, pan or cutoff, or a drum knob and choose Automate.");
            ui.label("For a plugin, move the control in the plugin's window and click Automate here, or pick it from Add lane.");
            ui.label("Or turn on Rec, press play and move the control: the move is written into the loop.");
        });
        return;
    }
    let map = LaneMap { head_w: HEAD_W, view: None, len, now: app.lane_now(AutoScope::Loop) };
    lane_list(app, ui, &map);
}

/// Every lane in the current scope, one row each, scrolling; a lane's Remove takes it out.
fn lane_list(app: &mut App, ui: &mut Ui, map: &LaneMap) {
    let mut remove = None;
    egui::ScrollArea::vertical().id_salt("lanes").auto_shrink([false, false]).show(ui, |ui| {
        for i in 0..app.lanes().len() {
            if lane_row(app, ui, i, map) {
                remove = Some(i);
            }
            ui.add_space(4.0);
        }
    });
    if let Some(i) = remove {
        app.auto.drag = None;
        app.auto.bend = None;
        app.edit_lanes(|lanes| {
            lanes.remove(i);
        });
    }
}

/// The song's lanes, under the song timeline: `view` is the timeline's first tick and
/// ticks per point, and the lanes' grid starts at `grid_left`, as the timeline's does.
pub fn song_lanes(app: &mut App, ui: &mut Ui, grid_left: f32, view: (f64, f64)) {
    ui.horizontal(|ui| {
        theme::title(ui, "SONG AUTOMATION", theme::AMBER);
        if toggle(ui, app.auto.rec, "Rec", theme::RED)
            .on_hover_text("While the song plays, turning an automatable control writes it into the song's lanes")
            .clicked()
        {
            app.auto.rec ^= true;
        }
        ui.label("Snap").on_hover_text("Hold Alt to place points off the grid");
        egui::ComboBox::from_id_salt("song_auto_snap").width(52.0).selected_text(AUTO_SNAPS[app.auto.snap].0).show_ui(ui, |ui| {
            for (i, (name, _)) in AUTO_SNAPS.iter().enumerate() {
                ui.selectable_value(&mut app.auto.snap, i, *name);
            }
        });
        add_lane_menu(app, ui);
        if let Some(target) = app.auto.touched {
            let name = app.target_name(target);
            if ui.button(format!("Automate {name}")).on_hover_text("The plugin control you moved last, in the plugin's own window").clicked() {
                app.automate(target);
            }
        }
        ui.label(
            egui::RichText::new("plays over the loops' own lanes · click: add · drag: move · ring: bend · right-click: delete")
                .small()
                .color(theme::DIM),
        );
    });
    ui.add_space(2.0);
    if app.lanes().is_empty() {
        ui.label(
            egui::RichText::new("Nothing is automated across the song. Add a lane, or right-click a control and choose Automate while on the song view.")
                .color(theme::DIM),
        );
        return;
    }
    // The lanes' grid (8 points inside the area, which starts 6 after the header) starts
    // where the timeline's does.
    let head_w = grid_left - ui.max_rect().left() - 14.0;
    let map = LaneMap { head_w, view: Some(view), len: u32::MAX / 2, now: app.lane_now(AutoScope::Song) };
    lane_list(app, ui, &map);
}

/// A menu of everything that can be automated: every bus's level, pan and effect controls,
/// the selected pad's volume, pan, cutoff and drum knobs, and every loaded plugin's
/// parameters.
pub fn add_lane_menu(app: &mut App, ui: &mut Ui) {
    let mut pick = None;
    ui.menu_button("Add lane", |ui| {
        ui.set_min_width(180.0);
        for b in 0..NUM_BUSES {
            ui.menu_button(bus_label(b), |ui| {
                let mut item = |ui: &mut Ui, target: AutoTarget, name: &str| {
                    if ui.button(name).clicked() {
                        pick = Some(target);
                        ui.close();
                    }
                };
                item(ui, AutoTarget::BusLevel(b as u8), "Level");
                item(ui, AutoTarget::BusPan(b as u8), "Pan");
                for (s, slot) in app.buses[b].slots.iter().enumerate() {
                    if matches!(slot.kind, FxKind::None | FxKind::Vst) {
                        continue;
                    }
                    ui.separator();
                    ui.label(egui::RichText::new(slot.kind.name()).small().color(theme::DIM));
                    let names = slot.kind.param_names();
                    for (k, name) in names.iter().enumerate() {
                        item(ui, AutoTarget::BusParam { bus: b as u8, slot: s as u8, param: k as u8 }, name);
                    }
                    if let Some(names) = slot.kind.slider_names() {
                        for (k, name) in names.iter().enumerate() {
                            item(ui, AutoTarget::BusParam { bus: b as u8, slot: s as u8, param: 3 + k as u8 }, name);
                        }
                    }
                }
                if b == MASTER {
                    ui.label(egui::RichText::new("the master").small().color(theme::FAINT));
                }
            });
        }
        ui.separator();
        let pad = app.selected as u16;
        let cfg = app.pads[app.selected].cfg;
        ui.menu_button(format!("Pad {}", crate::music::pad_label(app.selected)), |ui| {
            for (target, name) in [(AutoTarget::PadVol(pad), "Volume"), (AutoTarget::PadPan(pad), "Pan"), (AutoTarget::PadCutoff(pad), "Filter cutoff")] {
                if ui.button(name).clicked() {
                    pick = Some(target);
                    ui.close();
                }
            }
            if cfg.kind == PadKind::Drum {
                ui.separator();
                ui.label(egui::RichText::new(format!("{} knobs", cfg.drum.engine.name())).small().color(theme::DIM));
                for (m, name) in cfg.drum.engine.macros().iter().enumerate() {
                    if ui.button(*name).clicked() {
                        pick = Some(AutoTarget::DrumMacro { pad, m: m as u8 });
                        ui.close();
                    }
                }
            }
        });
        let plugins: Vec<(PluginTarget, String, std::sync::Arc<Vec<crate::hosted::Param>>)> =
            app.automatable_plugins().into_iter().map(|(at, s)| (at, s.name.clone(), s.params.clone())).collect();
        if !plugins.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new("Plugins").small().color(theme::DIM));
        }
        for (at, name, params) in plugins {
            let place = match at {
                PluginTarget::Pad(i) => crate::music::pad_label(i),
                PluginTarget::Fx(b, s) => format!("{} slot {}", bus_label(b), s + 1),
                PluginTarget::Analyzer(k) => format!("Analyzer {}", k + 1),
            };
            ui.menu_button(format!("{place} · {name}"), |ui| {
                ui.add(egui::TextEdit::singleline(&mut app.auto.query).hint_text("Filter").desired_width(200.0));
                let query = app.auto.query.to_lowercase();
                let found: Vec<&crate::hosted::Param> = params.iter().filter(|p| query.is_empty() || p.name.to_lowercase().contains(&query)).collect();
                if found.is_empty() {
                    ui.label(egui::RichText::new("No automatable parameter matches").small().color(theme::FAINT));
                }
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for p in found.iter().take(PARAM_LIST_MAX) {
                        if ui.button(&p.name).clicked() {
                            pick = at.param_target(p.id);
                            ui.close();
                        }
                    }
                    if found.len() > PARAM_LIST_MAX {
                        ui.label(egui::RichText::new(format!("{} more: type to narrow the list", found.len() - PARAM_LIST_MAX)).small().color(theme::FAINT));
                    }
                });
            });
        }
    });
    if let Some(target) = pick {
        app.automate(target);
    }
}

/// One lane: its name and value on the left, the curve to the right. Returns true if
/// its delete button was pressed.
fn lane_row(app: &mut App, ui: &mut Ui, i: usize, map: &LaneMap) -> bool {
    let lane = app.lanes()[i].clone();
    let len = map.len;
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), LANE_H), Sense::hover());
    let head = Rect::from_min_size(row.min, vec2(map.head_w, LANE_H));
    let area = Rect::from_min_max(Pos2::new(head.right() + 6.0, row.top()), row.max);
    let p = ui.painter_at(row);
    let salt = map.view.is_some();

    // ---- Header.
    p.rect_filled(head, 6.0, theme::WELL);
    let name = app.target_name(lane.target);
    let name_rect = Rect::from_min_size(head.left_top() + vec2(8.0, 6.0), vec2(map.head_w - 16.0, 14.0));
    p.with_clip_rect(name_rect).text(name_rect.left_top(), Align2::LEFT_TOP, &name, theme::bold(11.0), theme::TEXT);
    ui.interact(name_rect, egui::Id::new(("auto_name", salt, i)), Sense::hover()).on_hover_text(&name);
    let now = map.now;
    let shown = now.and_then(|t| lane.value_at(t)).or_else(|| lane.points.first().map(|p| p.value));
    if let Some(v) = shown {
        p.text(head.left_top() + vec2(8.0, 24.0), Align2::LEFT_TOP, app.target_value_text(lane.target, v), FontId::monospace(11.0), theme::AMBER);
    }
    let mut delete = false;
    let mut clear = false;
    let buttons = Rect::from_min_size(head.left_bottom() + vec2(6.0, -28.0), vec2(map.head_w - 12.0, 24.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(buttons).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
        if ui.small_button("Flat").on_hover_text("Back to a single point at the control's value as set by hand").clicked() {
            clear = true;
        }
        if ui.small_button("Remove").on_hover_text("Stop automating this control here").clicked() {
            delete = true;
        }
    });

    // ---- The grid.
    p.rect_filled(area, 6.0, theme::WELL);
    p.rect_stroke(area, 6.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Inside);
    let screen = area.shrink2(vec2(8.0, 8.0));
    let (first, per_pt) = map.view.unwrap_or((0.0, len.max(1) as f64 / screen.width().max(1.0) as f64));
    let x_of = |tick: f32| screen.left() + ((tick as f64 - first) / per_pt) as f32;
    let y_of = |v: f32| screen.bottom() - v * screen.height();
    // Everything from here is drawn inside the lane, whatever is scrolled out of it.
    let p = p.with_clip_rect(area);
    let beat_px = (PPQ as f64 / per_pt) as f32;
    let last = (first + screen.width() as f64 * per_pt).min(len as f64) as u32;
    for t in ((first.max(0.0) as u32 / PPQ * PPQ)..=last).step_by(PPQ as usize) {
        let bar = t % TICKS_PER_BAR == 0;
        if !bar && beat_px < 8.0 {
            continue;
        }
        p.vline(x_of(t as f32), screen.y_range(), Stroke::new(1.0, if bar { theme::FAINT } else { Color32::from_rgb(30, 33, 37) }));
        if bar && t < len && beat_px * 4.0 >= 18.0 {
            p.text(Pos2::new(x_of(t as f32) + 3.0, area.top() + 1.0), Align2::LEFT_TOP, (t / TICKS_PER_BAR + 1).to_string(), FontId::proportional(9.0), theme::FAINT);
        }
    }
    p.hline(screen.x_range(), y_of(0.5), Stroke::new(1.0, Color32::from_rgb(30, 33, 37)));
    // Where the control sits when nothing moves it.
    let base = app.control_value(lane.target);
    p.hline(screen.x_range(), y_of(base), Stroke::new(1.0, theme::FAINT.gamma_multiply(0.6)));

    // ---- Editing. Points are picked before bend handles, and both before adding.
    let resp = ui.interact(area, egui::Id::new(("auto_lane", salt, i)), Sense::click_and_drag());
    let alt = ui.input(|inp| inp.modifiers.alt);
    let step = if alt { 1 } else { AUTO_SNAPS[app.auto.snap].1.max(1) };
    let tick_at = |x: f32| {
        let raw = (first + (x.clamp(screen.left(), screen.right()) - screen.left()) as f64 * per_pt).max(0.0);
        (((raw / step as f64).round() as u32) * step).min(len.saturating_sub(1))
    };
    let value_at = |y: f32| ((screen.bottom() - y) / screen.height()).clamp(0.0, 1.0);
    let mut points = lane.points.clone();
    let near = |pts: &[AutoPoint], pos: Pos2| {
        pts.iter()
            .enumerate()
            .map(|(k, pt)| (k, Pos2::new(x_of(pt.tick as f32), y_of(pt.value)).distance(pos)))
            .filter(|(_, d)| *d <= GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(k, _)| k)
    };
    // The bend handle of the segment leaving point k sits halfway along it in time.
    let handle_of = |pts: &[AutoPoint], k: usize| {
        let (a, b) = (pts[k], *pts.get(k + 1)?);
        if (b.value - a.value).abs() < 0.01 || x_of(b.tick as f32) - x_of(a.tick as f32) < 3.0 * GRAB {
            return None;
        }
        let v = a.value + (b.value - a.value) * bend(0.5, a.curve);
        Some(Pos2::new((x_of(a.tick as f32) + x_of(b.tick as f32)) * 0.5, y_of(v)))
    };
    let near_handle = |pts: &[AutoPoint], pos: Pos2| {
        (0..pts.len().saturating_sub(1))
            .filter_map(|k| Some((k, handle_of(pts, k)?.distance(pos))))
            .filter(|(_, d)| *d <= GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(k, _)| k)
    };
    let mut changed = false;
    if resp.drag_started()
        && let Some(o) = ui.input(|inp| inp.pointer.press_origin())
    {
        if let Some(k) = near(&points, o) {
            app.auto.drag = Some((i, k));
        } else if let Some(k) = near_handle(&points, o) {
            app.auto.bend = Some((i, k));
        } else {
            points.push(AutoPoint::new(tick_at(o.x), value_at(o.y)));
            changed = true;
            let last = points.len() - 1;
            app.auto.drag = Some((i, sort_points(&mut points, last)));
        }
    }
    if resp.dragged()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        if let Some((lane_i, k)) = app.auto.drag
            && lane_i == i
            && k < points.len()
        {
            let curve = points[k].curve;
            points[k] = AutoPoint { tick: tick_at(pos.x), value: value_at(pos.y), curve };
            app.auto.drag = Some((i, sort_points(&mut points, k)));
            changed = true;
        }
        if let Some((lane_i, k)) = app.auto.bend
            && lane_i == i
            && k + 1 < points.len()
        {
            points[k].curve = curve_through(points[k].value, points[k + 1].value, value_at(pos.y));
            changed = true;
        }
    }
    if resp.drag_stopped() {
        app.auto.drag = None;
        app.auto.bend = None;
    }
    if resp.double_clicked()
        && let Some(k) = resp.interact_pointer_pos().and_then(|pos| near_handle(&points, pos))
    {
        points[k].curve = 0.0;
        changed = true;
    } else if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && near(&points, pos).is_none()
        && near_handle(&points, pos).is_none()
    {
        points.push(AutoPoint::new(tick_at(pos.x), value_at(pos.y)));
        let last = points.len() - 1;
        sort_points(&mut points, last);
        changed = true;
    }
    if resp.secondary_clicked()
        && let Some(k) = resp.interact_pointer_pos().and_then(|pos| near(&points, pos))
    {
        points.remove(k);
        changed = true;
    }

    // ---- The curve, as edited this frame.
    if let (Some(first), Some(last)) = (points.first(), points.last()) {
        let mut line = vec![Pos2::new(screen.left(), y_of(first.value))];
        for (k, pt) in points.iter().enumerate() {
            line.push(Pos2::new(x_of(pt.tick as f32), y_of(pt.value)));
            if let Some(next) = points.get(k + 1)
                && pt.curve.abs() > 1e-3
            {
                // A bent segment is drawn as a run of short straight pieces.
                let steps = ((x_of(next.tick as f32) - x_of(pt.tick as f32)) / 3.0).clamp(2.0, 64.0) as usize;
                for s in 1..steps {
                    let t = s as f32 / steps as f32;
                    let tick = pt.tick as f32 + (next.tick - pt.tick) as f32 * t;
                    let v = pt.value + (next.value - pt.value) * bend(t, pt.curve);
                    line.push(Pos2::new(x_of(tick), y_of(v)));
                }
            }
        }
        line.push(Pos2::new(screen.right(), y_of(last.value)));
        // Shade under the curve a slice at a time (the area is not convex in general).
        for w in line.windows(2) {
            let quad = vec![w[0], w[1], Pos2::new(w[1].x, screen.bottom()), Pos2::new(w[0].x, screen.bottom())];
            p.add(egui::Shape::convex_polygon(quad, theme::AMBER.gamma_multiply(0.12), Stroke::NONE));
        }
        p.add(egui::Shape::line(line, Stroke::new(1.8, theme::AMBER)));
    }
    if let Some(t) = now {
        p.vline(x_of(t as f32), area.y_range(), Stroke::new(1.5, theme::GREEN));
    }
    // Bend handles: small rings, brighter under the pointer.
    let hover_pos = resp.hover_pos();
    let hover_handle = hover_pos.filter(|pos| near(&points, *pos).is_none()).and_then(|pos| near_handle(&points, pos));
    for k in 0..points.len().saturating_sub(1) {
        let Some(c) = handle_of(&points, k) else { continue };
        let hot = hover_handle == Some(k) || app.auto.bend == Some((i, k));
        p.circle_filled(c, 3.5, theme::WELL);
        p.circle_stroke(c, if hot { 4.5 } else { 3.5 }, Stroke::new(1.2, if hot { theme::TEXT } else { theme::AMBER.gamma_multiply(0.7) }));
    }
    // Points over the line, the one under the pointer or being dragged larger.
    let hover = hover_pos.and_then(|pos| near(&points, pos));
    for (k, pt) in points.iter().enumerate() {
        let hot = hover == Some(k) || app.auto.drag == Some((i, k));
        let c = Pos2::new(x_of(pt.tick as f32), y_of(pt.value));
        p.circle_filled(c, if hot { 5.5 } else { 4.0 }, if hot { theme::TEXT } else { theme::AMBER });
        p.circle_stroke(c, if hot { 5.5 } else { 4.0 }, Stroke::new(1.0, Color32::BLACK));
    }
    let shown_point = app.auto.drag.filter(|(l, _)| *l == i).map(|(_, k)| k).or(hover);
    if let Some(pt) = shown_point.and_then(|k| points.get(k)) {
        let bar = pt.tick / TICKS_PER_BAR + 1;
        let beat = pt.tick % TICKS_PER_BAR / PPQ + 1;
        let text = format!("{bar}.{beat} · {}", app.target_value_text(lane.target, pt.value));
        p.text(area.right_top() + vec2(-8.0, 3.0), Align2::RIGHT_TOP, text, FontId::monospace(10.0), theme::TEXT);
    } else if let Some(k) = app.auto.bend.filter(|(l, _)| *l == i).map(|(_, k)| k).or(hover_handle)
        && let Some(pt) = points.get(k)
    {
        let text = if pt.curve.abs() < 1e-3 { "straight".to_string() } else { format!("bend {:+.0}% · double-click: straighten", -pt.curve * 100.0) };
        p.text(area.right_top() + vec2(-8.0, 3.0), Align2::RIGHT_TOP, text, FontId::monospace(10.0), theme::TEXT);
    }

    if clear {
        let value = base;
        app.edit_lanes(|lanes| lanes[i].points = vec![AutoPoint::new(0, value)]);
    } else if changed {
        app.edit_lanes(|lanes| lanes[i].points = points);
    }
    delete
}

/// The bend that makes a segment from `a` to `b` pass through `mid` halfway along.
fn curve_through(a: f32, b: f32, mid: f32) -> f32 {
    if (b - a).abs() < 1e-4 {
        return 0.0;
    }
    let f = ((mid - a) / (b - a)).clamp(0.02, 0.98);
    // bend(0.5, c) is 0.5^e for c > 0 and 1 - 0.5^e for c < 0, with e = 8^|c|.
    let (e, sign) = if f <= 0.5 { (f.ln() / 0.5f32.ln(), 1.0) } else { ((1.0 - f).ln() / 0.5f32.ln(), -1.0) };
    (sign * e.max(1.0).ln() / 8f32.ln()).clamp(-1.0, 1.0)
}

/// Puts the point at `k` back in order after it moved, and returns where it went.
fn sort_points(points: &mut Vec<AutoPoint>, k: usize) -> usize {
    let moved = points.remove(k);
    let at = points.partition_point(|p| p.tick <= moved.tick);
    points.insert(at, moved);
    at
}

/// A small mark on a control that automation moves in the loop on screen.
pub fn mark_automated(p: &egui::Painter, rect: Rect, colour: Color32) {
    let c = rect.right_top() + vec2(-3.0, 3.0);
    p.circle_filled(c, 3.0, colour);
    p.circle_stroke(c, 3.0, Stroke::new(1.0, Color32::BLACK));
}

/// While the transport runs, a thin amber bar along the bottom of an automated control
/// shows where automation has it now (the control itself shows where it was set by hand).
fn show_automated_value(app: &App, p: &egui::Painter, rect: Rect, target: AutoTarget) {
    let Some(tick) = app.lane_now(app.auto_scope()) else { return };
    let Some(v) = app.lanes().iter().find(|l| l.target == target).and_then(|l| l.value_at(tick)) else { return };
    let bar = Rect::from_min_max(Pos2::new(rect.left() + 2.0, rect.bottom() - 2.0), Pos2::new(rect.right() - 2.0, rect.bottom()));
    p.rect_filled(bar, 1.0, Color32::from_black_alpha(160));
    let x = |t: f32| bar.left() + t * bar.width();
    // Pans read from the centre.
    let from: f32 = if matches!(target, AutoTarget::BusPan(_) | AutoTarget::PadPan(_)) { 0.5 } else { 0.0 };
    let (a, b) = (from.min(v), from.max(v));
    p.rect_filled(Rect::from_min_max(Pos2::new(x(a), bar.top()), Pos2::new(x(b).max(x(a) + 1.5), bar.bottom())), 1.0, theme::AMBER);
}

/// Adds "Automate" to a control's right-click menu, and records the control while Rec is
/// on and it is being moved. `raw` is the control's own value.
pub fn hook(app: &mut App, ui: &Ui, resp: &egui::Response, target: AutoTarget, raw: f32) {
    if app.is_automated(target) {
        mark_automated(ui.painter(), resp.rect, theme::AMBER);
        show_automated_value(app, ui.painter(), resp.rect, target);
    }
    if resp.dragged() {
        app.record_control(target, raw);
    }
    let mut pick = false;
    resp.context_menu(|ui| {
        ui.label(egui::RichText::new(app.target_name(target)).small().color(theme::DIM));
        let label = if app.is_automated(target) { "Edit automation" } else { "Automate" };
        if ui.button(label).clicked() {
            pick = true;
            ui.close();
        }
    });
    if pick {
        app.automate(target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handle_dragged_to_a_height_bends_the_segment_through_it() {
        for (a, b, mid) in [(0.0, 1.0, 0.2), (0.0, 1.0, 0.8), (1.0, 0.0, 0.3), (0.2, 0.6, 0.5)] {
            let c = curve_through(a, b, mid);
            let got = a + (b - a) * bend(0.5, c);
            assert!((got - mid).abs() < 0.01, "{a}->{b} through {mid}: curve {c} gives {got}");
        }
        assert_eq!(curve_through(0.0, 1.0, 0.5), 0.0);
    }
}
