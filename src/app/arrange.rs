//! The arrangement: a song timeline outside the loops, as long as the song needs.
//!
//! Each pad that plays in the song has a track of clips. A clip plays the pad's pattern
//! from one of the loops, repeated to fill it; editing that loop's pattern changes every
//! clip that plays it, and "make unique" copies the loop so a clip can go its own way.
//! A clip can instead play notes of its own (recorded or captured onto the timeline) or a
//! recording (an audio clip). Sections mark which loop each stretch of the song came from
//! and carry that loop's automation. The engine plays a flattened copy (see
//! `crate::engine::ArrangementPlan`).
//!
//! Clips and sections are small copyable values; what they hold that has a size (notes,
//! recordings, names) lives in pools on the arrangement, referred to by index.

use super::*;
use crate::engine::{ArrangementPlan, Pattern, SongAudio, SongClip, SongCmd, SongSection, SongTrack};
use crate::sample::SampleData;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A whole number of every pattern length (1 to 16 bars), for wrapping clip offsets.
const OFFSET_WRAP: u64 = 720_720 * BAR;
/// Shortest clip, in ticks: a sixteenth.
pub const MIN_CLIP: u64 = (PPQ / 4) as u64;
const BAR: u64 = TICKS_PER_BAR as u64;

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Clip {
    /// Song ticks.
    pub start: u64,
    pub len: u64,
    /// Where in the pattern (or the recording) the clip starts playing, in ticks.
    #[serde(default)]
    pub offset: u64,
    /// The loop whose pattern for this pad the clip plays (unless it plays notes of its
    /// own or a recording).
    pub scene: usize,
    /// An audio clip plays this recording from the arrangement's pool instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<usize>,
    /// A clip with notes of its own plays this pattern from the arrangement's pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<usize>,
    /// Audio clips: a crossfade this long (ticks) with the audio clip that ends where
    /// this one starts.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub xfade: u64,
    /// Audio clips: gain in dB, and fades in ticks from each end.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gain_db: f32,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub fade_in: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub fade_out: u64,
}

pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

impl Clip {
    /// A clip playing loop `scene`'s pattern.
    pub fn pattern(start: u64, len: u64, offset: u64, scene: usize) -> Clip {
        Clip { start, len, offset, scene, audio: None, notes: None, xfade: 0, gain_db: 0.0, fade_in: 0, fade_out: 0 }
    }

    /// A clip playing pattern `notes` of the pool.
    pub fn own_notes(start: u64, len: u64, notes: usize) -> Clip {
        Clip { notes: Some(notes), ..Clip::pattern(start, len, 0, 0) }
    }

    /// A clip playing recording `source` of the pool from its start.
    pub fn audio(start: u64, len: u64, source: usize) -> Clip {
        Clip { audio: Some(source), ..Clip::pattern(start, len, 0, 0) }
    }

    pub fn end(&self) -> u64 {
        self.start + self.len
    }

    pub fn is_audio(&self) -> bool {
        self.audio.is_some()
    }

    /// Plays a loop's pattern (rather than notes of its own or a recording).
    pub fn plays_loop(&self) -> bool {
        self.audio.is_none() && self.notes.is_none()
    }
}

/// A recording the arrangement's audio clips play. Saved as `file` under `song/` in the
/// project folder.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AudioSource {
    pub file: String,
    /// What clips playing it are called.
    pub name: String,
    #[serde(skip)]
    pub data: Option<Arc<SampleData>>,
    /// Peaks of every `OVERVIEW_STEP` frames, for drawing.
    #[serde(skip)]
    pub overview: Arc<Vec<(f32, f32)>>,
    /// The tempo the recording is at, and whether it follows the project's: stretched
    /// (by `mode`) to the project tempo when that differs.
    #[serde(default)]
    pub bpm: f32,
    #[serde(default)]
    pub warp: bool,
    #[serde(default)]
    pub mode: crate::stretch::WarpMode,
    /// Other takes of the same recording (loop-brace passes), as pool indices.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub takes: Vec<usize>,
    /// The recording stretched to a tempo: (tempo, mode, audio).
    #[serde(skip)]
    pub warped: Option<(f32, crate::stretch::WarpMode, Arc<SampleData>)>,
}

/// Frames per point of an audio source's overview.
pub const OVERVIEW_STEP: usize = 256;

impl PartialEq for AudioSource {
    fn eq(&self, o: &Self) -> bool {
        let same = match (&self.data, &o.data) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        same && self.file == o.file && self.name == o.name && self.bpm == o.bpm && self.warp == o.warp && self.mode == o.mode && self.takes == o.takes
    }
}

impl AudioSource {
    /// A recording at tempo `bpm`, following the project's tempo.
    pub fn new(file: String, name: String, data: Arc<SampleData>, bpm: f32) -> AudioSource {
        let overview = Arc::new(overview(&data));
        AudioSource { file, name, data: Some(data), overview, bpm, warp: true, mode: Default::default(), takes: Vec::new(), warped: None }
    }

    /// The same recording with its audio loaded (after reading a project).
    pub fn with_data(&self, data: Arc<SampleData>) -> AudioSource {
        let overview = Arc::new(overview(&data));
        AudioSource { data: Some(data), overview, warped: None, ..self.clone() }
    }

    /// The audio to play at project tempo `bpm`, if it is ready.
    pub fn audio_at(&self, bpm: f32) -> Option<Arc<SampleData>> {
        let data = self.data.clone()?;
        if !self.needs_warp(bpm) {
            return Some(data);
        }
        match &self.warped {
            Some((t, m, w)) if (*t - bpm).abs() < 1e-3 && *m == self.mode => Some(w.clone()),
            // Not stretched yet: the original, until it is.
            _ => Some(data),
        }
    }

    /// Whether playing at `bpm` needs a stretched copy.
    pub fn needs_warp(&self, bpm: f32) -> bool {
        self.warp && self.bpm > 0.0 && (self.bpm - bpm).abs() > 1e-3
    }

    /// Length in frames.
    pub fn frames(&self) -> usize {
        self.data.as_ref().map_or(0, |d| d.len())
    }
}

/// Min and max of each `OVERVIEW_STEP` frames, both channels together.
fn overview(s: &SampleData) -> Vec<(f32, f32)> {
    s.l.chunks(OVERVIEW_STEP)
        .zip(s.r.chunks(OVERVIEW_STEP))
        .map(|(l, r)| l.iter().chain(r).fold((0f32, 0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v))))
        .collect()
}

/// A stretch of the song that came from a loop: shown in the loop lane, and playing that
/// loop's automation.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Section {
    pub start: u64,
    pub len: u64,
    pub scene: usize,
    /// A name of its own (an index into `Arrangement::labels`), instead of its loop's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<usize>,
    /// A colour of its own (an index into the palette), instead of its loop's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<u8>,
}

impl Section {
    pub fn new(start: u64, len: u64, scene: usize) -> Section {
        Section { start, len, scene, label: None, colour: None }
    }
}

impl Section {
    pub fn end(&self) -> u64 {
        self.start + self.len
    }
}

/// A named place in the song.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Marker {
    pub tick: u64,
    pub name: String,
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
    /// Recordings that audio clips play, by index.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio: Vec<AudioSource>,
    /// Song-length automation, in song ticks, over the sections' loop automation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automation: Vec<crate::engine::AutoLane>,
    /// In order of tick.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// Notes of clips that have their own, by index. Each pattern is as long as the clip
    /// it was made for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patterns: Vec<Pattern>,
    /// Names of sections that have their own, by index.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Frozen tracks: the clips each had before it was rendered to audio, to be put back
    /// when it is unfrozen. A frozen pad's instrument plugin is switched off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frozen: Vec<(usize, Vec<Clip>)>,
}

impl Arrangement {
    pub fn is_empty(&self) -> bool {
        self.tracks.iter().all(|(_, c)| c.is_empty()) && self.sections.is_empty() && self.automation.is_empty()
    }

    /// Where the song ends: the end of its last clip or section.
    pub fn end(&self) -> u64 {
        let clips = self.tracks.iter().flat_map(|(_, c)| c.last().map(Clip::end));
        clips.chain(self.sections.last().map(Section::end)).max().unwrap_or(0)
    }

    pub fn clips(&self, pad: usize) -> &[Clip] {
        self.tracks.iter().find(|(p, _)| *p == pad).map_or(&[], |(_, c)| c)
    }

    pub(super) fn track_mut(&mut self, pad: usize) -> &mut Vec<Clip> {
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

    pub fn is_frozen(&self, pad: usize) -> bool {
        self.frozen.iter().any(|(p, _)| *p == pad)
    }

    /// Every clip, by pad and index.
    pub fn all_clips(&self) -> Vec<(usize, usize)> {
        self.tracks.iter().flat_map(|(p, c)| (0..c.len()).map(move |i| (*p, i))).collect()
    }

    /// Every clip lying wholly inside `[start, end)`.
    pub fn within(&self, start: u64, end: u64) -> Vec<(usize, Clip)> {
        self.tracks
            .iter()
            .flat_map(|(p, clips)| clips.iter().filter(|c| c.start >= start && c.end() <= end).map(move |c| (*p, *c)))
            .collect()
    }

    /// Deletes clips (by pad and index), and sections (by index) with everything in them.
    /// Returns how many clips and sections went.
    pub fn delete(&mut self, clips: Vec<(usize, usize)>, mut sections: Vec<usize>) -> (usize, usize) {
        let n = self.take(clips).len();
        sections.sort_unstable();
        sections.dedup();
        let mut m = 0;
        for i in sections.into_iter().rev() {
            if i < self.sections.len() {
                let s = self.sections.remove(i);
                self.clear(s.start, s.end());
                m += 1;
            }
        }
        (n, m)
    }

    /// Copies clips and sections (each section with the clips inside it) to just after the
    /// last of them. A section copied over others replaces them; a clip cuts back what it
    /// lands on. Returns where the copies are, as clips and sections.
    pub fn duplicate(&mut self, clips: &[(usize, usize)], sections: &[usize]) -> (Vec<(usize, usize)>, Vec<usize>) {
        let sections: Vec<Section> = sections.iter().filter_map(|&i| self.sections.get(i).copied()).collect();
        let mut picked: Vec<(usize, Clip)> = clips.iter().filter_map(|&(pad, i)| Some((pad, *self.clips(pad).get(i)?))).collect();
        for s in &sections {
            for c in self.within(s.start, s.end()) {
                if !picked.contains(&c) {
                    picked.push(c);
                }
            }
        }
        let starts = picked.iter().map(|(_, c)| c.start).chain(sections.iter().map(|s| s.start));
        let ends = picked.iter().map(|(_, c)| c.end()).chain(sections.iter().map(Section::end));
        let (Some(first), Some(last)) = (starts.min(), ends.max()) else { return (Vec::new(), Vec::new()) };
        let shift = last - first;
        for s in &sections {
            self.clear(s.start + shift, s.end() + shift);
            self.sections.push(Section { start: s.start + shift, ..*s });
        }
        self.tidy();
        for &(pad, c) in &picked {
            self.place(pad, Clip { start: c.start + shift, ..c });
        }
        let clips = picked.iter().filter_map(|&(pad, c)| Some((pad, self.clips(pad).iter().position(|x| x.start == c.start + shift)?))).collect();
        let sections = sections.iter().filter_map(|s| self.sections.iter().position(|x| x.start == s.start + shift)).collect();
        (clips, sections)
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
        self.markers.sort_by_key(|m| m.tick);
    }

    /// Lays loop `scene` down from `at` for `len` ticks: a section, and a clip for every
    /// pad given, after clearing the span.
    pub fn put_loop(&mut self, scene: usize, pads: &[usize], at: u64, len: u64) {
        self.clear(at, at + len);
        self.sections.push(Section::new(at, len, scene));
        for &pad in pads {
            self.track_mut(pad).push(Clip::pattern(at, len, 0, scene));
        }
        self.tidy();
    }

    /// Drops anything that refers to a loop the project does not have (from a damaged or
    /// hand-edited file).
    pub fn loop_count_changed(&mut self, loops: usize) {
        for (_, track) in self.tracks.iter_mut() {
            track.retain(|c| (!c.plays_loop() || c.scene < loops) && c.len > 0);
        }
        self.sections.retain(|s| s.scene < loops && s.len > 0);
        self.tidy();
    }

    /// Opens a gap of `len` ticks at `at`: everything from there on moves later, and a
    /// clip or section across `at` is split, its second half moving with the rest.
    pub fn insert_time(&mut self, at: u64, len: u64) {
        if len == 0 {
            return;
        }
        for (_, track) in self.tracks.iter_mut() {
            let mut out = Vec::with_capacity(track.len() + 1);
            for c in track.drain(..) {
                if c.end() <= at {
                    out.push(c);
                } else if c.start >= at {
                    out.push(Clip { start: c.start + len, ..c });
                } else {
                    let cut = at - c.start;
                    out.push(Clip { len: cut, ..c });
                    out.push(Clip { start: at + len, len: c.len - cut, offset: c.offset + cut, ..c });
                }
            }
            *track = out;
        }
        let mut sections = Vec::with_capacity(self.sections.len() + 1);
        for s in self.sections.drain(..) {
            if s.end() <= at {
                sections.push(s);
            } else if s.start >= at {
                sections.push(Section { start: s.start + len, ..s });
            } else {
                sections.push(Section { len: at - s.start, ..s });
                sections.push(Section { start: at + len, len: s.end() - at, ..s });
            }
        }
        self.sections = sections;
        for m in self.markers.iter_mut().filter(|m| m.tick >= at) {
            m.tick += len;
        }
        // Song lanes hold their value across the gap.
        for lane in self.automation.iter_mut() {
            if let Some(v) = lane.value_at(at as f64) {
                let k = lane.points.partition_point(|p| (p.tick as u64) < at);
                for p in lane.points[k..].iter_mut() {
                    p.tick = (p.tick as u64 + len).min(u32::MAX as u64) as u32;
                }
                if k > 0 {
                    // The value it had at `at`, from both edges of the gap.
                    let pt = |t: u64| crate::engine::AutoPoint::new(t.min(u32::MAX as u64) as u32, v);
                    lane.points.insert(k, pt(at + len));
                    if lane.points[k - 1].tick as u64 != at {
                        lane.points.insert(k, pt(at));
                    }
                }
            }
        }
        self.tidy();
    }

    /// Takes `[start, end)` out of the song: what is in it goes, and everything after
    /// moves earlier to close the gap.
    pub fn delete_time(&mut self, start: u64, end: u64) {
        if end <= start {
            return;
        }
        let len = end - start;
        self.clear(start, end);
        for (_, track) in self.tracks.iter_mut() {
            for c in track.iter_mut().filter(|c| c.start >= end) {
                c.start -= len;
            }
        }
        for s in self.sections.iter_mut().filter(|s| s.start >= end) {
            s.start -= len;
        }
        self.markers.retain(|m| m.tick < start || m.tick >= end);
        for m in self.markers.iter_mut().filter(|m| m.tick >= end) {
            m.tick -= len;
        }
        for lane in self.automation.iter_mut() {
            lane.points.retain(|p| (p.tick as u64) < start || p.tick as u64 >= end);
            for p in lane.points.iter_mut().filter(|p| p.tick as u64 >= end) {
                p.tick -= len as u32;
            }
        }
        self.tidy();
    }

    /// Adds a marker at `tick`, numbered after the others.
    pub fn add_marker(&mut self, tick: u64) -> usize {
        let name = format!("Marker {}", self.markers.len() + 1);
        let at = self.markers.partition_point(|m| m.tick <= tick);
        self.markers.insert(at, Marker { tick, name });
        at
    }

    /// After loop `at` has been deleted: its clips and sections go, and later loops'
    /// numbers move down.
    pub fn loop_removed(&mut self, at: usize) {
        for (_, track) in self.tracks.iter_mut() {
            track.retain(|c| !c.plays_loop() || c.scene != at);
            for c in track.iter_mut().filter(|c| c.plays_loop() && c.scene > at) {
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

/// A place in the song for messages: "bar 8", "bar 7 beat 3", "bar 7 beat 3.25".
pub fn pos_text(tick: u64) -> String {
    let (bar, rest) = (tick / BAR + 1, tick % BAR);
    let beat = PPQ as u64;
    if rest == 0 {
        format!("bar {bar}")
    } else if rest.is_multiple_of(beat) {
        format!("bar {bar} beat {}", rest / beat + 1)
    } else {
        format!("bar {bar} beat {:.2}", rest as f64 / beat as f64 + 1.0)
    }
}

/// "2 bars", "1 beat", "3/16 of a bar"...
fn bars_text(ticks: u64) -> String {
    let beat = PPQ as u64;
    if ticks.is_multiple_of(BAR) {
        let n = ticks / BAR;
        format!("{n} bar{}", if n == 1 { "" } else { "s" })
    } else if ticks.is_multiple_of(beat) {
        let n = ticks / beat;
        format!("{n} beat{}", if n == 1 { "" } else { "s" })
    } else {
        format!("{:.2} beats", ticks as f64 / beat as f64)
    }
}

/// Grid the timeline snaps to: (name, ticks). 1 is free.
pub const ARR_SNAPS: [(&str, u64); 5] = [("Bar", BAR), ("Beat", PPQ as u64), ("1/8", (PPQ / 2) as u64), ("1/16", (PPQ / 4) as u64), ("1/32", (PPQ / 8) as u64)];

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
    /// Drawing a selection box from `from`, in screen points.
    Box { from: egui::Pos2 },
    /// Moving marker `index`.
    Marker { index: usize },
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
    /// The grid things snap to (an index into `ARR_SNAPS`), and whether they do. `Alt`
    /// held places off the grid either way.
    pub snap: usize,
    pub snap_on: bool,
    pub follow: bool,
    /// Selected clips, by pad and index.
    pub selected: Vec<(usize, usize)>,
    /// Selected sections, by index.
    pub sections: Vec<usize>,
    pub drag: Option<ArrDrag>,
    /// The clips being moved, as they were when the drag began.
    pub moving: Vec<(usize, Clip)>,
    /// What was selected when a `Shift` box began; the box adds to it.
    pub box_base: (Vec<(usize, usize)>, Vec<usize>),
    /// Where files dropped now would land: a track (or a new one) and a tick.
    pub drop_at: Option<(Option<usize>, u64)>,
    /// Where the menu of an empty stretch of timeline was opened.
    pub menu_at: (Option<usize>, u64),
    /// The song's automation lanes are open under the timeline.
    pub show_auto: bool,
    /// Where the ruler's menu was opened, in ticks.
    pub ruler_at: u64,
    /// Where a clip's menu was opened, in ticks.
    pub clip_menu_at: u64,
    /// Recordings being stretched to a tempo in the background: (pool index, tempo, mode).
    pub warping: Vec<(usize, f32, crate::stretch::WarpMode)>,
    /// The tempo, and since when it has stood still (stretching waits for it to settle).
    pub tempo_since: (f32, Option<std::time::Instant>),
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
            snap_on: true,
            follow: true,
            selected: Vec::new(),
            sections: Vec::new(),
            drag: None,
            moving: Vec::new(),
            box_base: (Vec::new(), Vec::new()),
            drop_at: None,
            menu_at: (None, 0),
            show_auto: false,
            ruler_at: 0,
            clip_menu_at: 0,
            warping: Vec::new(),
            tempo_since: (0.0, None),
            sent: None,
            dirty: true,
            overridden: false,
        }
    }
}

impl ArrangeUi {
    pub fn deselect(&mut self) {
        self.selected.clear();
        self.sections.clear();
    }

    pub fn has_selection(&self) -> bool {
        !self.selected.is_empty() || !self.sections.is_empty()
    }

    /// Selects just section `i`.
    pub fn pick_section(&mut self, i: usize) {
        self.selected.clear();
        self.sections = vec![i];
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
        self.arrangement.place(pad, Clip::pattern(at, len, 0, scene));
        self.arr.selected = self.arrangement.clips(pad).iter().position(|c| c.start == at).map(|i| vec![(pad, i)]).unwrap_or_default();
    }

    /// Selects every clip and section.
    pub fn select_all_clips(&mut self) {
        self.arr.selected = self.arrangement.all_clips();
        self.arr.sections = (0..self.arrangement.sections.len()).collect();
    }

    /// Deletes the selected clips, and the selected sections with every clip inside them.
    pub fn delete_selected_clips(&mut self) {
        let (n, m) = self.arrangement.delete(std::mem::take(&mut self.arr.selected), std::mem::take(&mut self.arr.sections));
        let plural = |k: usize| if k == 1 { "" } else { "s" };
        self.status = match (n, m) {
            (0, 0) => return,
            (n, 0) => format!("Deleted {n} clip{}", plural(n)),
            (0, m) => format!("Deleted {m} section{} with their clips", plural(m)),
            (n, m) => format!("Deleted {n} clip{} and {m} section{} with their clips", plural(n), plural(m)),
        };
    }

    /// Copies the selected clips and sections to just after the selection; the copies
    /// become the selection, so `Ctrl+D` again carries on the run.
    pub fn duplicate_selection(&mut self) {
        (self.arr.selected, self.arr.sections) = self.arrangement.duplicate(&self.arr.selected, &self.arr.sections);
    }

    /// Splits the selected clips (or every clip, if none is selected) at the insert marker.
    pub fn split_at_cursor(&mut self) {
        self.split_at(self.arr.cursor);
    }

    /// Splits the selected clips (or every clip, if none is selected) at `at`.
    pub fn split_at(&mut self, at: u64) {
        let picked: Vec<(usize, usize)> = if self.arr.selected.is_empty() {
            self.arrangement.all_clips()
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
            self.status = format!("Split {n} clip{} at {}", if n == 1 { "" } else { "s" }, pos_text(at));
        }
    }

    /// Gives the selected clips a loop of their own: a copy of the loop they play, which
    /// can then be edited without changing the others.
    pub fn make_unique(&mut self) {
        let picked = self.arr.selected.clone();
        let Some(&(pad, i)) = picked.first() else { return };
        // Clips with notes of their own get a copy of the notes, each.
        if self.arrangement.clips(pad).get(i).is_some_and(|c| c.notes.is_some()) {
            for (pad, i) in picked {
                let Some(own) = self.arrangement.clips(pad).get(i).and_then(|c| c.notes) else { continue };
                let copy = self.arrangement.patterns[own].clone();
                self.arrangement.patterns.push(copy);
                let id = self.arrangement.patterns.len() - 1;
                if let Some(c) = self.arrangement.track_mut(pad).get_mut(i) {
                    c.notes = Some(id);
                }
            }
            self.status = "The selected clips now have notes of their own, each".into();
            return;
        }
        let Some(scene) = self.arrangement.clips(pad).get(i).filter(|c| c.plays_loop()).map(|c| c.scene) else { return };
        let Some(copy) = self.duplicate_loop(scene) else { return };
        for (pad, i) in picked {
            let track = self.arrangement.track_mut(pad);
            if let Some(c) = track.get_mut(i).filter(|c| c.plays_loop() && c.scene == scene) {
                c.scene = copy;
            }
        }
        self.status = format!("The selected clips now play {}, a copy of {}", self.scene_label(copy), self.scene_label(scene));
    }

    /// Turns a clip's notes of its own into a new loop that it (and every clip sharing
    /// those notes) plays, so they can be edited on the pads. False if there is no room.
    pub fn notes_to_loop(&mut self, pad: usize, index: usize) -> bool {
        let Some(n) = self.arrangement.clips(pad).get(index).and_then(|c| c.notes) else { return false };
        if self.scenes.len() >= MAX_SCENES {
            self.status = format!("A project holds up to {MAX_SCENES} loops; delete one to edit these notes as a loop");
            return false;
        }
        let mut pattern = self.arrangement.patterns[n].clone();
        let bars = (pattern.len_ticks as u64).div_ceil(BAR).clamp(1, 16) as u32;
        if pattern.len_ticks as u64 > 16 * BAR {
            pattern.notes.retain(|x| (x.start as u64) < 16 * BAR);
        }
        pattern.len_ticks = bars * TICKS_PER_BAR;
        let mut scene = Scene::new(bars);
        scene.name = format!("{} notes", pad_label(pad));
        scene.patterns.push((pad, pattern));
        self.scenes.push(scene);
        let k = self.scenes.len() - 1;
        for (_, clips) in self.arrangement.tracks.iter_mut() {
            for c in clips.iter_mut().filter(|c| c.notes == Some(n)) {
                c.notes = None;
                c.scene = k;
            }
        }
        self.status = format!("These notes are now {}: edit them there, and every clip playing them follows", self.scene_label(k));
        true
    }

    /// What a section is called: its own name, or its loop's.
    pub fn section_name(&self, s: &Section) -> String {
        s.label.and_then(|l| self.arrangement.labels.get(l)).filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| self.scene_label(s.scene))
    }

    /// Gives section `i` a name of its own (empty: its loop's again).
    pub fn set_section_name(&mut self, i: usize, name: &str) {
        let Some(s) = self.arrangement.sections.get(i).copied() else { return };
        let labels = &mut self.arrangement.labels;
        match s.label {
            Some(l) if l < labels.len() => labels[l] = name.to_string(),
            _ => {
                labels.push(name.to_string());
                let l = labels.len() - 1;
                self.arrangement.sections[i].label = Some(l);
            }
        }
    }

    pub fn set_section_colour(&mut self, i: usize, colour: Option<u8>) {
        if let Some(s) = self.arrangement.sections.get_mut(i) {
            s.colour = colour;
        }
    }

    /// Switches an audio clip to another take of its recording.
    pub fn choose_take(&mut self, pad: usize, index: usize, source: usize) {
        if let Some(c) = self.arrangement.track_mut(pad).get_mut(index).filter(|c| c.is_audio()) {
            c.audio = Some(source);
        }
    }

    /// Sets the crossfade an audio clip has with the audio clip ending where it starts.
    pub fn set_xfade(&mut self, pad: usize, index: usize, ticks: u64) {
        if let Some(c) = self.arrangement.track_mut(pad).get_mut(index).filter(|c| c.is_audio()) {
            c.xfade = ticks.min(c.len);
        }
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

    pub fn clips_within(&self, start: u64, end: u64) -> Vec<(usize, Clip)> {
        self.arrangement.within(start, end)
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
        // An audio clip can't reach past either end of its recording.
        let (first, last) = match c.audio {
            Some(src) => {
                let begins = c.start.saturating_sub(c.offset);
                let frames = self.arrangement.audio.get(src).map_or(0, |a| a.frames());
                (begins, begins + (frames as f64 * self.source_ticks_per_frame(src)).round() as u64)
            }
            None => (0, u64::MAX),
        };
        let clip = if end {
            Clip { len: to.max(c.start + MIN_CLIP).min(last.max(c.start + MIN_CLIP)) - c.start, ..c }
        } else if c.is_audio() {
            let start = to.max(first).min(c.end() - MIN_CLIP);
            Clip { start, len: c.end() - start, offset: start - first, ..c }
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
        self.arr.sections = self.arrangement.sections.iter().position(|x| x.start == to).into_iter().collect();
    }

    /// Opens the loop a clip plays, on its pad, for editing.
    pub fn edit_clip(&mut self, pad: usize, index: usize) {
        let Some(c) = self.arrangement.clips(pad).get(index).copied() else { return };
        if c.is_audio() {
            self.select(pad);
            return;
        }
        if c.notes.is_some() {
            // Notes of its own are edited as a loop.
            if !self.notes_to_loop(pad, index) {
                return;
            }
            return self.edit_clip(pad, index);
        }
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
        let empty = Arc::new(Pattern::default());
        let mut tracks = Vec::new();
        for (pad, clips) in &self.arrangement.tracks {
            let mut out = Vec::new();
            for (k, c) in clips.iter().enumerate() {
                if let Some(src) = c.audio {
                    let Some(data) = self.arrangement.audio.get(src).and_then(|a| a.audio_at(self.bpm)) else { continue };
                    // Crossfades with audio clips that join this one, at either end.
                    let before = k.checked_sub(1).and_then(|j| clips.get(j)).filter(|o| o.is_audio() && o.end() == c.start);
                    let after = clips.get(k + 1).filter(|o| o.is_audio() && o.start == c.end());
                    let xin = before.map_or(0, |_| c.xfade.min(c.len));
                    let xout = after.map_or(0, |n| n.xfade.min(c.len));
                    let audio = SongAudio { data, gain: db_to_gain(c.gain_db), fade_in: c.fade_in, fade_out: c.fade_out, xin, xout };
                    out.push(SongClip { start: c.start, len: c.len, offset: c.offset, pattern: empty.clone(), audio: Some(audio) });
                    continue;
                }
                if let Some(n) = c.notes {
                    let Some(p) = self.arrangement.patterns.get(n).filter(|p| !p.notes.is_empty()) else { continue };
                    out.push(SongClip { start: c.start, len: c.len, offset: c.offset, pattern: Arc::new(p.clone()), audio: None });
                    continue;
                }
                let Some(scene) = scenes.get(c.scene) else { continue };
                let pattern = patterns.entry((c.scene, *pad)).or_insert_with(|| Arc::new(scene.pattern(*pad))).clone();
                if pattern.notes.is_empty() {
                    continue;
                }
                out.push(SongClip { start: c.start, len: c.len, offset: c.offset, pattern, audio: None });
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
        let automation = (!self.arrangement.automation.is_empty()).then(|| Arc::new(self.arrangement.automation.clone()));
        ArrangementPlan { tracks, sections, automation }
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

    /// Opens a gap of `len` ticks at `at`, moving the rest of the song later.
    pub fn insert_time(&mut self, at: u64, len: u64) {
        self.arrangement.insert_time(at, len);
        self.arr.deselect();
        if self.arr.cursor >= at {
            self.arr.cursor += len;
        }
        self.status = format!("Inserted {} at bar {}", bars_text(len), at / BAR + 1);
    }

    /// Takes `[start, end)` out of the song, closing the gap.
    pub fn delete_time(&mut self, start: u64, end: u64) {
        self.arrangement.delete_time(start, end);
        self.arr.deselect();
        if self.arr.cursor >= end {
            self.arr.cursor -= end - start;
        } else if self.arr.cursor > start {
            self.arr.cursor = start;
        }
        self.status = format!("Deleted {} from bar {}", bars_text(end - start), start / BAR + 1);
    }

    pub fn add_marker(&mut self, tick: u64) {
        let i = self.arrangement.add_marker(tick);
        self.status = format!("{} at {}: right-click it to rename it", self.arrangement.markers[i].name, pos_text(tick));
    }

    /// Keys that mean something on the timeline. True if `key` was one of them.
    pub(super) fn arrange_key(&mut self, key: egui::Key, ctrl: bool) -> bool {
        use egui::Key;
        let has_selection = self.arr.has_selection();
        match key {
            Key::Delete | Key::Backspace if has_selection => self.delete_selected_clips(),
            Key::A if ctrl => self.select_all_clips(),
            Key::D if ctrl && has_selection => self.duplicate_selection(),
            Key::E if ctrl => self.split_at_cursor(),
            Key::J if ctrl => self.consolidate(),
            Key::Home => self.locate(0),
            Key::Escape if has_selection => self.arr.deselect(),
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
        Clip::pattern(start * BAR, len * BAR, 0, 0)
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
        a.place(3, Clip::pattern(BAR, 2 * BAR, 0, 2));
        let c = a.clips(3);
        assert_eq!(c.iter().map(|c| (c.start / BAR, c.end() / BAR, c.scene)).collect::<Vec<_>>(), vec![(0, 1, 0), (1, 3, 2), (3, 4, 1), (4, 8, 0)]);
        // Never overlapping, always in order.
        assert!(c.windows(2).all(|w| w[0].end() <= w[1].start));
    }

    #[test]
    fn audio_clips_ignore_loops_and_save_without_their_audio() {
        let mut a = Arrangement::default();
        let data = Arc::new(SampleData::new(vec![0.1; 1000], vec![0.1; 1000]));
        a.audio.push(AudioSource::new("001_vox.wav".into(), "vox".into(), data, 120.0));
        a.put_loop(0, &[1], 0, 4 * BAR);
        a.place(4, Clip { gain_db: -3.0, fade_in: 120, ..Clip::audio(BAR, 2 * BAR, 0) });
        assert_eq!(a.audio[0].overview.len(), 4, "one overview point per 256 frames");
        // Deleting loop 0 takes its clips, not the audio clip, which plays no loop.
        a.loop_removed(0);
        a.loop_count_changed(1);
        assert!(a.clips(1).is_empty());
        assert_eq!(a.clips(4).len(), 1);
        // The project file holds where the audio is, not the audio itself.
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains("001_vox.wav") && json.len() < 600, "{json}");
        let back: Arrangement = serde_json::from_str(&json).unwrap();
        let c = back.clips(4)[0];
        assert_eq!((c.audio, c.gain_db, c.fade_in, c.start, c.len), (Some(0), -3.0, 120, BAR, 2 * BAR));
        assert!(back.audio[0].data.is_none());
        // Pattern clips are written as before, without the audio fields.
        let mut p = Arrangement::default();
        p.place(1, clip(0, 1));
        assert!(!serde_json::to_string(&p).unwrap().contains("gain_db"));
    }

    #[test]
    fn inserting_and_deleting_time_moves_the_rest_of_the_song() {
        use crate::engine::{AutoLane, AutoPoint, AutoTarget};
        let mut a = Arrangement::default();
        a.put_loop(0, &[1], 0, 4 * BAR);
        a.put_loop(1, &[1], 4 * BAR, 4 * BAR);
        a.add_marker(4 * BAR);
        let tb = TICKS_PER_BAR;
        a.automation.push(AutoLane { target: AutoTarget::BusLevel(0), points: vec![AutoPoint::new(0, 0.0), AutoPoint::new(8 * tb, 1.0)] });
        // Two bars in the middle of the first section: it is split around the gap.
        a.insert_time(2 * BAR, 2 * BAR);
        let spans = |a: &Arrangement| a.clips(1).iter().map(|c| (c.start / BAR, c.end() / BAR, c.offset / BAR)).collect::<Vec<_>>();
        assert_eq!(spans(&a), vec![(0, 2, 0), (4, 6, 2), (6, 10, 0)]);
        assert_eq!(a.sections.iter().map(|s| (s.start / BAR, s.end() / BAR)).collect::<Vec<_>>(), vec![(0, 2), (4, 6), (6, 10)]);
        assert_eq!(a.markers[0].tick, 6 * BAR);
        // The lane holds still over the gap, then carries on as it was.
        let lane = &a.automation[0];
        assert!((lane.value_at((2 * tb) as f64).unwrap() - 0.25).abs() < 1e-3);
        assert!((lane.value_at((4 * tb) as f64).unwrap() - 0.25).abs() < 1e-3);
        assert!((lane.value_at((10 * tb) as f64).unwrap() - 1.0).abs() < 1e-3);
        // Taking the same two bars out puts the song back as it was, but for the cut.
        a.delete_time(2 * BAR, 4 * BAR);
        assert_eq!(spans(&a), vec![(0, 2, 0), (2, 4, 2), (4, 8, 0)]);
        assert_eq!(a.markers[0].tick, 4 * BAR);
        assert_eq!(a.end(), 8 * BAR);
        // A marker inside deleted time goes with it.
        a.delete_time(3 * BAR, 5 * BAR);
        assert!(a.markers.is_empty());
        assert_eq!(a.end(), 6 * BAR);
    }

    #[test]
    fn a_recording_that_follows_the_tempo_plays_its_stretched_copy() {
        use crate::stretch::WarpMode;
        let data = Arc::new(SampleData::new(vec![0.1; 1000], vec![0.1; 1000]));
        let mut a = AudioSource::new("001_a.wav".into(), "a".into(), data.clone(), 90.0);
        // At its own tempo, or not following: the recording as it is.
        assert!(!a.needs_warp(90.0));
        assert!(Arc::ptr_eq(&a.audio_at(90.0).unwrap(), &data));
        assert!(a.needs_warp(120.0));
        // Until the stretch is ready the original plays; then the copy, for that tempo
        // and mode only.
        assert!(Arc::ptr_eq(&a.audio_at(120.0).unwrap(), &data));
        let fast = Arc::new(SampleData::new(vec![0.1; 750], vec![0.1; 750]));
        a.warped = Some((120.0, WarpMode::Elastic, fast.clone()));
        assert!(Arc::ptr_eq(&a.audio_at(120.0).unwrap(), &fast));
        assert!(Arc::ptr_eq(&a.audio_at(100.0).unwrap(), &data));
        a.mode = WarpMode::Beats;
        assert!(Arc::ptr_eq(&a.audio_at(120.0).unwrap(), &data));
        a.warp = false;
        assert!(!a.needs_warp(120.0));
        // A recording from before tempos were kept (bpm 0) is left alone.
        let old = AudioSource { bpm: 0.0, ..AudioSource::new("x".into(), "x".into(), data, 0.0) };
        assert!(!old.needs_warp(120.0));
    }

    #[test]
    fn deleting_everything_empties_the_arrangement() {
        let mut a = Arrangement::default();
        a.put_loop(0, &[1, 2], 0, 4 * BAR);
        a.put_loop(1, &[2, 5], 4 * BAR, 4 * BAR);
        a.place(7, clip(12, 2));
        a.brace = Some((0, 4 * BAR));
        let sections = (0..a.sections.len()).collect();
        assert_eq!(a.delete(a.all_clips(), sections), (5, 2));
        assert!(a.is_empty());
        assert_eq!(a.end(), 0);
        // The brace is a setting of the timeline, not part of the song.
        assert_eq!(a.brace, Some((0, 4 * BAR)));
    }

    #[test]
    fn deleting_a_section_takes_the_clips_inside_it() {
        let mut a = Arrangement::default();
        a.put_loop(0, &[1, 2], 0, 4 * BAR);
        a.put_loop(1, &[1], 4 * BAR, 4 * BAR);
        // One clip picked from the second section, and the first section: the clip goes,
        // and so does everything under the first section, picked or not. The second
        // section stays, now empty.
        let picked = vec![(1, 1)];
        assert_eq!(a.delete(picked, vec![0]), (1, 1));
        assert!(a.all_clips().is_empty());
        assert_eq!(a.sections.iter().map(|s| (s.start / BAR, s.scene)).collect::<Vec<_>>(), vec![(4, 1)]);
    }

    #[test]
    fn duplicating_sections_and_clips_copies_them_after_the_last() {
        let mut a = Arrangement::default();
        a.put_loop(0, &[1, 2], 0, 4 * BAR);
        a.put_loop(1, &[1], 4 * BAR, 2 * BAR);
        a.place(9, clip(2, 6));
        // Both sections and the long clip: the run is bars 0 to 8, so copies go at 8.
        let (clips, sections) = a.duplicate(&[(9, 0)], &[0, 1]);
        assert_eq!(a.sections.iter().map(|s| (s.start / BAR, s.scene)).collect::<Vec<_>>(), vec![(0, 0), (4, 1), (8, 0), (12, 1)]);
        assert_eq!(a.clips(1).iter().map(|c| c.start / BAR).collect::<Vec<_>>(), vec![0, 4, 8, 12]);
        assert_eq!(a.clips(2).iter().map(|c| c.start / BAR).collect::<Vec<_>>(), vec![0, 8]);
        assert_eq!(a.clips(9).iter().map(|c| (c.start / BAR, c.len / BAR)).collect::<Vec<_>>(), vec![(2, 6), (10, 6)]);
        // The copies are selected, so duplicating again carries on.
        assert_eq!(sections, vec![2, 3]);
        let mut picked: Vec<(usize, u64)> = clips.iter().map(|&(p, i)| (p, a.clips(p)[i].start / BAR)).collect();
        picked.sort_unstable();
        assert_eq!(picked, vec![(1, 8), (1, 12), (2, 8), (9, 10)]);
        let (_, sections) = a.duplicate(&clips, &sections);
        assert_eq!(sections, vec![4, 5]);
        assert_eq!(a.end(), 24 * BAR);
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
