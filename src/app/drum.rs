//! Drum synth pads: making them, loading kits, baking hits into takes and getting the
//! live synth back from a baked take.

use super::*;
use crate::drum::{self, DrumParams};

/// Which layer of the macros the knobs edit.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum MacroLayer {
    #[default]
    Value,
    Velocity,
    Random,
}

#[derive(Default)]
pub struct DrumUi {
    pub layer: MacroLayer,
    /// Run the pad's bus effects into a bake.
    pub bake_fx: bool,
    /// The patch last drawn on the envelope display and its render, so it is only
    /// rendered again when something changes.
    pub preview: Option<(DrumParams, Arc<SampleData>)>,
    /// The step whose locks the step sequencer is editing: the pad, and the ticks the
    /// step covers.
    pub lock_step: Option<(usize, u32, u32)>,
}

impl App {
    pub fn is_drum(&self, pad: usize) -> bool {
        self.pads.get(pad).is_some_and(|p| p.cfg.kind == PadKind::Drum)
    }

    /// Turns a pad into a drum synth pad, with a factory sound or the default kick.
    pub fn make_drum(&mut self, idx: usize, preset: Option<(&str, DrumParams)>) {
        if self.is_vocal(idx) {
            self.pads[idx].vocal = None;
        }
        let p = &mut self.pads[idx];
        p.cfg.kind = PadKind::Drum;
        match preset {
            Some((name, params)) => {
                p.cfg.drum = params;
                if p.name.is_empty() || p.takes.is_empty() {
                    p.name = name.into();
                }
            }
            None if p.name.is_empty() => p.name = "Drum".into(),
            None => {}
        }
        self.push_pad(idx);
        self.push_pattern(idx);
        if idx == self.selected && matches!(self.tab, Tab::Wave | Tab::Vocal) {
            self.tab = Tab::Drum;
        }
    }

    /// Fills a bank with a drum kit: the selected bank if it is empty, otherwise the next
    /// empty one (adding a bank if there is none).
    pub fn load_kit(&mut self, kit: usize) {
        let kits = drum::kits();
        let Some(kit) = kits.get(kit) else { return };
        let empty = |app: &App, b: usize| (0..16).all(|i| app.pads[b * 16 + i].is_empty() && app.pads[b * 16 + i].pattern.notes.is_empty());
        let bank = match (self.bank..self.banks).find(|&b| empty(self, b)) {
            Some(b) => b,
            None if self.add_bank() => self.banks - 1,
            None => {
                self.status = "No empty bank for the kit, and no room for another".into();
                return;
            }
        };
        // The hats share a choke group nothing else in the project uses.
        let used: Vec<u8> = self.pads.iter().map(|p| p.cfg.choke).collect();
        let group = (1..=32u8).find(|g| !used.contains(g)).unwrap_or(32);
        for (i, name) in kit.pads.iter().enumerate() {
            let idx = bank * 16 + i;
            let Some(mut params) = drum::preset_named(name) else { continue };
            (kit.flavour)(&mut params);
            self.pads[idx] = PadModel::default();
            self.pads[idx].name = (*name).into();
            self.pads[idx].cfg.choke = if drum::is_hat(name) { group } else { 0 };
            self.make_drum(idx, Some((name, params)));
        }
        self.bank = bank;
        self.select(bank * 16);
        self.tab = Tab::Drum;
        self.status = format!("{} kit on bank {}", kit.name, crate::music::bank_name(bank));
    }

    /// The patch that made the pad's active take, if a drum synth made it.
    pub fn take_patch(&self, idx: usize) -> Option<DrumParams> {
        let p = &self.pads[idx];
        let take = p.sample()?;
        p.drum_patches.iter().find(|(s, _)| Arc::ptr_eq(s, take)).map(|(_, d)| *d)
    }

    /// Renders a drum patch to a take on its pad, which then plays the take. With
    /// `mutate`, the patch is varied first. The synth comes back with [`App::drum_resynth`]
    /// or by stepping back past the first take.
    pub fn drum_bake(&mut self, idx: usize, mutate: bool) {
        let mut params = if self.is_drum(idx) { self.pads[idx].cfg.drum } else if let Some(p) = self.take_patch(idx) { p } else { return };
        if mutate {
            let seed = (self.shared.ring.written() as u32) ^ (idx as u32).wrapping_mul(2_654_435_761);
            params = params.mutate(0.14, seed);
        }
        let sr = self.sr();
        let mut audio = drum::render(&params, sr, self.key.mask(), self.bpm);
        if self.drum.bake_fx {
            audio = self.through_bus(crate::shared::bus_index(self.pads[idx].cfg.bus), &audio);
            // The effects are in the sound now; going through them again would double them.
            self.pads[idx].cfg.bus = 0;
        }
        self.pads[idx].cfg.drum = params;
        self.set_sample(idx, audio, None, None);
        let p = &mut self.pads[idx];
        let take = p.sample().cloned().expect("just set");
        // Forget the patches of takes that have been dropped from the stack.
        let takes = &p.takes;
        p.drum_patches.retain(|(s, _)| takes.iter().any(|t| Arc::ptr_eq(t, s)));
        p.drum_patches.push((take, params));
        let what = if mutate { "Mutated and baked" } else { "Baked" };
        self.status = format!("{what} {} to take {}/{}", pad_label(idx), p.take_idx + 1, p.takes.len());
    }

    /// Brings back the live drum synth that made the pad's active take, knobs and all.
    pub fn drum_resynth(&mut self, idx: usize) {
        let Some(params) = self.take_patch(idx) else {
            self.status = "This take was not made by the drum synth".into();
            return;
        };
        self.pads[idx].cfg.drum = params;
        self.pads[idx].cfg.kind = PadKind::Drum;
        self.push_pad(idx);
        if idx == self.selected {
            self.tab = Tab::Drum;
        }
        self.status = format!("{}: drum synth back, as it was for this take", pad_label(idx));
    }

    /// Leaves the live synth for the take the pad last played.
    pub fn drum_to_take(&mut self, idx: usize, take: usize) {
        let p = &mut self.pads[idx];
        if take >= p.takes.len() {
            return;
        }
        p.take_idx = take;
        p.cfg.kind = PadKind::Audio;
        p.refresh_mini();
        let (sr, sens) = (self.sr(), self.sensitivity);
        self.pads[idx].detect_transients(sr, sens);
        self.push_pad(idx);
        self.push_sample(idx);
        if idx == self.selected && self.tab == Tab::Drum {
            self.tab = Tab::Wave;
        }
    }

    /// Sets or clears one macro locked to the step being edited.
    pub fn set_step_lock(&mut self, pad: usize, (lo, hi): (u32, u32), k: usize, value: Option<f32>) {
        let pat = &mut self.pads[pad].pattern;
        let mut changed = false;
        for n in pat.notes.iter_mut().filter(|n| n.start >= lo && n.start < hi) {
            n.locks[k] = value.map_or(drum::NO_LOCK, drum::lock_value);
            changed = true;
        }
        if changed {
            self.push_pattern(pad);
        }
    }

    pub fn clear_step_locks(&mut self, pad: usize, (lo, hi): (u32, u32)) {
        for n in self.pads[pad].pattern.notes.iter_mut().filter(|n| n.start >= lo && n.start < hi) {
            n.locks = drum::NO_LOCKS;
        }
        self.push_pattern(pad);
    }
}
