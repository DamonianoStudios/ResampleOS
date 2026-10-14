//! Automation: lanes that move bus and pad controls through a loop.
//!
//! Each loop has its own lanes, kept on the loop itself, so they switch with it on the beat
//! and are saved and undone with it. A lane is drawn in the Automation tab, or recorded by
//! turning a control while the transport runs with Rec on. The engine plays the lanes of
//! the loop that is playing on top of the controls as they were set by hand, and lets go
//! of a control as soon as nothing automates it.

use super::*;
use crate::engine::{AutoLane, AutoPoint, AutoTarget};

/// Grid lane points are placed on: (name, ticks). 1 or less is free.
pub const AUTO_SNAPS: [(&str, u32); 5] = [("Off", 1), ("1/4", PPQ), ("1/8", PPQ / 2), ("1/16", PPQ / 4), ("1/32", PPQ / 8)];

#[derive(Default)]
pub struct AutoUi {
    /// Record moves of automatable controls while the transport runs.
    pub rec: bool,
    pub snap: usize,
    /// The point being dragged: (lane, point).
    pub drag: Option<(usize, usize)>,
    /// Where each control being recorded was last written this gesture, in ticks.
    pub writing: Vec<(AutoTarget, f64)>,
}

impl App {
    /// The lanes of the loop on screen.
    pub fn lanes(&self) -> &[AutoLane] {
        &self.scenes[self.scene].automation
    }

    pub fn is_automated(&self, target: AutoTarget) -> bool {
        self.lanes().iter().any(|l| l.target == target && !l.points.is_empty())
    }

    /// Sends the engine the playing loop's lanes.
    pub fn push_automation(&mut self) {
        let lanes = &self.scenes[self.scene].automation;
        let automation = (!lanes.is_empty()).then(|| Arc::new(lanes.clone()));
        self.send(Cmd::SetAutomation(automation));
    }

    /// A loop's lanes, for handing to the engine with the loop.
    pub(super) fn automation_of(&self, scene: usize) -> Option<crate::engine::Automation> {
        let lanes = &self.scenes[scene].automation;
        (!lanes.is_empty()).then(|| Arc::new(lanes.clone()))
    }

    /// Changes the lanes of the loop on screen and tells the engine.
    pub fn edit_lanes(&mut self, f: impl FnOnce(&mut Vec<AutoLane>)) {
        f(&mut self.scenes[self.scene].automation);
        self.push_automation();
    }

    /// Adds a lane for `target` to the loop on screen (if it has none), starting flat at
    /// the control's current value, and opens the Automation tab on it.
    pub fn automate(&mut self, target: AutoTarget) {
        if !self.lanes().iter().any(|l| l.target == target) {
            let value = self.control_value(target);
            self.edit_lanes(|lanes| lanes.push(AutoLane { target, points: vec![AutoPoint { tick: 0, value }] }));
        }
        self.tab = Tab::Auto;
        self.status = format!("Automating {} in this loop. Draw it here, or turn it with Rec on while playing.", self.target_name(target));
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
        };
        target.scale_down(v)
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
                let name = &self.pads[p as usize].name;
                format!("{}{}{} · {what}", pad_label(p as usize), if name.is_empty() { "" } else { " " }, name)
            }
        }
    }

    /// How a lane's value reads on its control: "-3.2 dB", "L40", "72%".
    pub fn target_value_text(&self, target: AutoTarget, v: f32) -> String {
        let raw = target.scale_up(v);
        match target {
            AutoTarget::BusLevel(_) | AutoTarget::PadVol(_) => {
                if raw < 1e-4 { "-inf".into() } else { format!("{:+.1} dB", 20.0 * raw.log10()) }
            }
            AutoTarget::BusPan(_) | AutoTarget::PadPan(_) => crate::ui::pan_text(raw),
            _ => format!("{:.0}%", raw * 100.0),
        }
    }

    /// Records a control's move into its lane while the transport runs with Rec on: the
    /// stretch of the lane the playhead has crossed since the last write is replaced.
    pub fn record_control(&mut self, target: AutoTarget, raw: f32) {
        if !self.auto.rec || !self.playing() {
            return;
        }
        let len = self.scenes[self.scene].bars * TICKS_PER_BAR;
        let now = self.shared.tick().rem_euclid(len.max(1) as f64);
        let value = target.scale_down(raw);
        let since = self.auto.writing.iter().find(|(t, _)| *t == target).map(|(_, at)| *at);
        self.edit_lanes(|lanes| {
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
            lane.points.insert(at, AutoPoint { tick: now as u32, value });
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
