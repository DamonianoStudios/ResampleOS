//! Step sequencer: one row per pad of the current bank, so drum patterns can be drawn
//! instead of finger-drummed. It edits the same per-pad patterns the piano roll does.

use super::{humanize, theme, toggle};
use crate::app::App;
use crate::engine::{Note, PadKind};
use crate::music::{PPQ, TICKS_PER_BAR, bank_name, pad_label};
use crate::shared::f32_load;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, vec2};

const LABEL_W: f32 = 132.0;
const RESOLUTIONS: [(&str, u32); 4] = [("1/8", 240), ("1/16", 120), ("1/16T", 80), ("1/32", 60)];

/// Pattern length shown for a bank: whatever its sequenced audio pads already use.
fn bank_len(app: &App, bank: usize) -> u32 {
    (0..16)
        .map(|i| &app.pads[bank * 16 + i])
        .filter(|p| p.cfg.kind == PadKind::Audio && !p.pattern.notes.is_empty())
        .map(|p| p.pattern.len_ticks)
        .max()
        .unwrap_or(app.step_bars * TICKS_PER_BAR)
}

pub fn draw(app: &mut App, ui: &mut Ui) {
    let bank = app.bank;
    let mut len = bank_len(app, bank);

    ui.horizontal_wrapped(|ui| {
        theme::title(ui, &format!("STEPS · BANK {}", bank_name(bank)), theme::AMBER);
        ui.label("Bars");
        let mut bars = len / TICKS_PER_BAR;
        egui::ComboBox::from_id_salt("step_bars").width(40.0).selected_text(bars.to_string()).show_ui(ui, |ui| {
            for b in [1, 2, 4] {
                ui.selectable_value(&mut bars, b, b.to_string());
            }
        });
        if bars * TICKS_PER_BAR != len {
            len = bars * TICKS_PER_BAR;
            app.step_bars = bars;
            for i in bank * 16..bank * 16 + 16 {
                if app.pads[i].cfg.kind == PadKind::Audio {
                    let pat = &mut app.pads[i].pattern;
                    pat.len_ticks = len;
                    pat.notes.retain(|n| n.start < len);
                    app.push_pattern(i);
                }
            }
        }
        ui.label("Grid");
        let name = RESOLUTIONS.iter().find(|r| r.1 == app.step_res).map_or("1/16", |r| r.0);
        egui::ComboBox::from_id_salt("step_res").width(58.0).selected_text(name).show_ui(ui, |ui| {
            for (n, t) in RESOLUTIONS {
                ui.selectable_value(&mut app.step_res, t, n);
            }
        });
        if toggle(ui, app.roll.rec, "Rec", theme::RED)
            .on_hover_text("While playing, pad hits are written into the pattern")
            .clicked()
        {
            app.roll.rec ^= true;
        }
        // One setting for the whole kit: shown from the first pad, written to all sixteen.
        let (mut time, mut vel) = (app.pads[bank * 16].cfg.human_time, app.pads[bank * 16].cfg.human_vel);
        if humanize(ui, &mut time, &mut vel) {
            for i in bank * 16..bank * 16 + 16 {
                app.pads[i].cfg.human_time = time;
                app.pads[i].cfg.human_vel = vel;
                app.push_pad(i);
            }
        }
        if ui.button("Clear bank").clicked() {
            for i in bank * 16..bank * 16 + 16 {
                if !app.pads[i].pattern.notes.is_empty() {
                    app.pads[i].pattern.notes.clear();
                    app.push_pattern(i);
                }
            }
        }
        ui.label(
            egui::RichText::new("click / drag: paint steps · right-click: accent, soft, normal · Space: play")
                .small()
                .color(theme::DIM),
        );
    });

    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    if rect.width() < LABEL_W + 80.0 || rect.height() < 60.0 {
        return;
    }
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, theme::BG);
    let res = app.step_res.max(1);
    let steps = (len / res).max(1) as usize;
    let row_h = (rect.height() / 16.0).min(30.0);
    let grid = Rect::from_min_size(rect.min + vec2(LABEL_W, 0.0), vec2(rect.width() - LABEL_W, row_h * 16.0));
    let cell_w = grid.width() / steps as f32;
    let per_beat = (PPQ / res).max(1) as usize;

    let (pressed, down, pointer) =
        ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.interact_pos()));
    if !down {
        app.step_paint = None;
    }
    let play_step = app.playing().then(|| (app.shared.tick().rem_euclid(len as f64) / res as f64) as usize);

    // Pad 1 at the bottom, matching the matrix.
    for row in 0..16 {
        let idx = bank * 16 + 15 - row;
        let y = rect.top() + row as f32 * row_h;
        let pad = &app.pads[idx];
        let empty = pad.is_empty();
        let midi = pad.cfg.kind == PadKind::Midi;
        let level = f32_load(&app.shared.pad_level[idx]);
        let accent = if midi { theme::CYAN } else { theme::AMBER };

        let label = Rect::from_min_size(Pos2::new(rect.left(), y), vec2(LABEL_W - 4.0, row_h - 2.0));
        let lab_col = if idx == app.selected { theme::RAISED } else { theme::PANEL };
        p.rect_filled(label, 2.0, if level > 0.0 { Color32::from_rgb(28, 62, 40) } else { lab_col });
        let name: String = pad.name.chars().take(13).collect();
        p.text(
            label.left_center() + vec2(5.0, 0.0),
            Align2::LEFT_CENTER,
            format!("{} {name}", pad_label(idx)),
            FontId::proportional(10.5),
            if empty { theme::LINE } else { theme::TEXT },
        );

        for s in 0..steps {
            let cell = Rect::from_min_size(Pos2::new(grid.left() + s as f32 * cell_w, y), vec2(cell_w, row_h))
                .shrink2(vec2((cell_w * 0.08).clamp(0.5, 2.0), 2.0));
            let (lo, hi) = (s as u32 * res, (s as u32 + 1) * res);
            let vel = pad.pattern.notes.iter().filter(|n| n.start >= lo && n.start < hi).map(|n| n.vel).max();
            let beat = (s / per_beat).is_multiple_of(2);
            let base = if beat { Color32::from_rgb(34, 38, 47) } else { Color32::from_rgb(26, 29, 36) };
            let fill = match vel {
                Some(v) => accent.gamma_multiply(0.4 + 0.6 * v as f32 / 127.0),
                None if empty => base.gamma_multiply(0.55),
                None => base,
            };
            p.rect_filled(cell, 2.0, fill);
            if play_step == Some(s) {
                p.rect_stroke(cell, 2.0, Stroke::new(1.5, theme::GREEN), StrokeKind::Inside);
            }
        }
    }
    // Bar lines.
    let per_bar = (TICKS_PER_BAR / res) as usize;
    for b in 1..steps.div_ceil(per_bar.max(1)) {
        let x = grid.left() + (b * per_bar) as f32 * cell_w;
        p.vline(x, grid.y_range(), Stroke::new(1.0, theme::DIM));
    }

    // ---- Editing.
    let Some(pos) = pointer.filter(|pos| rect.contains(*pos) && pos.y < grid.bottom()) else { return };
    let row = ((pos.y - rect.top()) / row_h) as usize;
    let idx = bank * 16 + 15 - row.min(15);
    if pos.x < grid.left() {
        // Clicking a row's name selects and auditions the pad.
        if pressed && resp.contains_pointer() {
            app.pad_hit(idx, 0x2000 + idx as u32);
            app.mouse_held = Some((idx as u16, 0x2000 + idx as u32));
        }
        return;
    }
    let s = (((pos.x - grid.left()) / cell_w) as usize).min(steps - 1);
    let (lo, hi) = (s as u32 * res, (s as u32 + 1) * res);
    let in_cell = |n: &Note| n.start >= lo && n.start < hi;
    let on = app.pads[idx].pattern.notes.iter().any(in_cell);

    if pressed && resp.contains_pointer() {
        app.step_paint = Some(!on);
        app.select(idx);
    }
    if let (true, Some(paint)) = (down, app.step_paint)
        && paint != on {
            if app.pads[idx].cfg.kind == PadKind::Audio {
                app.pads[idx].pattern.len_ticks = len;
            }
            let pat = &mut app.pads[idx].pattern;
            if paint {
                pat.notes.push(Note { start: lo, len: res.min(120), pitch: 60, vel: 100 });
                // Hear the step as it goes in, unless the transport is already playing it.
                if !app.playing() {
                    app.send(crate::engine::Cmd::PadOn { pad: idx as u16, vel: 0.8, semis: 0.0, key: 0x3100 });
                    app.send(crate::engine::Cmd::PadOff { pad: idx as u16, key: 0x3100 });
                }
            } else {
                pat.notes.retain(|n| !in_cell(n));
            }
            app.push_pattern(idx);
        }
    if resp.secondary_clicked() && on {
        for n in app.pads[idx].pattern.notes.iter_mut().filter(|n| in_cell(n)) {
            n.vel = match n.vel {
                v if v >= 120 => 55,
                v if v >= 80 => 127,
                _ => 100,
            };
        }
        app.push_pattern(idx);
    }
}
