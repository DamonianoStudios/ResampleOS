//! Loops in the pad zone: the strip that picks and edits them, the chain, and the
//! perform-mode launcher that takes the place of the pad matrix.

use super::{pill, theme, toggle};
use crate::app::{App, MAX_CHAIN, MAX_SCENES, PAD_KEY_LABELS};
use crate::music::TICKS_PER_BAR;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, vec2};

/// What the user asked for this frame; run after drawing so the borrow of `app` is free.
enum Act {
    Launch(usize),
    Add(bool),
    Clear,
    Remove(usize),
    Chain(usize),
}

fn run(app: &mut App, act: Act) {
    match act {
        Act::Launch(i) => app.launch_scene(i),
        Act::Add(copy) => {
            if let Some(i) = app.add_scene(copy) {
                app.launch_scene(i);
            }
        }
        Act::Clear => app.clear_scene(),
        Act::Remove(i) => app.remove_scene(i),
        Act::Chain(i) => app.chain_push(i),
    }
}

fn blink(ui: &Ui) -> bool {
    ui.input(|i| i.time * 3.0).fract() < 0.5
}

/// The loop strip and, under it, the chain.
pub fn strip(app: &mut App, ui: &mut Ui) {
    let mut act = None;
    let shift = ui.input(|i| i.modifiers.shift);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        theme::title(ui, "LOOPS", theme::GREEN);
        ui.add_space(2.0);
        for i in 0..app.scenes.len() {
            let tab = pill(ui, app.scene == i, &(i + 1).to_string());
            if app.queued_scene == Some(i) && blink(ui) {
                ui.painter().rect_stroke(tab.rect, 6.0, Stroke::new(1.5, theme::AMBER), StrokeKind::Inside);
            }
            if app.scene_pad_count(i) > 0 && app.scene != i {
                ui.painter().circle_filled(tab.rect.right_top() + vec2(-6.0, 6.0), 2.0, theme::GREEN);
            }
            if tab.clicked() {
                act = Some(if shift { Act::Chain(i) } else { Act::Launch(i) });
            }
            let tab = tab.on_hover_text(format!(
                "{}: {} bars. Click to switch (at the end of the playing loop while the transport runs). Shift+click adds it to the chain.",
                app.scene_label(i),
                app.scenes[i].bars
            ));
            tab.context_menu(|ui| {
                ui.label(egui::RichText::new(app.scene_label(i)).strong());
                ui.add(egui::TextEdit::singleline(&mut app.scenes[i].name).hint_text("Name").desired_width(120.0));
                if ui.button("Add to chain").clicked() {
                    act = Some(Act::Chain(i));
                    ui.close();
                }
                if ui.add_enabled(app.scenes.len() > 1, egui::Button::new("Delete loop")).clicked() {
                    act = Some(Act::Remove(i));
                    ui.close();
                }
            });
        }
        if app.scenes.len() < MAX_SCENES {
            let add = pill(ui, false, "+").on_hover_text("New loop: a copy of this one. Shift+click for an empty one.");
            if add.clicked() {
                act = Some(Act::Add(!shift));
            }
        }
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Bars").color(theme::DIM));
        let mut bars = app.scenes[app.scene].bars;
        egui::ComboBox::from_id_salt("scene_bars").width(38.0).selected_text(bars.to_string()).show_ui(ui, |ui| {
            for b in [1, 2, 4, 8, 12, 16] {
                ui.selectable_value(&mut bars, b, b.to_string());
            }
        });
        if bars != app.scenes[app.scene].bars {
            app.set_scene_bars(bars);
        }
        if ui.button("Clear").on_hover_text("Remove every note in this loop").clicked() {
            act = Some(Act::Clear);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if toggle(ui, app.perform, "PERFORM", theme::GREEN)
                .on_hover_text("P. The pads become loop launchers: pad keys and controller pads start loops in time.")
                .clicked()
            {
                app.perform ^= true;
            }
        });
    });

    if app.chain.is_empty() && !app.perform {
        if let Some(act) = act {
            run(app, act);
        }
        return;
    }
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if toggle(ui, app.chain_on, "CHAIN", theme::AMBER)
            .on_hover_text("Play the loops below one after another, starting from the first, and go round")
            .clicked()
        {
            app.chain_on = !app.chain_on && !app.chain.is_empty();
            app.chain_pos = 0;
        }
        ui.add_space(4.0);
        if app.chain.is_empty() {
            ui.label(egui::RichText::new("Shift+click a loop to add it here").small().color(theme::FAINT));
        }
        let playing = app.chain_on && app.playing();
        let pointer = ui.ctx().pointer_interact_pos();
        let mut slots = Vec::with_capacity(app.chain.len());
        let mut released = false;
        for (at, &scene) in app.chain.iter().enumerate() {
            let now = playing && at == app.chain_pos;
            let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 20.0), Sense::click_and_drag());
            slots.push(rect);
            let dragged = app.chain_drag == Some(at);
            let p = ui.painter();
            let fill = if now { theme::GREEN } else if resp.hovered() || dragged { theme::HOVER } else { theme::RAISED };
            p.rect_filled(rect, 4.0, if dragged { fill.gamma_multiply(0.45) } else { fill });
            let text = if now { theme::BG } else { theme::TEXT };
            p.text(rect.center(), Align2::CENTER_CENTER, (scene + 1).to_string(), theme::bold(11.0), text);
            if resp.drag_started() {
                app.chain_drag = Some(at);
            }
            released |= resp.drag_stopped();
            if resp.on_hover_text("Drag to reorder. Click to remove from the chain").clicked() {
                remove = Some(at);
            }
        }
        // While an entry is dragged, the slot nearest the pointer is where it will land.
        if let (Some(from), Some(pos)) = (app.chain_drag, pointer) {
            let target = slots
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.center().distance(pos).total_cmp(&b.1.center().distance(pos)))
                .map(|(i, _)| i);
            if let Some(to) = target {
                if to != from {
                    ui.painter().rect_stroke(slots[to].expand(1.5), 5.0, Stroke::new(2.0, theme::AMBER), StrokeKind::Outside);
                }
                if released {
                    app.chain_move(from, to);
                }
            }
            // The entry follows the pointer while it is carried.
            if !released && let Some(&scene) = app.chain.get(from) {
                let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("chain_drag"));
                let chip = egui::Rect::from_center_size(pos, vec2(22.0, 20.0));
                let lp = ui.ctx().layer_painter(layer);
                lp.rect_filled(chip, 4.0, theme::AMBER);
                lp.text(chip.center(), Align2::CENTER_CENTER, (scene + 1).to_string(), theme::bold(11.0), theme::BG);
            }
        }
        if released || !ui.input(|i| i.pointer.any_down()) {
            app.chain_drag = None;
        }
        if !app.chain.is_empty() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(format!("{}/{MAX_CHAIN}", app.chain.len())).small().color(theme::FAINT));
            if ui.small_button("Clear").clicked() {
                app.chain.clear();
                app.chain_on = false;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = if app.launch_on_bar { "next bar" } else { "loop end" };
            if ui
                .button(label)
                .on_hover_text("When a launched loop takes over: at the end of the playing loop, or on the next bar line")
                .clicked()
            {
                app.launch_on_bar ^= true;
            }
            ui.label(egui::RichText::new("Switch at").small().color(theme::DIM));
        });
    });
    if let Some(at) = remove {
        app.chain.remove(at);
        app.chain_pos = 0;
        app.chain_on &= !app.chain.is_empty();
    }
    if let Some(act) = act {
        run(app, act);
    }
}

/// Perform mode: sixteen launch pads, laid out like the pad matrix, one per loop.
pub fn launcher(app: &mut App, ui: &mut Ui, area: Rect) {
    let cell = area.width() / 4.0;
    let gap = (cell * 0.05).clamp(1.0, 5.0);
    let mut act = None;
    let lit = blink(ui);
    app.pad_rects.clear();
    for (i, key_label) in PAD_KEY_LABELS.iter().enumerate() {
        // Loop 1 sits bottom-left, where pad 1 is.
        let (cx, cy) = (i % 4, 3 - i / 4);
        let rect = Rect::from_min_size(area.min + vec2(cx as f32 * cell, cy as f32 * cell), vec2(cell, cell)).shrink(gap);
        let exists = i < app.scenes.len();
        let resp = ui.interact(rect, egui::Id::new(("scene_pad", i)), Sense::click());
        let playing = exists && app.scene == i && app.playing();
        let queued = app.queued_scene == Some(i);
        let current = exists && app.scene == i;

        let p = ui.painter_at(rect.expand(4.0));
        let radius = (rect.width() * 0.09).min(10.0);
        let body = if playing {
            Color32::from_rgb(26, 70, 44)
        } else if !exists {
            theme::WELL
        } else if resp.hovered() {
            theme::HOVER
        } else {
            theme::RAISED
        };
        if playing {
            p.rect_filled(rect.expand(2.5), radius + 2.0, theme::GREEN.gamma_multiply(0.22));
        }
        p.rect_filled(rect, radius, body);
        let border = if queued && lit {
            theme::AMBER
        } else if playing {
            theme::GREEN
        } else if current {
            theme::TEXT
        } else {
            theme::LINE
        };
        p.rect_stroke(rect, radius, Stroke::new(if current || queued { 2.0 } else { 1.0 }, border), StrokeKind::Inside);
        p.text(
            rect.right_bottom() - vec2(6.0, 4.0),
            Align2::RIGHT_BOTTOM,
            *key_label,
            FontId::monospace(10.0),
            theme::FAINT,
        );
        if !exists {
            p.text(rect.center(), Align2::CENTER_CENTER, (i + 1).to_string(), theme::bold(18.0), theme::LINE);
            continue;
        }

        let used = app.scene_pad_count(i);
        let head = if used == 0 { theme::FAINT } else { theme::TEXT };
        p.text(rect.left_top() + vec2(9.0, 6.0), Align2::LEFT_TOP, (i + 1).to_string(), theme::bold(22.0), head);
        if rect.width() > 80.0 {
            let name: String = app.scene_label(i).chars().take((rect.width() / 8.0) as usize).collect();
            p.text(rect.left_top() + vec2(10.0, 36.0), Align2::LEFT_TOP, name, FontId::proportional(11.0), theme::DIM);
            let bars = app.scenes[i].bars;
            let info = format!("{bars} bar{} · {used} pad{}", if bars == 1 { "" } else { "s" }, if used == 1 { "" } else { "s" });
            p.text(rect.left_bottom() + vec2(10.0, -8.0), Align2::LEFT_BOTTOM, info, FontId::proportional(10.5), theme::DIM);
            if queued {
                p.text(rect.right_top() + vec2(-9.0, 8.0), Align2::RIGHT_TOP, "NEXT", theme::bold(10.0), theme::AMBER);
            }
        }
        if playing {
            // How far through the loop the transport is.
            let len = (app.scenes[i].bars * TICKS_PER_BAR) as f32;
            let done = (app.shared.tick() as f32 / len).clamp(0.0, 1.0);
            let track = Rect::from_min_max(
                Pos2::new(rect.left() + 10.0, rect.center().y + 6.0),
                Pos2::new(rect.right() - 10.0, rect.center().y + 10.0),
            );
            p.rect_filled(track, 2.0, theme::BG.gamma_multiply(0.6));
            p.rect_filled(Rect::from_min_max(track.min, Pos2::new(track.left() + track.width() * done, track.bottom())), 2.0, theme::GREEN);
        }

        if resp.clicked() {
            act = Some(if ui.input(|i| i.modifiers.shift) { Act::Chain(i) } else { Act::Launch(i) });
        }
        resp.context_menu(|ui| {
            if ui.button("Add to chain").clicked() {
                act = Some(Act::Chain(i));
                ui.close();
            }
        });
    }
    if let Some(act) = act {
        run(app, act);
    }
}
