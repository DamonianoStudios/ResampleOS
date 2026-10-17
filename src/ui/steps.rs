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
/// Bars on screen at once. A longer pattern is shown a page at a time.
const PAGE_BARS: u32 = 4;

/// Pattern length shown for a bank: whatever its sequenced audio pads already use.
fn bank_len(app: &App, bank: usize) -> u32 {
    (0..16)
        .map(|i| &app.pads[bank * 16 + i])
        .filter(|p| steppable(p.cfg.kind) && !p.pattern.notes.is_empty())
        .map(|p| p.pattern.len_ticks)
        .max()
        .unwrap_or(app.step_bars * TICKS_PER_BAR)
}

/// Pads whose pattern length the step sequencer sets: the ones it is the editor for.
fn steppable(kind: PadKind) -> bool {
    matches!(kind, PadKind::Audio | PadKind::Drum)
}

pub fn draw(app: &mut App, ui: &mut Ui) {
    let bank = app.bank;
    let mut len = bank_len(app, bank);

    ui.horizontal_wrapped(|ui| {
        theme::title(ui, &format!("STEPS · BANK {}", bank_name(bank)), theme::AMBER);
        ui.label("Bars");
        let mut bars = len / TICKS_PER_BAR;
        egui::ComboBox::from_id_salt("step_bars").width(40.0).selected_text(bars.to_string()).show_ui(ui, |ui| {
            for b in 1..=16 {
                ui.selectable_value(&mut bars, b, b.to_string());
            }
        });
        if bars * TICKS_PER_BAR != len {
            len = bars * TICKS_PER_BAR;
            app.step_bars = bars;
            for i in bank * 16..bank * 16 + 16 {
                if steppable(app.pads[i].cfg.kind) {
                    let pat = &mut app.pads[i].pattern;
                    pat.len_ticks = len;
                    pat.notes.retain(|n| n.start < len);
                    app.push_pattern(i);
                }
            }
        }
        // Long patterns are edited four bars at a time.
        let pages = (len / TICKS_PER_BAR).div_ceil(PAGE_BARS);
        if pages > 1 {
            ui.spacing_mut().item_spacing.x = 2.0;
            for page in 0..pages {
                let (a, b) = (page * PAGE_BARS + 1, ((page + 1) * PAGE_BARS).min(len / TICKS_PER_BAR));
                let label = if a == b { a.to_string() } else { format!("{a}-{b}") };
                if super::pill(ui, app.step_page == page, &label).on_hover_text("Bars on screen").clicked() {
                    app.step_page = page;
                    app.roll.follow = false;
                }
            }
            if toggle(ui, app.roll.follow, "Follow", theme::GREEN).on_hover_text("Turn the page with the playhead while playing").clicked() {
                app.roll.follow ^= true;
            }
            ui.spacing_mut().item_spacing.x = 7.0;
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
            egui::RichText::new("click / drag: paint steps · right-click: accent, soft, normal · Alt+click a drum step: lock its knobs · Space: play")
                .small()
                .color(theme::DIM),
        );
    });

    lock_strip(app, ui);
    row_menu(app, ui.ctx());

    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    if rect.width() < LABEL_W + 80.0 || rect.height() < 60.0 {
        return;
    }
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, theme::BG);
    let res = app.step_res.max(1);
    let page_ticks = PAGE_BARS * TICKS_PER_BAR;
    let pages = len.div_ceil(page_ticks).max(1);
    if app.roll.follow && app.playing() {
        app.step_page = (app.shared.tick().rem_euclid(len as f64) as u32) / page_ticks;
    }
    app.step_page = app.step_page.min(pages - 1);
    // The page on screen, in ticks, and its first step.
    let (from, to) = (app.step_page * page_ticks, ((app.step_page + 1) * page_ticks).min(len));
    let first = (from / res) as usize;
    let steps = ((to - from) / res).max(1) as usize;
    // Room under the grid for the bar numbers.
    let row_h = ((rect.height() - 16.0) / 16.0).min(30.0);
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
        let level = f32_load(&app.shared.pad_level[idx]);
        let accent = match pad.cfg.kind {
            PadKind::Midi => theme::CYAN,
            PadKind::Drum => theme::LIME,
            _ => theme::AMBER,
        };

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
            let step = first + s;
            let (lo, hi) = (step as u32 * res, (step as u32 + 1) * res);
            let vel = pad.pattern.notes.iter().filter(|n| n.start >= lo && n.start < hi).map(|n| n.vel).max();
            let beat = (step / per_beat).is_multiple_of(2);
            let base = if beat { Color32::from_rgb(34, 38, 47) } else { Color32::from_rgb(26, 29, 36) };
            let fill = match vel {
                Some(v) => accent.gamma_multiply(0.4 + 0.6 * v as f32 / 127.0),
                None if empty => base.gamma_multiply(0.55),
                None => base,
            };
            p.rect_filled(cell, 2.0, fill);
            if play_step == Some(step) {
                p.rect_stroke(cell, 2.0, Stroke::new(1.5, theme::GREEN), StrokeKind::Inside);
            }
            // A dot on a step with knobs locked to it, and a ring on the one being edited.
            if pad.pattern.notes.iter().any(|n| n.start >= lo && n.start < hi && n.has_locks()) {
                p.circle_filled(cell.right_top() + vec2(-4.0, 4.0), 2.2, theme::TEXT);
            }
            if app.drum.lock_step == Some((idx, lo, hi)) {
                p.rect_stroke(cell.expand(1.0), 3.0, Stroke::new(2.0, theme::TEXT), StrokeKind::Outside);
            }
        }
    }
    // Bar lines, with the bar's number above each.
    let per_bar = (TICKS_PER_BAR / res).max(1) as usize;
    for b in 0..steps.div_ceil(per_bar) {
        let x = grid.left() + (b * per_bar) as f32 * cell_w;
        if b > 0 {
            p.vline(x, grid.y_range(), Stroke::new(1.0, theme::DIM));
        }
        let bar = first / per_bar + b + 1;
        p.text(Pos2::new(x + 3.0, grid.bottom() + 2.0), Align2::LEFT_TOP, bar.to_string(), theme::bold(10.0), theme::DIM);
    }

    // ---- Editing.
    let Some(pos) = pointer.filter(|pos| rect.contains(*pos) && pos.y < grid.bottom()) else { return };
    let row = ((pos.y - rect.top()) / row_h) as usize;
    let idx = bank * 16 + 15 - row.min(15);
    if pos.x < grid.left() {
        // Clicking a row's name selects and auditions the pad; right-click for its menu.
        if pressed && resp.contains_pointer() {
            app.pad_hit(idx, 0x2000 + idx as u32);
            app.mouse_held = Some((idx as u16, 0x2000 + idx as u32));
        }
        if resp.secondary_clicked() {
            app.step_menu = Some((idx, pos));
        }
        return;
    }
    let s = first + (((pos.x - grid.left()) / cell_w) as usize).min(steps - 1);
    let (lo, hi) = (s as u32 * res, (s as u32 + 1) * res);
    let in_cell = |n: &Note| n.start >= lo && n.start < hi;
    let on = app.pads[idx].pattern.notes.iter().any(in_cell);

    // Alt+click on a drum pad's step picks it for the lock strip above the grid.
    let alt = ui.input(|i| i.modifiers.alt);
    if pressed && resp.contains_pointer() && alt {
        if on && app.pads[idx].cfg.kind == PadKind::Drum {
            app.drum.lock_step = Some((idx, lo, hi));
            app.select(idx);
        } else {
            app.drum.lock_step = None;
        }
        return;
    }
    if pressed && resp.contains_pointer() {
        app.step_paint = Some(!on);
        app.select(idx);
    }
    if let (true, Some(paint)) = (down, app.step_paint)
        && paint != on {
            if steppable(app.pads[idx].cfg.kind) {
                app.pads[idx].pattern.len_ticks = len;
            }
            let pat = &mut app.pads[idx].pattern;
            if paint {
                pat.notes.push(Note::new(lo, res.min(120), 60, 100));
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

/// The menu of a row's name: open the pad's pattern in the piano roll (an audio pad's steps
/// become a MIDI pad that plays it).
fn row_menu(app: &mut App, ctx: &egui::Context) {
    let Some((idx, at)) = app.step_menu else { return };
    let kind = app.pads[idx].cfg.kind;
    let mut done = false;
    let area = egui::Area::new(egui::Id::new("step_row_menu")).order(egui::Order::Foreground).fixed_pos(at).show(ctx, |ui| {
        egui::Frame::menu(ui.style()).show(ui, |ui| {
            ui.set_min_width(220.0);
            ui.label(egui::RichText::new(format!("{} {}", pad_label(idx), app.pads[idx].name)).small().color(theme::DIM));
            let (label, tip) = match kind {
                PadKind::Audio => ("Convert to piano roll MIDI", "Moves this pad's steps (in every loop and on the song) to a new MIDI pad that plays this sample chromatically, and opens it in the piano roll"),
                _ => ("Open in piano roll", "Edits this pad's steps as notes in the piano roll"),
            };
            if ui.add_enabled(kind != PadKind::Vocal, egui::Button::new(label)).on_hover_text(tip).clicked() {
                app.open_in_roll(idx);
                done = true;
            }
        });
    });
    // Closes on a choice, or a click anywhere else.
    let clicked_out = ctx.input(|i| i.pointer.any_pressed()) && ctx.pointer_interact_pos().is_some_and(|p| !area.response.rect.contains(p)) && area.response.rect.area() > 0.0;
    if done || clicked_out || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.step_menu = None;
    }
}

/// Knobs for the step picked with Alt+click: turning one locks that macro to that step
/// only. Double-click a knob to let it follow the pad again.
fn lock_strip(app: &mut App, ui: &mut Ui) {
    let Some((pad, lo, hi)) = app.drum.lock_step else { return };
    let still_there = app.pads.get(pad).is_some_and(|p| p.cfg.kind == PadKind::Drum && p.pattern.notes.iter().any(|n| n.start >= lo && n.start < hi));
    if !still_there {
        app.drum.lock_step = None;
        return;
    }
    let params = app.pads[pad].cfg.drum;
    let locks = app.pads[pad].pattern.notes.iter().find(|n| n.start >= lo && n.start < hi).map_or(crate::drum::NO_LOCKS, |n| n.locks);
    let names = params.engine.macros();
    let mut set = None;
    let mut done = false;
    let mut clear = false;
    ui.horizontal(|ui| {
        theme::title(ui, "STEP LOCK", theme::LIME);
        let step = lo / app.step_res.max(1) + 1;
        ui.label(egui::RichText::new(format!("{} {} · step {step}", pad_label(pad), app.pads[pad].name)).color(theme::DIM));
        ui.add_space(6.0);
        for k in 0..8 {
            let locked = locks[k] != crate::drum::NO_LOCK;
            let mut v = if locked { locks[k] as f32 / 200.0 } else { params.m[k] };
            let r = super::knob_sized(ui, &mut v, names[k], params.m[k], if locked { theme::LIME } else { theme::FAINT }, 9.0)
                .on_hover_text(if locked { "Locked to this step. Double-click to follow the pad again." } else { "Following the pad. Turn to lock a value to this step." });
            if r.double_clicked() {
                set = Some((k, None));
            } else if r.dragged() {
                set = Some((k, Some(v)));
            }
        }
        if ui.button("Clear").on_hover_text("Unlock every knob on this step").clicked() {
            clear = true;
        }
        if ui.button("Done").clicked() {
            done = true;
        }
    });
    if let Some((k, v)) = set {
        app.set_step_lock(pad, (lo, hi), k, v);
    }
    if clear {
        app.clear_step_locks(pad, (lo, hi));
    }
    if done {
        app.drum.lock_step = None;
    }
}
