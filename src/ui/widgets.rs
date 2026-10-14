//! Custom controls shared by the zones: knobs, meters, pills and the transport button.

use super::theme;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Response, Sense, Stroke, Ui, vec2};

/// Rotary control for 0..1 values: drag up/down (Shift for fine), double-click to reset.
pub fn knob(ui: &mut Ui, value: &mut f32, label: &str, default: f32, color: Color32) -> Response {
    knob_sized(ui, value, label, default, color, 13.0)
}

/// A knob with a cap of radius `r`, for rows that have to fit more than three controls.
/// It keeps the standard knob's height so the two can share a row.
pub fn knob_sized(ui: &mut Ui, value: &mut f32, label: &str, default: f32, color: Color32, r: f32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(r * 2.0 + 20.0, 56.0), Sense::click_and_drag());
    if resp.dragged() {
        let d = resp.drag_delta();
        let fine = if ui.input(|i| i.modifiers.shift) { 0.25 } else { 1.0 };
        *value = (*value + (d.x * 0.003 - d.y * 0.006) * fine).clamp(0.0, 1.0);
        resp.mark_changed();
    }
    if resp.double_clicked() {
        *value = default;
        resp.mark_changed();
    }
    let hot = resp.hovered() || resp.dragged();
    let p = ui.painter_at(rect);
    let c = Pos2::new(rect.center().x, rect.top() + 21.0);
    let ang = |t: f32| (135.0 + 270.0 * t).to_radians();
    let arc = |from: f32, to: f32, col: Color32, w: f32| {
        let steps = 28;
        let pts: Vec<Pos2> = (0..=steps)
            .map(|i| {
                let a = ang(from + (to - from) * i as f32 / steps as f32);
                c + vec2(a.cos(), a.sin()) * (r + 4.5)
            })
            .collect();
        p.line(pts, Stroke::new(w, col));
    };
    arc(0.0, 1.0, theme::WELL, 3.5);
    if *value > 0.004 {
        arc(0.0, *value, color, 3.5);
    }
    // Cap with a soft top-lit edge, and a pointer dot rather than a line.
    p.circle_filled(c, r, if hot { theme::HOVER } else { theme::RAISED });
    p.circle_stroke(c, r, Stroke::new(1.0, theme::mix(theme::RAISED, theme::TEXT, 0.14)));
    let a = ang(*value);
    p.circle_filled(c + vec2(a.cos(), a.sin()) * (r - 4.5), 2.2, if hot { theme::TEXT } else { color });
    p.text(
        Pos2::new(rect.center().x, rect.bottom()),
        Align2::CENTER_BOTTOM,
        label,
        FontId::proportional(10.5),
        if hot { theme::TEXT } else { theme::DIM },
    );
    resp.on_hover_text(format!("{label}: {:.0}%", *value * 100.0))
}

/// Short vertical slider for 0..1 values, the same footprint height as a knob. Drag up and
/// down (Shift for fine), double-click to reset. The caller supplies the hover text, since
/// only it knows what the value means.
pub fn fader(ui: &mut Ui, value: &mut f32, label: &str, default: f32, color: Color32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(16.0, 56.0), Sense::click_and_drag());
    if resp.dragged() {
        let fine = if ui.input(|i| i.modifiers.shift) { 0.25 } else { 1.0 };
        *value = (*value - resp.drag_delta().y / 34.0 * fine).clamp(0.0, 1.0);
        resp.mark_changed();
    }
    if resp.double_clicked() {
        *value = default;
        resp.mark_changed();
    }
    let hot = resp.hovered() || resp.dragged();
    let p = ui.painter_at(rect);
    let track = Rect::from_min_max(Pos2::new(rect.center().x - 2.0, rect.top() + 4.0), Pos2::new(rect.center().x + 2.0, rect.top() + 40.0));
    p.rect_filled(track, 2.0, theme::WELL);
    let y = track.bottom() - *value * track.height();
    p.rect_filled(Rect::from_min_max(Pos2::new(track.left(), y), track.max), 2.0, color);
    let cap = Rect::from_center_size(Pos2::new(rect.center().x, y), vec2(12.0, 6.0));
    p.rect_filled(cap, 2.0, if hot { theme::TEXT } else { theme::mix(theme::RAISED, theme::TEXT, 0.55) });
    p.text(
        Pos2::new(rect.center().x, rect.bottom()),
        Align2::CENTER_BOTTOM,
        label,
        FontId::proportional(10.5),
        if hot { theme::TEXT } else { theme::DIM },
    );
    resp
}

/// A pan position as mixers write it: C, or L/R with a percentage.
pub fn pan_text(pan: f32) -> String {
    let pct = (pan.abs() * 100.0).round();
    if pct < 0.5 {
        "C".into()
    } else {
        format!("{}{pct:.0}", if pan < 0.0 { 'L' } else { 'R' })
    }
}

/// Compact pan control for -1..1: drag sideways (Shift for fine), double-click to centre.
/// A marker under the readout shows where it sits.
pub fn pan_box(ui: &mut Ui, pan: &mut f32, color: Color32, width: f32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, ui.spacing().interact_size.y), Sense::click_and_drag());
    if resp.dragged() {
        let fine = if ui.input(|i| i.modifiers.shift) { 0.25 } else { 1.0 };
        *pan = (*pan + resp.drag_delta().x * 0.01 * fine).clamp(-1.0, 1.0);
        // Easy to land back on the middle without hunting for it.
        if pan.abs() < 0.015 {
            *pan = 0.0;
        }
        resp.mark_changed();
    }
    if resp.double_clicked() {
        *pan = 0.0;
        resp.mark_changed();
    }
    let hot = resp.hovered() || resp.dragged();
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, if hot { theme::HOVER } else { theme::WELL });
    let track = rect.shrink2(vec2(5.0, 0.0));
    let y = rect.bottom() - 3.5;
    p.hline(track.x_range(), y, Stroke::new(1.0, theme::LINE));
    p.vline(track.center().x, y - 1.5..=y + 1.5, Stroke::new(1.0, theme::FAINT));
    let x = track.center().x + *pan * track.width() * 0.5;
    let lit = if pan.abs() > 0.004 { color } else { theme::DIM };
    p.line_segment([Pos2::new(track.center().x, y), Pos2::new(x, y)], Stroke::new(2.0, lit));
    p.circle_filled(Pos2::new(x, y), 2.2, if hot { theme::TEXT } else { lit });
    p.text(
        Pos2::new(rect.center().x, rect.top() + 1.5),
        Align2::CENTER_TOP,
        pan_text(*pan),
        FontId::proportional(10.5),
        if hot { theme::TEXT } else { theme::DIM },
    );
    resp.on_hover_text(format!("Pan: {}. Drag sideways, double-click to centre.", pan_text(*pan)))
}

pub fn db(v: f32) -> f32 {
    20.0 * v.max(1e-6).log10()
}

/// Vertical segmented peak meter, -60..+6 dB.
pub fn meter(p: &egui::Painter, rect: Rect, peak: f32) {
    p.rect_filled(rect, 2.0, theme::WELL);
    let lit = ((db(peak) + 60.0) / 66.0).clamp(0.0, 1.0);
    let seg = 3.0;
    let count = (rect.height() / seg) as usize;
    for i in 0..count {
        let t = (i as f32 + 0.5) / count as f32;
        if t > lit {
            break;
        }
        // 0 dBFS sits at 60/66 of the way up.
        let col = if t > 60.0 / 66.0 {
            theme::RED
        } else if t > 48.0 / 66.0 {
            theme::AMBER
        } else {
            theme::GREEN
        };
        let y = rect.bottom() - (i as f32 + 1.0) * seg;
        p.rect_filled(Rect::from_min_size(Pos2::new(rect.left(), y + 1.0), vec2(rect.width(), seg - 1.0)), 0.0, col);
    }
}

/// Button that is lit in `color` while `on`.
pub fn toggle(ui: &mut Ui, on: bool, text: &str, color: Color32) -> Response {
    let label = egui::RichText::new(text).color(if on { theme::BG } else { theme::TEXT });
    let mut b = egui::Button::new(label);
    if on {
        b = b.fill(color);
    }
    ui.add(b)
}

/// One segment of a tab row or filter: a key that stays down, lit, while it is selected.
pub fn pill(ui: &mut Ui, selected: bool, text: &str) -> Response {
    let font = if selected { theme::bold(13.0) } else { FontId::proportional(13.0) };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, theme::TEXT);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + vec2(20.0, 9.0), Sense::click());
    let p = ui.painter();
    let (fill, fg) = if selected {
        (theme::AMBER, theme::BG)
    } else if resp.hovered() {
        (theme::HOVER, theme::TEXT)
    } else {
        (theme::RAISED, theme::DIM)
    };
    // Drawn flat at the key radius; the theme's finishing pass gives it its bevel.
    p.rect_filled(rect, 6.0, fill);
    p.galley_with_override_text_color(rect.center() - galley.size() * 0.5, galley, fg);
    resp
}

/// Square play/stop button with a drawn icon.
pub fn transport_button(ui: &mut Ui, playing: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(46.0, 46.0), Sense::click());
    let p = ui.painter();
    let fill = if playing {
        theme::GREEN
    } else if resp.hovered() {
        theme::HOVER
    } else {
        theme::RAISED
    };
    theme::paint_key(p, rect, 8.0, fill, playing);
    let c = rect.center();
    if playing {
        p.rect_filled(Rect::from_center_size(c, vec2(15.0, 15.0)), 2.0, theme::BG);
    } else {
        let tri = vec![c + vec2(-6.0, -9.0), c + vec2(9.0, 0.0), c + vec2(-6.0, 9.0)];
        p.add(egui::Shape::convex_polygon(tri, theme::GREEN, Stroke::NONE));
    }
    resp.on_hover_text("Space")
}

/// Small rounded tag, used on pads and in lists.
pub fn tag(p: &egui::Painter, pos: Pos2, text: &str, fg: Color32, bg: Color32) -> f32 {
    let galley = p.layout_no_wrap(text.to_owned(), theme::bold(9.5), fg);
    let r = Rect::from_min_size(pos, galley.size() + vec2(8.0, 3.0));
    p.rect_filled(r, 3.0, bg);
    p.galley(r.min + vec2(4.0, 1.5), galley, fg);
    r.right()
}

/// Humanize amounts as two percentages. Returns true if either changed.
pub fn humanize(ui: &mut Ui, time: &mut f32, vel: &mut f32) -> bool {
    theme::caption(ui, "HUMANIZE");
    let mut changed = false;
    for (value, prefix, hint) in [
        (time, "Time ", "How far notes may drift off the grid: up to about 1/16 of a beat either way at 100%"),
        (vel, "Vel ", "How much each note's velocity varies"),
    ] {
        let mut pct = *value * 100.0;
        let r = ui
            .add(egui::DragValue::new(&mut pct).speed(0.5).range(0.0..=100.0).fixed_decimals(0).prefix(prefix).suffix("%"))
            .on_hover_text(format!("{hint}. Applied at playback, differently each pass; the pattern is not changed."));
        if r.changed() {
            *value = pct / 100.0;
            changed = true;
        }
    }
    changed
}
