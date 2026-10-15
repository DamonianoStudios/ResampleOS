//! The four-zone viewport plus floating windows.

mod arrange;
mod automation;
mod browser;
mod buses;
mod drum;
mod footer;
mod inspector;
mod modwarp;
mod pads;
mod prefs;
mod roll;
mod scenes;
mod steps;
pub mod theme;
mod top;
mod vocal;
mod widgets;
mod windows;

pub use theme::install as install_theme;
pub(crate) use widgets::{channel_fader, db, fader, humanize, knob, knob_sized, pan_box, pan_text, pill, tag, toggle, transport_button};

use crate::app::App;
use eframe::egui::{self, Ui};

pub fn draw(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();

    // Anything held with the mouse (a pad, an auditioned note) ends when the button lifts.
    if !ctx.input(|i| i.pointer.primary_down()) {
        app.end_recording();
    }
    if ctx.input(|i| i.pointer.primary_released())
        && let Some((pad, key)) = app.mouse_held.take() {
            app.note_off(pad, key);
        }

    // Files dropped from the OS land on the pad under the cursor, spilling onto the next pads.
    let dropped = ctx.input(|i| i.raw.dropped_files.clone());
    if !dropped.is_empty() && app.view == crate::app::View::Arrange {
        // On the timeline they become audio clips, on the track under the pointer.
        let (pad, at) = app.arr.drop_at.unwrap_or((None, app.arr.cursor));
        app.drop_song_audio(dropped.iter().map(|f| f.path().to_path_buf()).collect(), pad, at);
    } else if !dropped.is_empty() {
        let at = ctx.pointer_hover_pos().or_else(|| ctx.pointer_interact_pos());
        let mut pad = at
            .and_then(|p| app.pad_rects.iter().find(|(_, r)| r.contains(p)).map(|(i, _)| *i))
            .unwrap_or(app.selected);
        for f in dropped {
            if pad < app.pads.len() {
                app.load_file(pad, f.path().to_path_buf());
                pad += 1;
            }
        }
    }

    egui::Panel::top("top_bar").exact_size(126.0).frame(theme::gutter(8, 8, 8, 3)).show(ui, |ui| top::draw(app, ui));
    egui::Panel::bottom("footer").exact_size(38.0).frame(theme::gutter(10, 10, 2, 6)).show(ui, |ui| footer::draw(app, ui));
    let width = ui.available_width();
    if app.view == crate::app::View::Arrange {
        egui::Panel::bottom("zone_c").exact_size(372.0).frame(theme::gutter(3, 8, 3, 3)).show(ui, |ui| buses::draw(app, ui));
        egui::CentralPanel::default_margins().frame(theme::gutter(8, 8, 3, 3)).show(ui, |ui| {
            let rect = ui.available_rect_before_wrap();
            theme::card_in(ui, rect, None, |ui| arrange::draw(app, ui));
        });
        windows::draw(app, &ctx);
        browser::draw(app, &ctx);
        prefs::draw(app, &ctx);
        return;
    }
    egui::Panel::left("zone_a")
        .default_size((width * 0.36).clamp(380.0, 720.0))
        .resizable(true)
        .frame(theme::gutter(8, 3, 3, 3))
        .show(ui, |ui| {
            let rect = ui.available_rect_before_wrap();
            theme::card_in(ui, rect, None, |ui| pads::draw(app, ui));
        });
    egui::Panel::bottom("zone_c").exact_size(372.0).frame(theme::gutter(3, 8, 3, 3)).show(ui, |ui| buses::draw(app, ui));
    egui::CentralPanel::default_margins().frame(theme::gutter(3, 8, 3, 3)).show(ui, |ui| {
        let rect = ui.available_rect_before_wrap();
        theme::card_in(ui, rect, None, |ui| inspector::draw(app, ui));
    });

    windows::draw(app, &ctx);
    browser::draw(app, &ctx);
    prefs::draw(app, &ctx);
}
