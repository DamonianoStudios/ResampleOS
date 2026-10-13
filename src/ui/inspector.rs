//! Zone B: the contextual inspector for the selected pad.

use super::{knob, pill, roll, steps, theme, toggle};
use crate::app::{App, PadAction, Tab};
use crate::engine::{LfoTarget, PadCfg, PadKind, PlayMode};
use crate::music::pad_label;
use eframe::egui::{self, Align2, Color32, FontId, PointerButton, Pos2, Rect, Sense, Stroke, Ui, vec2};

/// Settings are edited on a copy and committed once at the end; anything that rewrites
/// the pad (a bake, a kind change) is returned as a [`PadAction`] and run after that, so
/// the two never fight over the pad's state mid-frame.
pub fn draw(app: &mut App, ui: &mut Ui) {
    let sel = app.selected;
    let before = app.pads[sel].cfg;
    let mut cfg = before;
    let mut action = None;

    ui.horizontal(|ui| {
        let accent = if cfg.kind == PadKind::Midi { theme::CYAN } else { theme::AMBER };
        ui.label(egui::RichText::new(pad_label(sel)).font(theme::bold(22.0)).color(accent));
        ui.add(egui::TextEdit::singleline(&mut app.pads[sel].name).desired_width(130.0).hint_text("name"));
        ui.add_space(6.0);
        ui.spacing_mut().item_spacing.x = 2.0;
        for (tab, label) in [(Tab::Wave, "Waveform"), (Tab::Roll, "Piano Roll"), (Tab::Steps, "Steps"), (Tab::Mod, "Mod / Warp")] {
            if pill(ui, app.tab == tab, label).clicked() {
                app.tab = tab;
                if tab == Tab::Roll {
                    app.set_kind_if_empty_midi();
                }
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if pill(ui, cfg.kind == PadKind::Midi, "MIDI").clicked() && cfg.kind != PadKind::Midi {
                action = Some(PadAction::SetKind(PadKind::Midi));
            }
            if pill(ui, cfg.kind == PadKind::Audio, "Audio").clicked() && cfg.kind != PadKind::Audio {
                action = Some(PadAction::SetKind(PadKind::Audio));
            }
        });
        let kind = cfg.kind;
        if kind != cfg.kind {
            action = Some(PadAction::SetKind(kind));
        }
    });

    ui.horizontal(|ui| {
        ui.label("Vol");
        ui.add(egui::Slider::new(&mut cfg.vol, 0.0..=1.5).show_value(false));
        ui.label("Pan");
        ui.add(egui::Slider::new(&mut cfg.pan, -1.0..=1.0).show_value(false));
        if toggle(ui, cfg.mute, "M", theme::RED).on_hover_text("Mute (hold M + pad)").clicked() {
            cfg.mute ^= true;
        }
        if toggle(ui, cfg.solo, "S", theme::AMBER).on_hover_text("Solo").clicked() {
            cfg.solo ^= true;
        }
        if ui.button("Stop").on_hover_text("Backspace, or hold Q and hit the pad").clicked() {
            app.stop_pad(sel);
        }
        ui.label("Bus");
        egui::ComboBox::from_id_salt("pad_bus")
            .width(58.0)
            .selected_text(bus_name(cfg.bus))
            .show_ui(ui, |ui| {
                for b in 0..=4u8 {
                    ui.selectable_value(&mut cfg.bus, b, bus_name(b));
                }
            });
        ui.label("Choke");
        let mut choke = cfg.choke as i32;
        ui.add(egui::DragValue::new(&mut choke).range(0..=32).speed(0.1).custom_formatter(|v, _| {
            if v < 0.5 { "Off".into() } else { format!("{v:.0}") }
        }));
        cfg.choke = choke as u8;
        if cfg.kind == PadKind::Audio {
            ui.label("Mode");
            egui::ComboBox::from_id_salt("pad_mode")
                .width(70.0)
                .selected_text(mode_name(cfg.mode))
                .show_ui(ui, |ui| {
                    for m in [PlayMode::OneShot, PlayMode::Gate, PlayMode::Loop] {
                        ui.selectable_value(&mut cfg.mode, m, mode_name(m));
                    }
                });
            ui.checkbox(&mut cfg.reverse, "Rev");
        }
    });
    ui.separator();

    // The roll and step tabs edit the pad themselves, so they get the committed settings.
    let own_editor = matches!(app.tab, Tab::Roll | Tab::Steps);
    match app.tab {
        Tab::Wave => action = wave_tab(app, ui, &mut cfg).or(action),
        Tab::Mod => mod_tab(app, ui, &mut cfg),
        Tab::Roll | Tab::Steps => {}
    }
    if cfg != before {
        app.pads[sel].cfg = cfg;
        app.push_pad(sel);
    }
    if let Some(action) = action {
        app.pad_action(sel, action);
    }
    if own_editor {
        match app.tab {
            Tab::Roll => roll::draw(app, ui),
            _ => steps::draw(app, ui),
        }
    }
}

fn bus_name(b: u8) -> String {
    if b == 0 { "Thru".into() } else { format!("Bus {b}") }
}

fn mode_name(m: PlayMode) -> &'static str {
    match m {
        PlayMode::OneShot => "1-Shot",
        PlayMode::Gate => "Gate",
        PlayMode::Loop => "Loop",
    }
}

fn wave_tab(app: &mut App, ui: &mut Ui, cfg: &mut PadCfg) -> Option<PadAction> {
    let sel = app.selected;
    let mut action = None;
    if cfg.kind == PadKind::Midi {
        ui.label(egui::RichText::new("MIDI pad: switch to the Piano Roll tab (Tab) to edit its pattern.").color(theme::DIM));
        return None;
    }
    let Some(s) = app.pads[sel].sample().cloned() else {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("Empty pad").size(18.0).color(theme::DIM));
            ui.label("Drop an audio file here or onto any pad, or resample / skip-back into it.");
            if ui.button("Load sample…").clicked() {
                action = Some(PadAction::Browse);
            }
            if ui.button("Make this a MIDI pad").clicked() {
                action = Some(PadAction::MakeMidi);
            }
        });
        return action;
    };
    let sr = app.sr();

    ui.horizontal_wrapped(|ui| {
        let mut pick = |on: bool, a: PadAction| {
            if on {
                action = Some(a);
            }
        };
        pick(
            ui.button("Chop to bank").on_hover_text("C: transient slices to the lowest empty bank").clicked(),
            PadAction::Chop,
        );
        ui.label("Sens");
        if ui.add(egui::Slider::new(&mut app.sensitivity, 0.0..=1.0).show_value(false)).changed() {
            app.redetect(sel);
        }
        if ui.button("Key/BPM…").on_hover_text("Ctrl+K: harmonizer").clicked() {
            app.open_harmonizer();
        }
        ui.separator();
        theme::caption(ui, "BAKE");
        pick(ui.button("Crop").on_hover_text("Keep only the selected region").clicked(), PadAction::Crop);
        pick(ui.button("Normalize").clicked(), PadAction::Normalize);
        pick(ui.button("Reverse").clicked(), PadAction::Reverse);
        pick(ui.button("Fade In").on_hover_text("Fade across the selected region").clicked(), PadAction::FadeIn);
        pick(ui.button("Fade Out").clicked(), PadAction::FadeOut);
        pick(
            ui.button("Pitch").on_hover_text("Print the pad's pitch into the buffer (varispeed)").clicked(),
            PadAction::BakePitch,
        );
        pick(
            ui.button("Bus FX").on_hover_text("Print this pad's bus effects into the buffer").clicked(),
            PadAction::BakeFx,
        );
    });
    ui.horizontal(|ui| {
        let mut pick = |on: bool, a: PadAction| {
            if on {
                action = Some(a);
            }
        };
        let p = &app.pads[sel];
        let (i, n) = (p.take_idx + 1, p.takes.len());
        pick(
            ui.add_enabled(i > 1, egui::Button::new("<")).on_hover_text("Ctrl+Shift+Z").clicked(),
            PadAction::StepTake(-1),
        );
        ui.label(format!("take {i}/{n}"));
        pick(
            ui.add_enabled(i < n, egui::Button::new(">")).on_hover_text("Ctrl+Shift+Y").clicked(),
            PadAction::StepTake(1),
        );
        ui.separator();
        ui.label("Attack");
        ui.add(egui::DragValue::new(&mut cfg.attack).speed(0.001).range(0.0..=2.0).suffix(" s").fixed_decimals(3));
        ui.label("Release");
        ui.add(egui::DragValue::new(&mut cfg.release).speed(0.002).range(0.002..=4.0).suffix(" s").fixed_decimals(3));
        let len = s.len();
        let (a, b) = app.pads[sel].region();
        let info = format!(
            "{:.2}s · region {:.2}-{:.2}s · {} transients",
            len as f32 / sr,
            a as f32 / sr,
            b as f32 / sr,
            app.pads[sel].transients.len()
        );
        ui.label(egui::RichText::new(info).color(theme::DIM));
        if let Some(an) = &app.pads[sel].analysis
            && an.bpm > 0.0 {
                let key = format!(
                    "{:.1} BPM · {} {}",
                    an.bpm,
                    crate::music::NOTE_NAMES[an.key_root as usize],
                    if an.key_minor { "min" } else { "maj" }
                );
                ui.label(egui::RichText::new(key).color(theme::CYAN));
            }
    });

    let len = s.len();
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    if rect.width() < 8.0 || rect.height() < 8.0 || len < 2 {
        return action;
    }
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, theme::BG);

    // Scroll zooms around the cursor; shift+scroll pans.
    if resp.hovered() {
        let (scroll, shift, hover) =
            ui.input(|i| (i.smooth_scroll_delta, i.modifiers.shift, i.pointer.hover_pos()));
        let (v0, v1) = app.wave_view;
        let span = v1 - v0;
        if shift && scroll.y.abs() + scroll.x.abs() > 0.0 {
            let d = -((scroll.y + scroll.x) as f64) * 0.002 * span;
            let v0 = (v0 + d).clamp(0.0, 1.0 - span);
            app.wave_view = (v0, v0 + span);
        } else if scroll.y != 0.0 {
            let t = hover.map_or(0.5, |h| ((h.x - rect.left()) / rect.width()) as f64);
            let new_span = (span * (-scroll.y as f64 * 0.004).exp()).clamp(64.0 / len as f64, 1.0);
            let pivot = v0 + span * t;
            let v0 = (pivot - new_span * t).clamp(0.0, 1.0 - new_span);
            app.wave_view = (v0, v0 + new_span);
        }
    }
    let (v0, v1) = app.wave_view;
    let (a, b) = ((v0 * len as f64) as usize, ((v1 * len as f64) as usize).max(2).min(len));
    let x_of = |f: usize| rect.left() + (f as f32 - a as f32) / (b - a).max(1) as f32 * rect.width();
    let frame_at = |x: f32| -> usize {
        let t = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        (a as f32 + t * (b - a) as f32) as usize
    };

    // Region edits: drag sets the play region, double-click resets it.
    if resp.drag_started_by(PointerButton::Primary) {
        app.wave_anchor = ui.input(|i| i.pointer.press_origin()).map(|o| frame_at(o.x));
    }
    if resp.dragged_by(PointerButton::Primary)
        && let (Some(anchor), Some(pos)) = (app.wave_anchor, resp.interact_pointer_pos()) {
            let f = frame_at(pos.x);
            cfg.start = anchor.min(f) as u32;
            cfg.end = anchor.max(f).min(len) as u32;
        }
    if resp.drag_stopped() {
        app.wave_anchor = None;
        if cfg.end != 0 && cfg.end.saturating_sub(cfg.start) < 32 {
            cfg.start = 0;
            cfg.end = 0;
        }
    }
    if resp.double_clicked() {
        cfg.start = 0;
        cfg.end = 0;
    }
    // Right-click toggles a transient marker.
    if resp.secondary_clicked()
        && let Some(pos) = resp.interact_pointer_pos() {
            let f = frame_at(pos.x) as u32;
            let near = (8.0 / rect.width() * (b - a) as f32) as u32;
            let marks = &mut app.pads[sel].transients;
            if let Some(i) = marks.iter().position(|m| m.abs_diff(f) <= near) {
                marks.remove(i);
            } else {
                marks.push(f);
                marks.sort_unstable();
            }
        }

    let end = if cfg.end == 0 || cfg.end as usize > len { len } else { cfg.end as usize };
    let start = (cfg.start as usize).min(end);
    let region = Rect::from_min_max(Pos2::new(x_of(start), rect.top()), Pos2::new(x_of(end), rect.bottom()))
        .intersect(rect);
    p.rect_filled(region, 0.0, Color32::from_rgb(24, 28, 36));

    let cols = rect.width() as usize;
    let peaks = app.wave_peaks.get(&s, a, b, cols);
    let mid = rect.center().y;
    let half = rect.height() * 0.5 - 4.0;
    for (c, (lo, hi)) in peaks.iter().enumerate() {
        let x = rect.left() + c as f32 + 0.5;
        let inside = x >= region.left() && x <= region.right();
        let col = if inside { theme::AMBER } else { theme::AMBER_DIM.gamma_multiply(0.6) };
        p.vline(x, mid - hi.min(1.0) * half..=mid - lo.max(-1.0) * half + 1.0, Stroke::new(1.0, col));
    }
    p.hline(rect.x_range(), mid, Stroke::new(1.0, theme::LINE));

    for &t in &app.pads[sel].transients {
        let t = t as usize;
        if t >= a && t <= b {
            let x = x_of(t);
            p.vline(x, rect.y_range(), Stroke::new(1.0, theme::CYAN.gamma_multiply(0.55)));
            p.rect_filled(Rect::from_min_size(Pos2::new(x - 2.0, rect.top()), vec2(5.0, 5.0)), 0.0, theme::CYAN);
        }
    }
    for (f, label) in [(start, "S"), (end, "E")] {
        if f >= a && f <= b {
            let x = x_of(f);
            p.vline(x, rect.y_range(), Stroke::new(2.0, theme::TEXT));
            let flag = Rect::from_min_size(Pos2::new(if label == "S" { x } else { x - 12.0 }, rect.bottom() - 14.0), vec2(12.0, 14.0));
            p.rect_filled(flag, 0.0, theme::TEXT);
            p.text(flag.center(), Align2::CENTER_CENTER, label, FontId::proportional(10.0), theme::BG);
        }
    }
    let pos = app.shared.pad_pos[sel].load(std::sync::atomic::Ordering::Relaxed);
    if pos != u32::MAX && (pos as usize) >= a && (pos as usize) <= b {
        p.vline(x_of(pos as usize), rect.y_range(), Stroke::new(1.5, theme::GREEN));
    }
    p.text(
        rect.right_top() + vec2(-6.0, 4.0),
        Align2::RIGHT_TOP,
        "drag: region · double-click: reset · right-click: transient · scroll: zoom",
        FontId::proportional(10.0),
        theme::DIM,
    );
    action
}

fn mod_tab(app: &mut App, ui: &mut Ui, cfg: &mut PadCfg) {
    let sel = app.selected;
    ui.columns(3, |cols| {
        let ui = &mut cols[0];
        ui.label(egui::RichText::new("PITCH").small().color(theme::AMBER));
        let mut coarse = cfg.pitch.round();
        let mut fine = (cfg.pitch - coarse) * 100.0;
        ui.horizontal(|ui| {
            ui.label("Coarse");
            ui.add(egui::DragValue::new(&mut coarse).speed(0.1).range(-36.0..=36.0).fixed_decimals(0).suffix(" st"));
        });
        ui.horizontal(|ui| {
            ui.label("Fine");
            ui.add(egui::DragValue::new(&mut fine).speed(0.5).range(-50.0..=50.0).fixed_decimals(0).suffix(" ct"));
        });
        cfg.pitch = coarse + fine / 100.0;
        ui.add_space(6.0);
        ui.label(egui::RichText::new("FILTER").small().color(theme::AMBER));
        ui.horizontal(|ui| {
            knob(ui, &mut cfg.cutoff, "Cutoff", 1.0, theme::AMBER);
            knob(ui, &mut cfg.reso, "Reso", 0.0, theme::AMBER);
        });

        let ui = &mut cols[1];
        ui.label(egui::RichText::new("LFO").small().color(theme::CYAN));
        egui::ComboBox::from_id_salt("lfo_target")
            .selected_text(format!("to {:?}", cfg.lfo_target))
            .show_ui(ui, |ui| {
                for t in [LfoTarget::Off, LfoTarget::Pitch, LfoTarget::Volume, LfoTarget::Pan, LfoTarget::Filter] {
                    ui.selectable_value(&mut cfg.lfo_target, t, format!("{t:?}"));
                }
            });
        ui.horizontal(|ui| {
            ui.label("Rate");
            ui.add(egui::DragValue::new(&mut cfg.lfo_rate).speed(0.02).range(0.05..=30.0).suffix(" Hz").fixed_decimals(2));
        });
        ui.horizontal(|ui| {
            knob(ui, &mut cfg.lfo_depth, "Depth", 0.0, theme::CYAN);
        });

        let ui = &mut cols[2];
        ui.label(egui::RichText::new("WARP (bakes a new take)").small().color(theme::RED));
        ui.horizontal(|ui| {
            ui.label("Shift");
            ui.add(egui::DragValue::new(&mut app.warp_semis).speed(0.05).range(-24.0..=24.0).fixed_decimals(2).suffix(" st"));
        });
        ui.horizontal(|ui| {
            ui.label("Length");
            ui.add(egui::DragValue::new(&mut app.warp_stretch).speed(0.5).range(25.0..=400.0).fixed_decimals(0).suffix(" %"));
        });
        ui.checkbox(&mut app.preserve_transients, "Lock transients");
        let has = app.pads[sel].sample().is_some();
        if ui.add_enabled(has, egui::Button::new("Bake warp")).clicked() {
            let note = format!("warped {:+.2} st, {:.0}% length", app.warp_semis, app.warp_stretch);
            app.warp(sel, app.warp_stretch as f64 / 100.0, app.warp_semis, note);
        }
        ui.label(
            egui::RichText::new("Pitch here keeps the length. Formants move with the pitch; independent formant control isn't implemented.")
                .small()
                .color(theme::DIM),
        );
    });
}
