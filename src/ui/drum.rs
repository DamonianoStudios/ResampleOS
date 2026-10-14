//! The Drum tab: a drum synth pad's engine, sound and takes on the left, a display of the
//! hit with handles to shape it in the middle, and the eight macros on the right.

use super::{knob_sized, pill, theme, toggle};
use crate::app::{App, MacroLayer};
use crate::drum::{self, DrumEngine, DrumParams};
use crate::engine::PadKind;
use crate::music::pad_label;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, vec2};
use std::f32::consts::PI;
use std::sync::Arc;

/// Width of the macro section.
const KNOBS_W: f32 = 300.0;

pub fn draw(app: &mut App, ui: &mut Ui) {
    let sel = app.selected;
    if app.pads[sel].cfg.kind != PadKind::Drum {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("Not a drum synth pad").size(16.0).color(theme::DIM));
            if app.take_patch(sel).is_some() {
                ui.label("This take was baked from the drum synth.");
                if ui.button("Bring the synth back").clicked() {
                    app.drum_resynth(sel);
                }
            } else if app.pads[sel].is_empty() {
                ui.label("Make it a drum synth pad, or load a whole kit onto a bank (right-click a bank tab).");
                if ui.button("Make drum synth pad").clicked() {
                    app.make_drum(sel, None);
                }
            }
        });
        return;
    }
    let before = app.pads[sel].cfg.drum;
    let mut p = before;
    let mut bake = None;

    // ---- Engine, sound, how it is tuned and cut, and baking.
    ui.horizontal(|ui| {
        theme::caption(ui, "ENGINE");
        egui::ComboBox::from_id_salt("drum_engine").width(120.0).selected_text(p.engine.name()).show_ui(ui, |ui| {
            for e in DrumEngine::ALL {
                if ui.selectable_label(p.engine == e, e.name()).clicked() && p.engine != e {
                    // A new engine starts from its first factory sound, not the old knobs.
                    p = drum::presets().into_iter().find(|s| s.params.engine == e).map_or(p, |s| s.params);
                }
            }
        });
        egui::ComboBox::from_id_salt("drum_sound").width(124.0).selected_text("Sounds…").show_ui(ui, |ui| {
            for s in drum::presets() {
                let tag = if s.params.engine == p.engine { theme::TEXT } else { theme::DIM };
                if ui.selectable_label(false, egui::RichText::new(s.name).color(tag)).clicked() {
                    p = s.params;
                    app.pads[sel].name = s.name.into();
                }
            }
        })
        .response
        .on_hover_text("Load a factory sound onto this pad");
        if p.engine == DrumEngine::Metal {
            ui.selectable_value(&mut p.open, false, "Closed").on_hover_text("Ring out with the Closed decay");
            ui.selectable_value(&mut p.open, true, "Open").on_hover_text("Ring out with the Open decay");
        }
        ui.separator();
        if toggle(ui, p.key_lock, "Key lock", theme::CYAN)
            .on_hover_text(format!("Tune the drum to the nearest note of {}", app.key.name()))
            .clicked()
        {
            p.key_lock ^= true;
        }
        ui.label("Tail");
        egui::ComboBox::from_id_salt("drum_sync").width(52.0).selected_text(drum::SYNC_NAMES[p.sync as usize % 6]).show_ui(ui, |ui| {
            for (i, name) in drum::SYNC_NAMES.iter().enumerate() {
                ui.selectable_value(&mut p.sync, i as u8, *name);
            }
        })
        .response
        .on_hover_text("Cut the tail after a note value at the project tempo, so a long kick never runs into the next one");
        ui.separator();
        theme::caption(ui, "BAKE");
        ui.checkbox(&mut app.drum.bake_fx, "FX").on_hover_text("Print the pad's bus effects into the bake (and route the pad straight to the master)");
        if ui.button("To take").on_hover_text("Ctrl+B: render this hit to a new take on the pad").clicked() {
            bake = Some(false);
        }
        if ui.button("Mutate").on_hover_text("Ctrl+M: vary the knobs a little and bake the result to a new take").clicked() {
            bake = Some(true);
        }
    });
    ui.horizontal(|ui| {
        theme::caption(ui, "TAKES");
        ui.spacing_mut().item_spacing.x = 2.0;
        pill(ui, true, "SYNTH").on_hover_text("The live synth is playing");
        let count = app.pads[sel].takes.len();
        let mut to = None;
        for i in 0..count {
            let take = app.pads[sel].takes[i].clone();
            let synth_made = app.pads[sel].drum_patches.iter().any(|(s, _)| Arc::ptr_eq(s, &take));
            let r = pill(ui, false, &format!("{}", i + 1)).on_hover_text(if synth_made {
                "A bake of the synth. Click to play this take; step back past take 1 (Ctrl+Shift+Z) to return to the synth."
            } else {
                "Click to play this take"
            });
            if synth_made {
                ui.painter().circle_filled(r.rect.right_top() + vec2(-5.0, 5.0), 2.0, theme::LIME);
            }
            if r.clicked() {
                to = Some(i);
            }
        }
        if count == 0 {
            ui.label(egui::RichText::new("bake to keep a hit as audio").small().color(theme::FAINT));
        }
        if let Some(i) = to {
            app.drum_to_take(sel, i);
        }
    });
    ui.add_space(4.0);

    // ---- Display and macros side by side.
    let area = ui.available_rect_before_wrap();
    ui.allocate_rect(area, Sense::hover());
    let knobs = Rect::from_min_max(Pos2::new(area.right() - KNOBS_W.min(area.width() * 0.5), area.top()), area.max);
    let display = Rect::from_min_max(area.min, Pos2::new(knobs.left() - 10.0, area.bottom()));
    if display.width() > 80.0 && display.height() > 60.0 {
        envelope(app, ui, display, &mut p);
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(knobs).layout(egui::Layout::top_down(egui::Align::Min)));
    macros(app, &mut child, &mut p);

    if p != before {
        app.pads[sel].cfg.drum = p;
        app.push_pad(sel);
        // Hear the change while the transport is stopped.
        if !app.playing() && ui.input(|i| i.pointer.any_released()) {
            app.send(crate::engine::Cmd::PadOn { pad: sel as u16, vel: 0.79, semis: 0.0, key: 0x3200 });
            app.send(crate::engine::Cmd::PadOff { pad: sel as u16, key: 0x3200 });
        }
    }
    if let Some(mutate) = bake {
        app.drum_bake(sel, mutate);
    }
}

/// The eight knobs, two rows of four, and which layer of them is being edited.
fn macros(app: &mut App, ui: &mut Ui, p: &mut DrumParams) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        theme::caption(ui, "MACROS");
        ui.add_space(4.0);
        for (layer, name, hint) in [
            (MacroLayer::Value, "Value", "Turn the knobs"),
            (MacroLayer::Velocity, "Velocity", "Set how far a hard hit moves each knob (the blue ring). Centre is none; left reverses it."),
            (MacroLayer::Random, "Random", "Set how far each knob wanders on every hit (the purple ring)"),
        ] {
            if pill(ui, app.drum.layer == layer, name).on_hover_text(hint).clicked() {
                app.drum.layer = layer;
            }
        }
    });
    ui.add_space(4.0);
    let names = p.engine.macros();
    let radius = ((ui.available_width() / 4.0 - 24.0) / 2.0).clamp(8.0, 13.0);
    for row in 0..2 {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ((ui.available_width() - 4.0 * (radius * 2.0 + 20.0)) / 3.0).max(2.0);
            for (k, name) in names.iter().enumerate().skip(row * 4).take(4) {
                let resp = macro_knob(ui, p, k, name, app.drum.layer, radius);
                // Only the knobs' own values can be automated, not their velocity or
                // randomness reach.
                if app.drum.layer == MacroLayer::Value {
                    let target = crate::engine::AutoTarget::DrumMacro { pad: app.selected as u16, m: k as u8 };
                    super::automation::hook(app, ui, &resp, target, p.m[k]);
                }
            }
        });
    }
}

/// One macro: the knob edits the chosen layer, and rings around it show the other two.
fn macro_knob(ui: &mut Ui, p: &mut DrumParams, k: usize, name: &str, layer: MacroLayer, r: f32) -> Response {
    let (mut v, default, colour) = match layer {
        MacroLayer::Value => (p.m[k], 0.5, theme::LIME),
        MacroLayer::Velocity => ((p.vel[k] + 1.0) * 0.5, 0.5, theme::CYAN),
        MacroLayer::Random => (p.rnd[k], 0.0, theme::PURPLE),
    };
    let resp = knob_sized(ui, &mut v, name, default, colour, r);
    match layer {
        MacroLayer::Value => p.m[k] = v,
        MacroLayer::Velocity => p.vel[k] = v * 2.0 - 1.0,
        MacroLayer::Random => p.rnd[k] = v,
    }
    // Rings outside the knob's own arc: the reach of velocity and of randomness.
    let c = Pos2::new(resp.rect.center().x, resp.rect.top() + 21.0);
    let ang = |t: f32| (135.0 + 270.0 * t.clamp(0.0, 1.0)).to_radians();
    let arc = |from: f32, to: f32, rad: f32, col: Color32, w: f32| {
        let (a, b) = (from.min(to), from.max(to));
        if b - a < 0.005 {
            return;
        }
        let pts: Vec<Pos2> = (0..=16).map(|i| {
            let t = ang(a + (b - a) * i as f32 / 16.0);
            c + vec2(t.cos(), t.sin()) * rad
        }).collect();
        ui.painter().add(egui::Shape::line(pts, Stroke::new(w, col)));
    };
    let m = p.m[k];
    // A full-velocity hit adds a quarter of the amount (see `DrumParams::resolve`).
    arc(m, m + p.vel[k] * 0.25, r + 8.0, theme::CYAN, 2.0);
    arc(m - p.rnd[k] * 0.5, m + p.rnd[k] * 0.5, r + 10.5, theme::PURPLE.gamma_multiply(0.8), 1.5);
    resp.on_hover_text(format!(
        "{name}: {:.0}%  ·  velocity {:+.0}%  ·  random ±{:.0}%",
        p.m[k] * 100.0,
        p.vel[k] * 25.0,
        p.rnd[k] * 50.0
    ))
}

/// The hit as it sounds now, with two handles that move pairs of macros at once.
fn envelope(app: &mut App, ui: &mut Ui, rect: Rect, p: &mut DrumParams) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, theme::WELL);
    painter.rect_stroke(rect, 8.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Inside);
    let screen = rect.shrink2(vec2(12.0, 14.0));

    let sr = app.sr();
    let fresh = app.drum.preview.as_ref().is_none_or(|(at, _)| at != p);
    if fresh {
        let audio = drum::render_capped(p, sr, app.key.mask(), app.bpm, 2.5);
        app.drum.preview = Some((*p, Arc::new(audio)));
    }
    let audio = app.drum.preview.as_ref().map(|(_, a)| a.clone()).expect("rendered above");
    let secs = (audio.len() as f32 / sr).max(0.2);

    // Time marks along the bottom.
    let step = [0.025f32, 0.05, 0.1, 0.25, 0.5, 1.0].into_iter().find(|s| secs / s <= 8.0).unwrap_or(1.0);
    let mut t = step;
    while t < secs {
        let x = screen.left() + t / secs * screen.width();
        painter.vline(x, screen.y_range(), Stroke::new(1.0, Color32::from_rgb(28, 31, 34)));
        let label = if t < 1.0 { format!("{:.0} ms", t * 1000.0) } else { format!("{t:.1} s") };
        painter.text(Pos2::new(x + 3.0, screen.bottom() + 1.0), Align2::LEFT_TOP, label, FontId::proportional(9.0), theme::FAINT);
        t += step;
    }
    // The waveform, and its outline.
    let cols = screen.width() as usize;
    let mid = screen.center().y;
    let half = screen.height() * 0.5;
    let mut outline = Vec::with_capacity(cols);
    for c in 0..cols {
        let (a, b) = (c * audio.len() / cols.max(1), ((c + 1) * audio.len() / cols.max(1)).max(c * audio.len() / cols.max(1) + 1));
        let peak = audio.l[a.min(audio.len())..b.min(audio.len())].iter().fold(0f32, |m, v| m.max(v.abs())).min(1.0);
        let x = screen.left() + c as f32 + 0.5;
        painter.vline(x, mid - peak * half..=mid + peak * half, Stroke::new(1.0, theme::LIME.gamma_multiply(0.45)));
        outline.push(Pos2::new(x, mid - peak * half));
    }
    painter.add(egui::Shape::line(outline, Stroke::new(1.5, theme::LIME)));
    painter.text(rect.left_top() + vec2(10.0, 6.0), Align2::LEFT_TOP, format!("{} · {}", p.engine.short(), pad_label(app.selected)), theme::bold(10.0), theme::DIM);

    // Handles: x and y each drive a macro, so one drag reshapes the hit.
    let names = p.engine.macros();
    for (h, pair) in p.engine.handles().into_iter().enumerate() {
        let Some((xm, ym)) = pair else { continue };
        let pos = Pos2::new(screen.left() + p.m[xm] * screen.width(), screen.bottom() - p.m[ym] * screen.height());
        let resp = ui.interact(Rect::from_center_size(pos, vec2(18.0, 18.0)), egui::Id::new(("drum_handle", h)), Sense::drag());
        if resp.dragged() {
            let d = resp.drag_delta();
            p.m[xm] = (p.m[xm] + d.x / screen.width()).clamp(0.0, 1.0);
            p.m[ym] = (p.m[ym] - d.y / screen.height()).clamp(0.0, 1.0);
        }
        let hot = resp.hovered() || resp.dragged();
        let colour = if h == 0 { theme::AMBER } else { theme::CYAN };
        painter.vline(pos.x, pos.y..=screen.bottom(), Stroke::new(1.0, colour.gamma_multiply(0.3)));
        painter.circle_filled(pos, if hot { 7.0 } else { 6.0 }, colour);
        painter.circle_stroke(pos, if hot { 7.0 } else { 6.0 }, Stroke::new(1.5, Color32::BLACK));
        let label = format!("{} · {}", names[xm], names[ym]);
        // One label above its handle and the other below, so two handles close together
        // do not write over each other.
        let left = pos.x > screen.center().x;
        let (dy, v) = if h == 0 { (-10.0, egui::Align::Max) } else { (10.0, egui::Align::Min) };
        let anchor = Align2([if left { egui::Align::Max } else { egui::Align::Min }, v]);
        painter.text(pos + vec2(if left { -10.0 } else { 10.0 }, dy), anchor, label, FontId::proportional(10.0), if hot { theme::TEXT } else { colour });
        resp.on_hover_text(format!("Drag: left and right for {}, up and down for {}", names[xm], names[ym]));
    }
}

/// A small drawing of the engine's kind of drum, for the pad face.
pub fn glyph(p: &egui::Painter, rect: Rect, engine: DrumEngine, colour: Color32) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.42;
    let s = Stroke::new((r * 0.14).clamp(1.0, 1.8), colour);
    match engine {
        DrumEngine::Kick => {
            p.circle_stroke(c, r, s);
            p.circle_stroke(c, r * 0.45, s);
        }
        DrumEngine::Snare => {
            p.circle_stroke(c, r, s);
            for k in [-0.35f32, 0.0, 0.35] {
                p.hline(c.x - r * 0.7..=c.x + r * 0.7, c.y + k * r, Stroke::new(s.width * 0.7, colour));
            }
        }
        DrumEngine::Metal => {
            // A cymbal seen from the side, on its stand.
            let pts: Vec<Pos2> = (0..=20).map(|i| {
                let t = PI * i as f32 / 20.0;
                Pos2::new(c.x - r * t.cos(), c.y - r * 0.35 * t.sin())
            }).collect();
            p.add(egui::Shape::line(pts, s));
            p.hline(c.x - r..=c.x + r, c.y, s);
            p.vline(c.x, c.y..=c.y + r, s);
        }
        DrumEngine::Fm => {
            let pts: Vec<Pos2> = (0..=24).map(|i| {
                let t = i as f32 / 24.0;
                Pos2::new(c.x - r + 2.0 * r * t, c.y - (t * 3.0 * 2.0 * PI).sin() * r * 0.6 * (1.0 - t))
            }).collect();
            p.add(egui::Shape::line(pts, s));
        }
        DrumEngine::Modal => {
            for (i, len) in [1.0f32, 0.75, 0.5].iter().enumerate() {
                let y = c.y - r * 0.6 + i as f32 * r * 0.6;
                p.hline(c.x - r * len..=c.x + r * len, y, Stroke::new(s.width * 1.6, colour));
            }
        }
    }
}

/// How a hit dies away, drawn over a fixed second and a half so long and short sounds
/// look different at a glance.
pub fn decay_curve(p: &egui::Painter, rect: Rect, params: &DrumParams, colour: Color32) {
    let t60 = params.tail_secs();
    let n = 32;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32 * 1.5;
            let level = (-6.91 * t / t60.max(0.01)).exp();
            Pos2::new(rect.left() + i as f32 / n as f32 * rect.width(), rect.bottom() - level * rect.height())
        })
        .collect();
    // Filled a slice at a time: the area under a decay is not convex as a whole.
    for w in pts.windows(2) {
        let quad = vec![w[0], w[1], Pos2::new(w[1].x, rect.bottom()), Pos2::new(w[0].x, rect.bottom())];
        p.add(egui::Shape::convex_polygon(quad, colour.gamma_multiply(0.15), Stroke::NONE));
    }
    p.add(egui::Shape::line(pts, Stroke::new(1.5, colour)));
}
