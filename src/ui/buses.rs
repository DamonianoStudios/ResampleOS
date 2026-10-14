//! Zone C: four performance buses, the vocal bus and the master, laid out like channels of
//! a mixing console: three effect slots, the output under them, and a long fader with the
//! bus meter at the right.

use super::{channel_fader, fader, knob, knob_sized, theme, toggle};
use crate::app::{App, PluginTarget};
use crate::engine::{BusCfg, SlotCfg};
use crate::fx::FxKind;
use crate::shared::{MASTER, NUM_BUSES, VOCAL, bus_label, f32_load};
use eframe::egui::{self, Pos2, Rect, Sense, Ui, vec2};

pub fn draw(app: &mut App, ui: &mut Ui) {
    let full = ui.available_rect_before_wrap();
    let gap = 6.0;
    let w = (full.width() - gap * (NUM_BUSES as f32 - 1.0)) / NUM_BUSES as f32;
    for b in 0..NUM_BUSES {
        let rect = Rect::from_min_size(full.min + vec2(b as f32 * (w + gap), 0.0), vec2(w, full.height()));
        let accent = accent_of(b);
        let border = (app.focus_bus == b).then_some(accent);
        theme::card_with_margin(ui, rect, border, vec2(8.0, 10.0), |ui| strip(app, ui, b));
    }
}

/// Width of the fader and meter column at the right of each strip.
const FADER_W: f32 = 28.0;

fn accent_of(b: usize) -> eframe::egui::Color32 {
    match b {
        MASTER => theme::CYAN,
        VOCAL => theme::MAGENTA,
        _ => theme::AMBER,
    }
}

fn hertz(hz: f32) -> String {
    if hz >= 1000.0 { format!("{:.1} kHz", hz / 1000.0) } else { format!("{hz:.0} Hz") }
}

/// What a strip's controls ask for that has to wait until the strip is drawn.
#[derive(Default)]
struct Requests {
    /// A slot to choose a VST3 effect for, one whose editor to open, and one that left
    /// its plugin for a built-in effect.
    browse: Option<usize>,
    editor: Option<usize>,
    unload: Option<usize>,
}

fn strip(app: &mut App, ui: &mut Ui, b: usize) {
    let before = app.buses[b];
    let mut cfg = before;
    let mut asks = Requests::default();
    header(app, ui, &mut cfg, b);

    // Effects on the left, the output under them; fader and meter on the right. The two get
    // fixed shares of the strip, so effects that run wide are cut off at their edge rather
    // than pushing the fader out of the strip.
    let full = ui.available_rect_before_wrap();
    ui.allocate_rect(full, Sense::hover());
    let fader_rect = Rect::from_min_max(Pos2::new(full.right() - FADER_W, full.top()), full.max);
    let left = Rect::from_min_max(full.min, Pos2::new(fader_rect.left() - 5.0, full.bottom()));
    let mut column = ui.new_child(egui::UiBuilder::new().max_rect(left).layout(egui::Layout::top_down(egui::Align::Min)));
    column.set_clip_rect(left.expand2(vec2(1.0, 4.0)).intersect(ui.clip_rect()));
    for s in 0..3 {
        effect_slot(app, &mut column, &mut cfg, b, s, left.width(), &mut asks);
    }
    if b != MASTER {
        output_menu(&mut column, &mut cfg, b, left.width());
    }
    let peaks = (f32_load(&app.shared.bus_peak[b * 2]), f32_load(&app.shared.bus_peak[b * 2 + 1]));
    let mut fader_ui = ui.new_child(egui::UiBuilder::new().max_rect(fader_rect));
    channel_fader(&mut fader_ui, &mut cfg.level, peaks, vec2(FADER_W, full.height() - 2.0), accent_of(b));

    if cfg != before {
        app.buses[b] = cfg;
        app.push_bus(b);
    }
    if let Some(s) = asks.unload {
        // The slot was switched to a built-in effect; let the plugin go.
        let kept = app.buses[b].slots[s];
        app.remove_plugin(PluginTarget::Fx(b, s));
        app.buses[b].slots[s] = kept;
        app.push_bus(b);
    }
    if let Some(s) = asks.browse {
        app.open_browser(PluginTarget::Fx(b, s));
    }
    if let Some(s) = asks.editor {
        app.open_editor(PluginTarget::Fx(b, s));
    }
}

/// Name (click to focus), mute, sidechain and pan. Six strips share the row, so it is
/// packed tight.
fn header(app: &mut App, ui: &mut Ui, cfg: &mut BusCfg, b: usize) {
    let accent = accent_of(b);
    let focused = app.focus_bus == b;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let title = bus_label(b).to_uppercase();
        let text = egui::RichText::new(title).font(theme::bold(12.5)).color(if focused { accent } else { theme::TEXT }).extra_letter_spacing(1.0);
        if ui.add(egui::Label::new(text).sense(Sense::click())).on_hover_text(format!("Focus: key {}", b + 1)).clicked() {
            app.focus_bus = b;
        }
        if toggle(ui, cfg.mute, "M", theme::RED).clicked() {
            cfg.mute ^= true;
        }
        sidechain_menu(ui, cfg, b);
        let width = ui.available_width().clamp(26.0, 40.0);
        super::pan_box(ui, &mut cfg.pan, accent, width);
    });
}

/// One effect slot: on/off and the effect, then its controls.
fn effect_slot(app: &mut App, ui: &mut Ui, cfg: &mut BusCfg, b: usize, s: usize, width: f32, asks: &mut Requests) {
    let accent = accent_of(b);
    let slot = &mut cfg.slots[s];
    let slot_focus = app.focus_bus == b && app.focus_slot == s;
    ui.horizontal(|ui| {
        ui.checkbox(&mut slot.on, "");
        let mut kind = slot.kind;
        egui::ComboBox::from_id_salt(("fx_kind", b, s))
            // The width given is the text's; the arrow and padding come on top.
            .width((ui.available_width() - 30.0).clamp(50.0, 98.0))
            .height(420.0)
            .selected_text(egui::RichText::new(kind.name()).color(if slot_focus { accent } else { theme::TEXT }))
            .show_ui(ui, |ui| {
                for k in FxKind::ALL {
                    ui.selectable_value(&mut kind, k, k.name());
                }
                if ui.selectable_label(kind == FxKind::Vst, "VST3 effect…").clicked() {
                    asks.browse = Some(s);
                }
            });
        if kind != slot.kind {
            if slot.kind == FxKind::Vst {
                asks.unload = Some(s);
            }
            *slot = SlotCfg::new(kind, true);
            app.focus_bus = b;
            app.focus_slot = s;
        }
    });
    let col = if slot.on { accent } else { theme::DIM };
    match slot.kind {
        FxKind::None => ui.add_space(10.0),
        FxKind::Vst => {
            ui.horizontal(|ui| {
                knob(ui, &mut slot.params[0], "Mix", 1.0, col);
                match app.fx_plugins[b * 3 + s].as_ref().map(|p| p.name.clone()) {
                    Some(n) => {
                        let short: String = n.chars().take(14).collect();
                        if ui.button(short).on_hover_text(format!("{n}: open editor")).clicked() {
                            asks.editor = Some(s);
                        }
                    }
                    None => {
                        if ui.button("Choose…").clicked() {
                            asks.browse = Some(s);
                        }
                    }
                }
            });
        }
        _ => {
            if ui.horizontal(|ui| built_in_controls(ui, slot, width, col)).inner {
                // The hardware encoders follow whichever slot was touched last.
                app.focus_bus = b;
                app.focus_slot = s;
            }
        }
    }
}

/// Spacing and knob radius for three knobs (and, for the EQ, two faders) in `room` points.
///
/// Below the standard size a knob gives up some of the room kept for its label (see
/// `knob_sized`), which is what lets three of them and the EQ's faders share a narrow strip.
fn knob_layout(room: f32, sliders: bool) -> (f32, f32) {
    let gap = if sliders { 2.0 } else if room < 120.0 { 3.0 } else { 6.0 };
    let faders = if sliders { 2.0 * (16.0 + gap) } else { 0.0 };
    let each = (room - faders - gap * 2.0) / 3.0;
    let full_size = (each - 20.0) / 2.0;
    let radius = if full_size >= 7.5 { full_size.min(if sliders { 8.5 } else { 13.0 }) } else { ((each - 12.0) / 2.0).clamp(6.0, 7.4) };
    (gap, radius)
}

/// A built-in effect's knobs and sliders. Returns whether one was touched.
fn built_in_controls(ui: &mut Ui, slot: &mut SlotCfg, width: f32, col: egui::Color32) -> bool {
    let names = slot.kind.param_names();
    let defaults = slot.kind.defaults();
    let sliders = slot.kind.slider_names();
    // The column's own width, not what is left of it: a row that ran wide earlier must not
    // make the knobs think there is more room.
    let (gap, radius) = knob_layout(width, sliders.is_some());
    ui.spacing_mut().item_spacing.x = gap;
    let mut touched = false;
    for k in 0..3 {
        let r = knob_sized(ui, &mut slot.params[k], names[k], defaults[k], col, radius);
        touched |= r.dragged() || r.clicked();
    }
    if let Some(names) = sliders {
        let defaults = slot.kind.slider_defaults();
        let hz = [crate::fx::eq_low_hz(slot.sliders[0]), crate::fx::eq_high_hz(slot.sliders[1])];
        let (mid, _) = crate::fx::eq_mid(hz[0], hz[1]);
        let what = ["Low shelf below", "High shelf above"];
        for k in 0..2 {
            let r = fader(ui, &mut slot.sliders[k], names[k], defaults[k], col);
            touched |= r.dragged() || r.clicked();
            r.on_hover_text(format!("{} {}. The mid bell sits at {}.", what[k], hertz(hz[k]), hertz(mid)));
        }
    }
    touched
}

/// Where the bus goes, centred under the effects.
fn output_menu(ui: &mut Ui, cfg: &mut BusCfg, b: usize, width: f32) {
    ui.add_space(4.0);
    // Centred by hand: a drop-down does not follow a centred layout. Its arrow and padding
    // add about 10 points to the width it is given.
    let menu = (width - 34.0).clamp(50.0, 84.0);
    ui.horizontal(|ui| {
        ui.add_space(((width - menu - 10.0) * 0.5).max(0.0));
        egui::ComboBox::from_id_salt(("bus_out", b))
            .width(menu)
            .selected_text(out_name(cfg.out, b))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut cfg.out, 0, "to Master");
                // Only later buses, so the chain can never loop back on itself.
                for o in b + 2..=MASTER {
                    ui.selectable_value(&mut cfg.out, o as u8, format!("to {}", bus_label(o - 1)));
                }
            })
            .response
            .on_hover_text("Where this bus goes");
    });
}

/// The "SC" button and its popup: which bus keys this one, and what the key does to it.
fn sidechain_menu(ui: &mut Ui, cfg: &mut BusCfg, b: usize) {
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
            for src in 1..=MASTER as u8 {
                if src as usize - 1 != b {
                    ui.selectable_value(&mut cfg.sc_src, src, bus_label(src as usize - 1));
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
    if out as usize > b + 1 && (out as usize) <= MASTER { format!("to {}", bus_label(out as usize - 1)) } else { "to Master".into() }
}
