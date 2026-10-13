//! Piano roll selection: the edits that act on every selected note at once.

use super::*;

impl App {
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
}
