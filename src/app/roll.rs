//! Piano roll selection: the edits that act on every selected note at once.

use super::*;

impl App {
    /// Opens a pad's pattern in the piano roll: a MIDI or drum pad's own; an audio pad's
    /// steps are first moved to a MIDI pad that plays it (see `steps_to_midi`).
    pub fn open_in_roll(&mut self, idx: usize) {
        match self.pads[idx].cfg.kind {
            PadKind::Midi | PadKind::Drum => {
                self.select(idx);
                self.tab = Tab::Roll;
            }
            PadKind::Audio => self.steps_to_midi(idx),
            PadKind::Vocal => self.status = "A vocal pad's takes are edited in the Vocal tab".into(),
        }
    }

    /// An audio pad's steps, in every loop and on the song timeline, moved to a new MIDI
    /// pad that plays this pad's sample chromatically, so they can be edited in the piano
    /// roll: each step a note at the sample's own pitch, to move up and down from there.
    /// The audio pad keeps its sample and its settings; the MIDI pad plays through them.
    pub fn steps_to_midi(&mut self, idx: usize) {
        let src = &self.pads[idx];
        if src.is_empty() {
            self.status = format!("{} has no sample to play from a piano roll", pad_label(idx));
            return;
        }
        let in_loops = !src.pattern.notes.is_empty() || self.scenes.iter().enumerate().any(|(k, s)| k != self.scene && s.patterns.iter().any(|(p, pat)| *p == idx && !pat.notes.is_empty()));
        let on_song = self.arrangement.clips(idx).iter().any(|c| !c.is_audio());
        if !in_loops && !on_song {
            self.status = format!("{} has no steps to convert", pad_label(idx));
            return;
        }
        let Some(to) = self.free_track_pad() else {
            self.status = "No free pad for the MIDI pad".into();
            return;
        };
        let name = if self.pads[idx].name.is_empty() { pad_label(idx) } else { self.pads[idx].name.clone() };
        self.set_kind(to, PadKind::Midi);
        self.pads[to].name = format!("{name} keys");
        self.pads[to].cfg.target = MidiTarget::Pad(idx as u16);
        // The loop on screen's pattern lives on the pads; the other loops keep theirs.
        let len = self.pads[idx].pattern.len_ticks;
        let notes = std::mem::take(&mut self.pads[idx].pattern.notes);
        self.pads[to].pattern = crate::engine::Pattern { len_ticks: len, notes };
        let current = self.scene;
        for (k, scene) in self.scenes.iter_mut().enumerate() {
            if k == current {
                continue;
            }
            if let Some(i) = scene.patterns.iter().position(|(p, _)| *p == idx) {
                let (_, pat) = scene.patterns.remove(i);
                scene.patterns.retain(|(p, _)| *p != to);
                scene.patterns.push((to, pat));
                scene.patterns.sort_by_key(|(p, _)| *p);
            }
        }
        // The song's clips of those steps move to the new pad's track; audio clips stay.
        let moving: Vec<(usize, usize)> = self.arrangement.clips(idx).iter().enumerate().filter(|(_, c)| !c.is_audio()).map(|(i, _)| (idx, i)).collect();
        for (_, c) in self.arrangement.take(moving) {
            self.arrangement.place(to, c);
        }
        self.push_pad(to);
        self.push_pattern(idx);
        self.push_pattern(to);
        self.arr.dirty = true;
        self.select(to);
        self.tab = Tab::Roll;
        self.status = format!("{name}'s steps are now notes on {}, which plays {}: edit them in the piano roll", pad_label(to), pad_label(idx));
    }

    /// True while the piano roll is showing and has notes selected, which is when the arrow
    /// keys, Delete and Ctrl+D belong to it.
    pub fn roll_has_selection(&self) -> bool {
        self.tab == Tab::Roll && self.roll.sel_pad == self.selected && !self.roll.sel.is_empty()
    }

    fn roll_edit(&mut self, f: impl Fn(&mut Note)) {
        let pad = self.selected;
        let notes = &mut self.pads[pad].pattern.notes;
        for &i in &self.roll.sel {
            if let Some(n) = notes.get_mut(i) {
                f(n);
            }
        }
        self.push_pattern(pad);
    }

    /// Moves the selection by `steps` semitones, or scale degrees while scale lock is on.
    /// Nothing moves if any note would leave the keyboard, so chords keep their shape.
    pub fn roll_transpose(&mut self, steps: i32) {
        let (key, lock) = (self.key, self.roll.scale_lock && steps.abs() < 12);
        let shifted = |pitch: u8| -> Option<u8> {
            let mut p = pitch as i32;
            if lock {
                for _ in 0..steps.abs() {
                    p += steps.signum();
                    while (0..=127).contains(&p) && !key.contains(p) {
                        p += steps.signum();
                    }
                }
            } else {
                p += steps;
            }
            (0..=127).contains(&p).then_some(p as u8)
        };
        let notes = &self.pads[self.selected].pattern.notes;
        if self.roll.sel.iter().filter_map(|&i| notes.get(i)).any(|n| shifted(n.pitch).is_none()) {
            return;
        }
        self.roll_edit(|n| n.pitch = shifted(n.pitch).unwrap_or(n.pitch));
    }

    /// Moves the selection in time by `dir` grid steps.
    pub fn roll_nudge(&mut self, dir: i64) {
        let snap = SNAPS[self.roll.snap].1;
        let by = dir * if snap > 1 { snap as i64 } else { 10 };
        let pat = &self.pads[self.selected].pattern;
        let len = pat.len_ticks.max(1) as i64;
        let fits = |n: &Note| (0..len).contains(&(n.start as i64 + by));
        if !self.roll.sel.iter().filter_map(|&i| pat.notes.get(i)).all(fits) {
            return;
        }
        self.roll_edit(|n| n.start = (n.start as i64 + by) as u32);
    }

    /// Repeats the selection directly after itself and selects the copies.
    pub fn roll_duplicate(&mut self) {
        let pad = self.selected;
        let snap = SNAPS[self.roll.snap].1.max(1);
        let pat = &mut self.pads[pad].pattern;
        let picked: Vec<Note> = self.roll.sel.iter().filter_map(|&i| pat.notes.get(i).copied()).collect();
        let Some(first) = picked.iter().map(|n| n.start).min() else { return };
        let last = picked.iter().map(|n| n.start + n.len).max().unwrap_or(first);
        // Round the span up to the grid so repeated duplicates stay in time.
        let span = (last - first).div_ceil(snap).max(1) * snap;
        self.roll.sel.clear();
        for n in picked {
            if n.start + span < pat.len_ticks {
                self.roll.sel.push(pat.notes.len());
                pat.notes.push(Note { start: n.start + span, ..n });
            }
        }
        self.push_pattern(pad);
    }

    pub fn roll_delete_selected(&mut self) {
        let pad = self.selected;
        let sel = std::mem::take(&mut self.roll.sel);
        let mut i = 0;
        self.pads[pad].pattern.notes.retain(|_| {
            i += 1;
            !sel.contains(&(i - 1))
        });
        self.push_pattern(pad);
    }

    pub fn roll_select_all(&mut self) {
        self.roll.sel_pad = self.selected;
        self.roll.sel = (0..self.pads[self.selected].pattern.notes.len()).collect();
    }

    /// The notes of the selected pad that belong to the selection, if it is that pad's.
    pub fn roll_selected(&self) -> &[usize] {
        if self.roll.sel_pad == self.selected { &self.roll.sel } else { &[] }
    }

    /// Moves each velocity by a random amount, up to `roll.vel_random` of the range either
    /// way: the selected notes, or every note when none is selected.
    pub fn roll_randomize_velocity(&mut self) {
        let sel = self.selected;
        let chosen: Vec<usize> = self.roll_selected().to_vec();
        let notes = &mut self.pads[sel].pattern.notes;
        let targets: Vec<usize> = if chosen.is_empty() { (0..notes.len()).collect() } else { chosen };
        if targets.is_empty() {
            self.status = "No notes to randomize".into();
            return;
        }
        let mut seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9e37_79b9, |d| d.subsec_nanos() | 1);
        let reach = self.roll.vel_random.clamp(0.0, 1.0) * 127.0;
        for &i in &targets {
            let Some(n) = notes.get_mut(i) else { continue };
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let r = seed as f32 / u32::MAX as f32 * 2.0 - 1.0;
            n.vel = (n.vel as f32 + r * reach).round().clamp(1.0, 127.0) as u8;
        }
        self.push_pattern(sel);
        self.status = format!("Randomized {} velocities by up to ±{:.0}", targets.len(), reach);
    }
}
