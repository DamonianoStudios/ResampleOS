//! The arrangement: a song timeline outside the loops, as long as the song needs.
//!
//! Each pad that plays in the song has a track of clips. A clip plays the pad's pattern
//! from one of the loops, repeated to fill it; editing that loop's pattern changes every
//! clip that plays it, and "make unique" copies the loop so a clip can go its own way.
//! Sections mark which loop each stretch of the song came from and carry that loop's
//! automation. The engine plays a flattened copy (see `crate::engine::ArrangementPlan`).

use super::*;
use crate::engine::{ArrangementPlan, Pattern, SongClip, SongCmd, SongSection, SongTrack};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A whole number of every pattern length (1 to 16 bars), for wrapping clip offsets.
const OFFSET_WRAP: u64 = 720_720 * BAR;
/// Shortest clip, in ticks: a sixteenth.
pub const MIN_CLIP: u64 = (PPQ / 4) as u64;
const BAR: u64 = TICKS_PER_BAR as u64;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Clip {
    /// Song ticks.
    pub start: u64,
    pub len: u64,
    /// Where in the pattern the clip starts playing, in ticks.
    #[serde(default)]
    pub offset: u64,
    /// The loop whose pattern for this pad the clip plays.
    pub scene: usize,
}

impl Clip {
    pub fn end(&self) -> u64 {
        self.start + self.len
    }
}

/// A stretch of the song that came from a loop: shown in the loop lane, and playing that
/// loop's automation.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Section {
    pub start: u64,
    pub len: u64,
    pub scene: usize,
}

impl Section {
    pub fn end(&self) -> u64 {
        self.start + self.len
    }
}

#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct Arrangement {
    /// Clips by pad, in order of pad; each list in order of start, never overlapping.
    pub tracks: Vec<(usize, Vec<Clip>)>,
    /// In order of start, never overlapping.
    pub sections: Vec<Section>,
    /// The loop brace, (start, end) in ticks, and whether playback goes round it.
    #[serde(default)]
    pub brace: Option<(u64, u64)>,
    #[serde(default)]
    pub brace_on: bool,
}

impl Arrangement {
    pub fn is_empty(&self) -> bool {
        self.tracks.iter().all(|(_, c)| c.is_empty()) && self.sections.is_empty()
    }

    /// Where the song ends: the end of its last clip or section.
    pub fn end(&self) -> u64 {
        let clips = self.tracks.iter().flat_map(|(_, c)| c.last().map(Clip::end));
        clips.chain(self.sections.last().map(Section::end)).max().unwrap_or(0)
    }

    pub fn clips(&self, pad: usize) -> &[Clip] {
        self.tracks.iter().find(|(p, _)| *p == pad).map_or(&[], |(_, c)| c)
    }

    fn track_mut(&mut self, pad: usize) -> &mut Vec<Clip> {
        let at = match self.tracks.binary_search_by_key(&pad, |(p, _)| *p) {
            Ok(i) => i,
            Err(i) => {
                self.tracks.insert(i, (pad, Vec::new()));
                i
            }
        };
        &mut self.tracks[at].1
    }

    /// Puts a clip on a pad's track. Whatever it lands on is cut back to make room, as in
    /// any timeline: covered clips go, and clips it overlaps are trimmed or split.
    pub fn place(&mut self, pad: usize, clip: Clip) {
        let track = self.track_mut(pad);
        clear_span(track, clip.start, clip.end());
        let at = track.partition_point(|c| c.start < clip.start);
        track.insert(at, clip);
        self.tidy();
    }

    /// Takes clips (by pad and index) off the arrangement, returning them.
    pub fn take(&mut self, mut picks: Vec<(usize, usize)>) -> Vec<(usize, Clip)> {
        // From the back, so earlier indices stay valid.
        picks.sort_unstable();
        picks.dedup();
        let mut out = Vec::new();
        for &(pad, i) in picks.iter().rev() {
            let track = self.track_mut(pad);
            if i < track.len() {
                out.push((pad, track.remove(i)));
            }
        }
        self.tidy();
        out.reverse();
        out
    }

    /// Clears `[start, end)` of every track and the loop lane.
    pub fn clear(&mut self, start: u64, end: u64) {
        for (_, track) in self.tracks.iter_mut() {
            clear_span(track, start, end);
        }
        let mut kept = Vec::new();
        for s in self.sections.drain(..) {
            if s.end() <= start || s.start >= end {
                kept.push(s);
                continue;
            }
            if s.start < start {
                kept.push(Section { len: start - s.start, ..s });
            }
            if s.end() > end {
                kept.push(Section { start: end, len: s.end() - end, ..s });
            }
        }
        self.sections = kept;
        self.tidy();
    }

    /// Drops empty tracks, keeps everything in order.
    fn tidy(&mut self) {
        self.tracks.retain(|(_, c)| !c.is_empty());
        for (_, c) in self.tracks.iter_mut() {
            c.sort_by_key(|c| c.start);
        }
        self.sections.sort_by_key(|s| s.start);
    }

    /// Lays loop `scene` down from `at` for `len` ticks: a section, and a clip for every
    /// pad given, after clearing the span.
    pub fn put_loop(&mut self, scene: usize, pads: &[usize], at: u64, len: u64) {
        self.clear(at, at + len);
        self.sections.push(Section { start: at, len, scene });
        for &pad in pads {
            self.track_mut(pad).push(Clip { start: at, len, offset: 0, scene });
        }
        self.tidy();
    }

    /// Drops anything that refers to a loop the project does not have (from a damaged or
    /// hand-edited file).
    pub fn loop_count_changed(&mut self, loops: usize) {
        for (_, track) in self.tracks.iter_mut() {
            track.retain(|c| c.scene < loops && c.len > 0);
        }
        self.sections.retain(|s| s.scene < loops && s.len > 0);
        self.tidy();
    }

    /// After loop `at` has been deleted: its clips and sections go, and later loops'
    /// numbers move down.
    pub fn loop_removed(&mut self, at: usize) {
        for (_, track) in self.tracks.iter_mut() {
            track.retain(|c| c.scene != at);
            for c in track.iter_mut().filter(|c| c.scene > at) {
                c.scene -= 1;
            }
        }
        self.sections.retain(|s| s.scene != at);
        for s in self.sections.iter_mut().filter(|s| s.scene > at) {
            s.scene -= 1;
        }
        self.tidy();
    }
}

/// Cuts `[start, end)` out of a track's clips.
fn clear_span(track: &mut Vec<Clip>, start: u64, end: u64) {
    let mut kept = Vec::with_capacity(track.len() + 1);
    for c in track.drain(..) {
        if c.end() <= start || c.start >= end {
            kept.push(c);
            continue;
        }
        if c.start < start {
            kept.push(Clip { len: start - c.start, ..c });
        }
        if c.end() > end {
            let cut = end - c.start;
            kept.push(Clip { start: end, len: c.end() - end, offset: c.offset + cut, ..c });
        }
    }
    *track = kept;
}

/// Grid the timeline snaps to: (name, ticks). 1 is free.
pub const ARR_SNAPS: [(&str, u64); 5] = [("Bar", BAR), ("Beat", PPQ as u64), ("1/8", (PPQ / 2) as u64), ("1/16", (PPQ / 4) as u64), ("Off", 1)];

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum View {
    #[default]
    Pads,
    Arrange,
}

/// What a drag on the timeline is doing.
#[derive(Clone, Copy, Debug)]
pub enum ArrDrag {
    /// Moving the selected clips; `grab` is the pointer's tick when it began.
    Move { grab: u64 },
    /// Trimming one clip's start or end.
    Trim { pad: usize, index: usize, end: bool },
    /// Moving a section and the clips that sit inside it.
    Section { index: usize, grab: u64 },
    /// Drawing the loop brace from `from`.
    Brace { from: u64 },
}

pub struct ArrangeUi {
    /// The insert marker: where playback starts and where things are put.
    pub cursor: u64,
    /// Pixels per bar.
    pub zoom: f32,
    /// First tick in view.
    pub scroll: f64,
    /// First track row in view, in pixels.
    pub vscroll: f32,
    pub snap: usize,
    pub follow: bool,
    /// Selected clips, by pad and index.
    pub selected: Vec<(usize, usize)>,
    pub section: Option<usize>,
    pub drag: Option<ArrDrag>,
    /// The clips being moved, as they were when the drag began.
    pub moving: Vec<(usize, Clip)>,
    /// The arrangement as the engine last heard it.
    sent: Option<Arrangement>,
    /// Something the plan is built from (a pattern, a loop's automation) has changed.
    pub dirty: bool,
    /// The transport was started on the arrangement, and a launched loop has taken over.
    pub overridden: bool,
}

impl Default for ArrangeUi {
    fn default() -> Self {
        ArrangeUi {
            cursor: 0,
            zoom: 120.0,
            scroll: 0.0,
            vscroll: 0.0,
            snap: 0,
            follow: true,
            selected: Vec::new(),
            section: None,
            drag: None,
            moving: Vec::new(),
            sent: None,
            dirty: true,
            overridden: false,
        }
    }
}

impl App {
    /// The pattern a clip plays: loop `scene`'s for `pad` (on the pads, for the loop on
    /// screen).
    pub fn clip_pattern(&self, scene: usize, pad: usize) -> Pattern {
        if scene == self.scene {
            self.pads.get(pad).map(|p| p.pattern.clone()).unwrap_or_default()
        } else {
            self.scenes.get(scene).map(|s| s.pattern(pad)).unwrap_or_default()
        }
    }

    /// The pads that play something in loop `scene`.
    pub fn pads_in_loop(&self, scene: usize) -> Vec<usize> {
        (0..self.pads.len()).filter(|&p| !self.clip_pattern(scene, p).notes.is_empty()).collect()
    }

    fn loop_ticks(&self, scene: usize) -> u64 {
        self.scenes.get(scene).map_or(BAR, |s| s.bars as u64 * BAR)
    }

    /// Writes the chain (or, with no chain, the loop on screen) into the arrangement after
    /// whatever is there: each entry a section, each of its pads a clip.
    pub fn chain_to_arrangement(&mut self) {
        let order: Vec<usize> = if self.chain.is_empty() { vec![self.scene] } else { self.chain.clone() };
        let mut at = self.arrangement.end().div_ceil(BAR) * BAR;
        let from = at;
        for scene in order {
            let len = self.loop_ticks(scene);
            let pads = self.pads_in_loop(scene);
            self.arrangement.put_loop(scene, &pads, at, len);
            at += len;
        }
        self.status = format!("Chain written into the arrangement, bars {} to {}", from / BAR + 1, at / BAR);
        self.view = View::Arrange;
    }

    /// Puts loop `scene` in at the insert marker, over whatever is there.
    pub fn insert_loop(&mut self, scene: usize) {
        let at = self.arr.cursor;
        let len = self.loop_ticks(scene);
        let pads = self.pads_in_loop(scene);
        self.arrangement.put_loop(scene, &pads, at, len);
        self.arr.cursor = at + len;
        self.status = format!("{} put in at bar {}", self.scene_label(scene), at / BAR + 1);
    }

    /// Adds a clip of pad `pad` playing loop `scene` at `at`, as long as that loop.
    pub fn add_clip(&mut self, pad: usize, scene: usize, at: u64) {
        let len = self.loop_ticks(scene);
        self.arrangement.place(pad, Clip { start: at, len, offset: 0, scene });
        self.arr.selected = self.arrangement.clips(pad).iter().position(|c| c.start == at).map(|i| vec![(pad, i)]).unwrap_or_default();
    }

    pub fn delete_selected_clips(&mut self) {
        let picks = std::mem::take(&mut self.arr.selected);
        let n = self.arrangement.take(picks).len();
        if let Some(i) = self.arr.section.take()
            && i < self.arrangement.sections.len()
        {
            let s = self.arrangement.sections.remove(i);
            self.arrangement.clear(s.start, s.end());
        }
        if n > 0 {
            self.status = format!("Deleted {n} clip{}", if n == 1 { "" } else { "s" });
        }
    }

    /// Copies the selected clips (or section) to just after the selection.
    pub fn duplicate_selection(&mut self) {
        if let Some(i) = self.arr.section {
            let Some(s) = self.arrangement.sections.get(i).copied() else { return };
            let inside: Vec<(usize, Clip)> = self.clips_within(s.start, s.end());
            self.arrangement.clear(s.end(), s.end() + s.len);
            self.arrangement.sections.push(Section { start: s.end(), ..s });
            for (pad, c) in inside {
                self.arrangement.place(pad, Clip { start: c.start + s.len, ..c });
            }
            self.arr.section = self.arrangement.sections.iter().position(|x| x.start == s.end());
            return;
        }
        let picked: Vec<(usize, Clip)> = self.selected_clips();
        let (Some(first), Some(last)) = (picked.iter().map(|(_, c)| c.start).min(), picked.iter().map(|(_, c)| c.end()).max()) else { return };
        let shift = last - first;
        for &(pad, c) in &picked {
            self.arrangement.place(pad, Clip { start: c.start + shift, ..c });
        }
        self.arr.selected = picked.iter().filter_map(|&(pad, c)| Some((pad, self.arrangement.clips(pad).iter().position(|x| x.start == c.start + shift)?))).collect();
    }

    /// Splits the selected clips (or every clip, if none is selected) at the insert marker.
    pub fn split_at_cursor(&mut self) {
        let at = self.arr.cursor;
        let picked: Vec<(usize, usize)> = if self.arr.selected.is_empty() {
            self.arrangement.tracks.iter().flat_map(|(p, c)| (0..c.len()).map(move |i| (*p, i))).collect()
        } else {
            self.arr.selected.clone()
        };
        let mut n = 0;
        for (pad, i) in picked.into_iter().rev() {
            let Some(c) = self.arrangement.clips(pad).get(i).copied() else { continue };
            if at <= c.start || at >= c.end() {
                continue;
            }
            let cut = at - c.start;
            let right = Clip { start: at, len: c.len - cut, offset: c.offset + cut, ..c };
            let track = self.arrangement.track_mut(pad);
            track[i].len = cut;
            track.insert(i + 1, right);
            n += 1;
        }
        self.arr.selected.clear();
        if n > 0 {
            self.status = format!("Split {n} clip{} at bar {}", if n == 1 { "" } else { "s" }, at / BAR + 1);
        }
    }

    /// Gives the selected clips a loop of their own: a copy of the loop they play, which
    /// can then be edited without changing the others.
    pub fn make_unique(&mut self) {
        let picked = self.arr.selected.clone();
        let Some(&(pad, i)) = picked.first() else { return };
        let Some(scene) = self.arrangement.clips(pad).get(i).map(|c| c.scene) else { return };
        let Some(copy) = self.duplicate_loop(scene) else { return };
        for (pad, i) in picked {
            let track = self.arrangement.track_mut(pad);
            if let Some(c) = track.get_mut(i).filter(|c| c.scene == scene) {
                c.scene = copy;
            }
        }
        self.status = format!("The selected clips now play {}, a copy of {}", self.scene_label(copy), self.scene_label(scene));
    }

    /// Adds a copy of loop `scene` after the last loop.
    pub fn duplicate_loop(&mut self, scene: usize) -> Option<usize> {
        if self.scenes.len() >= MAX_SCENES {
            self.status = format!("A project holds up to {MAX_SCENES} loops; delete one to make a clip unique");
            return None;
        }
        let mut copy = self.scenes_complete()[scene].clone();
        if !copy.name.is_empty() {
            copy.name.push_str(" copy");
        }
        self.scenes.push(copy);
        Some(self.scenes.len() - 1)
    }

    pub fn selected_clips(&self) -> Vec<(usize, Clip)> {
        self.arr.selected.iter().filter_map(|&(pad, i)| Some((pad, *self.arrangement.clips(pad).get(i)?))).collect()
    }

    /// Every clip lying wholly inside `[start, end)`.
    pub fn clips_within(&self, start: u64, end: u64) -> Vec<(usize, Clip)> {
        self.arrangement
            .tracks
            .iter()
            .flat_map(|(p, clips)| clips.iter().filter(|c| c.start >= start && c.end() <= end).map(move |c| (*p, *c)))
            .collect()
    }

    /// Puts clips back after a move by `delta` ticks (and their selection with them).
    pub fn drop_moved(&mut self, moved: &[(usize, Clip)], delta: i64) {
        let mut selected = Vec::new();
        for &(pad, c) in moved {
            let start = (c.start as i64 + delta).max(0) as u64;
            self.arrangement.place(pad, Clip { start, ..c });
            selected.push((pad, start));
        }
        self.arr.selected = selected
            .into_iter()
            .filter_map(|(pad, start)| Some((pad, self.arrangement.clips(pad).iter().position(|c| c.start == start)?)))
            .collect();
    }

    /// Changes one clip's start (keeping its end) or its end (keeping its start).
    pub fn trim_clip(&mut self, pad: usize, index: usize, end: bool, to: u64) {
        let Some(c) = self.arrangement.clips(pad).get(index).copied() else { return };
        let clip = if end {
            Clip { len: to.max(c.start + MIN_CLIP) - c.start, ..c }
        } else {
            let start = to.min(c.end() - MIN_CLIP);
            // The pattern stays put in time: moving the start moves the offset with it. The
            // engine reads offsets modulo the pattern's length, so any multiple of every
            // length a pattern can have keeps it positive without moving anything.
            let offset = (c.offset as i64 + start as i64 - c.start as i64).rem_euclid(OFFSET_WRAP as i64) as u64;
            Clip { start, len: c.end() - start, offset, ..c }
        };
        self.arrangement.take(vec![(pad, index)]);
        self.arrangement.place(pad, clip);
        self.arr.selected = self.arrangement.clips(pad).iter().position(|x| x.start == clip.start).map(|i| vec![(pad, i)]).unwrap_or_default();
    }

    /// Moves section `index`, and the clips inside it, to `to`.
    pub fn move_section(&mut self, index: usize, to: u64) {
        let Some(s) = self.arrangement.sections.get(index).copied() else { return };
        if to == s.start {
            return;
        }
        let inside = self.clips_within(s.start, s.end());
        self.arrangement.sections.remove(index);
        self.arrangement.clear(s.start, s.end());
        self.arrangement.clear(to, to + s.len);
        self.arrangement.sections.push(Section { start: to, ..s });
        for (pad, c) in inside {
            self.arrangement.place(pad, Clip { start: c.start - s.start + to, ..c });
        }
        self.arr.section = self.arrangement.sections.iter().position(|x| x.start == to);
    }

    /// Opens the loop a clip plays, on its pad, for editing.
    pub fn edit_clip(&mut self, pad: usize, index: usize) {
        let Some(c) = self.arrangement.clips(pad).get(index).copied() else { return };
        if !self.playing() || !self.arr_song_mode() {
            self.show_scene(c.scene, true);
        }
        self.select(pad);
        self.tab = match self.pads[pad].cfg.kind {
            PadKind::Midi => Tab::Roll,
            PadKind::Vocal => Tab::Vocal,
            _ => Tab::Steps,
        };
        self.view = View::Pads;
        self.status = format!("Editing {} on {}: every clip that plays this loop changes with it", self.scene_label(c.scene), pad_label(pad));
    }

    /// Whether the engine is playing the arrangement.
    pub fn arr_song_mode(&self) -> bool {
        self.song_mode
    }

    /// Builds what the engine needs from the arrangement.
    pub(super) fn song_plan(&self) -> ArrangementPlan {
        let scenes = self.scenes_complete();
        let mut patterns: HashMap<(usize, usize), Arc<Pattern>> = HashMap::new();
        let mut tracks = Vec::new();
        for (pad, clips) in &self.arrangement.tracks {
            let mut out = Vec::new();
            for c in clips {
                let Some(scene) = scenes.get(c.scene) else { continue };
                let pattern = patterns.entry((c.scene, *pad)).or_insert_with(|| Arc::new(scene.pattern(*pad))).clone();
                if pattern.notes.is_empty() {
                    continue;
                }
                out.push(SongClip { start: c.start, len: c.len, offset: c.offset, pattern });
            }
            if !out.is_empty() {
                tracks.push(SongTrack { pad: *pad as u16, clips: out });
            }
        }
        let sections = self
            .arrangement
            .sections
            .iter()
            .filter(|s| s.scene < scenes.len())
            .map(|s| SongSection {
                start: s.start,
                len: s.len,
                loop_len: scenes[s.scene].bars as u64 * BAR,
                automation: (!scenes[s.scene].automation.is_empty()).then(|| Arc::new(scenes[s.scene].automation.clone())),
            })
            .collect();
        ArrangementPlan { tracks, sections }
    }

    /// Sends the engine the arrangement whenever it, or anything it is built from, changes.
    pub(super) fn poll_arrangement(&mut self) {
        let changed = self.arr.sent.as_ref() != Some(&self.arrangement);
        if !changed && !self.arr.dirty {
            return;
        }
        self.arr.dirty = false;
        let brace = self.arrangement.brace.filter(|_| self.arrangement.brace_on).map(|(s, e)| (s as f64, e as f64));
        let plan = (!self.arrangement.is_empty()).then(|| Arc::new(self.song_plan()));
        self.send(Cmd::Song(SongCmd::Plan(plan)));
        self.send(Cmd::Song(SongCmd::Brace(brace)));
        self.arr.sent = Some(self.arrangement.clone());
    }

    /// Puts the engine in song mode or loop mode.
    pub(super) fn set_song_mode(&mut self, on: bool) {
        if self.song_mode != on {
            self.song_mode = on;
            self.send(Cmd::Song(SongCmd::Mode(on)));
        }
    }

    /// Moves the insert marker, and the playhead with it when the song is playing.
    pub fn locate(&mut self, tick: u64) {
        self.arr.cursor = tick;
        self.send(Cmd::Song(SongCmd::Locate(tick as f64)));
    }

    /// Where the song playhead is, in ticks.
    pub fn song_pos(&self) -> f64 {
        f64::from_bits(self.shared.song_pos.load(Ordering::Relaxed))
    }

    /// Keys that mean something on the timeline. True if `key` was one of them.
    pub(super) fn arrange_key(&mut self, key: egui::Key, ctrl: bool) -> bool {
        use egui::Key;
        let has_selection = !self.arr.selected.is_empty() || self.arr.section.is_some();
        match key {
            Key::Delete | Key::Backspace if has_selection => self.delete_selected_clips(),
            Key::D if ctrl && has_selection => self.duplicate_selection(),
            Key::E if ctrl => self.split_at_cursor(),
            Key::Home => self.locate(0),
            Key::Escape if has_selection => {
                self.arr.selected.clear();
                self.arr.section = None;
            }
            _ => return false,
        }
        true
    }

    /// Switches between the pads and the arrangement.
    pub fn toggle_view(&mut self) {
        self.view = if self.view == View::Pads { View::Arrange } else { View::Pads };
    }

    /// While a launched loop has taken over from the arrangement: back to it, in time.
    pub fn back_to_arrangement(&mut self) {
        self.arr.overridden = false;
        self.set_song_mode(true);
        self.status = "Back to the arrangement".into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(start: u64, len: u64) -> Clip {
        Clip { start: start * BAR, len: len * BAR, offset: 0, scene: 0 }
    }

    #[test]
    fn placing_a_clip_cuts_back_what_it_lands_on() {
        let mut a = Arrangement::default();
        a.place(3, clip(0, 8));
        // In the middle: the long clip is split around it, its second half picking up the
        // pattern where it would have been.
        a.place(3, Clip { scene: 1, ..clip(2, 2) });
        let c = a.clips(3);
        assert_eq!(c.len(), 3);
        assert_eq!((c[0].start, c[0].len), (0, 2 * BAR));
        assert_eq!((c[1].start, c[1].scene), (2 * BAR, 1));
        assert_eq!((c[2].start, c[2].len, c[2].offset), (4 * BAR, 4 * BAR, 4 * BAR));
        // Over the end of one and the start of the next.
        a.place(3, Clip { scene: 2, ..Clip { start: BAR, len: 2 * BAR, offset: 0, scene: 0 } });
        let c = a.clips(3);
        assert_eq!(c.iter().map(|c| (c.start / BAR, c.end() / BAR, c.scene)).collect::<Vec<_>>(), vec![(0, 1, 0), (1, 3, 2), (3, 4, 1), (4, 8, 0)]);
        // Never overlapping, always in order.
        assert!(c.windows(2).all(|w| w[0].end() <= w[1].start));
    }

    #[test]
    fn a_loop_put_down_clears_its_span_and_marks_a_section() {
        let mut a = Arrangement::default();
        a.put_loop(0, &[1, 2], 0, 4 * BAR);
        a.put_loop(1, &[2, 5], 2 * BAR, 4 * BAR);
        assert_eq!(a.sections.iter().map(|s| (s.start / BAR, s.len / BAR, s.scene)).collect::<Vec<_>>(), vec![(0, 2, 0), (2, 4, 1)]);
        assert_eq!(a.clips(1).len(), 1);
        assert_eq!(a.clips(1)[0].len, 2 * BAR);
        assert_eq!(a.clips(2).iter().map(|c| c.scene).collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(a.end(), 6 * BAR);
        // Deleting loop 0 takes its clips and section with it, and renumbers the rest.
        a.loop_removed(0);
        assert!(a.clips(1).is_empty());
        assert_eq!(a.clips(5)[0].scene, 0);
        assert_eq!(a.sections.len(), 1);
    }
}
