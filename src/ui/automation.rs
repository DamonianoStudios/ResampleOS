//! The Automation tab: the lanes of the loop on screen, one row each, drawn over the loop's
//! bars and beats.

use super::{theme, toggle};
use crate::app::{AUTO_SNAPS, App};
use crate::engine::{AutoPoint, AutoTarget};
use crate::fx::FxKind;
use crate::music::{PPQ, TICKS_PER_BAR};
use crate::shared::{MASTER, NUM_BUSES, bus_label};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, vec2};

const HEAD_W: f32 = 190.0;
const LANE_H: f32 = 72.0;
/// How close the pointer must be to a point to pick it up, in points.
const GRAB: f32 = 8.0;

pub fn draw(app: &mut App, ui: &mut Ui) {
    let scene = &app.scenes[app.scene];
    let len = scene.bars * TICKS_PER_BAR;
    let title = format!("AUTOMATION · LOOP {}{}", app.scene + 1, if scene.name.is_empty() { String::new() } else { format!(" {}", scene.name.to_uppercase()) });
    ui.horizontal_wrapped(|ui| {
        theme::title(ui, &title, theme::AMBER);
        if toggle(ui, app.auto.rec, "Rec", theme::RED)
            .on_hover_text("While playing, turning an automatable control writes it into this loop's lanes")
            .clicked()
        {
            app.auto.rec ^= true;
        }
        ui.label("Snap");
        egui::ComboBox::from_id_salt("auto_snap").width(52.0).selected_text(AUTO_SNAPS[app.auto.snap].0).show_ui(ui, |ui| {
            for (i, (name, _)) in AUTO_SNAPS.iter().enumerate() {
                ui.selectable_value(&mut app.auto.snap, i, *name);
            }
        });
        add_lane_menu(app, ui);
        ui.label(
            egui::RichText::new("click: add a point · drag: move · right-click: delete · Alt: no snap · right-click any knob or fader: Automate")
                .small()
                .color(theme::DIM),
        );
    });
    ui.add_space(4.0);

    if app.lanes().is_empty() {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("Nothing is automated in this loop").size(16.0).color(theme::DIM));
            ui.label("Right-click a bus knob, a bus fader or pan, or a pad's volume, pan or cutoff and choose Automate.");
            ui.label("Or turn on Rec, press play and move the control: the move is written into the loop.");
        });
        return;
    }
    let mut remove = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for i in 0..app.lanes().len() {
            if lane_row(app, ui, i, len) {
                remove = Some(i);
            }
            ui.add_space(4.0);
        }
    });
    if let Some(i) = remove {
        app.auto.drag = None;
        app.edit_lanes(|lanes| {
            lanes.remove(i);
        });
    }
}

/// A menu of everything that can be automated: every bus's level, pan and effect controls,
/// and the selected pad's volume, pan and cutoff.
fn add_lane_menu(app: &mut App, ui: &mut Ui) {
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
        ui.menu_button(format!("Pad {}", crate::music::pad_label(app.selected)), |ui| {
            for (target, name) in [(AutoTarget::PadVol(pad), "Volume"), (AutoTarget::PadPan(pad), "Pan"), (AutoTarget::PadCutoff(pad), "Filter cutoff")] {
                if ui.button(name).clicked() {
                    pick = Some(target);
                    ui.close();
                }
            }
        });
    });
    if let Some(target) = pick {
        app.automate(target);
    }
}

/// One lane: its name and value on the left, the curve to the right. Returns true if
/// its delete button was pressed.
fn lane_row(app: &mut App, ui: &mut Ui, i: usize, len: u32) -> bool {
    let lane = app.lanes()[i].clone();
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), LANE_H), Sense::hover());
    let head = Rect::from_min_size(row.min, vec2(HEAD_W, LANE_H));
    let area = Rect::from_min_max(Pos2::new(head.right() + 6.0, row.top()), row.max);
    let p = ui.painter_at(row);

    // ---- Header.
    p.rect_filled(head, 6.0, theme::WELL);
    let name = app.target_name(lane.target);
    p.text(head.left_top() + vec2(8.0, 6.0), Align2::LEFT_TOP, name, theme::bold(11.0), theme::TEXT);
    let now = app.playing().then(|| app.shared.tick().rem_euclid(len.max(1) as f64));
    let shown = now.and_then(|t| lane.value_at(t)).or_else(|| lane.points.first().map(|p| p.value));
    if let Some(v) = shown {
        p.text(head.left_top() + vec2(8.0, 24.0), Align2::LEFT_TOP, app.target_value_text(lane.target, v), FontId::monospace(11.0), theme::AMBER);
    }
    let mut delete = false;
    let mut clear = false;
    let buttons = Rect::from_min_size(head.left_bottom() + vec2(6.0, -28.0), vec2(HEAD_W - 12.0, 24.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(buttons).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
        if ui.small_button("Flat").on_hover_text("Back to a single point at the control's value as set by hand").clicked() {
            clear = true;
        }
        if ui.small_button("Remove").on_hover_text("Stop automating this control in this loop").clicked() {
            delete = true;
        }
    });

    // ---- The curve.
    p.rect_filled(area, 6.0, theme::WELL);
    p.rect_stroke(area, 6.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Inside);
    let screen = area.shrink2(vec2(8.0, 8.0));
    let x_of = |tick: f32| screen.left() + tick / len.max(1) as f32 * screen.width();
    let y_of = |v: f32| screen.bottom() - v * screen.height();
    let beat_px = PPQ as f32 / len.max(1) as f32 * screen.width();
    for t in (0..=len).step_by(PPQ as usize) {
        let bar = t % TICKS_PER_BAR == 0;
        if !bar && beat_px < 8.0 {
            continue;
        }
        p.vline(x_of(t as f32), screen.y_range(), Stroke::new(1.0, if bar { theme::FAINT } else { Color32::from_rgb(30, 33, 37) }));
        if bar && t < len {
            p.text(Pos2::new(x_of(t as f32) + 3.0, area.top() + 1.0), Align2::LEFT_TOP, (t / TICKS_PER_BAR + 1).to_string(), FontId::proportional(9.0), theme::FAINT);
        }
    }
    p.hline(screen.x_range(), y_of(0.5), Stroke::new(1.0, Color32::from_rgb(30, 33, 37)));
    // Where the control sits when nothing moves it.
    let base = app.control_value(lane.target);
    p.hline(screen.x_range(), y_of(base), Stroke::new(1.0, theme::FAINT.gamma_multiply(0.6)));

    if let (Some(first), Some(last)) = (lane.points.first(), lane.points.last()) {
        let mut line = vec![Pos2::new(screen.left(), y_of(first.value))];
        line.extend(lane.points.iter().map(|pt| Pos2::new(x_of(pt.tick as f32), y_of(pt.value))));
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

    // ---- Editing.
    let resp = ui.interact(area, egui::Id::new(("auto_lane", i)), Sense::click_and_drag());
    let alt = ui.input(|inp| inp.modifiers.alt);
    let step = if alt { 1 } else { AUTO_SNAPS[app.auto.snap].1.max(1) };
    let tick_at = |x: f32| {
        let raw = ((x - screen.left()) / screen.width()).clamp(0.0, 1.0) * len as f32;
        (((raw / step as f32).round() as u32) * step).min(len.saturating_sub(1))
    };
    let value_at = |y: f32| ((screen.bottom() - y) / screen.height()).clamp(0.0, 1.0);
    let nearest = |pos: Pos2| {
        lane.points
            .iter()
            .enumerate()
            .map(|(k, pt)| (k, Pos2::new(x_of(pt.tick as f32), y_of(pt.value)).distance(pos)))
            .filter(|(_, d)| *d <= GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(k, _)| k)
    };
    let mut points = lane.points.clone();
    let mut changed = false;
    if resp.drag_started()
        && let Some(o) = ui.input(|inp| inp.pointer.press_origin())
    {
        let k = match nearest(o) {
            Some(k) => k,
            None => {
                points.push(AutoPoint { tick: tick_at(o.x), value: value_at(o.y) });
                changed = true;
                let last = points.len() - 1;
                sort_points(&mut points, last)
            }
        };
        app.auto.drag = Some((i, k));
    }
    if resp.dragged()
        && let (Some((lane_i, k)), Some(pos)) = (app.auto.drag, resp.interact_pointer_pos())
        && lane_i == i
        && k < points.len()
    {
        points[k] = AutoPoint { tick: tick_at(pos.x), value: value_at(pos.y) };
        app.auto.drag = Some((i, sort_points(&mut points, k)));
        changed = true;
    }
    if resp.drag_stopped() {
        app.auto.drag = None;
    }
    if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && nearest(pos).is_none()
    {
        points.push(AutoPoint { tick: tick_at(pos.x), value: value_at(pos.y) });
        let last = points.len() - 1;
        sort_points(&mut points, last);
        changed = true;
    }
    if resp.secondary_clicked()
        && let Some(k) = resp.interact_pointer_pos().and_then(nearest)
    {
        points.remove(k);
        changed = true;
    }
    // Draw the points over the line, the one under the pointer or being dragged larger.
    let hover = resp.hover_pos().and_then(nearest);
    for (k, pt) in points.iter().enumerate() {
        let hot = hover == Some(k) || app.auto.drag == Some((i, k));
        let c = Pos2::new(x_of(pt.tick as f32), y_of(pt.value));
        p.circle_filled(c, if hot { 5.5 } else { 4.0 }, if hot { theme::TEXT } else { theme::AMBER });
        p.circle_stroke(c, if hot { 5.5 } else { 4.0 }, Stroke::new(1.0, Color32::BLACK));
    }
    if hover.is_some() || app.auto.drag.is_some_and(|(l, _)| l == i) {
        let k = app.auto.drag.filter(|(l, _)| *l == i).map(|(_, k)| k).or(hover);
        if let Some(pt) = k.and_then(|k| points.get(k)) {
            let bar = pt.tick / TICKS_PER_BAR + 1;
            let beat = pt.tick % TICKS_PER_BAR / PPQ + 1;
            let text = format!("{bar}.{beat} · {}", app.target_value_text(lane.target, pt.value));
            p.text(area.right_top() + vec2(-8.0, 3.0), Align2::RIGHT_TOP, text, FontId::monospace(10.0), theme::TEXT);
        }
    }

    if clear {
        let value = base;
        app.edit_lanes(|lanes| lanes[i].points = vec![AutoPoint { tick: 0, value }]);
    } else if changed {
        app.edit_lanes(|lanes| lanes[i].points = points);
    }
    delete
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

/// Adds "Automate" to a control's right-click menu, and records the control while Rec is
/// on and it is being moved. `raw` is the control's own value.
pub fn hook(app: &mut App, ui: &Ui, resp: &egui::Response, target: AutoTarget, raw: f32) {
    if app.is_automated(target) {
        mark_automated(ui.painter(), resp.rect, theme::AMBER);
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
