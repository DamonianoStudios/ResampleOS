//! The arrangement view: a song timeline with a track per pad, a loop lane above it
//! showing which loop each stretch came from, a ruler, and the loop brace.

use super::{theme, toggle};
use crate::app::{ARR_SNAPS, App, ArrDrag, Clip, View};
use crate::engine::{PadKind, Pattern};
use crate::music::{PPQ, TICKS_PER_BAR, pad_label};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, vec2};

const HEAD_W: f32 = 196.0;
const BRACE_H: f32 = 12.0;
/// The ruler: markers along its top, bar numbers below them.
const RULER_H: f32 = 30.0;
const LANE_H: f32 = 26.0;
const TRACK_H: f32 = 36.0;
/// How near a clip's edge the pointer must be to trim rather than move, in points.
const EDGE: f32 = 6.0;
const BAR: u64 = TICKS_PER_BAR as u64;

/// The palette loops (and sections given a colour of their own) are drawn in.
const COLOURS: [Color32; 8] = [theme::AMBER, theme::CYAN, theme::LIME, theme::MAGENTA, theme::PURPLE, theme::GREEN, theme::RED, Color32::from_rgb(120, 160, 255)];

/// A colour for each loop, so clips from one loop read as one.
fn loop_colour(scene: usize) -> Color32 {
    COLOURS[scene % COLOURS.len()]
}

/// Clips with notes of their own (recorded or captured onto the timeline).
const NOTES_COLOUR: Color32 = Color32::from_rgb(214, 186, 120);

/// Where ticks fall on screen this frame.
#[derive(Clone, Copy)]
struct View2 {
    grid: Rect,
    zoom: f32,
    scroll: f64,
}

impl View2 {
    fn x_of(&self, tick: f64) -> f32 {
        self.grid.left() + ((tick - self.scroll) / BAR as f64) as f32 * self.zoom
    }

    fn tick_at(&self, x: f32) -> f64 {
        (self.scroll + ((x - self.grid.left()) / self.zoom) as f64 * BAR as f64).max(0.0)
    }
}

/// A pattern of loop `scene` for `pad`, without copying it.
fn pattern_ref(app: &App, scene: usize, pad: usize) -> Option<&Pattern> {
    if scene == app.scene {
        app.pads.get(pad).map(|p| &p.pattern)
    } else {
        app.scenes.get(scene)?.patterns.iter().find(|(p, _)| *p == pad).map(|(_, pat)| pat)
    }
}

/// The pads that get a track: those with clips, and those that play in any loop (so a
/// clip can be added to them).
fn track_pads(app: &App) -> Vec<usize> {
    let mut pads: Vec<usize> = app.arrangement.tracks.iter().map(|(p, _)| *p).chain(app.song_rec.armed.iter().copied()).collect();
    for (k, s) in app.scenes.iter().enumerate() {
        if k == app.scene {
            pads.extend((0..app.pads.len()).filter(|&i| !app.pads[i].pattern.notes.is_empty()));
        } else {
            pads.extend(s.patterns.iter().filter(|(_, p)| !p.notes.is_empty()).map(|(i, _)| *i));
        }
    }
    pads.sort_unstable();
    pads.dedup();
    pads
}

pub fn draw(app: &mut App, ui: &mut Ui) {
    toolbar(app, ui);
    ui.add_space(4.0);
    let avail = ui.available_size();
    let lanes_h = if app.arr.show_auto { (avail.y * 0.45).clamp(150.0, 400.0) } else { 0.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(avail.x, avail.y - lanes_h), Sense::hover());
    timeline(app, ui, rect);
    if app.arr.show_auto {
        ui.add_space(4.0);
        // The song's lanes, lined up with the timeline above.
        let view = (app.arr.scroll, BAR as f64 / app.arr.zoom as f64);
        super::automation::song_lanes(app, ui, rect.left() + HEAD_W, view);
    }
}

fn toolbar(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        theme::title(ui, "ARRANGEMENT", theme::GREEN);
        if ui
            .button("Chain to song")
            .on_hover_text("Writes the chain (or, with no chain, the loop on screen) after the end of the song: a section per loop, a clip per pad")
            .clicked()
        {
            app.chain_to_arrangement();
        }
        ui.menu_button("Put loop", |ui| {
            ui.label(egui::RichText::new("At the insert marker").small().color(theme::DIM));
            for k in 0..app.scenes.len() {
                if ui.button(app.scene_label(k)).clicked() {
                    app.insert_loop(k);
                    ui.close();
                }
            }
        });
        let rec = egui::RichText::new("● Rec").color(if app.song_rec.on { theme::BG } else { theme::RED });
        let rec_button = egui::Button::new(rec).fill(if app.song_rec.on { theme::RED } else { theme::WELL });
        if ui
            .add(rec_button)
            .on_hover_text("Record while playing (F9): armed tracks (R on their headers) record the input or the notes played on them, and loops launched while the song is overridden become sections. Laid down when you stop or turn Rec off.")
            .clicked()
        {
            app.toggle_song_rec();
        }
        ui.menu_button("Capture", |ui| {
            ui.label(egui::RichText::new("What was just played, put on the timeline where it was played").small().color(theme::DIM));
            for bars in [4, 8, 16, 32, 64] {
                if ui.button(format!("Last {bars} bars")).clicked() {
                    app.capture_performance(bars);
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text("Loops launched, pads played and controls moved are remembered, whether or not Rec was on");
        if ui
            .button("Import audio…")
            .on_hover_text("Puts audio files on the timeline at the insert marker, each on a new track. Files can also be dropped straight onto a track.")
            .clicked()
        {
            app.import_audio_dialog(None);
        }
        ui.separator();
        if toggle(ui, app.arr.snap_on, "Snap", theme::CYAN)
            .on_hover_text("Clips, markers and the insert marker land on the grid chosen beside it. Hold Alt to place anywhere, snapping or not.")
            .clicked()
        {
            app.arr.snap_on ^= true;
        }
        egui::ComboBox::from_id_salt("arr_snap").width(52.0).selected_text(ARR_SNAPS[app.arr.snap].0).show_ui(ui, |ui| {
            for (i, (name, _)) in ARR_SNAPS.iter().enumerate() {
                ui.selectable_value(&mut app.arr.snap, i, *name);
            }
        });
        let brace = app.arrangement.brace.is_some();
        if toggle(ui, app.arrangement.brace_on && brace, "Loop", theme::AMBER)
            .on_hover_text("Play round the loop brace. Drag in the strip above the ruler to set it.")
            .clicked()
        {
            if brace {
                app.arrangement.brace_on ^= true;
            } else {
                // A first brace: the four bars from the insert marker.
                let s = app.arr.cursor / BAR * BAR;
                app.arrangement.brace = Some((s, s + 4 * BAR));
                app.arrangement.brace_on = true;
            }
        }
        if toggle(ui, app.arr.follow, "Follow", theme::GREEN).on_hover_text("Keep the playhead in view while playing").clicked() {
            app.arr.follow ^= true;
        }
        let n = app.arrangement.automation.len();
        let label = if n > 0 { format!("Automation {n}") } else { "Automation".to_string() };
        if toggle(ui, app.arr.show_auto, &label, theme::AMBER)
            .on_hover_text("Song-length automation lanes under the timeline, over the lanes each loop brings")
            .clicked()
        {
            app.arr.show_auto ^= true;
        }
        if ui.small_button("−").on_hover_text("Zoom out (Ctrl + scroll)").clicked() {
            app.arr.zoom = (app.arr.zoom / 1.4).max(12.0);
        }
        if ui.small_button("+").on_hover_text("Zoom in (Ctrl + scroll)").clicked() {
            app.arr.zoom = (app.arr.zoom * 1.4).min(1200.0);
        }
        ui.separator();
        let label = if app.exporting { "Exporting…" } else { "Export song…" };
        if ui
            .add_enabled(!app.exporting, egui::Button::new(label))
            .on_hover_text("Renders the song (or the loop brace, when it is on) to a WAV file, with the tails of the last notes and effects")
            .clicked()
        {
            app.export_song_dialog();
        }
        ui.menu_button("Stems", |ui| {
            ui.label(egui::RichText::new("One WAV per part, before the master's effects").small().color(theme::DIM));
            if ui.button("By track…").on_hover_text("Each pad that plays in the song, through its bus").clicked() {
                app.export_stems_dialog(false);
                ui.close();
            }
            if ui.button("By bus…").on_hover_text("Each bus, with the pads routed to it").clicked() {
                app.export_stems_dialog(true);
                ui.close();
            }
        });
        egui::ComboBox::from_id_salt("export_depth").width(84.0).selected_text(app.export_depth.name()).show_ui(ui, |ui| {
            for d in crate::app::Depth::ALL {
                ui.selectable_value(&mut app.export_depth, d, d.name());
            }
        });
        if app.arr.overridden && app.playing() {
            let back = egui::Button::new(egui::RichText::new("Back to arrangement").color(theme::BG)).fill(theme::AMBER);
            if ui.add(back).on_hover_text("A launched loop is playing; carry on with the song where it has got to").clicked() {
                app.back_to_arrangement();
            }
        }
        ui.label(
            egui::RichText::new("double-click: add a clip / open its loop · drag: move · edges: trim · drag empty space: box select · Ctrl+A all · Ctrl+D duplicate · Ctrl+E split · Del")
                .small()
                .color(theme::DIM),
        );
    });
}

fn timeline(app: &mut App, ui: &mut Ui, rect: Rect) {
    let p = ui.painter_at(rect);
    let grid = Rect::from_min_max(Pos2::new(rect.left() + HEAD_W, rect.top()), rect.max);
    let brace_r = Rect::from_min_size(grid.min, vec2(grid.width(), BRACE_H));
    let ruler = Rect::from_min_size(Pos2::new(grid.left(), brace_r.bottom()), vec2(grid.width(), RULER_H));
    let lane = Rect::from_min_size(Pos2::new(grid.left(), ruler.bottom()), vec2(grid.width(), LANE_H));
    let tracks_r = Rect::from_min_max(Pos2::new(rect.left(), lane.bottom() + 2.0), rect.max);
    let pads = track_pads(app);

    // ---- Scrolling and zoom.
    let hover = ui.input(|i| i.pointer.hover_pos()).filter(|p| rect.contains(*p));
    if let Some(pos) = hover {
        let (scroll, zoom, shift) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers.shift));
        let v = View2 { grid, zoom: app.arr.zoom, scroll: app.arr.scroll };
        if (zoom - 1.0).abs() > 1e-3 {
            // Zoom around the pointer.
            let at = v.tick_at(pos.x);
            app.arr.zoom = (app.arr.zoom * zoom).clamp(12.0, 1200.0);
            app.arr.scroll = (at - ((pos.x - grid.left()) / app.arr.zoom) as f64 * BAR as f64).max(0.0);
        } else if shift || scroll.x.abs() > scroll.y.abs() {
            let d = if scroll.x != 0.0 { scroll.x } else { scroll.y };
            app.arr.scroll = (app.arr.scroll - (d / app.arr.zoom) as f64 * BAR as f64).max(0.0);
        } else if scroll.y != 0.0 {
            let max = (pads.len() as f32 * TRACK_H - tracks_r.height() + 20.0).max(0.0);
            app.arr.vscroll = (app.arr.vscroll - scroll.y).clamp(0.0, max);
        }
    }
    let playing_song = app.playing() && app.song_mode;
    let head = app.song_pos();
    if playing_song && app.arr.follow {
        let v = View2 { grid, zoom: app.arr.zoom, scroll: app.arr.scroll };
        let x = v.x_of(head);
        if x > grid.right() - 40.0 || x < grid.left() {
            app.arr.scroll = (head - BAR as f64 * 0.5).max(0.0);
        }
    }
    let v = View2 { grid, zoom: app.arr.zoom, scroll: app.arr.scroll };
    // Off the grid with snapping off, or while Alt is held.
    let free = !app.arr.snap_on || ui.input(|i| i.modifiers.alt);
    let snap = if free { 1 } else { ARR_SNAPS[app.arr.snap].1 };
    let snapped = |t: f64| ((t / snap as f64).round() as u64) * snap;
    let floor_snap = |t: f64| ((t / snap as f64).floor() as u64) * snap;

    // ---- Background, bar lines and the ruler.
    p.rect_filled(grid, 3.0, theme::WELL);
    let first_bar = (v.scroll / BAR as f64).floor() as u64;
    let last_bar = v.tick_at(grid.right()) as u64 / BAR + 1;
    let every = if v.zoom < 24.0 { 8 } else if v.zoom < 50.0 { 4 } else if v.zoom < 90.0 { 2 } else { 1 };
    for bar in first_bar..=last_bar {
        let x = v.x_of((bar * BAR) as f64);
        if x < grid.left() - 1.0 {
            continue;
        }
        let major = bar.is_multiple_of(every);
        p.vline(x, ruler.top()..=grid.bottom(), Stroke::new(1.0, if bar % 4 == 0 { theme::FAINT.gamma_multiply(0.6) } else { Color32::from_rgb(34, 37, 42) }));
        if major {
            p.text(Pos2::new(x + 3.0, ruler.bottom() - 9.0), Align2::LEFT_CENTER, (bar + 1).to_string(), FontId::proportional(10.0), theme::DIM);
        }
        if v.zoom >= 90.0 {
            for b in 1..4 {
                let bx = v.x_of((bar * BAR + b * PPQ as u64) as f64);
                p.vline(bx, ruler.bottom() - 4.0..=ruler.bottom(), Stroke::new(1.0, theme::FAINT));
            }
        }
    }
    p.hline(grid.x_range(), ruler.bottom(), Stroke::new(1.0, theme::FAINT));

    // ---- The loop brace.
    if let Some((s, e)) = app.arrangement.brace {
        let r = Rect::from_min_max(Pos2::new(v.x_of(s as f64), brace_r.top() + 2.0), Pos2::new(v.x_of(e as f64), brace_r.bottom() - 1.0)).intersect(brace_r);
        let colour = if app.arrangement.brace_on { theme::AMBER } else { theme::FAINT };
        p.rect_filled(r, 2.0, colour.gamma_multiply(0.8));
    }
    let br = ui.interact(brace_r, egui::Id::new("arr_brace"), Sense::click_and_drag());
    if br.drag_started()
        && let Some(o) = ui.input(|i| i.pointer.press_origin())
    {
        app.arr.drag = Some(ArrDrag::Brace { from: snapped(v.tick_at(o.x)) });
    }
    if let (Some(ArrDrag::Brace { from }), Some(pos)) = (app.arr.drag, br.interact_pointer_pos()) {
        let to = snapped(v.tick_at(pos.x));
        if to != from {
            app.arrangement.brace = Some((from.min(to), from.max(to)));
            app.arrangement.brace_on = true;
        }
    }
    if br.clicked() {
        app.arrangement.brace_on ^= true;
    }

    // ---- Ruler: click to put the insert marker (and the playhead) there; right-click for
    // markers and for inserting and deleting time.
    let rr = ui.interact(ruler, egui::Id::new("arr_ruler"), Sense::click_and_drag());
    if (rr.clicked() || rr.dragged())
        && let Some(pos) = rr.interact_pointer_pos()
    {
        app.locate(snapped(v.tick_at(pos.x)));
    }
    if rr.secondary_clicked()
        && let Some(pos) = rr.interact_pointer_pos()
    {
        app.arr.ruler_at = snapped(v.tick_at(pos.x));
    }
    rr.context_menu(|ui| ruler_menu(app, ui));

    // ---- Markers, along the top of the ruler: click to go there, drag to move,
    // right-click to rename or delete.
    for i in 0..app.arrangement.markers.len() {
        let (tick, name) = (app.arrangement.markers[i].tick, app.arrangement.markers[i].name.clone());
        let x = v.x_of(tick as f64);
        if x < grid.left() - 1.0 || x > grid.right() {
            continue;
        }
        let galley = p.layout_no_wrap(name.clone(), FontId::proportional(9.5), theme::CYAN);
        let r = Rect::from_min_size(Pos2::new(x, ruler.top() + 1.0), vec2(galley.size().x + 8.0, 13.0)).intersect(ruler);
        p.rect_filled(r, 2.0, theme::CYAN.gamma_multiply(0.18));
        p.vline(x, r.y_range(), Stroke::new(1.5, theme::CYAN));
        p.galley(Pos2::new(x + 4.0, r.top() + 0.5), galley, theme::CYAN);
        let resp = ui.interact(r, egui::Id::new(("arr_marker", i)), Sense::click_and_drag());
        if resp.clicked() {
            app.locate(tick);
        }
        if resp.drag_started() {
            app.arr.drag = Some(ArrDrag::Marker { index: i });
        }
        if let (Some(ArrDrag::Marker { index }), Some(pos)) = (app.arr.drag, resp.interact_pointer_pos())
            && index == i
        {
            app.arrangement.markers[i].tick = snapped(v.tick_at(pos.x));
        }
        let mut delete = false;
        egui::Popup::context_menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.label(egui::RichText::new(format!("Marker at {}", crate::app::pos_text(tick))).small().color(theme::DIM));
            ui.text_edit_singleline(&mut app.arrangement.markers[i].name);
            if ui.button("Delete marker").clicked() {
                delete = true;
                ui.close();
            }
        });
        if delete {
            app.arrangement.markers.remove(i);
            break;
        }
    }

    // ---- The empty timeline under the clips and sections: click for the marker,
    // double-click a track to add a clip, drag a box to select.
    let vscroll = app.arr.vscroll;
    let row_at = |y: f32| ((y - tracks_r.top() + vscroll) / TRACK_H).floor();
    let bg_r = Rect::from_min_max(lane.min, grid.max);
    let bg = ui.interact(bg_r, egui::Id::new("arr_bg"), Sense::click_and_drag());
    let shift = ui.input(|i| i.modifiers.shift || i.modifiers.command);
    if bg.clicked()
        && let Some(pos) = bg.interact_pointer_pos()
    {
        if !shift {
            app.arr.deselect();
        }
        app.arr.cursor = snapped(v.tick_at(pos.x));
        if !app.playing() {
            app.locate(app.arr.cursor);
        }
    }
    if bg.double_clicked()
        && let Some(pos) = bg.interact_pointer_pos()
        && tracks_r.contains(pos)
        && let Some(&pad) = usize::try_from(row_at(pos.y) as i64).ok().and_then(|r| pads.get(r))
    {
        // The loop on screen if this pad plays in it, otherwise the first that it does.
        let scene = std::iter::once(app.scene)
            .chain(0..app.scenes.len())
            .find(|&k| pattern_ref(app, k, pad).is_some_and(|p| !p.notes.is_empty()));
        if let Some(scene) = scene {
            app.add_clip(pad, scene, floor_snap(v.tick_at(pos.x)));
        }
    }
    if bg.drag_started()
        && let Some(o) = ui.input(|i| i.pointer.press_origin())
    {
        app.arr.box_base = if shift { (app.arr.selected.clone(), app.arr.sections.clone()) } else { Default::default() };
        app.arr.drag = Some(ArrDrag::Box { from: o });
    }
    // Where a file dropped now would land (a track, or a new one), and where the menu of
    // an empty stretch was opened.
    let target = |pos: Pos2| {
        let pad = if tracks_r.contains(pos) { usize::try_from(row_at(pos.y) as i64).ok().and_then(|r| pads.get(r)).copied() } else { None };
        (pad, floor_snap(v.tick_at(pos.x)))
    };
    app.arr.drop_at = ui.input(|i| i.pointer.hover_pos()).filter(|p| bg_r.contains(*p)).map(target);
    if bg.secondary_clicked()
        && let Some(pos) = bg.interact_pointer_pos()
    {
        app.arr.menu_at = target(pos);
    }
    bg.context_menu(|ui| {
        let (pad, at) = app.arr.menu_at;
        let bar = at / BAR + 1;
        match pad {
            Some(pad) => {
                ui.label(egui::RichText::new(format!("{} · bar {bar}", pad_label(pad))).small().color(theme::DIM));
                let has_sample = app.pads[pad].sample().is_some();
                if ui.add_enabled(has_sample, egui::Button::new("Put the pad's sample here")).on_hover_text("An audio clip of the pad's sample, as long as the sample").clicked() {
                    app.put_pad_sample(pad, at);
                    ui.close();
                }
                if ui.button("Import audio here…").clicked() {
                    app.arr.cursor = at;
                    app.import_audio_dialog(Some(pad));
                    ui.close();
                }
            }
            None => {
                ui.label(egui::RichText::new(format!("Bar {bar}")).small().color(theme::DIM));
                if ui.button("Import audio on a new track…").clicked() {
                    app.arr.cursor = at;
                    app.import_audio_dialog(None);
                    ui.close();
                }
                if ui.button("New recording track").on_hover_text("An empty track, armed to record the input with Rec").clicked() {
                    app.new_recording_track();
                    ui.close();
                }
            }
        }
    });

    // ---- Loop lane: sections.
    p.rect_filled(lane, 2.0, Color32::from_rgb(18, 20, 24));
    p.text(Pos2::new(rect.left() + 8.0, lane.center().y), Align2::LEFT_CENTER, "LOOPS", theme::bold(10.0), theme::FAINT);
    for (i, s) in app.arrangement.sections.clone().iter().enumerate() {
        let r = Rect::from_min_max(Pos2::new(v.x_of(s.start as f64), lane.top() + 2.0), Pos2::new(v.x_of(s.end() as f64), lane.bottom() - 2.0));
        if r.right() < grid.left() || r.left() > grid.right() {
            continue;
        }
        let colour = s.colour.map_or(loop_colour(s.scene), |c| COLOURS[c as usize % COLOURS.len()]);
        let selected = app.arr.sections.contains(&i);
        let painter = p.with_clip_rect(r.intersect(lane));
        painter.rect_filled(r, 3.0, colour.gamma_multiply(if selected { 0.9 } else { 0.55 }));
        if selected {
            painter.rect_stroke(r, 3.0, Stroke::new(1.5, theme::TEXT), StrokeKind::Inside);
        }
        painter.text(r.left_center() + vec2(5.0, 0.0), Align2::LEFT_CENTER, app.section_name(s), theme::bold(10.5), theme::BG);
        let resp = ui.interact(r.intersect(lane), egui::Id::new(("arr_section", i)), Sense::click_and_drag());
        if resp.clicked() {
            if shift {
                match app.arr.sections.iter().position(|k| *k == i) {
                    Some(at) => drop(app.arr.sections.remove(at)),
                    None => app.arr.sections.push(i),
                }
            } else {
                app.arr.pick_section(i);
            }
        }
        if resp.double_clicked() {
            app.show_scene(s.scene, true);
            app.view = View::Pads;
        }
        if resp.drag_started()
            && let Some(o) = ui.input(|i| i.pointer.press_origin())
        {
            app.arr.pick_section(i);
            app.arr.drag = Some(ArrDrag::Section { index: i, grab: v.tick_at(o.x) as u64 });
        }
        if resp.secondary_clicked() && !selected {
            app.arr.pick_section(i);
        }
        // Stays open while typing a name (a click inside would close a plain menu).
        egui::Popup::context_menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            let mut name = s.label.and_then(|l| app.arrangement.labels.get(l)).cloned().unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label("Name");
                if ui.add(egui::TextEdit::singleline(&mut name).hint_text(app.scene_label(s.scene)).desired_width(140.0)).changed() {
                    app.set_section_name(i, &name);
                }
            });
            ui.horizontal(|ui| {
                ui.label("Colour");
                for (k, c) in COLOURS.iter().enumerate() {
                    let (rect, swatch) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
                    ui.painter().rect_filled(rect, 3.0, *c);
                    if s.colour == Some(k as u8) {
                        ui.painter().rect_stroke(rect, 3.0, Stroke::new(2.0, theme::TEXT), StrokeKind::Inside);
                    }
                    if swatch.clicked() {
                        app.set_section_colour(i, Some(k as u8));
                    }
                }
                if ui.small_button("Loop's").on_hover_text("The loop's own colour").clicked() {
                    app.set_section_colour(i, None);
                }
            });
            ui.separator();
            if ui.button("Open the loop").clicked() {
                app.show_scene(s.scene, true);
                app.view = View::Pads;
                ui.close();
            }
            if ui.button("Duplicate    Ctrl+D").clicked() {
                app.duplicate_selection();
                ui.close();
            }
            if ui.button("Delete with its clips    Del").clicked() {
                app.delete_selected_clips();
                ui.close();
            }
        });
        resp.on_hover_text(format!("{} ({}): bars {} to {}. Double-click to open the loop; right-click to name or colour it.", app.section_name(s), app.scene_label(s.scene), s.start / BAR + 1, s.end() / BAR));
    }

    // ---- Tracks.
    let painter = p.with_clip_rect(tracks_r);
    let mut clip_hit: Option<(usize, usize, bool, bool)> = None; // pad, index, near start, near end
    let pointer = ui.input(|i| i.pointer.hover_pos());
    for (row, &pad) in pads.iter().enumerate() {
        let top = tracks_r.top() + row as f32 * TRACK_H - app.arr.vscroll;
        if top + TRACK_H < tracks_r.top() || top > tracks_r.bottom() {
            continue;
        }
        let row_r = Rect::from_min_size(Pos2::new(rect.left(), top), vec2(rect.width(), TRACK_H - 2.0));
        let head_r = Rect::from_min_size(row_r.min, vec2(HEAD_W - 6.0, row_r.height()));
        let lane_r = Rect::from_min_max(Pos2::new(grid.left(), row_r.top()), row_r.max);
        track_head(app, ui, &painter, head_r, pad);
        painter.rect_filled(lane_r, 2.0, Color32::from_rgba_unmultiplied(255, 255, 255, if row % 2 == 0 { 6 } else { 3 }));

        let clips: Vec<Clip> = app.arrangement.clips(pad).to_vec();
        for (i, c) in clips.iter().enumerate() {
            let r = Rect::from_min_max(Pos2::new(v.x_of(c.start as f64), lane_r.top() + 1.0), Pos2::new(v.x_of(c.end() as f64), lane_r.bottom() - 1.0));
            if r.right() < grid.left() || r.left() > grid.right() {
                continue;
            }
            let selected = app.arr.selected.contains(&(pad, i));
            paint_clip(app, &painter, r.intersect(Rect::from_min_max(Pos2::new(grid.left(), tracks_r.top()), tracks_r.max)), r, pad, c, &v, selected);
            if let Some(pos) = pointer.filter(|p| r.contains(*p) && tracks_r.contains(*p)) {
                let near_start = pos.x - r.left() < EDGE && r.width() > EDGE * 3.0;
                let near_end = r.right() - pos.x < EDGE;
                clip_hit = Some((pad, i, near_start, near_end));
            }
            let id = egui::Id::new(("arr_clip", pad, c.start));
            let resp = ui.interact(r.intersect(tracks_r).intersect(grid), id, Sense::click_and_drag());
            clip_events(app, ui, &resp, pad, i, &v, snap);
        }
    }
    if let Some((_, _, s, e)) = clip_hit
        && (s || e)
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }

    // ---- Drags under way.
    if let (Some(drag), Some(pos)) = (app.arr.drag, ui.input(|i| i.pointer.interact_pos())) {
        let at = v.tick_at(pos.x);
        match drag {
            ArrDrag::Move { grab } => {
                // Shown as outlines until the drop, when they land and cut into what is there.
                let delta = snapped(at) as i64 - floor_snap(grab as f64) as i64;
                let delta = delta.max(-(app.arr.moving.iter().map(|(_, c)| c.start).min().unwrap_or(0) as i64));
                for (pad, c) in &app.arr.moving {
                    let Some(row) = pads.iter().position(|p| p == pad) else { continue };
                    let top = tracks_r.top() + row as f32 * TRACK_H - app.arr.vscroll;
                    let x0 = v.x_of((c.start as i64 + delta) as f64);
                    let r = Rect::from_min_size(Pos2::new(x0, top + 1.0), vec2(c.len as f32 / BAR as f32 * v.zoom, TRACK_H - 4.0));
                    painter.rect_stroke(r, 3.0, Stroke::new(2.0, theme::TEXT), StrokeKind::Inside);
                }
            }
            ArrDrag::Trim { pad, index, end } => {
                if let Some(c) = app.arrangement.clips(pad).get(index).copied() {
                    let t = snapped(at);
                    let x = v.x_of(t as f64);
                    let row = pads.iter().position(|p| *p == pad).unwrap_or(0);
                    let top = tracks_r.top() + row as f32 * TRACK_H - app.arr.vscroll;
                    let (a, b) = if end { (v.x_of(c.start as f64), x) } else { (x, v.x_of(c.end() as f64)) };
                    painter.rect_stroke(Rect::from_min_max(Pos2::new(a.min(b), top + 1.0), Pos2::new(a.max(b), top + TRACK_H - 3.0)), 3.0, Stroke::new(2.0, theme::TEXT), StrokeKind::Inside);
                }
            }
            ArrDrag::Section { index, grab } => {
                if let Some(s) = app.arrangement.sections.get(index) {
                    let to = (s.start as i64 + snapped(at) as i64 - floor_snap(grab as f64) as i64).max(0) as f64;
                    let r = Rect::from_min_max(Pos2::new(v.x_of(to), lane.top() + 1.0), Pos2::new(v.x_of(to + s.len as f64), grid.bottom()));
                    p.rect_stroke(r, 3.0, Stroke::new(2.0, theme::TEXT), StrokeKind::Inside);
                }
            }
            ArrDrag::Brace { .. } | ArrDrag::Marker { .. } => {}
            ArrDrag::Box { from } => {
                // Everything the box touches is selected, as it is drawn.
                let band = Rect::from_two_pos(from, pos);
                let (mut clips, mut sections) = app.arr.box_base.clone();
                for (row, &pad) in pads.iter().enumerate() {
                    let top = tracks_r.top() + row as f32 * TRACK_H - app.arr.vscroll;
                    for (i, c) in app.arrangement.clips(pad).iter().enumerate() {
                        let r = Rect::from_min_max(Pos2::new(v.x_of(c.start as f64), top + 1.0), Pos2::new(v.x_of(c.end() as f64), top + TRACK_H - 3.0)).intersect(tracks_r);
                        if r.is_positive() && r.intersects(band) && !clips.contains(&(pad, i)) {
                            clips.push((pad, i));
                        }
                    }
                }
                for (i, s) in app.arrangement.sections.iter().enumerate() {
                    let r = Rect::from_min_max(Pos2::new(v.x_of(s.start as f64), lane.top() + 2.0), Pos2::new(v.x_of(s.end() as f64), lane.bottom() - 2.0));
                    if r.intersects(band) && !sections.contains(&i) {
                        sections.push(i);
                    }
                }
                app.arr.selected = clips;
                app.arr.sections = sections;
                p.rect_filled(band, 0.0, theme::TEXT.gamma_multiply(0.06));
                p.rect_stroke(band, 0.0, Stroke::new(1.0, theme::TEXT.gamma_multiply(0.6)), StrokeKind::Inside);
            }
        }
    }
    if ui.input(|i| i.pointer.any_released()) && let Some(drag) = app.arr.drag.take() {
        let at = ui.input(|i| i.pointer.interact_pos()).map(|p| v.tick_at(p.x));
        match (drag, at) {
            (ArrDrag::Move { grab }, Some(at)) => {
                let moving = std::mem::take(&mut app.arr.moving);
                let delta = snapped(at) as i64 - floor_snap(grab as f64) as i64;
                let delta = delta.max(-(moving.iter().map(|(_, c)| c.start).min().unwrap_or(0) as i64));
                if delta != 0 {
                    app.arrangement.take(app.arr.selected.clone());
                    app.drop_moved(&moving, delta);
                }
            }
            (ArrDrag::Trim { pad, index, end }, Some(at)) => app.trim_clip(pad, index, end, snapped(at)),
            (ArrDrag::Marker { .. }, _) => app.arrangement.markers.sort_by_key(|m| m.tick),
            (ArrDrag::Section { index, grab }, Some(at)) => {
                if let Some(s) = app.arrangement.sections.get(index) {
                    let to = (s.start as i64 + snapped(at) as i64 - floor_snap(grab as f64) as i64).max(0) as u64;
                    app.move_section(index, to);
                }
            }
            _ => {}
        }
        app.arr.moving.clear();
    }

    // ---- Markers through the song, the insert marker, and the playhead.
    for m in &app.arrangement.markers {
        let x = v.x_of(m.tick as f64);
        if x >= grid.left() {
            p.vline(x, ruler.bottom()..=grid.bottom(), Stroke::new(1.0, theme::CYAN.gamma_multiply(0.35)));
        }
    }
    let cx = v.x_of(app.arr.cursor as f64);
    if cx >= grid.left() {
        p.vline(cx, ruler.top()..=grid.bottom(), Stroke::new(1.0, theme::TEXT.gamma_multiply(0.35)));
        let tri = vec![Pos2::new(cx - 5.0, ruler.top()), Pos2::new(cx + 5.0, ruler.top()), Pos2::new(cx, ruler.top() + 7.0)];
        p.add(egui::Shape::convex_polygon(tri, theme::TEXT, Stroke::NONE));
    }
    if playing_song {
        let x = v.x_of(head);
        if x >= grid.left() {
            p.vline(x, ruler.top()..=grid.bottom(), Stroke::new(1.5, theme::GREEN));
        }
    }
    if pads.is_empty() {
        p.text(
            tracks_r.center(),
            Align2::CENTER_CENTER,
            "Nothing to arrange yet. Make loops on the pads, then press Chain to song or Put loop.",
            FontId::proportional(14.0),
            theme::DIM,
        );
    } else if app.arrangement.is_empty() {
        p.text(
            Pos2::new(grid.center().x, tracks_r.top() + pads.len() as f32 * TRACK_H + 30.0).min(tracks_r.center_bottom() - vec2(0.0, 20.0)),
            Align2::CENTER_CENTER,
            "Press Chain to song to lay the chain out here, or double-click a track to add a clip",
            FontId::proportional(13.0),
            theme::DIM,
        );
    }
}

/// The ruler's right-click menu: a marker, or a bar (or the braced stretch) of time put
/// in or taken out of the song.
fn ruler_menu(app: &mut App, ui: &mut Ui) {
    let at = app.arr.ruler_at;
    let bar = at / BAR * BAR;
    ui.label(egui::RichText::new(format!("Bar {}", bar / BAR + 1)).small().color(theme::DIM));
    if ui.button("Add marker here").clicked() {
        app.add_marker(at);
        ui.close();
    }
    ui.separator();
    if ui.button(format!("Insert a bar before bar {}", bar / BAR + 1)).on_hover_text("Everything from here on moves a bar later").clicked() {
        app.insert_time(bar, BAR);
        ui.close();
    }
    if ui.button(format!("Delete bar {}", bar / BAR + 1)).on_hover_text("What is in the bar goes, and the rest of the song moves up to close the gap").clicked() {
        app.delete_time(bar, bar + BAR);
        ui.close();
    }
    if let Some((s, e)) = app.arrangement.brace {
        ui.separator();
        let bars = (e - s) as f64 / BAR as f64;
        if ui.button(format!("Insert {bars:.4} bars at the brace").replace(".0000", "")).on_hover_text("A gap as long as the loop brace, at its start").clicked() {
            app.insert_time(s, e - s);
            ui.close();
        }
        if ui.button("Delete the time under the brace").clicked() {
            app.delete_time(s, e);
            ui.close();
        }
    }
}

/// A track's name, mute and solo.
fn track_head(app: &mut App, ui: &mut Ui, painter: &egui::Painter, r: Rect, pad: usize) {
    painter.rect_filled(r, 3.0, theme::WELL);
    let selected = app.selected == pad;
    let label = pad_label(pad);
    let name = app.pads[pad].name.clone();
    let kind = match app.pads[pad].cfg.kind {
        PadKind::Audio => "audio",
        PadKind::Midi => "midi",
        PadKind::Vocal => "vocal",
        PadKind::Drum => "drum",
    };
    // R, M and S: small squares at the right, inside the header.
    const BTN: f32 = 20.0;
    const GAP: f32 = 3.0;
    let buttons_left = r.right() - 6.0 - 3.0 * BTN - 2.0 * GAP;
    painter.text(r.left_top() + vec2(8.0, 5.0), Align2::LEFT_TOP, &label, theme::bold(11.0), if selected { theme::AMBER } else { theme::TEXT });
    // The name, cut short with "…" where it would run under the buttons.
    let name_left = r.left() + 40.0;
    let name_w = (buttons_left - 6.0 - name_left).max(0.0);
    let galley = elided(painter, &name, FontId::proportional(11.0), theme::DIM, name_w);
    let cut = galley.size().x + 0.5 < painter.layout_no_wrap(name.clone(), FontId::proportional(11.0), theme::DIM).size().x;
    painter.galley(Pos2::new(name_left, r.top() + 5.0), galley, theme::DIM);
    let frozen = app.arrangement.is_frozen(pad);
    if frozen {
        painter.text(r.left_bottom() + vec2(8.0, -4.0), Align2::LEFT_BOTTOM, format!("{kind} · frozen"), FontId::proportional(9.0), theme::CYAN);
    } else {
        painter.text(r.left_bottom() + vec2(8.0, -4.0), Align2::LEFT_BOTTOM, kind, FontId::proportional(9.0), theme::FAINT);
    }
    let hit = ui.interact(Rect::from_min_max(r.min, Pos2::new(buttons_left - 2.0, r.bottom())).intersect(painter.clip_rect()), egui::Id::new(("arr_head", pad)), Sense::click());
    if hit.clicked() {
        app.select(pad);
    }
    hit.context_menu(|ui| {
        ui.label(egui::RichText::new(format!("{label} {name}")).small().color(theme::DIM));
        if frozen {
            if ui.button("Unfreeze").on_hover_text("Puts the track's clips back and switches its plugin on").clicked() {
                app.unfreeze_track(pad);
                ui.close();
            }
        } else if ui.button("Freeze").on_hover_text("Renders the track's clips to audio and switches its instrument plugin off, to spare the processor").clicked() {
            app.freeze_track(pad);
            ui.close();
        }
    });
    if cut {
        hit.on_hover_text(&name);
    }
    let mut cfg = app.pads[pad].cfg;
    let armed = app.is_armed(pad);
    let what = if app.records_audio(pad) { "the input (audio)" } else { "the notes played on this pad" };
    let at = |k: usize| Rect::from_min_size(Pos2::new(buttons_left + k as f32 * (BTN + GAP), r.center().y - BTN * 0.5), vec2(BTN, BTN));
    if small_toggle(ui, painter, at(0), ("arm", pad), armed, "R", theme::RED).on_hover_text(format!("Arm: with Rec on, records {what} onto this track")).clicked() {
        app.arm_track(pad, !armed);
    }
    if small_toggle(ui, painter, at(1), ("mute", pad), cfg.mute, "M", theme::RED).on_hover_text("Mute").clicked() {
        cfg.mute ^= true;
    }
    if small_toggle(ui, painter, at(2), ("solo", pad), cfg.solo, "S", theme::AMBER).on_hover_text("Solo").clicked() {
        cfg.solo ^= true;
    }
    if cfg != app.pads[pad].cfg {
        app.pads[pad].cfg = cfg;
        app.push_pad(pad);
    }
}

/// `text` on one line, cut short with "…" to fit in `width`.
fn elided(painter: &egui::Painter, text: &str, font: FontId, colour: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, colour);
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    painter.layout_job(job)
}

/// A small square on/off button for a track header.
fn small_toggle(ui: &mut Ui, p: &egui::Painter, rect: Rect, id: (&str, usize), on: bool, text: &str, colour: Color32) -> egui::Response {
    // Only the part of it that is in view (the tracks scroll under the ruler) responds.
    let resp = ui.interact(rect.intersect(p.clip_rect()), egui::Id::new(("arr_head_btn", id.0, id.1)), Sense::click());
    let fill = if on {
        colour
    } else if resp.hovered() {
        theme::FAINT.gamma_multiply(0.5)
    } else {
        Color32::from_rgb(46, 50, 56)
    };
    p.rect_filled(rect, 4.0, fill);
    p.rect_stroke(rect, 4.0, Stroke::new(1.0, Color32::from_black_alpha(160)), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, text, theme::bold(11.0), if on { theme::BG } else { theme::TEXT });
    resp
}

/// A clip: its loop's colour, name, and the notes it plays.
#[allow(clippy::too_many_arguments)]
fn paint_clip(app: &App, p: &egui::Painter, visible: Rect, r: Rect, pad: usize, c: &Clip, v: &View2, selected: bool) {
    if let Some(src) = c.audio {
        paint_audio_clip(app, p, visible, r, c, src, v, selected);
        return;
    }
    let colour = if c.notes.is_some() { NOTES_COLOUR } else { loop_colour(c.scene) };
    let clip_p = p.with_clip_rect(visible);
    clip_p.rect_filled(r, 3.0, colour.gamma_multiply(if selected { 0.55 } else { 0.32 }));
    clip_p.rect_stroke(r, 3.0, Stroke::new(if selected { 1.5 } else { 1.0 }, if selected { theme::TEXT } else { colour.gamma_multiply(0.8) }), StrokeKind::Inside);
    let label = if c.notes.is_some() { "Notes".to_string() } else { app.scene_label(c.scene) };
    clip_p.text(r.left_top() + vec2(4.0, 2.0), Align2::LEFT_TOP, label, FontId::proportional(9.5), theme::TEXT);
    let own = c.notes.and_then(|n| app.arrangement.patterns.get(n));
    let Some(pat) = own.or_else(|| pattern_ref(app, c.scene, pad)).filter(|p| !p.notes.is_empty() && p.len_ticks > 0) else { return };
    // The notes, each repeat of the pattern across the clip.
    let (lo, hi) = pat.notes.iter().fold((127u8, 0u8), |(a, b), n| (a.min(n.pitch), b.max(n.pitch)));
    let span = (hi - lo).max(1) as f32;
    let body = Rect::from_min_max(r.left_top() + vec2(0.0, 14.0), r.right_bottom() - vec2(0.0, 3.0));
    let len = pat.len_ticks as u64;
    let first = c.offset / len;
    let mut rep = first;
    loop {
        let base = c.start as i64 + (rep * len) as i64 - c.offset as i64;
        if base >= c.end() as i64 || v.x_of(base as f64) > visible.right() {
            break;
        }
        for n in &pat.notes {
            let t = base + n.start as i64;
            if t < c.start as i64 || t >= c.end() as i64 {
                continue;
            }
            let x0 = v.x_of(t as f64);
            let x1 = v.x_of((t + n.len as i64).min(c.end() as i64) as f64).max(x0 + 1.5);
            let y = body.bottom() - (n.pitch - lo) as f32 / span * (body.height() - 2.0) - 1.0;
            clip_p.line_segment([Pos2::new(x0, y), Pos2::new(x1, y)], Stroke::new(2.0, colour));
        }
        rep += 1;
        if rep - first > 4096 {
            break;
        }
    }
}

/// Colour of audio clips.
const AUDIO_COLOUR: Color32 = Color32::from_rgb(150, 160, 175);

/// An audio clip: the recording's name, its waveform, and its fades.
#[allow(clippy::too_many_arguments)]
fn paint_audio_clip(app: &App, p: &egui::Painter, visible: Rect, r: Rect, c: &Clip, src: usize, v: &View2, selected: bool) {
    let clip_p = p.with_clip_rect(visible);
    clip_p.rect_filled(r, 3.0, AUDIO_COLOUR.gamma_multiply(if selected { 0.4 } else { 0.22 }));
    clip_p.rect_stroke(r, 3.0, Stroke::new(if selected { 1.5 } else { 1.0 }, if selected { theme::TEXT } else { AUDIO_COLOUR.gamma_multiply(0.7) }), StrokeKind::Inside);
    let Some(a) = app.arrangement.audio.get(src) else { return };
    let gain = crate::app::db_to_gain(c.gain_db);
    let label = if c.gain_db.abs() >= 0.05 { format!("{}  {:+.1} dB", a.name, c.gain_db) } else { a.name.clone() };
    clip_p.text(r.left_top() + vec2(4.0, 2.0), Align2::LEFT_TOP, label, FontId::proportional(9.5), theme::TEXT);

    // The waveform, one column per point, from the source's overview.
    let body = Rect::from_min_max(r.left_top() + vec2(0.0, 13.0), r.right_bottom() - vec2(0.0, 2.0));
    let mid = body.center().y;
    let half = body.height() * 0.5;
    let ticks_per_frame = app.source_ticks_per_frame(src);
    let (x0, x1) = (r.left().max(visible.left()), r.right().min(visible.right()));
    let mut x = x0.floor();
    while x < x1 {
        let (ta, tb) = (v.tick_at(x), v.tick_at(x + 1.0));
        // Ticks into the clip, then frames into the recording.
        let fa = ((ta - c.start as f64 + c.offset as f64) / ticks_per_frame).max(0.0) as usize / crate::app::OVERVIEW_STEP;
        let fb = (((tb - c.start as f64 + c.offset as f64) / ticks_per_frame).max(0.0) as usize / crate::app::OVERVIEW_STEP + 1).min(a.overview.len());
        if fa < fb {
            let (lo, hi) = a.overview[fa..fb].iter().fold((0f32, 0f32), |(l, h), (a, b)| (l.min(*a), h.max(*b)));
            let (lo, hi) = ((lo * gain).max(-1.0), (hi * gain).min(1.0));
            clip_p.vline(x + 0.5, (mid - hi * half)..=(mid - lo * half).max(mid - hi * half + 1.0), Stroke::new(1.0, AUDIO_COLOUR));
        }
        x += 1.0;
    }
    // Fades, as lines from the corners.
    let fade_x = |ticks: u64| ticks as f32 / BAR as f32 * v.zoom;
    if c.fade_in > 0 {
        clip_p.line_segment([Pos2::new(r.left(), r.bottom()), Pos2::new(r.left() + fade_x(c.fade_in), r.top() + 1.0)], Stroke::new(1.0, theme::TEXT));
    }
    if c.fade_out > 0 {
        clip_p.line_segment([Pos2::new(r.right() - fade_x(c.fade_out), r.top() + 1.0), Pos2::new(r.right(), r.bottom())], Stroke::new(1.0, theme::TEXT));
    }
}

/// Gain and fades of an audio clip, in its right-click menu.
fn audio_clip_menu(app: &mut App, ui: &mut Ui, pad: usize, i: usize) {
    let Some(c) = app.arrangement.clips(pad).get(i).copied() else { return };
    let (mut gain, mut fade_in, mut fade_out) = (c.gain_db, c.fade_in as f32 / PPQ as f32, c.fade_out as f32 / PPQ as f32);
    let beats = c.len as f32 / PPQ as f32;
    egui::Grid::new("audio_clip_menu").num_columns(2).show(ui, |ui| {
        ui.label("Gain");
        ui.add(egui::Slider::new(&mut gain, -24.0..=12.0).suffix(" dB").step_by(0.1));
        ui.end_row();
        ui.label("Fade in");
        ui.add(egui::Slider::new(&mut fade_in, 0.0..=beats.min(16.0)).suffix(" beats").step_by(0.0625));
        ui.end_row();
        ui.label("Fade out");
        ui.add(egui::Slider::new(&mut fade_out, 0.0..=beats.min(16.0)).suffix(" beats").step_by(0.0625));
        ui.end_row();
    });
    let ticks = |b: f32| (b * PPQ as f32).round() as u64;
    if gain != c.gain_db || ticks(fade_in) != c.fade_in || ticks(fade_out) != c.fade_out {
        app.set_clip_audio(pad, i, gain, ticks(fade_in), ticks(fade_out));
    }
    if ui.button("Reset gain and fades").clicked() {
        app.set_clip_audio(pad, i, 0.0, 0, 0);
    }
    // A crossfade, when an audio clip ends where this one starts.
    let joined = i > 0 && app.arrangement.clips(pad).get(i - 1).is_some_and(|p| p.is_audio() && p.end() == c.start);
    if joined {
        let mut x = c.xfade as f32 / PPQ as f32;
        ui.horizontal(|ui| {
            ui.label("Crossfade");
            if ui.add(egui::Slider::new(&mut x, 0.0..=4.0f32.min(beats)).suffix(" beats").step_by(0.03125)).on_hover_text("With the clip before: both play on under each other, so the join is smooth").changed() {
                app.set_xfade(pad, i, (x * PPQ as f32).round() as u64);
            }
        });
    }
    let Some(src) = c.audio else { return };
    let Some(a) = app.arrangement.audio.get(src).cloned() else { return };
    // Other takes of the recording (passes round the loop brace).
    if a.takes.len() > 1 {
        ui.menu_button("Takes", |ui| {
            for (k, &t) in a.takes.iter().enumerate() {
                if ui.radio(t == src, format!("Take {}", k + 1)).clicked() {
                    app.choose_take(pad, i, t);
                    ui.close();
                }
            }
        });
    }
    ui.separator();
    let (mut warp, mut bpm, mut mode) = (a.warp, a.bpm, a.mode);
    ui.checkbox(&mut warp, "Follow the tempo").on_hover_text("Stretched to the project tempo when it differs from the recording's");
    ui.add_enabled_ui(warp, |ui| {
        ui.horizontal(|ui| {
            ui.label("Recorded at");
            ui.add(egui::DragValue::new(&mut bpm).range(20.0..=300.0).speed(0.1).suffix(" BPM"));
        });
        ui.horizontal(|ui| {
            for m in crate::stretch::WarpMode::ALL {
                ui.selectable_value(&mut mode, m, m.name());
            }
        });
    });
    if (warp, bpm, mode) != (a.warp, a.bpm, a.mode) {
        app.set_source_warp(src, warp, bpm, mode);
    }
    if app.arr.warping.iter().any(|w| w.0 == src) {
        ui.label(egui::RichText::new("Stretching to the tempo…").small().color(theme::DIM));
    }
}

/// Clicks and drags on one clip.
fn clip_events(app: &mut App, ui: &Ui, resp: &egui::Response, pad: usize, i: usize, v: &View2, snap: u64) {
    if resp.secondary_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        app.arr.clip_menu_at = ((v.tick_at(pos.x) / snap as f64).round() as u64) * snap;
    }
    let Some(c) = app.arrangement.clips(pad).get(i).copied() else { return };
    let shift = ui.input(|inp| inp.modifiers.shift || inp.modifiers.command);
    if resp.clicked() {
        if !shift {
            app.arr.sections.clear();
        }
        if shift {
            match app.arr.selected.iter().position(|s| *s == (pad, i)) {
                Some(at) => drop(app.arr.selected.remove(at)),
                None => app.arr.selected.push((pad, i)),
            }
        } else {
            app.arr.selected = vec![(pad, i)];
        }
        app.select(pad);
    }
    if resp.double_clicked() {
        app.edit_clip(pad, i);
        return;
    }
    if resp.drag_started()
        && let Some(o) = ui.input(|inp| inp.pointer.press_origin())
    {
        app.arr.sections.clear();
        let r = resp.rect;
        let (x_start, x_end) = (v.x_of(c.start as f64), v.x_of(c.end() as f64));
        if (o.x - x_start).abs() < EDGE && x_end - x_start > EDGE * 3.0 {
            app.arr.drag = Some(ArrDrag::Trim { pad, index: i, end: false });
        } else if (x_end - o.x).abs() < EDGE {
            app.arr.drag = Some(ArrDrag::Trim { pad, index: i, end: true });
        } else {
            if !app.arr.selected.contains(&(pad, i)) {
                app.arr.selected = vec![(pad, i)];
            }
            app.arr.moving = app.selected_clips();
            app.arr.drag = Some(ArrDrag::Move { grab: v.tick_at(o.x) as u64 });
        }
        let _ = r;
    }
    resp.context_menu(|ui| {
        if let Some(src) = c.audio {
            let name = app.arrangement.audio.get(src).map_or(String::new(), |a| a.name.clone());
            ui.label(egui::RichText::new(format!("{} · {name}", pad_label(pad))).small().color(theme::DIM));
            audio_clip_menu(app, ui, pad, i);
            ui.separator();
        } else if c.notes.is_some() {
            ui.label(egui::RichText::new(format!("{} · notes of its own", pad_label(pad))).small().color(theme::DIM));
            if ui.button("Open as a loop").on_hover_text("Makes these notes a loop of their own, to edit in the piano roll or steps").clicked() {
                app.edit_clip(pad, i);
                ui.close();
            }
            if ui.button("Make unique").on_hover_text("A copy of the notes, so this clip can change without the others that share them").clicked() {
                pick_unless_selected(app, pad, i);
                app.make_unique();
                ui.close();
            }
            ui.separator();
        } else {
            ui.label(egui::RichText::new(format!("{} · {}", pad_label(pad), app.scene_label(c.scene))).small().color(theme::DIM));
            pattern_clip_menu(app, ui, pad, i, &c);
        }
        if ui.button("Consolidate    Ctrl+J").on_hover_text("The selected clips of this track rendered into one audio clip in their place, as the pad sounds before its bus").clicked() {
            pick_unless_selected(app, pad, i);
            app.consolidate();
            ui.close();
        }
        if ui.button("Bake to a new track").on_hover_text("The selected clips (any tracks) rendered through their buses and effects onto a new track, in their place").clicked() {
            pick_unless_selected(app, pad, i);
            app.bake_selection();
            ui.close();
        }
        if ui.button("Duplicate    Ctrl+D").clicked() {
            pick_unless_selected(app, pad, i);
            app.duplicate_selection();
            ui.close();
        }
        let here = app.arr.clip_menu_at;
        if here > c.start && here < c.end() && ui.button(format!("Split here ({})", crate::app::pos_text(here))).clicked() {
            app.arr.selected = vec![(pad, i)];
            app.split_at(here);
            ui.close();
        }
        if ui.button("Split at marker    Ctrl+E").clicked() {
            app.arr.selected = vec![(pad, i)];
            app.split_at_cursor();
            ui.close();
        }
        if ui.button("Delete    Del").clicked() {
            pick_unless_selected(app, pad, i);
            app.delete_selected_clips();
            ui.close();
        }
    });
}

/// The part of a pattern clip's menu about its loop.
fn pattern_clip_menu(app: &mut App, ui: &mut Ui, pad: usize, i: usize, c: &Clip) {
    if ui.button("Open its loop").clicked() {
        app.edit_clip(pad, i);
        ui.close();
    }
    ui.menu_button("Play another loop", |ui| {
        for k in 0..app.scenes.len() {
            let plays = pattern_ref(app, k, pad).is_some_and(|p| !p.notes.is_empty());
            if ui.add_enabled(plays && k != c.scene, egui::Button::new(app.scene_label(k))).clicked() {
                let track = app.arrangement.tracks.iter_mut().find(|(p, _)| *p == pad);
                if let Some(clip) = track.and_then(|(_, t)| t.get_mut(i)) {
                    clip.scene = k;
                }
                ui.close();
            }
        }
    });
    if ui.button("Make unique").on_hover_text("Copies the loop, so this clip can be edited without changing the others").clicked() {
        if !app.arr.selected.contains(&(pad, i)) {
            app.arr.selected = vec![(pad, i)];
        }
        app.make_unique();
        ui.close();
    }
}

/// A menu on a selected clip acts on the whole selection; on any other, on that clip.
fn pick_unless_selected(app: &mut App, pad: usize, i: usize) {
    if !app.arr.selected.contains(&(pad, i)) {
        app.arr.selected = vec![(pad, i)];
        app.arr.sections.clear();
    }
}
