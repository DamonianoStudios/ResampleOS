//! Zone C: four performance buses and the master, three effect slots each.

use super::{fader, knob, knob_sized, meter, theme, toggle};
use crate::app::{App, PluginTarget};
use crate::engine::SlotCfg;
use crate::fx::FxKind;
use crate::shared::{MASTER, NUM_BUSES, f32_load};
use eframe::egui::{self, Rect, Sense, Ui, vec2};

pub fn draw(app: &mut App, ui: &mut Ui) {
    let full = ui.available_rect_before_wrap();
    let gap = 6.0;
    let w = (full.width() - gap * (NUM_BUSES as f32 - 1.0)) / NUM_BUSES as f32;
    for b in 0..NUM_BUSES {
        let rect = Rect::from_min_size(full.min + vec2(b as f32 * (w + gap), 0.0), vec2(w, full.height()));
        let accent = if b == MASTER { theme::CYAN } else { theme::AMBER };
        let border = (app.focus_bus == b).then_some(accent);
        theme::card_in(ui, rect, border, |ui| strip(app, ui, b));
    }
}

fn hertz(hz: f32) -> String {
    if hz >= 1000.0 { format!("{:.1} kHz", hz / 1000.0) } else { format!("{hz:.0} Hz") }
}

fn strip(app: &mut App, ui: &mut Ui, b: usize) {
    let before = app.buses[b];
    let mut cfg = before;
    let focused = app.focus_bus == b;
    let (mut browse, mut editor, mut unload) = (None, None, None);
    let accent = if b == MASTER { theme::CYAN } else { theme::AMBER };

    {
        ui.horizontal(|ui| {
            let title = if b == MASTER { "MASTER".to_string() } else { format!("BUS {}", b + 1) };
            let text = egui::RichText::new(title).font(theme::bold(12.5)).color(if focused { accent } else { theme::TEXT }).extra_letter_spacing(1.0);
            if ui.add(egui::Label::new(text).sense(Sense::click())).on_hover_text(format!("Focus: key {}", b + 1)).clicked() {
                app.focus_bus = b;
            }
            if toggle(ui, cfg.mute, "M", theme::RED).clicked() {
                cfg.mute ^= true;
            }
            sidechain_menu(ui, &mut cfg, b);
        });

        ui.horizontal(|ui| {
            // Level meter down the left edge.
            let (mrect, _) = ui.allocate_exact_size(vec2(12.0, 286.0), Sense::hover());
            let p = ui.painter_at(mrect);
            let half = (mrect.width() - 2.0) * 0.5;
            meter(&p, Rect::from_min_size(mrect.min, vec2(half, mrect.height())), f32_load(&app.shared.bus_peak[b * 2]));
            meter(
                &p,
                Rect::from_min_size(mrect.min + vec2(half + 2.0, 0.0), vec2(half, mrect.height())),
                f32_load(&app.shared.bus_peak[b * 2 + 1]),
            );

            ui.vertical(|ui| {
                for s in 0..3 {
                    let slot = &mut cfg.slots[s];
                    let slot_focus = focused && app.focus_slot == s;
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut slot.on, "");
                        let mut kind = slot.kind;
                        egui::ComboBox::from_id_salt(("fx_kind", b, s))
                            .width(98.0)
                            .height(360.0)
                            .selected_text(
                                egui::RichText::new(kind.name()).color(if slot_focus { accent } else { theme::TEXT }),
                            )
                            .show_ui(ui, |ui| {
                                for k in FxKind::ALL {
                                    ui.selectable_value(&mut kind, k, k.name());
                                }
                                if ui.selectable_label(kind == FxKind::Vst, "VST3 effect…").clicked() {
                                    browse = Some(s);
                                }
                            });
                        if kind != slot.kind {
                            if slot.kind == FxKind::Vst {
                                unload = Some(s);
                            }
                            *slot = SlotCfg::new(kind, true);
                            app.focus_bus = b;
                            app.focus_slot = s;
                        }
                    });
                    if slot.kind == FxKind::Vst {
                        ui.horizontal(|ui| {
                            knob(ui, &mut slot.params[0], "Mix", 1.0, if slot.on { accent } else { theme::DIM });
                            let name = app.fx_plugins[b * 3 + s].as_ref().map(|p| p.name.clone());
                            match name {
                                Some(n) => {
                                    let short: String = n.chars().take(14).collect();
                                    if ui.button(short).on_hover_text(format!("{n}: open editor")).clicked() {
                                        editor = Some(s);
                                    }
                                }
                                None => {
                                    if ui.button("Choose…").clicked() {
                                        browse = Some(s);
                                    }
                                }
                            }
                        });
                    } else if slot.kind != FxKind::None {
                        ui.horizontal(|ui| {
                            let names = slot.kind.param_names();
                            let defaults = slot.kind.defaults();
                            let col = if slot.on { accent } else { theme::DIM };
                            // An effect with sliders has five controls in the room of three.
                            let sliders = slot.kind.slider_names();
                            let radius = if sliders.is_some() { 8.5 } else { 13.0 };
                            if sliders.is_some() {
                                ui.spacing_mut().item_spacing.x = 3.0;
                            }
                            let mut touched = false;
                            for k in 0..3 {
                                let r = knob_sized(ui, &mut slot.params[k], names[k], defaults[k], col, radius);
                                touched |= r.dragged() || r.clicked();
                            }
                            if let Some(names) = sliders {
                                let defaults = slot.kind.slider_defaults();
                                let hz = [crate::fx::eq_low_hz(slot.sliders[0]), crate::fx::eq_high_hz(slot.sliders[1])];
                                let what = ["Low shelf below", "High shelf above"];
                                for k in 0..2 {
                                    let r = fader(ui, &mut slot.sliders[k], names[k], defaults[k], col);
                                    touched |= r.dragged() || r.clicked();
                                    let (mid, _) = crate::fx::eq_mid(hz[0], hz[1]);
                                    r.on_hover_text(format!("{} {}. The mid bell sits at {}.", what[k], hertz(hz[k]), hertz(mid)));
                                }
                            }
                            if touched {
                                // The hardware encoders follow whichever slot was touched last.
                                app.focus_bus = b;
                                app.focus_slot = s;
                            }
                        });
                    } else {
                        ui.add_space(10.0);
                    }
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().slider_width = 76.0;
                    ui.add(egui::Slider::new(&mut cfg.level, 0.0..=1.5).show_value(false)).on_hover_text("Bus level");
                    if b != MASTER {
                        egui::ComboBox::from_id_salt(("bus_out", b))
                            .width(64.0)
                            .selected_text(out_name(cfg.out, b))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut cfg.out, 0, "to Master");
                                // Only later buses, so the chain can never loop back on itself.
                                for o in b + 2..=MASTER {
                                    ui.selectable_value(&mut cfg.out, o as u8, format!("to Bus {o}"));
                                }
                            });
                    }

                });
            });
        });
    }

    if cfg != before {
        app.buses[b] = cfg;
        app.push_bus(b);
    }
    if let Some(s) = unload {
        // The slot was switched to a built-in effect; let the plugin go.
        let kept = app.buses[b].slots[s];
        app.remove_plugin(PluginTarget::Fx(b, s));
        app.buses[b].slots[s] = kept;
        app.push_bus(b);
    }
    if let Some(s) = browse {
        app.open_browser(PluginTarget::Fx(b, s));
    }
    if let Some(s) = editor {
        app.open_editor(PluginTarget::Fx(b, s));
    }
}

/// The "SC" button and its popup: which bus keys this one, and what the key does to it.
fn sidechain_menu(ui: &mut Ui, cfg: &mut crate::engine::BusCfg, b: usize) {
    use egui::containers::menu::{MenuButton, MenuConfig};
    let active = cfg.sidechain_source(b).is_some();
    let label = egui::RichText::new("SC").color(if active { theme::BG } else { theme::TEXT });
    let mut button = egui::Button::new(label);
    if active {
        button = button.fill(if cfg.sc_amount < 0.0 { theme::PURPLE } else { theme::GREEN });
    }
    let config = MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let (resp, _) = MenuButton::from_button(button).config(config).ui(ui, |ui| {
        ui.set_min_width(230.0);
        theme::title(ui, "SIDECHAIN", theme::AMBER);
        ui.label(egui::RichText::new("Let what is sent into another bus control this one's level.").small().color(theme::DIM));
        ui.add_space(4.0);
        theme::caption(ui, "KEYED BY");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut cfg.sc_src, 0, "Off");
            for src in 1..=4u8 {
                if src as usize - 1 != b {
                    ui.selectable_value(&mut cfg.sc_src, src, format!("Bus {src}"));
                }
            }
        });
        ui.add_space(4.0);
        theme::caption(ui, "WHEN THE KEY SOUNDS");
        let mut swell = cfg.sc_amount > 0.0;
        let mut depth = cfg.sc_amount.abs() * 100.0;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut swell, false, "Duck").on_hover_text("Turn this bus down: bass under a kick");
            ui.selectable_value(&mut swell, true, "Swell").on_hover_text("Keep this bus down and let it rise with the key: crackle on the snare");
        });
        ui.add(egui::Slider::new(&mut depth, 5.0..=100.0).fixed_decimals(0).suffix("%").text("Depth"));
        cfg.sc_amount = depth / 100.0 * if swell { 1.0 } else { -1.0 };
        let mut ms = cfg.sc_release * 1000.0;
        ui.add(egui::Slider::new(&mut ms, 20.0..=1000.0).logarithmic(true).fixed_decimals(0).suffix(" ms").text("Release"));
        cfg.sc_release = ms / 1000.0;
    });
    resp.on_hover_text("Sidechain: duck or swell this bus from another bus");
}

fn out_name(out: u8, b: usize) -> String {
    if out as usize > b + 1 && (out as usize) <= MASTER { format!("to Bus {out}") } else { "to Master".into() }
}
