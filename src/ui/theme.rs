//! Colours, fonts and the egui style. The look is a piece of rack hardware: a near-black
//! chassis, brushed-steel panels, and buttons that stand proud of them like the keys of a
//! tape deck. Amber is audio, cyan is MIDI, magenta is vocal.
//!
//! The metal and the keys are not drawn by the widgets. Every zone, window and menu fills
//! its background with [`PANEL`], and every button with one of a few flat colours; once
//! the frame has been laid out, [`finish`] goes through what was painted and dresses those
//! flat rectangles up. That way egui's own buttons, menus and drop-downs get the same
//! treatment as ours without each call site having to ask for it.

use eframe::egui::epaint::{Mesh, RectShape, Vertex, WHITE_UV};
use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, Pos2, Rect, Shape, Stroke, StrokeKind, TextStyle, Ui, vec2};
use std::sync::{Arc, OnceLock};

pub const BG: Color32 = Color32::from_rgb(10, 11, 14);
pub const PANEL: Color32 = Color32::from_rgb(21, 23, 29);
pub const WELL: Color32 = Color32::from_rgb(14, 15, 19);
/// A key at rest, under the pointer, and held down.
pub const RAISED: Color32 = Color32::from_rgb(58, 61, 68);
pub const HOVER: Color32 = Color32::from_rgb(74, 78, 87);
const PRESSED: Color32 = Color32::from_rgb(40, 42, 48);
pub const LINE: Color32 = Color32::from_rgb(24, 25, 29);
pub const TEXT: Color32 = Color32::from_rgb(240, 241, 244);
pub const DIM: Color32 = Color32::from_rgb(166, 171, 182);
pub const FAINT: Color32 = Color32::from_rgb(108, 113, 124);
/// What the brushed texture is tinted with for panels, and for the chassis behind them.
const STEEL: Color32 = Color32::from_rgb(88, 91, 98);
const CHASSIS: Color32 = Color32::from_rgb(30, 31, 34);
/// Corner radius of every key. [`finish`] knows a key by it, so nothing else uses it.
const KEY_RADIUS: u8 = 6;
pub const AMBER: Color32 = Color32::from_rgb(255, 176, 46);
pub const AMBER_DIM: Color32 = Color32::from_rgb(140, 96, 26);
pub const GREEN: Color32 = Color32::from_rgb(64, 230, 118);
pub const CYAN: Color32 = Color32::from_rgb(64, 208, 228);
pub const CYAN_DIM: Color32 = Color32::from_rgb(28, 106, 122);
pub const RED: Color32 = Color32::from_rgb(255, 70, 92);
pub const PURPLE: Color32 = Color32::from_rgb(168, 118, 255);
/// Everything vocal: the vocal bus, vocal pads, takes.
pub const MAGENTA: Color32 = Color32::from_rgb(255, 92, 196);
pub const MAGENTA_DIM: Color32 = Color32::from_rgb(138, 44, 104);
/// The drum synth.
pub const LIME: Color32 = Color32::from_rgb(178, 228, 64);
pub const LIME_DIM: Color32 = Color32::from_rgb(92, 120, 34);

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

static METAL: OnceLock<egui::TextureHandle> = OnceLock::new();
/// Size of the brushed-metal tile, in texels and in points.
const METAL_SIZE: [usize; 2] = [1024, 256];

/// A tile of brushed metal: fine horizontal scratches, each a slightly different shade and
/// each wandering a little in brightness along its length. Grey only; the tint is applied
/// where it is drawn. It repeats without a seam in both directions.
fn brushed_metal() -> egui::ColorImage {
    let [w, h] = METAL_SIZE;
    let mut seed = 0x9e37_79b9u32;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
    };
    let mut rows = vec![0f32; w * h];
    let mut noise = vec![0f32; w];
    for y in 0..h {
        let shade = rand();
        noise.iter_mut().for_each(|n| *n = rand());
        // A moving average that wraps round the tile, run at two lengths: long drifts
        // along the scratch plus shorter flecks.
        for (span, weight) in [(220usize, 3.2f32), (14, 1.0)] {
            let mut sum: f32 = (0..span).map(|k| noise[k]).sum();
            for x in 0..w {
                rows[y * w + x] += sum / span as f32 * weight;
                sum += noise[(x + span) % w] - noise[x];
            }
        }
        for x in 0..w {
            rows[y * w + x] += shade * 0.24 + rand() * 0.03;
        }
    }
    let gray: Vec<u8> = (0..w * h)
        .map(|i| {
            // Scratches are not one texel sharp: let each bleed a little into the next.
            let (x, y) = (i % w, i / w);
            let v = rows[i] * 0.6 + rows[(y + 1) % h * w + x] * 0.25 + rows[(y + h - 1) % h * w + x] * 0.15;
            ((0.78 + v * 0.55).clamp(0.45, 1.0) * 255.0) as u8
        })
        .collect();
    egui::ColorImage::from_gray(METAL_SIZE, &gray)
}

static KNOB: OnceLock<egui::TextureHandle> = OnceLock::new();
const KNOB_PX: usize = 256;

/// A knob cap seen from above: a spun-metal face whose fine concentric grooves catch the
/// light in a cross, a bevelled chamfer lit from the top left, and a dark knurled rim.
/// The edge is anti-aliased into transparency, so the image can be drawn as a circle.
fn knob_cap() -> egui::ColorImage {
    let n = KNOB_PX;
    // Groove depth by radius: fine random rings, softened a little.
    let mut seed = 0x51ed_270bu32;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
    };
    let raw: Vec<f32> = (0..n).map(|_| rand()).collect();
    let rings: Vec<f32> = (0..n).map(|i| raw[i] * 0.6 + raw[(i + 1) % n] * 0.2 + raw[(i + n - 1) % n] * 0.2).collect();
    let light = -3.0 * std::f32::consts::FRAC_PI_4; // from the top left
    let mut px = Vec::with_capacity(n * n);
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = ((x as f32 + 0.5) / n as f32 * 2.0 - 1.0, (y as f32 + 0.5) / n as f32 * 2.0 - 1.0);
            let d = (fx * fx + fy * fy).sqrt();
            let a = fy.atan2(fx);
            let lit = (a - light).cos();
            let (face, chamfer) = (0.80, 0.90);
            let b = if d < face {
                let groove = rings[((d / face) * (n as f32 - 1.0)) as usize] * 0.10;
                // Spun metal reflects in a cross that stays put as the knob turns.
                let sheen = (2.0 * (a - std::f32::consts::FRAC_PI_4)).cos();
                0.46 + groove + sheen * 0.13 + lit * 0.05 + (1.0 - d / face) * 0.04
            } else if d < chamfer {
                // A bright line where the face turns down into the chamfer.
                let edge = (1.0 - ((d - face) / 0.012).abs()).max(0.0) * 0.25;
                0.50 + lit * 0.28 + edge
            } else {
                // Knurling: fine ridges round the edge, darker towards the outside.
                let ridge = if (a * 72.0).sin() > 0.0 { 0.07 } else { -0.05 };
                let out = (d - chamfer) / (1.0 - chamfer);
                0.30 + ridge + lit * 0.10 - out * 0.12
            };
            let v = (b.clamp(0.0, 1.0) * 255.0) as u8;
            let alpha = ((1.0 - d) * n as f32 * 0.5).clamp(0.0, 1.0);
            px.push(Color32::from_rgba_unmultiplied(v, v, (v as f32 * 0.97) as u8, (alpha * 255.0) as u8));
        }
    }
    egui::ColorImage::new([n, n], px)
}

/// Paints a knob cap of radius `r` centred on `c`, with its shadow.
pub fn paint_knob_cap(p: &egui::Painter, c: Pos2, r: f32, hot: bool) {
    p.circle_filled(c + vec2(0.0, 1.5), r + 1.0, Color32::from_black_alpha(150));
    match KNOB.get() {
        Some(tex) => {
            let tint = if hot { Color32::WHITE } else { Color32::from_gray(222) };
            p.image(tex.id(), Rect::from_center_size(c, vec2(r * 2.0, r * 2.0)), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), tint);
        }
        None => {
            p.circle_filled(c, r, if hot { HOVER } else { RAISED });
        }
    }
}

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    KNOB.get_or_init(|| {
        // Mipmapped: knobs are drawn far smaller than the image, and its grooves would
        // shimmer without it.
        let options = egui::TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..egui::TextureOptions::LINEAR };
        ctx.load_texture("knob-cap", knob_cap(), options)
    });
    METAL.get_or_init(|| {
        let options = egui::TextureOptions { wrap_mode: egui::TextureWrapMode::Repeat, ..egui::TextureOptions::LINEAR };
        ctx.load_texture("brushed-metal", brushed_metal(), options)
    });

    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = WELL;
    v.faint_bg_color = Color32::from_rgb(26, 28, 35);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1.0, Color32::BLACK);
    v.slider_trailing_fill = true;

    let radius = CornerRadius::same(KEY_RADIUS);
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
    w.active.bg_fill = PRESSED;
    w.active.weak_bg_fill = PRESSED;
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
    card_with_margin(ui, rect, accent, vec2(12.0, 10.0), add);
}

/// A card with its own inner margin, for narrow cards that need every point of width.
pub fn card_with_margin(ui: &mut Ui, rect: Rect, accent: Option<Color32>, margin: egui::Vec2, add: impl FnOnce(&mut Ui)) {
    ui.allocate_rect(rect, egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 10.0, PANEL);
    p.rect_stroke(rect, 10.0, Stroke::new(1.0, accent.unwrap_or(Color32::BLACK)), StrokeKind::Inside);
    let inner = rect.shrink2(margin);
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

fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgba_premultiplied(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()), f(a.a(), b.a()))
}

/// A rounded rectangle shaded from `top` to `bottom`.
fn shaded(rect: Rect, radius: f32, top: Color32, bottom: Color32) -> Shape {
    let r = radius.min(rect.width() * 0.5).min(rect.height() * 0.5);
    let mut mesh = Mesh::default();
    let mut point = |pos: Pos2| {
        let t = ((pos.y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 1.0);
        mesh.vertices.push(Vertex { pos, uv: WHITE_UV, color: blend(top, bottom, t) });
    };
    point(rect.center());
    let corners = [
        (Pos2::new(rect.right() - r, rect.top() + r), -90f32),
        (Pos2::new(rect.right() - r, rect.bottom() - r), 0.0),
        (Pos2::new(rect.left() + r, rect.bottom() - r), 90.0),
        (Pos2::new(rect.left() + r, rect.top() + r), 180.0),
    ];
    const STEPS: usize = 4;
    for (centre, from) in corners {
        for k in 0..=STEPS {
            let a = (from + 90.0 * k as f32 / STEPS as f32).to_radians();
            point(centre + vec2(a.cos(), a.sin()) * r);
        }
    }
    let rim = (STEPS as u32 + 1) * 4;
    for k in 0..rim {
        mesh.indices.extend([0, 1 + k, 1 + (k + 1) % rim]);
    }
    Shape::mesh(mesh)
}

/// The shapes of a key standing proud of the panel: the shadow it casts, a face that
/// catches the light at the top, and a dark rim. `down` is a key pushed in: no shadow to
/// speak of, and the light falls the other way.
fn key_shapes(rect: Rect, radius: f32, fill: Color32, down: bool) -> Vec<Shape> {
    let lift = if down { 0.6 } else { 2.0 };
    let (top, bottom) = if down {
        (blend(fill, Color32::BLACK, 0.30), blend(fill, Color32::WHITE, 0.04))
    } else {
        (blend(fill, Color32::WHITE, 0.20), blend(fill, Color32::BLACK, 0.30))
    };
    let mut shapes = vec![
        Shape::rect_filled(rect.translate(vec2(0.0, lift)).expand(0.5), radius + 0.5, Color32::from_black_alpha(150)),
        shaded(rect, radius, top, bottom),
        Shape::rect_stroke(rect, radius, Stroke::new(1.0, Color32::from_black_alpha(190)), StrokeKind::Inside),
    ];
    let inset = radius * 0.6 + 1.0;
    if rect.width() > inset * 2.0 + 2.0 {
        // The bevel: a bright edge where the top of the key turns over, and on a raised
        // key a darker lip along the bottom.
        let edge = |y: f32, colour: Color32| Shape::line_segment([Pos2::new(rect.left() + inset, y), Pos2::new(rect.right() - inset, y)], Stroke::new(1.0, colour));
        if down {
            shapes.push(edge(rect.top() + 1.5, Color32::from_black_alpha(110)));
        } else {
            shapes.push(edge(rect.top() + 1.5, Color32::from_white_alpha(62)));
            shapes.push(edge(rect.bottom() - 1.5, Color32::from_black_alpha(70)));
        }
    }
    shapes
}

/// Paints a key directly, for controls that draw themselves (pads, the transport button).
pub fn paint_key(p: &egui::Painter, rect: Rect, radius: f32, fill: Color32, down: bool) {
    p.extend(key_shapes(rect, radius, fill, down));
}

/// A panel of brushed steel in place of a flat one.
fn plate(rect: &RectShape, metal: egui::TextureId, tint: Color32) -> Shape {
    let (w, h) = (METAL_SIZE[0] as f32, METAL_SIZE[1] as f32);
    let r = rect.rect;
    let uv = Rect::from_min_max(Pos2::new(r.left() / w, r.top() / h), Pos2::new(r.right() / w, r.bottom() / h));
    let mut steel = rect.clone().with_texture(metal, uv);
    steel.fill = tint;
    let radius = rect.corner_radius.nw as f32;
    // Light falling from above: a little brighter at the top of the panel, darker below.
    let sheen = shaded(r.shrink(1.0), (radius - 1.0).max(0.0), Color32::from_white_alpha(16), Color32::from_black_alpha(58));
    let lip = Shape::line_segment(
        [Pos2::new(r.left() + radius, r.top() + 1.5), Pos2::new(r.right() - radius, r.top() + 1.5)],
        Stroke::new(1.0, Color32::from_white_alpha(30)),
    );
    Shape::Vec(vec![steel.into(), sheen, lip])
}

fn dress(shape: &mut Shape, metal: egui::TextureId) {
    match shape {
        Shape::Vec(shapes) => shapes.iter_mut().for_each(|s| dress(s, metal)),
        Shape::Rect(rect) if rect.brush.is_none() => {
            let size = rect.rect.size();
            let fill = rect.fill;
            if fill == PANEL {
                *shape = plate(rect, metal, STEEL);
            } else if fill == BG && size.x > 80.0 && size.y > 30.0 {
                *shape = plate(rect, metal, CHASSIS);
            } else if fill == RAISED && size.y <= 7.0 {
                // A slider's rail is a groove cut into the panel, not a raised strip.
                rect.fill = Color32::from_rgb(12, 12, 15);
            } else if rect.corner_radius == CornerRadius::same(KEY_RADIUS)
                && (10.0..=52.0).contains(&size.y)
                && fill.a() > 40
                && fill != WELL
                && fill != BG
            {
                // Lit keys (a toggle that is on, a selected tab) are latched down, like the
                // play key of a tape deck.
                let lit = fill.r().max(fill.g()).max(fill.b()) as i32 - fill.r().min(fill.g()).min(fill.b()) as i32 > 50;
                let mut shapes = key_shapes(rect.rect, KEY_RADIUS as f32, fill, lit || fill == PRESSED);
                if rect.stroke.width > 0.0 {
                    shapes.push(Shape::rect_stroke(rect.rect, rect.corner_radius, rect.stroke, StrokeKind::Inside));
                }
                *shape = Shape::Vec(shapes);
            }
        }
        _ => {}
    }
}

/// Turns the flat panels and buttons painted this frame into brushed steel and bevelled
/// keys. Call once, after everything has been laid out.
pub fn finish(ctx: &egui::Context) {
    let Some(metal) = METAL.get().map(|t| t.id()) else { return };
    let mut layers: Vec<egui::LayerId> = ctx.memory(|m| m.layer_ids().collect());
    layers.push(egui::LayerId::background());
    ctx.graphics_mut(|graphics| {
        for layer in layers {
            let Some(list) = graphics.get_mut(layer) else { continue };
            for i in 0..list.next_idx().0 {
                list.mutate_shape(egui::layers::ShapeIdx(i), |clipped| dress(&mut clipped.shape, metal));
            }
        }
    });
}
