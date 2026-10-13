//! VST3 plugin browser. Built for big libraries: the list is virtualised, filtering is a
//! substring match on a pre-lowercased key, and scanning never blocks it.

use super::theme;
use crate::app::{App, PluginTarget};
use crate::music::pad_label;
use eframe::egui::{self, Key};
use std::sync::atomic::Ordering;

pub fn draw(app: &mut App, ctx: &egui::Context) {
    if !app.browser.open {
        return;
    }
    let target_name = match app.browser.target {
        PluginTarget::Pad(i) => format!("instrument for pad {}", pad_label(i)),
        PluginTarget::Fx(b, s) => {
            let bus = crate::shared::bus_label(b);
            format!("effect for {bus}, slot {}", s + 1)
        }
    };
    let mut open = true;
    let mut load: Option<(std::path::PathBuf, Option<String>)> = None;
    let extra_dirs = app.prefs.vst3_dirs.clone();
    egui::Window::new(format!("VST3 Plugins: {target_name}"))
        .open(&mut open)
        .default_size([620.0, 560.0])
        .min_width(460.0)
        .min_height(300.0)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let search = ui.add(
                    egui::TextEdit::singleline(&mut app.browser.query).hint_text("Search name or vendor").desired_width(240.0),
                );
                // Type straight away when the window opens.
                if ui.memory(|m| m.focused().is_none()) {
                    search.request_focus();
                }
                for (i, label) in ["All", "Instruments", "Effects", "Failed"].iter().enumerate() {
                    ui.selectable_value(&mut app.browser.filter, i as u8, *label);
                }
            });

            let cat = &mut app.catalog;
            ui.horizontal(|ui| {
                if cat.scanning {
                    let done = cat.done.load(Ordering::Relaxed);
                    let frac = done as f32 / cat.total.max(1) as f32;
                    ui.add(egui::ProgressBar::new(frac).desired_width(240.0).text(format!("Scanning {done} / {}", cat.total)));
                    if ui.button("Cancel").clicked() {
                        cat.cancel();
                    }
                } else {
                    if ui.button("Rescan").on_hover_text("Checks for new or changed plugins; unchanged ones come from the cache").clicked() {
                        cat.start_scan(false, &extra_dirs);
                    }
                    if cat.failed() > 0 && ui.button("Retry failed").clicked() {
                        cat.start_scan(true, &extra_dirs);
                    }
                }
                ui.label(
                    egui::RichText::new(format!("{} plugins, {} unreadable", cat.entries.len() - cat.failed(), cat.failed()))
                        .small()
                        .color(theme::DIM),
                );
            });
            ui.separator();

            let query = app.browser.query.to_lowercase();
            let filter = app.browser.filter;
            let shown: Vec<usize> = app
                .catalog
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| match filter {
                    1 => e.error.is_none() && e.instrument,
                    2 => e.error.is_none() && !e.instrument,
                    3 => e.error.is_some(),
                    _ => e.error.is_none(),
                })
                .filter(|(_, e)| query.is_empty() || e.key.contains(&query))
                .map(|(i, _)| i)
                .collect();

            if shown.is_empty() {
                ui.add_space(20.0);
                let msg = if app.catalog.scanning {
                    "Scanning your VST3 folders…"
                } else if app.catalog.entries.is_empty() {
                    "No plugins found in the standard VST3 folders."
                } else {
                    "Nothing matches."
                };
                ui.vertical_centered(|ui| ui.label(egui::RichText::new(msg).color(theme::DIM)));
                return;
            }
            // Enter loads the top match, so search-and-go needs no mouse.
            let enter = ui.input(|i| i.key_pressed(Key::Enter));
            if enter && filter != 3 {
                let e = &app.catalog.entries[shown[0]];
                load = Some((e.path.clone(), e.class_id.clone()));
            }

            let row_h = 22.0;
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_h, shown.len(), |ui, range| {
                for row in range {
                    let e = &app.catalog.entries[shown[row]];
                    ui.horizontal(|ui| {
                        ui.set_height(row_h - 4.0);
                        let name = egui::RichText::new(&e.name).color(if e.error.is_some() { theme::DIM } else { theme::TEXT });
                        let resp = ui.selectable_label(row == 0 && !query.is_empty(), name);
                        ui.label(egui::RichText::new(&e.vendor).small().color(theme::DIM));
                        match &e.error {
                            Some(err) => {
                                ui.label(egui::RichText::new(err).small().color(theme::RED));
                            }
                            None => {
                                let col = if e.instrument { theme::CYAN } else { theme::AMBER };
                                ui.label(egui::RichText::new(&e.category).small().color(col));
                            }
                        }
                        if e.error.is_none() && (resp.double_clicked() || resp.clicked() && ui.input(|i| i.modifiers.command)) {
                            load = Some((e.path.clone(), e.class_id.clone()));
                        }
                        resp.on_hover_text(format!("{}\nDouble-click to load", e.path.display()));
                    });
                }
            });
        });

    if let Some((path, class_id)) = load {
        let target = app.browser.target;
        if app.load_plugin(&path, class_id.as_deref(), target, None) {
            app.browser.open = false;
            app.open_editor(target);
            if let PluginTarget::Pad(i) = target {
                app.select(i);
            }
        }
    } else if !open {
        app.browser.open = false;
    }
    if !app.browser.open {
        // The search field is gone; don't leave keyboard focus pointing at it, or the
        // next shortcut would be treated as typing.
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
    }
}
