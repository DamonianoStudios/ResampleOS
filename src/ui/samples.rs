//! The sample browser panel: the user's sample folders, searchable, with auditioning, and
//! files dragged (or sent with Enter) onto pads or the song timeline.

use super::{pill, theme, toggle};
use crate::app::{App, SampleList};
use eframe::egui::{self, Align2, FontId, Sense, Ui, vec2};
use std::path::{Path, PathBuf};

const ROW_H: f32 = 22.0;

pub fn draw(app: &mut App, ui: &mut Ui) {
    let mut close = false;
    ui.horizontal(|ui| {
        theme::title(ui, "SAMPLES", theme::AMBER);
        if ui.small_button("Add folder…").on_hover_text("Add a folder of samples (they are remembered with the settings)").clicked() {
            app.add_sample_folder_dialog();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("×").on_hover_text("Close (F4)").clicked() {
                close = true;
            }
        });
    });
    if close {
        app.toggle_samples();
        return;
    }
    let mut search_changed = false;
    ui.horizontal(|ui| {
        let on = app.samples.filter.active();
        let w = ui.available_width() - 70.0;
        search_changed = ui.add(egui::TextEdit::singleline(&mut app.samples.query).hint_text("Search every folder").desired_width(w)).changed();
        if toggle(ui, on || app.samples.show_filter, if on { "Filter ●" } else { "Filter" }, theme::CYAN).on_hover_text("Show only one-shots or loops, a key, a tempo range, a length").clicked() {
            app.samples.show_filter ^= true;
        }
    });
    if search_changed {
        app.samples.focused = true;
    }
    ui.horizontal(|ui| {
        for (list, name) in [(SampleList::Folder, "Folders"), (SampleList::Favourites, "★"), (SampleList::Recent, "Recent"), (SampleList::Project, "Project")] {
            if pill(ui, app.samples.list == list, name).clicked() {
                app.samples.list = list;
            }
        }
    });
    if app.samples.show_filter {
        filter_bar(app, ui);
    }
    ui.add_space(2.0);

    if app.prefs.sample_folders.is_empty() && app.samples.list == SampleList::Folder && app.samples.query.trim().is_empty() {
        ui.add_space(12.0);
        ui.label(egui::RichText::new("No sample folders yet.").color(theme::DIM));
        ui.label(egui::RichText::new("Add the folders your samples are in; they are listed in the background and remembered.").small().color(theme::DIM));
        if ui.button("Add folder…").clicked() {
            app.add_sample_folder_dialog();
        }
        footer(app, ui);
        return;
    }

    // Folder picker and where in it, for the folder list.
    let mut subs: Vec<String> = Vec::new();
    if app.samples.list == SampleList::Folder && app.samples.query.trim().is_empty() && !app.samples.filter.active() {
        folder_bar(app, ui);
        match app.folder_entries(app.samples.folder) {
            Some(entries) => subs = crate::app::samples_folder_view(entries, &app.samples.dir).0,
            None => {
                ui.label(egui::RichText::new("Listing the folder…").small().color(theme::DIM));
            }
        }
    }

    let rows = app.sample_rows();
    let footer_h = 58.0;
    let height = (ui.available_height() - footer_h).max(60.0);
    let mut open_sub = None;
    let mut select = None;
    let mut use_it = None;
    let mut star = None;
    let mut drag = None;
    egui::ScrollArea::vertical().id_salt("sample_rows").max_height(height).auto_shrink([false, false]).show_rows(ui, ROW_H, subs.len() + rows.len(), |ui, range| {
        for k in range {
            if let Some(sub) = subs.get(k) {
                let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                if r.hovered() {
                    ui.painter().rect_filled(rect, 3.0, theme::RAISED);
                }
                ui.painter().text(rect.left_center() + vec2(6.0, 0.0), Align2::LEFT_CENTER, format!("›  {sub}"), theme::bold(12.0), theme::DIM);
                if r.on_hover_text("Open this folder").clicked() {
                    open_sub = Some(sub.clone());
                }
                continue;
            }
            let path = &rows[k - subs.len()];
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click_and_drag());
            let selected = app.samples.selected.as_ref() == Some(path);
            let p = ui.painter();
            if selected {
                p.rect_filled(rect, 3.0, theme::AMBER.gamma_multiply(0.25));
            } else if resp.hovered() {
                p.rect_filled(rect, 3.0, theme::RAISED);
            }
            // The star, then the name (with its folder when searching across them).
            let star_r = egui::Rect::from_min_size(rect.min, vec2(ROW_H, ROW_H));
            let fav = app.is_favourite(path);
            p.text(star_r.center(), Align2::CENTER_CENTER, if fav { "★" } else { "☆" }, FontId::proportional(13.0), if fav { theme::AMBER } else { theme::FAINT });
            let name = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
            let midi = crate::app::is_midi(path);
            let colour = if midi { theme::CYAN } else { theme::TEXT };
            // What analysis found, at the right; the name is cut short before it.
            let info = app.samples.info.get(path).map(|i| i.label()).unwrap_or_default();
            let info_w = if info.is_empty() { 0.0 } else { p.layout_no_wrap(info.clone(), FontId::proportional(10.5), theme::DIM).size().x + 8.0 };
            if !info.is_empty() {
                p.text(rect.right_center() - vec2(4.0, 0.0), Align2::RIGHT_CENTER, &info, FontId::proportional(10.5), theme::DIM);
            }
            let name_r = egui::Rect::from_min_max(rect.min, rect.max - vec2(info_w, 0.0)).shrink(1.0);
            p.with_clip_rect(name_r).text(rect.left_center() + vec2(ROW_H + 2.0, 0.0), Align2::LEFT_CENTER, &name, FontId::proportional(12.0), colour);
            if resp.clicked() {
                if ui.input(|i| i.pointer.interact_pos()).is_some_and(|pos| star_r.contains(pos)) {
                    star = Some(path.clone());
                } else {
                    select = Some(path.clone());
                }
            }
            if resp.double_clicked() {
                use_it = Some(path.clone());
            }
            if resp.drag_started() {
                drag = Some(path.clone());
            }
            resp.on_hover_text(path.display().to_string());
        }
    });
    if let Some(sub) = open_sub {
        app.samples.dir.push(sub);
    }
    if let Some(path) = star {
        app.toggle_favourite(&path);
    }
    if let Some(path) = select {
        app.select_sample(&path);
    }
    if let Some(path) = use_it {
        app.use_sample(&path, false);
    }
    if let Some(path) = drag {
        app.samples.selected = Some(path.clone());
        app.samples.drag = Some(path);
    }
    if rows.is_empty() && subs.is_empty() {
        let what = match app.samples.list {
            SampleList::Favourites => "No favourites yet: click the star beside a file.",
            SampleList::Recent => "Files you use from here appear here.",
            SampleList::Project if app.project_dir.is_none() => "The project has not been saved yet: its files are listed here once it is.",
            SampleList::Project => "The project uses no audio files yet.",
            SampleList::Folder if !app.samples.query.trim().is_empty() => "Nothing matches.",
            SampleList::Folder => "No audio or MIDI files here.",
        };
        ui.label(egui::RichText::new(what).small().color(theme::DIM));
    }
    footer(app, ui);
}

/// Which folder, where in it (each step back up is a button), and the folder's menu.
fn folder_bar(app: &mut App, ui: &mut Ui) {
    let folders = app.prefs.sample_folders.clone();
    let k = app.samples.folder.min(folders.len().saturating_sub(1));
    let mut remove = false;
    let mut rescan = false;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("sample_folder").width(ui.available_width() - 48.0).selected_text(folders.get(k).map_or("", |f| f.name.as_str())).show_ui(ui, |ui| {
            for (i, f) in folders.iter().enumerate() {
                if ui.selectable_label(i == k, &f.name).on_hover_text(f.path.display().to_string()).clicked() {
                    app.samples.folder = i;
                    app.samples.dir = PathBuf::new();
                }
            }
        });
        ui.menu_button("…", |ui| {
            if ui.button("List it again").on_hover_text("After adding files to the folder").clicked() {
                rescan = true;
                ui.close();
            }
            if ui.button("Remove from the browser").on_hover_text("The files themselves are left alone").clicked() {
                remove = true;
                ui.close();
            }
        });
    });
    if remove {
        app.remove_sample_folder(k);
        return;
    }
    if rescan {
        app.rescan_sample_folder(k);
    }
    // Where in the folder: each part takes you back there.
    if !app.samples.dir.as_os_str().is_empty() {
        let parts: Vec<String> = app.samples.dir.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let mut go = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if ui.small_button("Top").on_hover_text("The top of the folder").clicked() {
                go = Some(0);
            }
            for (i, part) in parts.iter().enumerate() {
                ui.label(egui::RichText::new("/").color(theme::FAINT));
                if ui.small_button(part).clicked() {
                    go = Some(i + 1);
                }
            }
        });
        if let Some(n) = go {
            app.samples.dir = parts.iter().take(n).collect::<PathBuf>();
        }
    }
}

/// Kind, key, tempo and length. Files not analysed yet are hidden while a filter is on.
fn filter_bar(app: &mut App, ui: &mut Ui) {
    let f = &mut app.samples.filter;
    ui.horizontal_wrapped(|ui| {
        for (k, name) in [(0, "Any"), (1, "One-shots"), (2, "Loops")] {
            if pill(ui, f.kind == k, name).clicked() {
                f.kind = k;
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Key");
        let names = crate::music::NOTE_NAMES;
        let text = match f.key {
            None => "Any".to_string(),
            Some(None) => "Project key".to_string(),
            Some(Some((r, m))) => format!("{}{}", names[r as usize], if m { "m" } else { "" }),
        };
        egui::ComboBox::from_id_salt("sample_key").width(96.0).selected_text(text).show_ui(ui, |ui| {
            ui.selectable_value(&mut f.key, None, "Any");
            ui.selectable_value(&mut f.key, Some(None), "Project key");
            for minor in [false, true] {
                for (r, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut f.key, Some(Some((r as u8, minor))), format!("{n}{}", if minor { "m" } else { "" }));
                }
            }
        });
        ui.label("BPM");
        let mut on = f.bpm.1 > 0.0;
        if ui.checkbox(&mut on, "").on_hover_text("Loops within a tempo range").changed() {
            f.bpm = if on { (80.0, 100.0) } else { (0.0, 0.0) };
        }
        if on {
            ui.add(egui::DragValue::new(&mut f.bpm.0).range(20.0..=300.0).speed(0.5));
            ui.label("to");
            ui.add(egui::DragValue::new(&mut f.bpm.1).range(20.0..=300.0).speed(0.5));
            f.bpm.1 = f.bpm.1.max(f.bpm.0);
        }
    });
    ui.horizontal(|ui| {
        ui.label("Up to");
        ui.add(egui::DragValue::new(&mut f.max_secs).range(0.0..=600.0).speed(0.1).custom_formatter(|v, _| if v <= 0.0 { "any length".into() } else { format!("{v:.1} s") }));
        if ui.small_button("Clear").on_hover_text("Show everything again").clicked() {
            *f = Default::default();
        }
    });
}

fn footer(app: &mut App, ui: &mut Ui) {
    ui.separator();
    ui.horizontal(|ui| {
        if toggle(ui, app.samples.auto_play, "Audition", theme::GREEN).on_hover_text("Play each file as it is selected").clicked() {
            app.samples.auto_play ^= true;
            if !app.samples.auto_play {
                app.stop_preview();
            }
        }
        if toggle(ui, app.samples.sync, "Sync", theme::CYAN)
            .on_hover_text("Play loops stretched to the project tempo, going round, and starting on the beat while the transport runs")
            .clicked()
        {
            app.samples.sync ^= true;
            if let Some(p) = app.samples.selected.clone().filter(|_| app.samples.auto_play) {
                app.preview_sample(&p);
            }
        }
        let mut db = app.prefs.preview_db;
        if ui.add(egui::Slider::new(&mut db, -36.0..=0.0).suffix(" dB").show_value(true)).on_hover_text("Audition level").changed() {
            app.prefs.preview_db = db;
            app.prefs_edit.preview_db = db;
        }
    });
    match app.samples_to_analyse() {
        None => {
            ui.label(egui::RichText::new("Listing folders…").small().color(theme::DIM));
        }
        Some(0) => {}
        Some(n) => {
            ui.label(egui::RichText::new(format!("Analysing: {n} file{} to go (tempo, key, length)", if n == 1 { "" } else { "s" })).small().color(theme::DIM));
        }
    }
    ui.label(egui::RichText::new("click: audition · double-click or Enter: use · Shift+Enter: next empty pad · drag onto a pad or the timeline").small().color(theme::DIM));
}

/// While a file is dragged out of the browser: its name by the pointer; on release, onto
/// the pad or the stretch of timeline under it.
pub fn drag(app: &mut App, ctx: &egui::Context) {
    let Some(path) = app.samples.drag.clone() else { return };
    let pos = ctx.pointer_hover_pos().or_else(|| ctx.pointer_interact_pos());
    if let Some(pos) = pos {
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("sample_drag")));
        // Where it would land if let go here.
        let to = if app.view == crate::app::View::Arrange {
            app.arr.drop_at.map(|(pad, at)| match pad {
                Some(p) => format!("{} · {}", crate::music::pad_label(p), crate::app::pos_text(at)),
                None => format!("new track · {}", crate::app::pos_text(at)),
            })
        } else {
            app.pad_rects.iter().find(|(_, r)| r.contains(pos)).map(|(i, r)| {
                painter.rect_stroke(*r, 6.0, egui::Stroke::new(2.5, theme::AMBER), egui::StrokeKind::Outside);
                format!("onto {}", crate::music::pad_label(*i))
            })
        };
        let name = match to {
            Some(t) => format!("{}  →  {t}", file_name(&path)),
            None => file_name(&path),
        };
        let galley = painter.layout_no_wrap(name, FontId::proportional(12.0), theme::BG);
        let r = egui::Rect::from_min_size(pos + vec2(14.0, 6.0), galley.size() + vec2(12.0, 6.0));
        painter.rect_filled(r, 4.0, theme::AMBER);
        painter.galley(r.min + vec2(6.0, 3.0), galley, theme::BG);
    }
    if !ctx.input(|i| i.pointer.any_released()) {
        return;
    }
    app.samples.drag = None;
    let Some(pos) = pos else { return };
    if app.view == crate::app::View::Arrange {
        if let Some((pad, at)) = app.arr.drop_at {
            app.drop_sample_on_song(&path, pad, at);
        }
    } else if let Some(pad) = app.pad_rects.iter().find(|(_, r)| r.contains(pos)).map(|(i, _)| *i) {
        app.drop_sample_on_pad(&path, pad);
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned())
}
