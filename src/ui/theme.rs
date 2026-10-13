//! Colours, fonts and the egui style. The look is a dark hardware panel: near-black
//! chassis, charcoal cards, amber for audio, cyan for MIDI.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, Rect, Stroke, StrokeKind, TextStyle, Ui, vec2};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(10, 11, 14);
pub const PANEL: Color32 = Color32::from_rgb(21, 23, 29);
pub const WELL: Color32 = Color32::from_rgb(14, 15, 19);
pub const RAISED: Color32 = Color32::from_rgb(36, 39, 48);
pub const HOVER: Color32 = Color32::from_rgb(48, 52, 64);
pub const LINE: Color32 = Color32::from_rgb(44, 48, 59);
pub const TEXT: Color32 = Color32::from_rgb(236, 238, 243);
pub const DIM: Color32 = Color32::from_rgb(134, 141, 158);
pub const FAINT: Color32 = Color32::from_rgb(78, 84, 99);
pub const AMBER: Color32 = Color32::from_rgb(255, 176, 46);
pub const AMBER_DIM: Color32 = Color32::from_rgb(140, 96, 26);
pub const GREEN: Color32 = Color32::from_rgb(64, 230, 118);
pub const CYAN: Color32 = Color32::from_rgb(64, 208, 228);
pub const CYAN_DIM: Color32 = Color32::from_rgb(28, 106, 122);
pub const RED: Color32 = Color32::from_rgb(255, 70, 92);
pub const PURPLE: Color32 = Color32::from_rgb(168, 118, 255);

/// Linear blend between two colours, `t` in 0..1.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// Semibold face for headings and labels; falls back to the regular face.
pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut load = |key: &str, file: &str| -> bool {
        let dir = std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
        match std::fs::read(dir.join("Fonts").join(file)) {
            Ok(bytes) => {
                fonts.font_data.insert(key.into(), Arc::new(egui::FontData::from_owned(bytes)));
                true
            }
            Err(_) => false,
        }
    };
    // The system UI face reads crisper at small sizes than the bundled one and covers
    // more symbols. Without it, everything falls back to egui's defaults.
    let regular = load("ui", "segoeui.ttf");
    let semibold = load("ui-bold", "seguisb.ttf");
    let mut base = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    if regular {
        base.insert(0, "ui".into());
        fonts.families.insert(FontFamily::Proportional, base.clone());
    }
    let mut heavy = base;
    if semibold {
        heavy.insert(0, "ui-bold".into());
    }
    fonts.families.insert(FontFamily::Name("bold".into()), heavy);
    ctx.set_fonts(fonts);
}

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);

    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = WELL;
    v.faint_bg_color = Color32::from_rgb(26, 28, 35);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1.0, LINE);
    v.slider_trailing_fill = true;

    let radius = CornerRadius::same(6);
    let w = &mut v.widgets;
    for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        s.corner_radius = radius;
        s.expansion = 0.0;
    }
    w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    // Text colour comes from the widget states rather than a global override, so that
    // selected items can switch to dark text on amber.
    for s in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        s.fg_stroke = Stroke::new(1.0, TEXT);
    }
    w.inactive.bg_fill = RAISED;
    w.inactive.weak_bg_fill = RAISED;
    w.inactive.bg_stroke = Stroke::NONE;
    w.hovered.bg_fill = HOVER;
    w.hovered.weak_bg_fill = HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, FAINT);
    w.active.bg_fill = HOVER;
    w.active.weak_bg_fill = HOVER;
    w.active.bg_stroke = Stroke::new(1.0, AMBER);
    w.open.bg_fill = HOVER;
    w.open.weak_bg_fill = HOVER;
    // Selected things are amber with dark text on top.
    v.selection.bg_fill = AMBER;
    v.selection.stroke = Stroke::new(1.0, BG);
    ctx.set_visuals(v);

    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = vec2(7.0, 6.0);
        s.spacing.button_padding = vec2(9.0, 4.0);
        s.spacing.interact_size.y = 24.0;
        s.spacing.slider_rail_height = 5.0;
        s.spacing.combo_height = 320.0;
        s.text_styles = [
            (TextStyle::Heading, bold(18.0)),
            (TextStyle::Body, FontId::proportional(13.5)),
            (TextStyle::Button, FontId::proportional(13.5)),
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Monospace, FontId::monospace(12.5)),
        ]
        .into();
    });
}

/// Draws a card filling exactly `rect` and lays `add` out inside it. Zones use this
/// rather than a growing frame so a card can never spill over its neighbours.
pub fn card_in(ui: &mut Ui, rect: Rect, accent: Option<Color32>, add: impl FnOnce(&mut Ui)) {
    ui.allocate_rect(rect, egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 10.0, PANEL);
    p.rect_stroke(rect, 10.0, Stroke::new(1.0, accent.unwrap_or(LINE)), StrokeKind::Inside);
    let inner = rect.shrink2(vec2(12.0, 10.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(rect.shrink(1.0).intersect(ui.clip_rect()));
    add(&mut child);
}

/// Margins for the panels the cards sit in.
pub fn gutter(left: i8, right: i8, top: i8, bottom: i8) -> egui::Frame {
    egui::Frame::NONE.fill(BG).inner_margin(Margin { left, right, top, bottom })
}

/// Small tracked caption used to title a group of controls.
pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).font(bold(10.5)).color(DIM).extra_letter_spacing(1.2));
}

/// Same, in an accent colour, for zone titles.
pub fn title(ui: &mut Ui, text: &str, color: Color32) {
    ui.label(egui::RichText::new(text).font(bold(11.5)).color(color).extra_letter_spacing(1.6));
}
