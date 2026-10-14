//! Top bar: transport, key/scale, swing, resample arm and the skip-back waveform.

use super::{pill, theme, toggle, transport_button};
use crate::app::{App, Dest, Resample};
use crate::engine::Cmd;
use crate::music::{NOTE_NAMES, Scale, pad_label};
use crate::shared::PEAK_HOP;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, vec2};

pub fn draw(app: &mut App, ui: &mut Ui) {
    let full = ui.available_rect_before_wrap();
    let split = full.left() + 600.0;
    let left = Rect::from_min_max(full.min, Pos2::new(split, full.bottom()));
    let right = Rect::from_min_max(Pos2::new(split + 6.0, full.top()), full.max);
    theme::card_in(ui, left, None, |ui| transport(app, ui));
    theme::card_in(ui, right, None, |ui| skip_back(app, ui));
}

fn transport(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        if transport_button(ui, app.playing()).clicked() {
            app.toggle_play();
        }
        ui.add_space(4.0);
        ui.vertical(|ui| {
            theme::caption(ui, "TEMPO");
            let mut bpm = app.bpm;
            ui.scope(|ui| {
                ui.style_mut().override_font_id = Some(theme::bold(19.0));
                ui.add(egui::DragValue::new(&mut bpm).speed(0.1).range(20.0..=300.0).fixed_decimals(1));
            });
            if bpm != app.bpm {
                app.bpm = bpm;
                app.send(Cmd::SetBpm(bpm));
            }
        });
        ui.add_space(4.0);
        ui.vertical(|ui| {
            theme::caption(ui, "KEY");
            ui.horizontal(|ui| {
                let mut key = app.key;
                egui::ComboBox::from_id_salt("key_root")
                    .width(46.0)
                    .selected_text(NOTE_NAMES[key.root as usize])
                    .show_ui(ui, |ui| {
                        for (i, n) in NOTE_NAMES.iter().enumerate() {
                            ui.selectable_value(&mut key.root, i as u8, *n);
                        }
                    });
                egui::ComboBox::from_id_salt("key_scale")
                    .width(104.0)
                    .selected_text(key.scale.name())
                    .show_ui(ui, |ui| {
                        for s in Scale::ALL {
                            ui.selectable_value(&mut key.scale, s, s.name());
                        }
                    });
                app.key = key;
            });
        });
        ui.add_space(4.0);
        ui.vertical(|ui| {
            theme::caption(ui, "SWING");
            let mut swing = app.swing * 100.0;
            ui.spacing_mut().slider_width = 92.0;
            ui.add(egui::Slider::new(&mut swing, 50.0..=75.0).fixed_decimals(0).suffix("%"));
            if (swing / 100.0 - app.swing).abs() > 1e-4 {
                app.swing = swing / 100.0;
                app.send(Cmd::SetSwing(app.swing));
            }
        });
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let blink = ui.input(|i| i.time).fract() < 0.5;
        let (text, lit) = match app.resample {
            Resample::Idle => ("RESAMPLE".to_string(), false),
            Resample::Armed => ("HIT THE TARGET PAD".to_string(), blink),
            Resample::Recording { target, .. } => (format!("REC to {}  ·  R to bake", pad_label(target)), true),
        };
        let label = egui::RichText::new(text).font(theme::bold(12.5)).color(if lit { theme::BG } else { theme::RED });
        let mut button = egui::Button::new(label).min_size(vec2(118.0, 26.0));
        button = if lit { button.fill(theme::RED) } else { button.stroke(Stroke::new(1.0, theme::RED.gamma_multiply(0.6))) };
        if ui.add(button).on_hover_text("R: arm, pick a pad, perform, R again to bake").clicked() {
            app.resample_key();
        }
        if toggle(ui, app.metronome, "Click", theme::CYAN).clicked() {
            app.metronome ^= true;
            app.send(Cmd::Metronome(app.metronome));
        }
        ui.add_space(4.0);
        if ui.add_enabled(app.history.can_undo(), egui::Button::new("Undo")).on_hover_text("Ctrl+Z").clicked() {
            app.undo();
        }
        if ui.add_enabled(app.history.can_redo(), egui::Button::new("Redo")).on_hover_text("Ctrl+Y").clicked() {
            app.redo();
        }
        let stop = egui::Button::new(egui::RichText::new("Stop All").color(theme::RED));
        if ui.add(stop).on_hover_text("Shift+Space: stop the transport and silence everything, effect tails included").clicked() {
            app.stop_all();
        }
        let song = app.view == crate::app::View::Arrange;
        if toggle(ui, song, "Song", theme::GREEN)
            .on_hover_text("F3: the arrangement, where loops become a song. Play from here plays the song.")
            .clicked()
        {
            app.toggle_view();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Help").on_hover_text("F1").clicked() {
                app.show_help ^= true;
            }
            if ui.button("Prefs").on_hover_text("Preferences: Ctrl+,").clicked() {
                app.open_prefs();
            }
            ui.menu_button("File", |ui| {
                if ui.button("New project    Ctrl+N").clicked() {
                    app.request_new();
                    ui.close();
                }
                if ui.button("Open project…    Ctrl+O").clicked() {
                    crate::project::open_dialog(app);
                    ui.close();
                }
                if ui.button("Save project    Ctrl+S").clicked() {
                    crate::project::save_dialog(app);
                    ui.close();
                }
                if ui.button("Save project as…").clicked() {
                    app.project_dir = None;
                    crate::project::save_dialog(app);
                    ui.close();
                }
                ui.separator();
                if ui.button("Load sample to selected pad…").clicked() {
                    app.browse_sample(app.selected);
                    ui.close();
                }
                if ui.button("Export selected pad as WAV…").clicked() {
                    app.export_pad(app.selected);
                    ui.close();
                }
            });
        });
    });
}

fn skip_back(app: &mut App, ui: &mut Ui) {
    let frozen = app.skip.frozen_end.is_some();
    ui.horizontal(|ui| {
        theme::title(ui, "SKIP-BACK", theme::AMBER);
        let now = app.shared.ring.written();
        if ui.button("Mark In").clicked() {
            app.skip.resume();
            app.skip.mark_in = Some(now);
        }
        if ui.button("Mark Out").clicked() {
            // With no In set, Out grabs the last four bars.
            let start = app.skip.mark_in.take().unwrap_or(now.saturating_sub((app.frames_per_bar() * 4.0) as u64));
            app.skip.sel = Some((start, now));
            app.skip.frozen_end = Some(now);
        }
        let has = app.skip.selection().is_some();
        ui.add_enabled_ui(has, |ui| {
            if ui.button("To Next Pad").clicked() {
                let (a, b) = app.skip.selection().unwrap();
                app.capture(a, b, Dest::NextEmpty);
                app.skip.resume();
            }
            if ui.button("Slice to Bank").clicked() {
                let (a, b) = app.skip.selection().unwrap();
                app.capture(a, b, Dest::Slice);
                app.skip.resume();
            }
        });
        if frozen && toggle(ui, true, "Go Live", theme::AMBER).on_hover_text("Esc").clicked() {
            app.skip.resume();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let max = app.shared.ring.frames as f32 / app.shared.sample_rate as f32;
            for secs in [300.0, 60.0, 30.0, 10.0] {
                if secs <= max + 0.5 && pill(ui, app.skip.window_secs == secs, &window_label(secs)).clicked() {
                    app.skip.window_secs = secs;
                }
            }
            ui.label(egui::RichText::new("drag to select, then drag onto a pad").small().color(theme::FAINT));
        });
    });

    let size = vec2(ui.available_width(), (ui.available_height() - 2.0).max(36.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 6.0, theme::WELL);

    let ring = &app.shared.ring;
    let sr = app.shared.sample_rate as f64;
    let written = ring.written();
    let end = app.skip.frozen_end.unwrap_or(written);
    let win = ((app.skip.window_secs as f64 * sr) as u64).max(1);
    let x_of = |f: u64| rect.right() - (end as f64 - f as f64) as f32 / win as f32 * rect.width();
    let frame_at = |x: f32| -> u64 {
        let back = ((rect.right() - x) / rect.width()).clamp(0.0, 1.0) as f64 * win as f64;
        (end as f64 - back).max(0.0) as u64
    };

    // Bar lines, when they're not too dense to read.
    let bar = app.frames_per_bar();
    if bar / win as f64 * rect.width() as f64 > 8.0 {
        let mut k = (end as f64 / bar).floor();
        while k >= 0.0 && k * bar > end as f64 - win as f64 {
            p.vline(x_of((k * bar) as u64), rect.y_range(), Stroke::new(1.0, theme::PANEL));
            k -= 1.0;
        }
    }

    // The waveform, brighter towards "now".
    let oldest = ring.oldest();
    let cols = rect.width() as usize;
    let mid = rect.center().y - 4.0;
    let half = rect.height() * 0.5 - 8.0;
    for c in 0..cols {
        let x = rect.left() + c as f32 + 0.5;
        let (a, b) = (frame_at(x - 0.5), frame_at(x + 0.5));
        if b <= oldest || a >= written || a == b {
            continue;
        }
        let (ca, cb) = (a / PEAK_HOP as u64, (b / PEAK_HOP as u64).max(a / PEAK_HOP as u64 + 1));
        let mut peak = 0f32;
        for chunk in ca..cb {
            if chunk * (PEAK_HOP as u64) < written.saturating_sub(PEAK_HOP as u64) {
                peak = peak.max(ring.peak(chunk));
            }
        }
        if peak > 0.0005 {
            let h = (peak.min(1.0).sqrt() * half).max(1.0);
            let age = c as f32 / cols as f32;
            let col = theme::mix(theme::AMBER_DIM, theme::AMBER, age * age);
            p.vline(x, mid - h..=mid + h, Stroke::new(1.0, col));
        }
    }
    p.hline(rect.x_range(), mid, Stroke::new(1.0, theme::LINE.gamma_multiply(0.7)));

    // Time ruler along the bottom.
    let step = [1.0, 2.0, 5.0, 10.0, 30.0, 60.0].into_iter().find(|s| s / app.skip.window_secs * rect.width() >= 70.0).unwrap_or(60.0);
    let mut t = step;
    while t < app.skip.window_secs {
        let x = rect.right() - t / app.skip.window_secs * rect.width();
        p.vline(x, rect.bottom() - 5.0..=rect.bottom(), Stroke::new(1.0, theme::FAINT));
        p.text(Pos2::new(x, rect.bottom() - 6.0), Align2::CENTER_BOTTOM, format!("-{}", window_label(t)), FontId::proportional(9.5), theme::FAINT);
        t += step;
    }

    // Selection: a fresh drag selects (and holds the display still while it does); a drag
    // that starts inside the selection carries it out to a pad. A plain click, or a drag too
    // short to mean anything, clears the selection and lets the display run again.
    let pointer = resp.interact_pointer_pos();
    if resp.drag_started()
        && let Some(pos) = ui.input(|i| i.pointer.press_origin())
    {
        let f = frame_at(pos.x);
        if app.skip.selection().is_some_and(|(a, b)| f >= a && f <= b) {
            app.skip.dragging_out = true;
        } else {
            app.skip.frozen_end = Some(end);
            app.skip.sel = Some((f, f));
            app.skip.mark_in = None;
        }
    }
    if resp.dragged()
        && !app.skip.dragging_out
        && let (Some(pos), Some((a, _))) = (pointer, app.skip.sel)
    {
        app.skip.sel = Some((a, frame_at(pos.x)));
    }
    if resp.drag_stopped() {
        if app.skip.dragging_out {
            app.skip.dragging_out = false;
            let hit = ui
                .ctx()
                .pointer_interact_pos()
                .and_then(|pos| app.pad_rects.iter().find(|(_, r)| r.contains(pos)).map(|(i, _)| *i));
            if let (Some(pad), Some((a, b))) = (hit, app.skip.selection()) {
                app.capture(a, b, Dest::Pad(pad));
                app.skip.resume();
            }
        } else if app.skip.selection().is_none() {
            app.skip.resume();
        }
    }
    if resp.clicked() {
        app.skip.resume();
    }

    if let Some((a, b)) = app.skip.selection() {
        let r = Rect::from_min_max(Pos2::new(x_of(a), rect.top()), Pos2::new(x_of(b), rect.bottom())).intersect(rect);
        p.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(255, 176, 46, 46));
        p.vline(r.left(), rect.y_range(), Stroke::new(1.5, theme::AMBER));
        p.vline(r.right(), rect.y_range(), Stroke::new(1.5, theme::AMBER));
        let secs = (b - a) as f64 / sr;
        p.text(
            r.left_top() + vec2(5.0, 3.0),
            Align2::LEFT_TOP,
            format!("{secs:.2}s · {:.2} bars", (b - a) as f64 / bar),
            theme::bold(10.5),
            theme::TEXT,
        );
    }
    if let Some(m) = app.skip.mark_in {
        p.vline(x_of(m), rect.y_range(), Stroke::new(1.5, theme::GREEN));
    }
    if let Resample::Recording { start, .. } = app.resample
        && start <= written
    {
        let r = Rect::from_min_max(Pos2::new(x_of(start), rect.top()), Pos2::new(x_of(written), rect.bottom())).intersect(rect);
        p.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(255, 70, 92, 44));
    }

    // Say plainly when the display is being held, since a held display shows no new audio.
    let (badge, col) = if app.skip.frozen_end.is_some() {
        ("HELD FOR SELECTION · click to go live", theme::AMBER)
    } else {
        ("LIVE", theme::GREEN)
    };
    let galley = p.layout_no_wrap(badge.to_owned(), theme::bold(9.5), col);
    let plate = Rect::from_min_size(rect.left_top() + vec2(6.0, 5.0), galley.size() + vec2(10.0, 4.0));
    p.rect_filled(plate, 4.0, theme::WELL.gamma_multiply(0.9));
    p.galley(plate.min + vec2(5.0, 2.0), galley, col);

    if app.skip.dragging_out
        && let Some(pos) = ui.ctx().pointer_interact_pos()
    {
        let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("skip_drag"));
        let lp = ui.ctx().layer_painter(layer);
        let tag = Rect::from_min_size(pos + vec2(10.0, 10.0), vec2(104.0, 22.0));
        lp.rect_filled(tag, 5.0, theme::AMBER);
        lp.text(tag.center(), Align2::CENTER_CENTER, "drop on a pad", theme::bold(11.0), theme::BG);
    }
}

fn window_label(secs: f32) -> String {
    if secs >= 60.0 { format!("{:.0}m", secs / 60.0) } else { format!("{secs:.0}s") }
}
