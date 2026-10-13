//! Loops: up to sixteen sets of patterns over the same pads, launched one after another by
//! hand or from a chain.
//!
//! The pads always hold the patterns of the loop on screen, which is also the loop playing.
//! The other loops' patterns wait in `App::scenes`. When the transport is running, a change
//! of loop is handed to the engine in advance and happens there, on the beat; the engine
//! reports it and the model follows.

use super::*;
use crate::engine::{Pattern, SceneSwap};
use serde::{Deserialize, Serialize};

pub const MAX_SCENES: usize = 16;
pub const MAX_CHAIN: usize = 64;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub name: String,
    /// Length in bars, 1 to 16. Shorter patterns repeat inside it.
    pub bars: u32,
    /// Patterns by pad index, in order. Out of date for the loop on screen, whose
    /// patterns live on the pads until another loop takes its place.
    pub patterns: Vec<(usize, Pattern)>,
}

impl Scene {
    pub fn new(bars: u32) -> Scene {
        Scene { name: String::new(), bars: bars.clamp(1, 16), patterns: Vec::new() }
    }

    fn pattern(&self, pad: usize) -> Pattern {
        self.patterns.iter().find(|(i, _)| *i == pad).map(|(_, p)| p.clone()).unwrap_or_default()
    }
}

/// Whether a pad with this pattern is sequenced at all.
fn sequenced(pad: &PadModel, pattern: &Pattern) -> bool {
    pad.cfg.kind == PadKind::Midi || !pattern.notes.is_empty()
}

impl App {
    /// The patterns on the pads, as they would be stored in a loop.
    pub fn live_patterns(&self) -> impl Iterator<Item = (usize, &Pattern)> {
        self.pads.iter().enumerate().filter(|(_, p)| p.pattern != Pattern::default()).map(|(i, p)| (i, &p.pattern))
    }

    /// Copies the pads' patterns into the loop they belong to.
    pub fn stash_scene(&mut self) {
        let patterns = self.live_patterns().map(|(i, p)| (i, p.clone())).collect();
        self.scenes[self.scene].patterns = patterns;
    }

    /// Every loop complete with its patterns, including the one whose patterns are on
    /// the pads: the form projects and undo snapshots are stored in.
    pub fn scenes_complete(&self) -> Vec<Scene> {
        let mut scenes = self.scenes.clone();
        scenes[self.scene].patterns = self.live_patterns().map(|(i, p)| (i, p.clone())).collect();
        scenes
    }

    fn scene_ticks(&self, scene: usize) -> u32 {
        self.scenes[scene].bars * TICKS_PER_BAR
    }

    pub fn push_scene_len(&mut self) {
        self.send(Cmd::SceneLen(self.scene_ticks(self.scene)));
    }

    /// Puts loop `to` on the pads. `push` is false when the engine made the switch itself
    /// and already holds these patterns.
    pub fn show_scene(&mut self, to: usize, push: bool) {
        if to >= self.scenes.len() {
            return;
        }
        self.stash_scene();
        self.scene = to;
        for i in 0..self.pads.len() {
            let pattern = self.scenes[to].pattern(i);
            if self.pads[i].pattern != pattern {
                self.pads[i].pattern = pattern;
                if push {
                    self.push_pattern(i);
                }
            }
        }
        if push {
            self.push_scene_len();
            // The engine only reports loops it switched to itself.
            self.shared.scene_index.store(to as u32, Ordering::Relaxed);
        }
        self.roll.sel.clear();
        self.roll.drag = None;
    }

    /// Switches to a loop: at once when stopped, otherwise when the playing loop ends (or
    /// on the next bar line, if that is how launching is set).
    pub fn launch_scene(&mut self, to: usize) {
        if to >= self.scenes.len() {
            return;
        }
        if !self.playing() {
            self.show_scene(to, true);
            return;
        }
        if to == self.scene {
            // Already playing: this just cancels whatever was lined up.
            if self.queued_scene.take().is_some() {
                self.send(Cmd::QueueScene(None));
            }
            return;
        }
        self.queue_scene(to);
    }

    /// Hands the engine loop `to`, to bring in when the playing loop allows.
    fn queue_scene(&mut self, to: usize) {
        let incoming = &self.scenes[to];
        let mut patterns = Vec::new();
        for (i, pad) in self.pads.iter().enumerate() {
            let next = incoming.pattern(i);
            let (now, then) = (sequenced(pad, &pad.pattern), sequenced(pad, &next));
            if now || then {
                patterns.push((i as u16, then.then(|| Arc::new(next))));
            }
        }
        let swap = SceneSwap { index: to as u32, len_ticks: self.scene_ticks(to), next_bar: self.launch_on_bar, patterns };
        self.send(Cmd::QueueScene(Some(Box::new(swap))));
        self.queued_scene = Some(to);
    }

    /// Called every frame: follows the engine through loop changes and feeds it the chain.
    pub(super) fn poll_scenes(&mut self) {
        self.shared.ui_perform.store(self.perform, Ordering::Relaxed);
        let starts = self.shared.scene_starts.load(Ordering::Relaxed);
        if starts == self.seen_starts {
            return;
        }
        self.seen_starts = starts;
        let playing = self.shared.scene_index.load(Ordering::Relaxed) as usize;
        if playing != self.scene && playing < self.scenes.len() {
            self.show_scene(playing, false);
        }
        if self.queued_scene == Some(self.scene) {
            self.queued_scene = None;
        }
        if self.chain_on && !self.chain.is_empty() && self.playing() {
            self.chain_pos = (self.chain_pos + 1) % self.chain.len();
            self.queue_chain_step();
        }
    }

    /// Lines up the chain entry after the one playing, unless something was launched by hand.
    fn queue_chain_step(&mut self) {
        if self.queued_scene.is_some() || self.chain.is_empty() {
            return;
        }
        let next = self.chain[(self.chain_pos + 1) % self.chain.len()];
        if next != self.scene {
            self.queue_scene(next);
        }
    }

    /// Start: with the chain on, playback begins at its first entry.
    pub(super) fn scenes_on_play(&mut self) {
        self.seen_starts = self.shared.scene_starts.load(Ordering::Relaxed);
        self.queued_scene = None;
        if self.chain_on && !self.chain.is_empty() {
            self.chain_pos = 0;
            self.show_scene(self.chain[0], true);
        }
        self.push_scene_len();
    }

    /// Called right after the engine has been told to play.
    pub(super) fn scenes_after_play(&mut self) {
        if self.chain_on {
            let next = self.chain.get(1 % self.chain.len().max(1)).copied();
            if let Some(next) = next.filter(|n| *n != self.scene) {
                self.queue_scene(next);
            }
        }
    }

    pub(super) fn scenes_on_stop(&mut self) {
        if self.queued_scene.take().is_some() {
            self.send(Cmd::QueueScene(None));
        }
    }

    pub fn set_scene_bars(&mut self, bars: u32) {
        self.scenes[self.scene].bars = bars.clamp(1, 16);
        self.push_scene_len();
    }

    /// A pattern longer than its loop would never be heard to the end, so the loop grows.
    pub(super) fn fit_scene_to(&mut self, pattern_ticks: u32) {
        let bars = pattern_ticks.div_ceil(TICKS_PER_BAR).min(16);
        if bars > self.scenes[self.scene].bars {
            self.set_scene_bars(bars);
        }
    }

    /// Adds a loop after the last one: empty, or a copy of the one on screen.
    pub fn add_scene(&mut self, copy: bool) -> Option<usize> {
        if self.scenes.len() >= MAX_SCENES {
            self.status = format!("A project holds up to {MAX_SCENES} loops");
            return None;
        }
        self.stash_scene();
        let mut scene = Scene::new(self.scenes[self.scene].bars);
        if copy {
            scene.patterns = self.scenes[self.scene].patterns.clone();
        }
        self.scenes.push(scene);
        Some(self.scenes.len() - 1)
    }

    pub fn clear_scene(&mut self) {
        for i in 0..self.pads.len() {
            if !self.pads[i].pattern.notes.is_empty() {
                self.pads[i].pattern.notes.clear();
                self.push_pattern(i);
            }
        }
    }

    pub fn remove_scene(&mut self, at: usize) {
        if self.scenes.len() <= 1 || at >= self.scenes.len() {
            return;
        }
        if self.queued_scene.take().is_some() {
            self.send(Cmd::QueueScene(None));
        }
        if at == self.scene {
            // Move off it first, so its patterns are not written back.
            let to = if at == 0 { 1 } else { at - 1 };
            self.show_scene(to, true);
        }
        self.scenes.remove(at);
        if self.scene > at {
            self.scene -= 1;
        }
        self.chain.retain(|s| *s != at);
        for s in self.chain.iter_mut().filter(|s| **s > at) {
            *s -= 1;
        }
        self.chain_pos = 0;
        self.shared.scene_index.store(self.scene as u32, Ordering::Relaxed);
    }

    pub fn chain_push(&mut self, scene: usize) {
        if self.chain.len() < MAX_CHAIN && scene < self.scenes.len() {
            self.chain.push(scene);
        }
    }

    pub fn scene_label(&self, scene: usize) -> String {
        match self.scenes.get(scene) {
            Some(s) if !s.name.is_empty() => s.name.clone(),
            _ => format!("Loop {}", scene + 1),
        }
    }

    /// Pads with notes in a loop, for the launcher.
    pub fn scene_pad_count(&self, scene: usize) -> usize {
        if scene == self.scene {
            self.pads.iter().filter(|p| !p.pattern.notes.is_empty()).count()
        } else {
            self.scenes[scene].patterns.iter().filter(|(_, p)| !p.notes.is_empty()).count()
        }
    }
}
