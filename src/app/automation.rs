//! Automation: lanes that move bus, pad, drum and plugin controls through a loop or the song.
//!
//! Each loop has its own lanes, kept on the loop itself, so they switch with it on the beat
//! and are saved and undone with it. The arrangement has song-length lanes of its own, which
//! play over the lanes its sections bring from their loops. A lane is drawn in the
//! Automation tab (the song's under the timeline), or recorded by turning a control while
//! the transport runs with Rec on. The engine plays the lanes on top of the controls as
//! they were set by hand, and lets go of a control as soon as nothing automates it.

use super::*;
use crate::engine::{AutoLane, AutoPoint, AutoTarget};
use crate::hosted::Edit;

/// Grid lane points are placed on: (name, ticks). 1 or less is free.
pub const AUTO_SNAPS: [(&str, u32); 5] = [("Off", 1), ("1/4", PPQ), ("1/8", PPQ / 2), ("1/16", PPQ / 4), ("1/32", PPQ / 8)];

impl PluginTarget {
    /// The lane target for parameter `id` of the plugin here. Analyzers only listen, so
    /// there is nothing of theirs to automate.
    pub fn param_target(self, id: u32) -> Option<AutoTarget> {
        match self {
            PluginTarget::Pad(i) => Some(AutoTarget::PadPlugin { pad: i as u16, id }),
            PluginTarget::Fx(b, s) => Some(AutoTarget::FxPlugin { slot: (b * 3 + s) as u8, id }),
            PluginTarget::Analyzer(_) => None,
        }
    }
}

/// Which lanes the automation tools work on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AutoScope {
    /// The loop on screen's.
    Loop,
    /// The song's, in song ticks.
    Song,
}

#[derive(Default)]
pub struct AutoUi {
    /// Record moves of automatable controls while the transport runs.
    pub rec: bool,
    pub snap: usize,
    /// The point being dragged: (lane, point).
    pub drag: Option<(usize, usize)>,
    /// Where each control being recorded was last written this gesture, in ticks.
    pub writing: Vec<(AutoTarget, f64)>,
    /// The plugin parameter last moved in a plugin's own editor, for "Automate last touched".
    pub touched: Option<AutoTarget>,
    /// Filter for the plugin parameter lists in the Add lane menu.
    pub query: String,
    /// The bend handle being dragged: (lane, point the segment leaves).
    pub bend: Option<(usize, usize)>,
}

impl App {
    /// The song's lanes on the song view, the loop on screen's on the pads.
    pub fn auto_scope(&self) -> AutoScope {
        if self.view == View::Arrange { AutoScope::Song } else { AutoScope::Loop }
    }

    /// The lanes the automation tools are working on (see `auto_scope`).
    pub fn lanes(&self) -> &[AutoLane] {
        self.lanes_in(self.auto_scope())
    }

    pub fn lanes_in(&self, scope: AutoScope) -> &[AutoLane] {
        match scope {
            AutoScope::Loop => &self.scenes[self.scene].automation,
            AutoScope::Song => &self.arrangement.automation,
        }
    }

    /// Where lanes in `scope` are now, in their own ticks, while the transport runs.
    pub fn lane_now(&self, scope: AutoScope) -> Option<f64> {
        if !self.playing() {
            return None;
        }
        match scope {
            AutoScope::Loop => Some(self.shared.tick().rem_euclid((self.scenes[self.scene].bars * TICKS_PER_BAR).max(1) as f64)),
            AutoScope::Song => self.song_mode.then(|| self.song_pos()),
        }
    }

    pub fn is_automated(&self, target: AutoTarget) -> bool {
        self.lanes().iter().any(|l| l.target == target && !l.points.is_empty())
    }

    /// Sends the engine the playing loop's lanes.
    pub fn push_automation(&mut self) {
        self.arr.dirty = true;
        let lanes = &self.scenes[self.scene].automation;
        let automation = (!lanes.is_empty()).then(|| Arc::new(lanes.clone()));
        self.send(Cmd::SetAutomation(automation));
    }

    /// A loop's lanes, for handing to the engine with the loop.
    pub(super) fn automation_of(&self, scene: usize) -> Option<crate::engine::Automation> {
        let lanes = &self.scenes[scene].automation;
        (!lanes.is_empty()).then(|| Arc::new(lanes.clone()))
    }

    /// Changes the lanes the automation tools are working on, and tells the engine.
    pub fn edit_lanes(&mut self, f: impl FnOnce(&mut Vec<AutoLane>)) {
        self.edit_lanes_in(self.auto_scope(), f);
    }

    pub fn edit_lanes_in(&mut self, scope: AutoScope, f: impl FnOnce(&mut Vec<AutoLane>)) {
        match scope {
            AutoScope::Loop => {
                f(&mut self.scenes[self.scene].automation);
                self.push_automation();
            }
            // The engine hears of it with the rest of the arrangement.
            AutoScope::Song => f(&mut self.arrangement.automation),
        }
    }

    /// Adds a lane for `target` (if there is none), starting flat at the control's current
    /// value, and shows it: in the loop on screen, or on the song view in the song.
    pub fn automate(&mut self, target: AutoTarget) {
        if !self.lanes().iter().any(|l| l.target == target) {
            let value = self.control_value(target);
            self.edit_lanes(|lanes| lanes.push(AutoLane { target, points: vec![AutoPoint::new(0, value)] }));
        }
        let name = self.target_name(target);
        match self.auto_scope() {
            AutoScope::Loop => {
                self.tab = Tab::Auto;
                self.status = format!("Automating {name} in this loop. Draw it here, or turn it with Rec on while playing.");
            }
            AutoScope::Song => {
                self.arr.show_auto = true;
                self.status = format!("Automating {name} in the song. Draw it under the timeline, or turn it with Rec on while the song plays.");
            }
        }
    }

    /// The control's value as set by hand, as a lane value.
    pub fn control_value(&self, target: AutoTarget) -> f32 {
        let v = match target {
            AutoTarget::BusParam { bus, slot, param } => {
                let s = &self.buses[bus as usize].slots[slot as usize];
                if param < 3 { s.params[param as usize] } else { s.sliders[(param as usize - 3).min(1)] }
            }
            AutoTarget::BusLevel(b) => self.buses[b as usize].level,
            AutoTarget::BusPan(b) => self.buses[b as usize].pan,
            AutoTarget::PadVol(p) => self.pads[p as usize].cfg.vol,
            AutoTarget::PadPan(p) => self.pads[p as usize].cfg.pan,
            AutoTarget::PadCutoff(p) => self.pads[p as usize].cfg.cutoff,
            AutoTarget::DrumMacro { pad, m } => self.pads[pad as usize].cfg.drum.m[(m as usize).min(7)],
            AutoTarget::PadPlugin { id, .. } | AutoTarget::FxPlugin { id, .. } => {
                self.plugin_of(target).and_then(|s| s.handle.parameter(id)).unwrap_or(0.5) as f32
            }
        };
        target.scale_down(v)
    }

    /// The plugin a plugin-parameter lane moves.
    pub fn plugin_of(&self, target: AutoTarget) -> Option<&crate::model::PluginSlot> {
        match target {
            AutoTarget::PadPlugin { pad, .. } => self.plugin_slot(PluginTarget::Pad(pad as usize)),
            AutoTarget::FxPlugin { slot, .. } => self.plugin_slot(PluginTarget::Fx(slot as usize / 3, slot as usize % 3)),
            _ => None,
        }
    }

    /// The lane target for parameter `param` of the plugin instance `instance`.
    fn plugin_target(&self, instance: usize, param: u32) -> Option<AutoTarget> {
        self.automatable_plugins().into_iter().find(|(_, s)| s.id() == instance).and_then(|(at, _)| at.param_target(param))
    }

    /// Moves made in plugin editors since the last frame: the last one is remembered for
    /// "Automate last touched", and with Rec on and the transport running they are recorded.
    pub(super) fn plugin_edits(&mut self, edits: Vec<(usize, Edit)>) {
        for (instance, edit) in edits {
            match edit {
                Edit::Value(param, v) => {
                    let Some(target) = self.plugin_target(instance, param) else { continue };
                    self.auto.touched = Some(target);
                    self.record_control(target, v as f32);
                }
                Edit::End(param) => {
                    if let Some(target) = self.plugin_target(instance, param) {
                        self.auto.writing.retain(|(t, _)| *t != target);
                    }
                }
            }
        }
    }

    /// "Bus 1 · FET Comp · Thresh", "A03 Snare · Vol".
    pub fn target_name(&self, target: AutoTarget) -> String {
        match target {
            AutoTarget::BusParam { bus, slot, param } => {
                let kind = self.buses[bus as usize].slots[slot as usize].kind;
                let name = if param < 3 {
                    kind.param_names()[param as usize]
                } else {
                    kind.slider_names().map_or("", |n| n[(param as usize - 3).min(1)])
                };
                format!("{} · {} · {name}", crate::shared::bus_label(bus as usize), kind.name())
            }
            AutoTarget::BusLevel(b) => format!("{} · Level", crate::shared::bus_label(b as usize)),
            AutoTarget::BusPan(b) => format!("{} · Pan", crate::shared::bus_label(b as usize)),
            AutoTarget::PadVol(p) | AutoTarget::PadPan(p) | AutoTarget::PadCutoff(p) => {
                let what = match target {
                    AutoTarget::PadVol(_) => "Vol",
                    AutoTarget::PadPan(_) => "Pan",
                    _ => "Cutoff",
                };
                format!("{} · {what}", self.pad_title(p as usize))
            }
            AutoTarget::DrumMacro { pad, m } => {
                let names = self.pads[pad as usize].cfg.drum.engine.macros();
                format!("{} · {}", self.pad_title(pad as usize), names[(m as usize).min(7)])
            }
            AutoTarget::PadPlugin { pad, id } => {
                let slot = self.plugin_of(target);
                let param = slot.and_then(|s| s.param_name(id)).map_or_else(|| format!("#{id}"), str::to_string);
                format!("{} · {} · {param}", pad_label(pad as usize), slot.map_or("no plugin", |s| s.name.as_str()))
            }
            AutoTarget::FxPlugin { slot: at, id } => {
                let slot = self.plugin_of(target);
                let param = slot.and_then(|s| s.param_name(id)).map_or_else(|| format!("#{id}"), str::to_string);
                let bus = crate::shared::bus_label(at as usize / 3);
                format!("{bus} · {} · {param}", slot.map_or("no plugin", |s| s.name.as_str()))
            }
        }
    }

    /// "A03 Snare", or just "A03" for a pad without a name.
    fn pad_title(&self, p: usize) -> String {
        let name = &self.pads[p].name;
        format!("{}{}{}", pad_label(p), if name.is_empty() { "" } else { " " }, name)
    }

    /// How a lane's value reads on its control: "-3.2 dB", "L40", "72%".
    pub fn target_value_text(&self, target: AutoTarget, v: f32) -> String {
        let raw = target.scale_up(v);
        match target {
            AutoTarget::BusLevel(_) | AutoTarget::PadVol(_) => {
                if raw < 1e-4 { "-inf".into() } else { format!("{:+.1} dB", 20.0 * raw.log10()) }
            }
            AutoTarget::BusPan(_) | AutoTarget::PadPan(_) => crate::ui::pan_text(raw),
            AutoTarget::PadPlugin { id, .. } | AutoTarget::FxPlugin { id, .. } => self
                .plugin_of(target)
                .and_then(|s| s.handle.format(id, raw as f64))
                .unwrap_or_else(|| format!("{:.0}%", raw * 100.0)),
            _ => format!("{:.0}%", raw * 100.0),
        }
    }

    /// Records a control's move into its lane while the transport runs with Rec on: the
    /// stretch of the lane the playhead has crossed since the last write is replaced.
    /// Writes go into what is playing: the song's lanes while the song plays, otherwise
    /// the loop's.
    pub fn record_control(&mut self, target: AutoTarget, raw: f32) {
        self.perf_control(target, target.scale_down(raw));
        if !self.auto.rec || !self.playing() {
            return;
        }
        let scope = if self.song_mode { AutoScope::Song } else { AutoScope::Loop };
        let Some(now) = self.lane_now(scope) else { return };
        let value = target.scale_down(raw);
        // In the song, going back round the loop brace is a jump, not a lap: nothing
        // between is overwritten.
        let since = self.auto.writing.iter().find(|(t, _)| *t == target).map(|(_, at)| *at).filter(|from| scope == AutoScope::Loop || *from <= now);
        self.edit_lanes_in(scope, |lanes| {
            let lane = match lanes.iter().position(|l| l.target == target) {
                Some(i) => &mut lanes[i],
                None => {
                    lanes.push(AutoLane { target, points: Vec::new() });
                    lanes.last_mut().expect("just pushed")
                }
            };
            if let Some(from) = since {
                // The playhead may have gone round the loop since the last write.
                lane.points.retain(|p| {
                    let t = p.tick as f64;
                    if from <= now { t <= from || t > now } else { t <= from && t > now }
                });
            }
            let at = lane.points.partition_point(|p| (p.tick as f64) < now);
            lane.points.insert(at, AutoPoint::new(now as u32, value));
        });
        match self.auto.writing.iter_mut().find(|(t, _)| *t == target) {
            Some(w) => w.1 = now,
            None => self.auto.writing.push((target, now)),
        }
    }

    /// Ends every recording gesture; called when the mouse button lifts.
    pub fn end_recording(&mut self) {
        self.auto.writing.clear();
    }
}
