//! Zone A: bank tabs and the pad matrix.

use super::{pill, tag, theme};
use crate::app::{App, PAD_KEY_LABELS, PluginTarget, Resample};
use crate::engine::PadKind;
use crate::music::{bank_name, pad_label};
use crate::shared::f32_load;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, vec2};

pub fn draw(app: &mut App, ui: &mut Ui) {
    let mut remove = None;
    let mut trim = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        theme::title(ui, "PADS", theme::AMBER);
        ui.add_space(8.0);
        for b in 0..app.banks {
            let used = (0..16).any(|i| !app.pads[b * 16 + i].is_empty());
            let tab = pill(ui, app.bank == b, &bank_name(b));
            if used && app.bank != b {
                // A dot marks banks that hold something.
                ui.painter().circle_filled(tab.rect.right_top() + vec2(-6.0, 6.0), 2.0, theme::AMBER);
            }
            if tab.clicked() {
                app.bank = b;
            }
            tab.context_menu(|ui| {
                let label = if used { format!("Remove bank {} and its pads", bank_name(b)) } else { format!("Remove bank {}", bank_name(b)) };
                if ui.add_enabled(app.banks > 1, egui::Button::new(label)).clicked() {
                    remove = Some(b);
                    ui.close();
                }
                if ui.button("Remove all empty banks").clicked() {
                    trim = true;
                    ui.close();
                }
            });
        }
        if pill(ui, false, "+").on_hover_text("Add a bank. Right-click a bank to remove it.").clicked() && app.add_bank() {
            app.bank = app.banks - 1;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (z, label) in ["16×16", "8×8", "4×4"].iter().enumerate() {
                let z = 2 - z as u8;
                if pill(ui, app.zoom == z, label).clicked() {
                    app.zoom = z;
                }
            }
        });
    });
    super::scenes::strip(app, ui);
    ui.add_space(2.0);

    if let Some(b) = remove {
        app.remove_bank(b);
    }
    if trim {
        app.trim_banks();
    }

    let avail = ui.available_rect_before_wrap();
    let side = avail.width().min(avail.height()) - 4.0;
    // Centred in whatever space the zone has.
    let area = Rect::from_center_size(avail.center(), vec2(side, side));
    ui.allocate_rect(avail, Sense::hover());
    if app.perform {
        super::scenes::launcher(app, ui, area);
        return;
    }

    // Ctrl+scroll or pinch over the matrix steps the zoom level.
    if ui.rect_contains_pointer(area) {
        let z = ui.input(|i| i.zoom_delta());
        if z > 1.02 && app.zoom > 0 {
            app.zoom -= 1;
        } else if z < 0.98 && app.zoom < 2 {
            app.zoom += 1;
        }
    }

    let per_row = 1usize << app.zoom; // bank blocks per row
    let group = per_row * per_row;
    let first_bank = if app.zoom == 0 { app.bank } else { app.bank / group * group };
    let n = per_row * 4;
    let cell = side / n as f32;
    let gap = (cell * 0.05).clamp(1.0, 5.0);

    app.pad_rects.clear();
    for bi in 0..group {
        let bank = first_bank + bi;
        if bank >= app.banks {
            continue;
        }
        let (bx, by) = (bi % per_row, bi / per_row);
        if app.zoom > 0 {
            let block = Rect::from_min_size(
                area.min + vec2(bx as f32 * 4.0 * cell, by as f32 * 4.0 * cell),
                vec2(cell * 4.0, cell * 4.0),
            );
            let col = if bank == app.bank { theme::AMBER } else { theme::LINE };
            ui.painter().rect_stroke(block.shrink(0.5), 3.0, Stroke::new(1.0, col), StrokeKind::Inside);
        }
        for i in 0..16 {
            // Pad 1 sits bottom-left, as on the hardware.
            let (cx, cy) = (bx * 4 + i % 4, by * 4 + 3 - i / 4);
            let rect = Rect::from_min_size(area.min + vec2(cx as f32 * cell, cy as f32 * cell), vec2(cell, cell))
                .shrink(gap);
            pad_cell(app, ui, rect, bank * 16 + i);
        }
    }
}

fn pad_cell(app: &mut App, ui: &mut Ui, rect: Rect, idx: usize) {
    app.pad_rects.push((idx, rect));
    let resp = ui.interact(rect, egui::Id::new(("pad", idx)), Sense::click());
    let slot = idx % 16;
    let shift = ui.input(|i| i.modifiers.shift);

    if resp.contains_pointer() && ui.input(|i| i.pointer.primary_pressed()) {
        if shift {
            // Shift selects silently, even while a resample is waiting for its target.
            app.select(idx);
        } else if app.chromatic && idx / 16 == app.bank {
            app.mouse_held = Some(app.slot_hit(slot, 0x2000 + slot as u32));
        } else {
            app.bank = idx / 16;
            app.pad_hit(idx, 0x2000 + idx as u32);
            app.mouse_held = Some((idx as u16, 0x2000 + idx as u32));
        }
    }
    resp.context_menu(|ui| {
        ui.label(egui::RichText::new(pad_label(idx)).strong());
        if ui.button("Stop playback").clicked() {
            app.stop_pad(idx);
            ui.close();
        }
        if ui.button("Load sample…").clicked() {
            app.browse_sample(idx);
            ui.close();
        }
        if ui.button("Load VST3 instrument…").clicked() {
            app.open_browser(PluginTarget::Pad(idx));
            ui.close();
        }
        if app.pads[idx].plugin.is_some() && ui.button("Open plugin editor").clicked() {
            app.open_editor(PluginTarget::Pad(idx));
            ui.close();
        }
        let kind = app.pads[idx].cfg.kind;
        if kind == PadKind::Audio && ui.button("Make MIDI / piano roll pad").clicked() {
            app.set_kind(idx, PadKind::Midi);
            app.select(idx);
            ui.close();
        }
        if kind == PadKind::Midi && ui.button("Make audio pad").clicked() {
            app.set_kind(idx, PadKind::Audio);
            ui.close();
        }
        if ui.button("Copy").clicked() {
            app.clipboard = Some(idx);
            ui.close();
        }
        if let Some(from) = app.clipboard
            && ui.button(format!("Paste {}", pad_label(from))).clicked() {
                app.copy_pad(from, idx);
                ui.close();
            }
        if ui.button("Export WAV…").clicked() {
            app.export_pad(idx);
            ui.close();
        }
        if ui.button("Clear").clicked() {
            app.clear_pad(idx);
            ui.close();
        }
    });

    let pad = &app.pads[idx];
    let level = f32_load(&app.shared.pad_level[idx]);
    let blink = ui.input(|i| i.time * 3.0).fract() < 0.5;
    let rec_target = matches!(app.resample, Resample::Recording { target, .. } if target == idx);
    let armed = app.resample == Resample::Armed;
    let empty = pad.is_empty();

    // LED colour, by the spec's priority.
    let led = if rec_target && blink {
        theme::RED
    } else if level > 0.0 {
        theme::GREEN
    } else if pad.cfg.mute || pad.cfg.choke > 0 {
        theme::PURPLE
    } else if pad.cfg.kind == PadKind::Midi {
        theme::CYAN_DIM
    } else if !empty {
        theme::AMBER_DIM
    } else {
        theme::RAISED
    };

    let p = ui.painter_at(rect.expand(4.0));
    // A sounding pad lights up in proportion to how loud it is.
    let glow = level.sqrt().min(1.0);
    let body = if level > 0.0 {
        theme::mix(Color32::from_rgb(22, 48, 32), Color32::from_rgb(34, 92, 56), glow)
    } else if empty {
        theme::WELL
    } else if resp.hovered() {
        theme::HOVER
    } else {
        theme::RAISED
    };
    let radius = (rect.width() * 0.09).min(10.0);
    if level > 0.0 {
        p.rect_filled(rect.expand(2.5), radius + 2.0, theme::GREEN.gamma_multiply(0.10 + 0.22 * glow));
    }
    p.rect_filled(rect, radius, body);
    if !empty && rect.height() > 40.0 {
        // A lighter upper half reads as a moulded pad catching the light.
        let top = Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.top() + rect.height() * 0.5));
        p.rect_filled(top, egui::CornerRadius { nw: radius as u8, ne: radius as u8, sw: 0, se: 0 }, Color32::from_white_alpha(5));
    }
    let selected = idx == app.selected;
    let border = if rec_target || (armed && blink) {
        theme::RED
    } else if selected {
        theme::TEXT
    } else if resp.hovered() && !empty {
        theme::DIM
    } else if empty {
        theme::LINE
    } else {
        led
    };
    p.rect_stroke(rect, radius, Stroke::new(if selected { 2.0 } else { 1.0 }, border), StrokeKind::Inside);

    let w = rect.width();
    if w < 34.0 {
        // 16x16 overview: just the LED.
        p.rect_filled(rect.shrink(w * 0.28), 2.0, led);
        return;
    }

    let compact = w < 80.0;
    let font = theme::bold(if compact { 9.5 } else { 12.5 });
    let label_col = if empty { theme::FAINT } else { theme::TEXT };
    p.text(rect.left_top() + vec2(7.0, 5.0), Align2::LEFT_TOP, pad_label(idx), font, label_col);
    let led_at = rect.right_top() + vec2(-11.0, 11.0);
    if !empty {
        p.circle_filled(led_at, if compact { 5.0 } else { 7.0 }, led.gamma_multiply(0.25));
    }
    p.circle_filled(led_at, if compact { 3.0 } else { 4.0 }, led);

    // Mini waveform or MIDI lane.
    let inner = Rect::from_min_max(
        rect.left_top() + vec2(7.0, if compact { 16.0 } else { 38.0 }),
        rect.right_bottom() - vec2(7.0, if compact { 5.0 } else { 24.0 }),
    );
    if inner.height() > 6.0 {
        let content = if level > 0.0 { theme::GREEN } else { led };
        match pad.cfg.kind {
            PadKind::Audio => {
                let n = pad.mini.len();
                let mid = inner.center().y;
                for (i, (lo, hi)) in pad.mini.iter().enumerate() {
                    let x = inner.left() + (i as f32 + 0.5) / n as f32 * inner.width();
                    let h = inner.height() * 0.5;
                    p.vline(x, mid - hi * h..=mid - lo * h + 0.5, Stroke::new((inner.width() / n as f32 * 0.7).max(1.0), content));
                }
                let pos = app.shared.pad_pos[idx].load(std::sync::atomic::Ordering::Relaxed);
                if let (true, Some(s)) = (pos != u32::MAX, pad.sample()) {
                    let x = inner.left() + pos as f32 / s.len().max(1) as f32 * inner.width();
                    p.vline(x, inner.y_range(), Stroke::new(1.0, theme::TEXT));
                }
            }
            PadKind::Midi => {
                let pat = &pad.pattern;
                let (lo, hi) = pat.notes.iter().fold((127u8, 0u8), |(l, h), n| (l.min(n.pitch), h.max(n.pitch)));
                let span = (hi.saturating_sub(lo) as f32 + 1.0).max(8.0);
                for n in &pat.notes {
                    let x0 = inner.left() + n.start as f32 / pat.len_ticks as f32 * inner.width();
                    let x1 = inner.left() + (n.start + n.len) as f32 / pat.len_ticks as f32 * inner.width();
                    let y = inner.bottom() - (n.pitch - lo) as f32 / span * inner.height() - 2.0;
                    p.rect_filled(
                        Rect::from_min_max(Pos2::new(x0, y - 1.5), Pos2::new(x1.max(x0 + 2.0).min(inner.right()), y + 1.5)),
                        0.0,
                        if level > 0.0 { theme::GREEN } else { theme::CYAN },
                    );
                }
                if app.playing() && pat.len_ticks > 0 {
                    let t = app.shared.tick().rem_euclid(pat.len_ticks as f64) as f32;
                    let x = inner.left() + t / pat.len_ticks as f32 * inner.width();
                    p.vline(x, inner.y_range(), Stroke::new(1.0, theme::TEXT));
                }
            }
        }
    }
    if compact {
        return;
    }

    let name: String = pad.name.chars().take((w / 7.5) as usize).collect();
    p.text(rect.left_top() + vec2(7.0, 22.0), Align2::LEFT_TOP, name, FontId::proportional(10.5), theme::DIM);

    // Bottom tags: bus target, choke group, mute/solo, and the computer key that plays it.
    if !empty {
        let at = |x: f32| Pos2::new(x, rect.bottom() - 19.0);
        let bus = if pad.cfg.bus == 0 { "THRU".to_string() } else { format!("BUS {}", pad.cfg.bus) };
        let mut x = tag(&p, at(rect.left() + 7.0), &bus, theme::TEXT, theme::BG.gamma_multiply(0.6)) + 4.0;
        if pad.cfg.choke > 0 {
            x = tag(&p, at(x), &format!("CK {}", pad.cfg.choke), theme::BG, theme::PURPLE) + 4.0;
        }
        if pad.cfg.mute {
            x = tag(&p, at(x), "M", theme::BG, theme::RED) + 4.0;
        }
        if pad.cfg.solo {
            tag(&p, at(x), "S", theme::BG, theme::AMBER);
        }
    }
    if idx / 16 == app.bank {
        p.text(
            rect.right_bottom() - vec2(6.0, 4.0),
            Align2::RIGHT_BOTTOM,
            PAD_KEY_LABELS[slot],
            FontId::monospace(10.0),
            theme::FAINT,
        );
    }
}
