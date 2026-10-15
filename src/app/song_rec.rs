//! Recording onto the song timeline, and the performance history behind Capture
//! performance.
//!
//! Everything played while the transport runs goes into a history with its song time:
//! pad hits (as notes), loop passes, and moves of automatable controls. **Capture
//! performance** writes the last bars of it onto the timeline after the fact, as clips,
//! sections and song lanes. With **Rec** on, the same happens as you go for what is armed:
//! armed audio and vocal tracks record the input (one take per pass of the loop brace),
//! armed instrument and drum tracks record the notes played on them, and loops launched
//! while recording become sections.

use super::arrange::AudioSource;
use super::*;
use crate::engine::{AutoLane, AutoPoint, AutoTarget, Pattern};
use std::collections::VecDeque;
use std::time::Instant;

/// How much history is kept, in seconds of playing.
const HISTORY_SECS: f64 = 600.0;
/// Notes shorter than this (or never let go) are given this length, in ticks.
const MIN_NOTE: f64 = 30.0;
const BAR: u64 = TICKS_PER_BAR as u64;

/// One thing played, at its song time.
#[derive(Clone, Copy, Debug)]
pub enum PerfEvent {
    Note { pad: usize, pitch: u8, vel: u8, at: f64, len: f64 },
    /// A pass of loop `scene` began.
    Loop { scene: usize, at: f64 },
    Control { target: AutoTarget, value: f32, at: f64 },
}

impl PerfEvent {
    fn at(&self) -> f64 {
        match *self {
            PerfEvent::Note { at, .. } | PerfEvent::Loop { at, .. } | PerfEvent::Control { at, .. } => at,
        }
    }
}

/// A recording in progress.
struct Take {
    /// Song tick and input-ring frame where it began.
    tick: f64,
    frame: u64,
    /// When it began, to pick its events out of the history.
    since: f64,
    /// The input from `frame` on, copied out of the ring as it comes.
    l: Vec<f32>,
    r: Vec<f32>,
    /// The loop brace while recording.
    brace: Option<(u64, u64)>,
}

#[derive(Default)]
pub struct SongRec {
    /// Rec: armed tracks record while the transport runs.
    pub on: bool,
    pub armed: Vec<usize>,
    /// What has been played, oldest first, each with when (seconds of play).
    pub history: VecDeque<(f64, PerfEvent)>,
    /// Notes still held: (key, pad, pitch, vel, song tick, when).
    held: Vec<(u32, usize, u8, u8, f64, f64)>,
    take: Option<Take>,
    /// Seconds played before the current run, and when the current run began.
    clock: f64,
    run: Option<Instant>,
    seen_plays: u32,
    seen_loops: u32,
    play_frame: Option<u64>,
}

impl SongRec {
    /// Seconds of play so far: the history's clock, which only runs with the transport.
    fn now(&self) -> f64 {
        self.clock + self.run.map_or(0.0, |r| r.elapsed().as_secs_f64())
    }

    fn push(&mut self, e: PerfEvent) {
        let now = self.now();
        self.history.push_back((now, e));
        while self.history.front().is_some_and(|(t, _)| now - *t > HISTORY_SECS) {
            self.history.pop_front();
        }
    }
}

impl App {
    /// A hit on `pad` begins (from `note_on`).
    pub(super) fn perf_note_on(&mut self, pad: usize, semis: f32, vel: f32, key: u32) {
        if !self.playing() || pad >= self.pads.len() {
            return;
        }
        let semis = if self.pads[pad].cfg.kind == PadKind::Midi { semis } else { 0.0 };
        let pitch = (60.0 + semis).round().clamp(0.0, 127.0) as u8;
        let (tick, now) = (self.song_pos(), self.song_rec.now());
        self.song_rec.held.retain(|h| h.0 != key);
        self.song_rec.held.push((key, pad, pitch, (vel * 127.0).clamp(1.0, 127.0) as u8, tick, now));
    }

    /// The hit under `key` ends: it goes into the history as a note.
    pub(super) fn perf_note_off(&mut self, key: u32) {
        let Some(at) = self.song_rec.held.iter().position(|h| h.0 == key) else { return };
        let (_, pad, pitch, vel, tick, when) = self.song_rec.held.swap_remove(at);
        let len = (self.song_pos() - tick).max(MIN_NOTE);
        let len = if len > 64.0 * BAR as f64 { MIN_NOTE } else { len };
        // In the history at the time it began.
        let e = PerfEvent::Note { pad, pitch, vel, at: tick, len };
        let k = self.song_rec.history.partition_point(|(t, _)| *t <= when);
        self.song_rec.history.insert(k, (when, e));
    }

    /// A control was moved to `value` (lane units) while playing.
    pub(super) fn perf_control(&mut self, target: AutoTarget, value: f32) {
        if self.playing() {
            let at = self.song_pos();
            self.song_rec.push(PerfEvent::Control { target, value, at });
        }
    }

    pub fn arm_track(&mut self, pad: usize, on: bool) {
        self.song_rec.armed.retain(|p| *p != pad);
        if on {
            self.song_rec.armed.push(pad);
        }
    }

    pub fn is_armed(&self, pad: usize) -> bool {
        self.song_rec.armed.contains(&pad)
    }

    /// Rec on or off. Turned off while recording, what was recorded is laid down.
    pub fn toggle_song_rec(&mut self) {
        self.song_rec.on ^= true;
        if self.song_rec.on {
            let armed = self.song_rec.armed.len();
            let audio = self.song_rec.armed.iter().any(|&p| self.records_audio(p));
            self.status = if armed == 0 {
                "Rec on: launched loops are recorded as sections. Arm a track (R on its header) to record onto it.".into()
            } else if audio && self.input_sel.is_none() {
                "Rec on, but no input is chosen (Input, at the bottom): an armed audio track would record silence".into()
            } else {
                format!("Rec on: {armed} armed track{} record while playing", if armed == 1 { "" } else { "s" })
            };
        } else if self.song_rec.take.is_some() {
            self.finish_take();
        }
    }

    /// Whether an armed `pad` records the input rather than the notes played on it: a
    /// vocal pad, or an audio pad with no sample to play.
    pub fn records_audio(&self, pad: usize) -> bool {
        self.pads.get(pad).is_some_and(|p| p.cfg.kind == PadKind::Vocal || p.is_empty())
    }

    /// A pad with nothing on it, made a track to record audio onto, and armed.
    pub fn new_recording_track(&mut self) -> Option<usize> {
        let pad = self.free_track_pad()?;
        let n = self.song_rec.armed.len() + 1;
        self.pads[pad].name = format!("Rec {n}");
        self.arm_track(pad, true);
        self.status = format!("{} is a new track for recording, armed", pad_label(pad));
        Some(pad)
    }

    /// Every frame: keeps the history's clock and loop passes, and the recording going.
    pub(super) fn poll_song_rec(&mut self) {
        let playing = self.playing();
        let rec = &mut self.song_rec;
        match (playing, rec.run) {
            (true, None) => rec.run = Some(Instant::now()),
            (false, Some(r)) => {
                rec.clock += r.elapsed().as_secs_f64();
                rec.run = None;
            }
            _ => {}
        }
        let plays = self.shared.play_starts.load(Ordering::Relaxed);
        let loops = self.shared.scene_starts.load(Ordering::Relaxed);
        let fresh = plays != rec.seen_plays;
        let round = loops != rec.seen_loops;
        rec.seen_plays = plays;
        rec.seen_loops = loops;
        if fresh {
            rec.play_frame = Some(self.shared.loop_frame.load(Ordering::Relaxed));
        }
        // The song tick of an input frame, from the last block's anchor.
        let (anchor_frame, anchor_tick) = self.shared.song_anchor();
        let tpf = self.bpm as f64 / 60.0 * PPQ as f64 / self.sr() as f64;
        let anchored = playing && self.song_rec.play_frame.is_none_or(|f| anchor_frame >= f);
        // Loop passes, while the loops (not the song) play.
        if playing && !self.song_mode && (fresh || round) && anchored {
            let frame = self.shared.loop_frame.load(Ordering::Relaxed);
            let at = anchor_tick + (frame as f64 - anchor_frame as f64) * tpf;
            let scene = self.shared.scene_index.load(Ordering::Relaxed) as usize;
            self.song_rec.push(PerfEvent::Loop { scene, at: at.max(0.0) });
        }

        // Recording.
        if self.song_rec.on && playing && self.song_rec.take.is_none() && anchored {
            // From where playback started, if it has only just; otherwise from now.
            let (frame, tick) = match self.song_rec.play_frame.filter(|f| *f == anchor_frame) {
                Some(f) => (f, anchor_tick),
                None => (anchor_frame, anchor_tick),
            };
            let brace = self.arrangement.brace.filter(|_| self.arrangement.brace_on && self.song_mode);
            let since = self.song_rec.now();
            self.song_rec.take = Some(Take { tick, frame, since, l: Vec::new(), r: Vec::new(), brace });
            self.status = "Recording".into();
        }
        if let Some(t) = self.song_rec.take.as_mut() {
            let from = t.frame + t.l.len() as u64;
            let to = self.shared.input_ring.written();
            if to > from {
                let (l, r) = self.shared.input_ring.read(from, to);
                t.l.extend_from_slice(&l);
                t.r.extend_from_slice(&r);
            }
            if !playing || !self.song_rec.on {
                self.finish_take();
            }
        }
    }

    /// Lays down what was recorded: loop passes as sections, notes on armed instrument
    /// tracks, the input on armed audio tracks.
    fn finish_take(&mut self) {
        let Some(t) = self.song_rec.take.take() else { return };
        let events: Vec<PerfEvent> = self.song_rec.history.iter().filter(|(w, _)| *w >= t.since - 0.05).map(|(_, e)| *e).collect();
        let tpf = self.bpm as f64 / 60.0 * PPQ as f64 / self.sr() as f64;
        let end = t.tick + t.l.len() as f64 * tpf;
        let armed = self.song_rec.armed.clone();
        let mut parts = Vec::new();
        // Sections from the loops launched while recording (only when the loops played).
        let n = self.write_loops(&events, end);
        if n > 0 {
            parts.push(format!("{n} section{}", if n == 1 { "" } else { "s" }));
        }
        // Notes, on armed instrument tracks.
        let note_pads: Vec<usize> = armed.iter().copied().filter(|&p| !self.records_audio(p)).collect();
        let n = self.write_notes(&events, Some(&note_pads));
        if n > 0 {
            parts.push(format!("notes on {n} track{}", if n == 1 { "" } else { "s" }));
        }
        // Audio, on armed audio tracks.
        let audio_pads: Vec<usize> = armed.iter().copied().filter(|&p| self.records_audio(p)).collect();
        if !audio_pads.is_empty() && !t.l.is_empty() {
            let takes = self.write_audio(&t, &audio_pads);
            parts.push(format!("audio on {} track{} ({takes} take{})", audio_pads.len(), if audio_pads.len() == 1 { "" } else { "s" }, if takes == 1 { "" } else { "s" }));
        }
        self.status = if parts.is_empty() { "Recorded nothing: no armed track was played and no loop was launched".into() } else { format!("Recorded {}", parts.join(", ")) };
    }

    /// The input of a take onto `pads`: one recording per pass of the loop brace, the last
    /// complete one playing and the others kept as its takes. Returns how many passes.
    fn write_audio(&mut self, t: &Take, pads: &[usize]) -> usize {
        let sr = self.sr();
        let tpf = self.bpm as f64 / 60.0 * PPQ as f64 / sr as f64;
        // The input heard at a moment arrives this much later.
        let lat = (self.prefs.input_latency_ms.max(0.0) / 1000.0 * sr) as usize;
        let total = t.l.len().saturating_sub(lat);
        let passes = brace_passes(t.tick, total, tpf, t.brace);
        let stamp = self.song_rec.history.len();
        let mut sources = Vec::new();
        for (k, &(at, first, frames, complete)) in passes.iter().enumerate() {
            if frames < (sr * 0.05) as usize {
                continue;
            }
            let (a, b) = (lat + first, lat + first + frames);
            let data = Arc::new(SampleData::new(t.l[a..b].to_vec(), t.r[a..b].to_vec()));
            let name = format!("Take {}", k + 1);
            let pool = &mut self.arrangement.audio;
            let file = format!("{:03}_rec{stamp}_{}.wav", pool.len() + 1, k + 1);
            pool.push(AudioSource::new(file, name, data, self.bpm));
            sources.push((pool.len() - 1, at, frames, complete));
        }
        // Complete passes over the same stretch are takes of one another.
        let full: Vec<usize> = sources.iter().filter(|s| s.3 && t.brace.is_some_and(|(b, _)| s.1 == b as f64)).map(|s| s.0).collect();
        for &src in &full {
            self.arrangement.audio[src].takes = full.clone();
        }
        // Laid down in order, so later passes cover earlier ones; a last part-pass is laid
        // down only if no complete pass covers its stretch.
        let last_full = full.last().copied();
        for &(src, at, frames, complete) in &sources {
            if !complete && last_full.is_some() {
                continue;
            }
            let start = at.round().max(0.0) as u64;
            let len = ((frames as f64 * tpf).round() as u64).max(super::arrange::MIN_CLIP);
            for &pad in pads {
                self.arrangement.place(pad, Clip::audio(start, len, src));
            }
        }
        sources.len()
    }

    /// Notes from `events` as clips with notes of their own, one per pad played (only
    /// `pads`, if given), spanning the bars they were played in. Returns how many tracks.
    fn write_notes(&mut self, events: &[PerfEvent], pads: Option<&[usize]>) -> usize {
        let clips = note_clips(events, pads);
        for (pad, start, pattern) in &clips {
            let len = pattern.len_ticks as u64;
            self.arrangement.patterns.push(pattern.clone());
            let id = self.arrangement.patterns.len() - 1;
            self.arrangement.place(*pad, Clip::own_notes(*start, len, id));
        }
        clips.len()
    }

    /// Loop passes from `events` as sections (and their clips), each running until the
    /// next loop began, the last until `end`. Returns how many sections.
    fn write_loops(&mut self, events: &[PerfEvent], end: f64) -> usize {
        let mut n = 0;
        for (scene, from, to) in loop_runs(events, end) {
            let (a, b) = (from.round().max(0.0) as u64, to.round().max(0.0) as u64);
            if scene >= self.scenes.len() || b <= a + super::arrange::MIN_CLIP {
                continue;
            }
            let pads = self.pads_in_loop(scene);
            self.arrangement.put_loop(scene, &pads, a, b - a);
            n += 1;
        }
        n
    }

    /// Control moves from `events` into the song's lanes, replacing what the lanes had
    /// between the first and last move of each control. Returns how many controls.
    fn write_controls(&mut self, events: &[PerfEvent]) -> usize {
        let mut targets: Vec<AutoTarget> = Vec::new();
        for e in events {
            if let PerfEvent::Control { target, .. } = *e
                && !targets.contains(&target)
            {
                targets.push(target);
            }
        }
        for &target in &targets {
            let moves: Vec<(u32, f32)> = events
                .iter()
                .filter_map(|e| match *e {
                    PerfEvent::Control { target: t, value, at } if t == target => Some((at.max(0.0) as u32, value)),
                    _ => None,
                })
                .collect();
            let (lo, hi) = (moves.iter().map(|m| m.0).min().unwrap_or(0), moves.iter().map(|m| m.0).max().unwrap_or(0));
            let base = self.control_value(target);
            let lanes = &mut self.arrangement.automation;
            let lane = match lanes.iter().position(|l| l.target == target) {
                Some(i) => &mut lanes[i],
                None => {
                    lanes.push(AutoLane { target, points: vec![AutoPoint::new(0, base)] });
                    lanes.last_mut().expect("just pushed")
                }
            };
            lane.points.retain(|p| p.tick < lo || p.tick > hi);
            for (tick, value) in moves {
                let at = lane.points.partition_point(|p| p.tick <= tick);
                lane.points.insert(at, AutoPoint::new(tick, value));
            }
        }
        targets.len()
    }

    /// Writes the last `bars` bars of what was played onto the timeline, where it was
    /// played: loops as sections, hits as clips of notes, control moves as song lanes.
    pub fn capture_performance(&mut self, bars: u32) {
        let secs = bars as f64 * 4.0 * 60.0 / self.bpm as f64;
        let now = self.song_rec.now();
        let since = now - secs;
        let mut events: Vec<PerfEvent> = self.song_rec.history.iter().filter(|(w, _)| *w >= since).map(|(_, e)| *e).collect();
        // Notes still held count as far as they have got.
        let pos = self.song_pos();
        for h in &self.song_rec.held {
            if h.5 >= since {
                events.push(PerfEvent::Note { pad: h.1, pitch: h.2, vel: h.3, at: h.4, len: (pos - h.4).max(MIN_NOTE) });
            }
        }
        // A loop that was already playing when the stretch began counts from its start.
        let loop_before = self.song_rec.history.iter().rev().find(|(w, e)| *w < since && matches!(e, PerfEvent::Loop { .. })).map(|(_, e)| *e);
        if let Some(PerfEvent::Loop { scene, at }) = loop_before
            && !events.iter().any(|e| matches!(e, PerfEvent::Loop { .. }))
        {
            let from = events.iter().map(|e| e.at()).fold(pos, f64::min);
            let len = self.scenes.get(scene).map_or(BAR, |s| s.bars as u64 * BAR) as f64;
            // Back to the start of the pass the stretch began in.
            let start = at + ((from - at) / len).floor().max(0.0) * len;
            events.insert(0, PerfEvent::Loop { scene, at: start });
        }
        if events.is_empty() {
            self.status = format!("Nothing was played in the last {bars} bars");
            return;
        }
        let end = if self.playing() { pos } else { events.iter().map(|e| match *e { PerfEvent::Note { at, len, .. } => at + len, e => e.at() }).fold(0.0, f64::max) };
        let loops = self.write_loops(&events, end);
        let notes = self.write_notes(&events, None);
        let controls = self.write_controls(&events);
        self.view = View::Arrange;
        self.status = format!("Captured the last {bars} bars: {loops} section{}, notes on {notes} track{}, {controls} control{}", plural(loops), plural(notes), plural(controls));
    }
}

/// How a take splits into passes of the loop brace: (song tick it starts at, first
/// frame, frames, whether it is a whole pass). Recording began at `tick`; `frames` came in,
/// `tpf` ticks each. The first pass runs from where recording began to the brace's end;
/// the next ones round the brace; the last may stop part-way.
fn brace_passes(tick: f64, frames: usize, tpf: f64, brace: Option<(u64, u64)>) -> Vec<(f64, usize, usize, bool)> {
    let Some((s, e)) = brace.filter(|(_, e)| tick < *e as f64) else { return vec![(tick, 0, frames, true)] };
    let (s, e) = (s as f64, e as f64);
    let mut passes = Vec::new();
    let (mut at, mut frame) = (tick, 0usize);
    while frame < frames {
        let whole = ((e - at) / tpf).round() as usize;
        let n = whole.min(frames - frame);
        if n == 0 {
            break;
        }
        passes.push((at, frame, n, n == whole));
        frame += n;
        at = s;
    }
    passes
}

/// Loop passes in `events` as runs of one loop: (loop, from, to) in song ticks, each until
/// the next loop began, the last until `end`.
fn loop_runs(events: &[PerfEvent], end: f64) -> Vec<(usize, f64, f64)> {
    let passes: Vec<(usize, f64)> = events.iter().filter_map(|e| if let PerfEvent::Loop { scene, at } = *e { Some((scene, at)) } else { None }).collect();
    let mut runs: Vec<(usize, f64, f64)> = Vec::new();
    for (k, &(scene, at)) in passes.iter().enumerate() {
        let until = passes.get(k + 1).map_or(end, |p| p.1);
        match runs.last_mut() {
            // Passes of the same loop in a row make one section.
            Some(r) if r.0 == scene && (r.2 - at).abs() < 1.0 => r.2 = until,
            _ => runs.push((scene, at, until)),
        }
    }
    runs
}

/// The notes in `events` (on `pads`, if given) as one clip per pad: (pad, start tick, the
/// notes as a pattern as long as the clip). A clip spans the whole bars its notes were
/// played in.
fn note_clips(events: &[PerfEvent], pads: Option<&[usize]>) -> Vec<(usize, u64, Pattern)> {
    // Per pad: (pitch, velocity, song tick, length).
    type Played = Vec<(u8, u8, f64, f64)>;
    let mut by_pad: Vec<(usize, Played)> = Vec::new();
    for e in events {
        let PerfEvent::Note { pad, pitch, vel, at, len } = *e else { continue };
        if pads.is_some_and(|p| !p.contains(&pad)) {
            continue;
        }
        match by_pad.iter_mut().find(|(p, _)| *p == pad) {
            Some((_, v)) => v.push((pitch, vel, at, len)),
            None => by_pad.push((pad, vec![(pitch, vel, at, len)])),
        }
    }
    by_pad
        .into_iter()
        .map(|(pad, notes)| {
            let first = notes.iter().map(|n| n.2).fold(f64::MAX, f64::min).max(0.0);
            let last = notes.iter().map(|n| n.2 + n.3).fold(0.0, f64::max);
            let start = (first as u64) / BAR * BAR;
            let end = (last.ceil() as u64).div_ceil(BAR).max(start / BAR + 1) * BAR;
            let len = end - start;
            let mut pattern = Pattern { len_ticks: len.min(u32::MAX as u64) as u32, notes: Vec::new() };
            for &(pitch, vel, at, l) in &notes {
                let s = (at - start as f64).max(0.0).round() as u32;
                pattern.notes.push(Note::new(s, (l.round() as u32).max(1), pitch, vel));
            }
            pattern.notes.sort_by_key(|n| (n.start, n.pitch));
            (pad, start, pattern)
        })
        .collect()
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_take_round_the_brace_splits_into_passes() {
        // Brace over bars 2 and 3 (960..2880 at 480 ticks a beat... 1920 a bar), recording
        // from bar 2.5, one tick a frame: half a pass, two whole ones, and a quarter.
        let bar = BAR as f64;
        let brace = Some((BAR, 3 * BAR));
        let frames = (0.5 * bar + 4.0 * bar + 0.5 * bar) as usize;
        let p = brace_passes(2.5 * bar, frames, 1.0, brace);
        let got: Vec<(f64, usize, bool)> = p.iter().map(|x| (x.0 / bar, x.2 / BAR as usize, x.3)).collect();
        assert_eq!(got, vec![(2.5, 0, true), (1.0, 2, true), (1.0, 2, true), (1.0, 0, false)]);
        assert_eq!(p.iter().map(|x| x.2).sum::<usize>(), frames);
        assert_eq!(p[1].1, (0.5 * bar) as usize);
        // No brace, or starting past it: one pass.
        assert_eq!(brace_passes(0.0, 1000, 1.0, None), vec![(0.0, 0, 1000, true)]);
        assert_eq!(brace_passes(4.0 * bar, 1000, 1.0, brace).len(), 1);
    }

    #[test]
    fn loop_passes_become_runs_and_notes_become_clips() {
        let bar = BAR as f64;
        let events = [
            PerfEvent::Loop { scene: 0, at: 0.0 },
            PerfEvent::Loop { scene: 0, at: 2.0 * bar },
            PerfEvent::Note { pad: 3, pitch: 60, vel: 100, at: 2.5 * bar, len: 240.0 },
            PerfEvent::Loop { scene: 2, at: 4.0 * bar },
            PerfEvent::Note { pad: 3, pitch: 64, vel: 90, at: 5.25 * bar, len: 120.0 },
            PerfEvent::Note { pad: 7, pitch: 60, vel: 127, at: 4.0 * bar, len: 30.0 },
        ];
        assert_eq!(loop_runs(&events, 6.0 * bar), vec![(0, 0.0, 4.0 * bar), (2, 4.0 * bar, 6.0 * bar)]);
        let clips = note_clips(&events, None);
        assert_eq!(clips.len(), 2);
        let (pad, start, p) = &clips[0];
        // Pad 3 played in bars 3 and 6: one clip over bars 3 to 6, notes where they were.
        assert_eq!((*pad, *start, p.len_ticks as u64), (3, 2 * BAR, 4 * BAR));
        assert_eq!(p.notes.iter().map(|n| (n.start, n.pitch)).collect::<Vec<_>>(), vec![((0.5 * bar) as u32, 60), ((3.25 * bar) as u32, 64)]);
        // Only the pads asked for.
        assert_eq!(note_clips(&events, Some(&[7])).len(), 1);
    }
}
