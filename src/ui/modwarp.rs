//! The Mod / Warp tab: pitch and filter, the LFO with its scope, and the warp bake.

use super::{knob, theme, toggle};
use crate::app::App;
use crate::engine::{LfoShape, LfoTarget, PadCfg, lfo_phase};
use crate::music::{LFO_FEELS, LFO_VALUES, PPQ, lfo_sync, lfo_sync_name, lfo_sync_parts, lfo_sync_ticks};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, vec2};
use std::sync::atomic::Ordering;

const GAP: f32 = 8.0;

pub fn draw(app: &mut App, ui: &mut Ui, cfg: &mut PadCfg) {
    let full = ui.available_rect_before_wrap();
    ui.allocate_rect(full, Sense::hover());
    // The LFO gets the room; the columns either side hold a handful of controls each.
    let side = ((full.width() - GAP * 2.0) * 0.26).clamp(190.0, 300.0);
    let mid = full.width() - side * 2.0 - GAP * 2.0;
    let column = |x: f32, w: f32| Rect::from_min_size(Pos2::new(full.left() + x, full.top()), vec2(w, full.height()));

    section(ui, column(0.0, side), "PITCH · FILTER", theme::AMBER, |ui| pitch_filter(ui, cfg));
    section(ui, column(side + GAP, mid), "LFO", theme::CYAN, |ui| lfo(app, ui, cfg));
    section(ui, column(side + mid + GAP * 2.0, side), "WARP", theme::PURPLE, |ui| warp(app, ui));
}

/// A titled well, the same recessed look as the waveform and the meters.
fn section(ui: &mut Ui, rect: Rect, title: &str, color: Color32, add: impl FnOnce(&mut Ui)) {
    let p = ui.painter();
    p.rect_filled(rect, 8.0, theme::WELL);
    p.rect_stroke(rect, 8.0, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);
    let inner = rect.shrink2(vec2(12.0, 9.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(rect.shrink(1.0).intersect(ui.clip_rect()));
    theme::title(&mut child, title, color);
    child.add_space(6.0);
    add(&mut child);
}

/// A caption above a control.
fn field(ui: &mut Ui, caption: &str, add: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        theme::caption(ui, caption);
        add(ui);
    });
}

/// The label at the start of a row of chips, in a fixed column so the rows line up.
fn row_caption(ui: &mut Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(48.0, 22.0), Sense::hover());
    ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, text, theme::bold(10.5), theme::DIM);
}

/// A small selectable chip in the section's colour.
fn chip(ui: &mut Ui, selected: bool, text: &str, color: Color32) -> Response {
    let font = if selected { theme::bold(11.5) } else { FontId::proportional(11.5) };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, theme::TEXT);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + vec2(14.0, 8.0), Sense::click());
    let (fill, fg) = if selected {
        (color, theme::BG)
    } else if resp.hovered() {
        (theme::HOVER, theme::TEXT)
    } else {
        (theme::RAISED, theme::DIM)
    };
    let p = ui.painter();
    p.rect_filled(rect, 5.0, fill);
    p.galley_with_override_text_color(rect.center() - galley.size() * 0.5, galley, fg);
    resp
}

fn pitch_filter(ui: &mut Ui, cfg: &mut PadCfg) {
    let mut coarse = cfg.pitch.round();
    let mut fine = (cfg.pitch - coarse) * 100.0;
    ui.horizontal(|ui| {
        field(ui, "COARSE", |ui| {
            ui.add(egui::DragValue::new(&mut coarse).speed(0.1).range(-36.0..=36.0).fixed_decimals(0).suffix(" st"));
        });
        ui.add_space(6.0);
        field(ui, "FINE", |ui| {
            ui.add(egui::DragValue::new(&mut fine).speed(0.5).range(-50.0..=50.0).fixed_decimals(0).suffix(" ct"));
        });
    });
    cfg.pitch = coarse + fine / 100.0;
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        knob(ui, &mut cfg.cutoff, "Cutoff", 1.0, theme::AMBER);
        knob(ui, &mut cfg.reso, "Reso", 0.0, theme::AMBER);
    });
    ui.add_space(4.0);

    // The low-pass as the engine applies it, 20 Hz to 20 kHz.
    let height = ui.available_height().clamp(0.0, 84.0);
    if height < 30.0 {
        return;
    }
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 5.0, theme::BG);
    let open = cfg.cutoff >= 0.995;
    let corner = 20.0 * 1000f32.powf(cfg.cutoff);
    let k = 1.0 / (0.7 + cfg.reso * 11.0);
    let (top_db, bottom_db) = (24.0, -36.0);
    let y_of = |db: f32| rect.top() + (top_db - db.clamp(bottom_db, top_db)) / (top_db - bottom_db) * rect.height();
    p.hline(rect.x_range(), y_of(0.0), Stroke::new(1.0, theme::LINE));
    let steps = 64;
    let curve: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let w = 20.0 * 1000f32.powf(t) / corner;
            let gain = if open { 1.0 } else { 1.0 / ((1.0 - w * w).powi(2) + (k * w).powi(2)).sqrt() };
            Pos2::new(rect.left() + t * rect.width(), y_of(20.0 * gain.max(1e-4).log10()))
        })
        .collect();
    p.line(curve, Stroke::new(1.5, if open { theme::FAINT } else { theme::AMBER }));
    let label = if open { "filter open".to_string() } else if corner >= 1000.0 { format!("{:.1} kHz", corner / 1000.0) } else { format!("{corner:.0} Hz") };
    p.text(rect.left_top() + vec2(6.0, 4.0), Align2::LEFT_TOP, label, FontId::proportional(10.0), theme::DIM);
}

fn lfo(app: &mut App, ui: &mut Ui, cfg: &mut PadCfg) {
    let on = cfg.lfo_target != LfoTarget::Off;
    let sync_ticks = lfo_sync_ticks(cfg.lfo_sync);
    let hz = match sync_ticks {
        Some(ticks) => (app.bpm as f64 / 60.0 * PPQ as f64 / ticks) as f32,
        None => cfg.lfo_rate,
    };

    // ---- Scope: two cycles of the wave at its depth, with the engine's position on it.
    let height = (ui.available_height() - 132.0).clamp(46.0, 120.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 5.0, theme::BG);
    let plot = rect.shrink2(vec2(8.0, 16.0));
    let mid = plot.center().y;
    p.hline(plot.x_range(), mid, Stroke::new(1.0, theme::LINE));
    for cycle in 0..=2 {
        let x = plot.left() + plot.width() * cycle as f32 / 2.0;
        p.vline(x, plot.y_range(), Stroke::new(1.0, theme::LINE.gamma_multiply(0.6)));
    }
    let amp = cfg.lfo_depth.max(0.04);
    let y_of = |v: f32| mid - v * amp * plot.height() * 0.5;
    let color = if on { theme::CYAN } else { theme::FAINT };
    let steps = (plot.width() as usize).clamp(32, 400);
    let wave: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let t = i as f64 / steps as f64 * 2.0;
            Pos2::new(plot.left() + plot.width() * (t / 2.0) as f32, y_of(cfg.lfo_shape.at(t)))
        })
        .collect();
    p.line(wave, Stroke::new(1.6, color));
    if on {
        let ticks = f64::from_bits(app.shared.lfo_ticks.load(Ordering::Relaxed));
        let secs = f64::from_bits(app.shared.lfo_secs.load(Ordering::Relaxed));
        let phase = lfo_phase(cfg, ticks, secs);
        let at = Pos2::new(plot.left() + plot.width() * (phase.rem_euclid(2.0) / 2.0) as f32, y_of(cfg.lfo_shape.at(phase)));
        p.vline(at.x, plot.y_range(), Stroke::new(1.0, theme::CYAN.gamma_multiply(0.35)));
        p.circle_filled(at, 7.0, theme::CYAN.gamma_multiply(0.25));
        p.circle_filled(at, 3.5, theme::TEXT);
    }
    let rate = match sync_ticks {
        Some(_) => format!("{} · {hz:.2} Hz", lfo_sync_name(cfg.lfo_sync)),
        None => format!("{hz:.2} Hz · {:.2} s", 1.0 / hz.max(0.001)),
    };
    p.text(rect.left_top() + vec2(8.0, 3.0), Align2::LEFT_TOP, rate, FontId::proportional(10.5), theme::DIM);
    let depth = cfg.lfo_depth;
    let range = match cfg.lfo_target {
        LfoTarget::Off => "not routed".to_string(),
        LfoTarget::Pitch => format!("pitch ±{:.1} st", depth * 12.0),
        LfoTarget::Volume if depth >= 0.999 => "volume down to silence".to_string(),
        LfoTarget::Volume => format!("volume down to {:.1} dB", 20.0 * (1.0 - depth).log10()),
        LfoTarget::Pan => format!("pan ±{:.0}%", depth * 100.0),
        LfoTarget::Filter => format!("cutoff ±{:.1} oct", depth * 0.5 * 1000f32.log2()),
    };
    p.text(rect.right_top() + vec2(-8.0, 3.0), Align2::RIGHT_TOP, range, FontId::proportional(10.5), color);
    ui.add_space(6.0);

    // ---- Controls.
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                row_caption(ui, "TO");
                for target in [LfoTarget::Off, LfoTarget::Pitch, LfoTarget::Volume, LfoTarget::Pan, LfoTarget::Filter] {
                    if chip(ui, cfg.lfo_target == target, &format!("{target:?}"), theme::CYAN).clicked() {
                        cfg.lfo_target = target;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                row_caption(ui, "SHAPE");
                for shape in LfoShape::ALL {
                    if shape_button(ui, shape, cfg.lfo_shape == shape).clicked() {
                        cfg.lfo_shape = shape;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                row_caption(ui, "RATE");
                if toggle(ui, cfg.lfo_sync != 0, "SYNC", theme::CYAN).on_hover_text("Lock the rate to the project tempo").clicked() {
                    // Into sync at the nearest note value to the free rate, and back out at
                    // the rate the note value came to.
                    cfg.lfo_sync = if cfg.lfo_sync != 0 { 0 } else { nearest_value(app.bpm, cfg.lfo_rate) };
                    if cfg.lfo_sync == 0 {
                        cfg.lfo_rate = hz.clamp(0.05, 30.0);
                    }
                }
                ui.add_space(4.0);
                match lfo_sync_parts(cfg.lfo_sync) {
                    None => {
                        ui.add(egui::DragValue::new(&mut cfg.lfo_rate).speed(0.02).range(0.05..=30.0).suffix(" Hz").fixed_decimals(2));
                    }
                    Some((value, feel)) => {
                        for (i, (name, _)) in LFO_VALUES.iter().enumerate() {
                            if chip(ui, i == value, name, theme::CYAN).clicked() {
                                cfg.lfo_sync = lfo_sync(i, feel);
                            }
                        }
                    }
                }
            });
            if let Some((value, feel)) = lfo_sync_parts(cfg.lfo_sync) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    row_caption(ui, "");
                    for (i, (name, _, _)) in LFO_FEELS.iter().enumerate() {
                        if chip(ui, i == feel, name, theme::CYAN).clicked() {
                            cfg.lfo_sync = lfo_sync(value, i);
                        }
                    }
                });
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            knob(ui, &mut cfg.lfo_depth, "Depth", 0.0, theme::CYAN);
        });
    });
}

/// The synced setting whose rate at `bpm` is closest to `hz`, among the plain note values.
fn nearest_value(bpm: f32, hz: f32) -> u8 {
    let rate = |ticks: u32| bpm / 60.0 * PPQ as f32 / ticks as f32;
    let value = (0..LFO_VALUES.len())
        .min_by(|a, b| {
            let off = |i: &usize| (rate(LFO_VALUES[*i].1) / hz.max(0.001)).ln().abs();
            off(a).total_cmp(&off(b))
        })
        .unwrap_or(4);
    lfo_sync(value, 0)
}

/// A button showing one cycle of the shape it selects.
fn shape_button(ui: &mut Ui, shape: LfoShape, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 22.0), Sense::click());
    let p = ui.painter();
    let fill = if selected { theme::CYAN } else if resp.hovered() { theme::HOVER } else { theme::RAISED };
    p.rect_filled(rect, 5.0, fill);
    let plot = rect.shrink2(vec2(8.0, 5.0));
    // The square's vertical edges need both ends of each jump, so sample finely.
    let points: Vec<Pos2> = (0..=48)
        .map(|i| {
            let t = i as f64 / 48.0 * 0.999;
            Pos2::new(plot.left() + plot.width() * t as f32, plot.center().y - shape.at(t) * plot.height() * 0.5)
        })
        .collect();
    p.line(points, Stroke::new(1.5, if selected { theme::BG } else { theme::DIM }));
    resp.on_hover_text(format!("{shape:?}"))
}

fn warp(app: &mut App, ui: &mut Ui) {
    let sel = app.selected;
    ui.horizontal(|ui| {
        field(ui, "SHIFT", |ui| {
            ui.add(egui::DragValue::new(&mut app.warp_semis).speed(0.05).range(-24.0..=24.0).fixed_decimals(2).suffix(" st"));
        });
        ui.add_space(6.0);
        field(ui, "LENGTH", |ui| {
            ui.add(egui::DragValue::new(&mut app.warp_stretch).speed(0.5).range(25.0..=400.0).fixed_decimals(0).suffix(" %"));
        });
    });
    ui.add_space(6.0);
    if toggle(ui, app.preserve_transients, "Lock transients", theme::PURPLE)
        .on_hover_text("Keep each hit's attack intact and stretch only what lies between them")
        .clicked()
    {
        app.preserve_transients ^= true;
    }
    ui.add_space(6.0);

    let sample = app.pads[sel].sample();
    let changes = (app.warp_stretch - 100.0).abs() > 0.5 || app.warp_semis.abs() > 0.005;
    let result = match sample {
        Some(s) => {
            let secs = s.len() as f32 / app.sr();
            format!("{secs:.2} s becomes {:.2} s", secs * app.warp_stretch / 100.0)
        }
        None => "No sample on this pad".to_string(),
    };
    ui.label(egui::RichText::new(result).small().color(theme::DIM));
    ui.add_space(4.0);
    let label = egui::RichText::new("Bake as new take").color(if sample.is_some() && changes { theme::BG } else { theme::DIM });
    let mut button = egui::Button::new(label).min_size(vec2(ui.available_width(), 28.0));
    if sample.is_some() && changes {
        button = button.fill(theme::PURPLE);
    }
    if ui.add_enabled(sample.is_some() && changes, button).clicked() {
        let note = format!("warped {:+.2} st, {:.0}% length", app.warp_semis, app.warp_stretch);
        app.warp(sel, app.warp_stretch as f64 / 100.0, app.warp_semis, note);
    }
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("Pitch here keeps the length; the Pitch control on the left does not. Formants move with the pitch.")
            .small()
            .color(theme::FAINT),
    );
}
