//! Undo and redo for the project.
//!
//! Rather than have every action record how to reverse itself, the whole project state is
//! compared with the last committed snapshot once per frame. When it differs and the user
//! has let go (no mouse button down, no text being typed), the old snapshot goes on the
//! undo stack. A knob drag or a typed name is therefore one step, and nothing can be
//! forgotten. Sample buffers and plugins are shared by reference, so snapshots are small.

use super::*;
use crate::engine::Pattern;

const MAX_STEPS: usize = 64;

/// Everything undo covers. Audio device state, selection and view are deliberately left out.
#[derive(Clone)]
pub struct Snapshot {
    pads: Vec<PadModel>,
    banks: usize,
    bpm: f32,
    swing: f32,
    key: Key,
    buses: [BusCfg; NUM_BUSES],
    fx_plugins: Vec<Option<PluginSlot>>,
    /// Every loop with its patterns. The pads above carry none, so that moving between
    /// loops, which only changes where the patterns are kept, is not an edit.
    scenes: Vec<Scene>,
    chain: Vec<usize>,
    arrangement: Arrangement,
    vocal_fx: crate::engine::VocalFx,
}

impl Snapshot {
    fn fx_ids(&self) -> Vec<Option<usize>> {
        self.fx_plugins.iter().map(|s| s.as_ref().map(PluginSlot::id)).collect()
    }
}

#[derive(Default)]
pub struct History {
    committed: Option<Snapshot>,
    /// The state when the project was last saved, opened or started.
    saved: Option<Snapshot>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl History {
    /// Counts the project as unsaved, as one carried across a restart may be.
    pub fn forget_saved(&mut self) {
        self.saved = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

impl App {
    fn snapshot(&self) -> Snapshot {
        let mut pads = self.pads.clone();
        for p in pads.iter_mut() {
            p.pattern = Pattern::default();
        }
        Snapshot {
            pads,
            scenes: self.scenes_complete(),
            chain: self.chain.clone(),
            arrangement: self.arrangement.clone(),
            vocal_fx: self.vocal_fx,
            banks: self.banks,
            bpm: self.bpm,
            swing: self.swing,
            key: self.key,
            buses: self.buses,
            fx_plugins: self.fx_plugins.clone(),
        }
    }

    fn differs_from(&self, s: &Snapshot) -> bool {
        self.banks != s.banks
            || self.bpm != s.bpm
            || self.swing != s.swing
            || self.key != s.key
            || self.buses != s.buses
            || self.pads.len() != s.pads.len()
            || self.fx_plugins.iter().map(|p| p.as_ref().map(PluginSlot::id)).ne(s.fx_ids())
            || self.chain != s.chain
            || self.arrangement != s.arrangement
            || self.vocal_fx != s.vocal_fx
            || self.scenes.len() != s.scenes.len()
            || self.pads.iter().zip(&s.pads).any(|(a, b)| !a.same_as(b, false))
            || self.scenes.iter().zip(&s.scenes).enumerate().any(|(i, (a, b))| {
                if a.bars != b.bars || a.name != b.name || a.automation != b.automation {
                    true
                } else if i == self.scene {
                    !self.live_patterns().eq(b.patterns.iter().map(|(i, p)| (*i, p)))
                } else {
                    a.patterns != b.patterns
                }
            })
    }

    /// Forgets all history and treats the current state as the starting point.
    pub fn reset_history(&mut self) {
        let now = self.snapshot();
        self.history = History { committed: Some(now.clone()), saved: Some(now), ..History::default() };
    }

    /// Records the project as it is now as saved.
    pub fn mark_saved(&mut self) {
        self.history.saved = Some(self.snapshot());
    }

    /// True if the project has changed since it was last saved, opened or started.
    pub fn has_unsaved_changes(&self) -> bool {
        self.history.saved.as_ref().is_none_or(|s| self.differs_from(s))
    }

    /// Called once per frame, after the UI has run.
    pub(super) fn track_history(&mut self, ctx: &egui::Context) {
        let Some(committed) = &self.history.committed else {
            self.reset_history();
            return;
        };
        // Wait until a gesture is finished so it lands as a single step.
        if ctx.input(|i| i.pointer.any_down()) || ctx.text_edit_focused() || !self.differs_from(committed) {
            return;
        }
        let now = self.snapshot();
        let before = self.history.committed.replace(now).unwrap();
        self.history.undo.push(before);
        if self.history.undo.len() > MAX_STEPS {
            self.history.undo.remove(0);
        }
        self.history.redo.clear();
    }

    pub fn undo(&mut self) {
        let Some(target) = self.history.undo.pop() else {
            self.status = "Nothing to undo".into();
            return;
        };
        let current = self.snapshot();
        self.history.redo.push(current);
        self.restore(target);
        self.status = format!("Undo ({} more)", self.history.undo.len());
    }

    pub fn redo(&mut self) {
        let Some(target) = self.history.redo.pop() else {
            self.status = "Nothing to redo".into();
            return;
        };
        let current = self.snapshot();
        self.history.undo.push(current);
        self.restore(target);
        self.status = format!("Redo ({} more)", self.history.redo.len());
    }

    /// Makes `s` the project state and brings the engine in line, touching only what changed.
    fn restore(&mut self, s: Snapshot) {
        let old_len = self.pads.len();
        let changed: Vec<usize> = (0..s.pads.len().max(old_len))
            .filter(|&i| match (self.pads.get(i), s.pads.get(i)) {
                (Some(a), Some(b)) => !a.same_as(b, false),
                _ => true,
            })
            .collect();
        let fx_changed: Vec<usize> = {
            let (old, new) = (self.snapshot().fx_ids(), s.fx_ids());
            (0..old.len()).filter(|&i| old[i] != new[i]).collect()
        };
        self.history.committed = Some(s.clone());
        let old_patterns: Vec<Pattern> = self.pads.iter().map(|p| p.pattern.clone()).collect();
        if self.queued_scene.take().is_some() {
            self.send(Cmd::QueueScene(None));
        }
        self.scenes = s.scenes;
        self.chain = s.chain;
        self.arrangement = s.arrangement;
        self.arr.selected.clear();
        self.arr.section = None;
        self.vocal_fx = s.vocal_fx;
        self.vocal.selected = None;
        self.chain_pos = 0;
        // Which loop is on screen is a matter of view; stay on it if it still exists.
        self.scene = self.scene.min(self.scenes.len() - 1);
        self.shared.scene_index.store(self.scene as u32, Ordering::Relaxed);
        self.pads = s.pads;
        for (i, p) in self.scenes[self.scene].patterns.clone() {
            if let Some(pad) = self.pads.get_mut(i) {
                pad.pattern = p;
            }
        }
        self.banks = s.banks;
        self.bpm = s.bpm;
        self.swing = s.swing;
        self.key = s.key;
        self.buses = s.buses;
        self.fx_plugins = s.fx_plugins;

        for i in 0..self.pads.len() {
            if !changed.contains(&i) && old_patterns.get(i) != Some(&self.pads[i].pattern) {
                self.push_pattern(i);
            }
        }
        for i in changed {
            if i < self.pads.len() {
                self.resync_pad(i);
            } else {
                self.reset_engine_pad(i);
            }
        }
        self.push_scene_len();
        for i in fx_changed {
            let plugin = self.fx_plugins[i].as_ref().map(|slot| self.rt_for(slot));
            self.send(Cmd::SetFxPlugin { slot: i as u8, plugin });
        }
        for b in 0..NUM_BUSES {
            self.push_bus(b);
        }
        self.push_transport();
        self.selected = self.selected.min(self.pads.len() - 1);
        self.bank = self.bank.min(self.banks - 1);
        self.roll.drag = None;
    }
}
