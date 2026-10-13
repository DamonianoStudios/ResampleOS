//! Vocal takes: the data behind a vocal pad, and turning it into audio.
//!
//! A vocal pad keeps every pass that was recorded on it as a lane of clips. The *comp*
//! says which take is heard where; play-through lanes sound alongside it. None of this
//! reaches the engine: whenever the stack changes it is rendered to one buffer, and that
//! buffer is what the pad plays. Positions are frames at the engine's sample rate.

use crate::sample::SampleData;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Frames summarised by each entry of a take's peak cache.
pub const PEAK_HOP: usize = 256;
/// Length of the fade at every clip edge and of the crossfade where the comp changes take.
pub const FADE_SECS: f32 = 0.002;

/// Looks up a pitch-shifted copy of a take: (take, semitones) to audio, if one is ready.
pub type Shifter<'a> = dyn Fn(&Arc<TakeAudio>, f32) -> Option<Arc<SampleData>> + 'a;

/// A recorded pass, with what is needed to draw it quickly.
pub struct TakeAudio {
    pub data: Arc<SampleData>,
    peaks: Vec<(f32, f32)>,
}

impl TakeAudio {
    pub fn new(data: SampleData) -> Arc<TakeAudio> {
        let data = centred(data);
        let peaks = (0..data.len().div_ceil(PEAK_HOP))
            .map(|i| {
                let (a, b) = (i * PEAK_HOP, ((i + 1) * PEAK_HOP).min(data.len()));
                (a..b).fold((0f32, 0f32), |(lo, hi), j| {
                    let v = (data.l[j] + data.r[j]) * 0.5;
                    (lo.min(v), hi.max(v))
                })
            })
            .collect();
        Arc::new(TakeAudio { data: Arc::new(data), peaks })
    }

    pub fn len(&self) -> i64 {
        self.data.len() as i64
    }

    /// Lowest and highest sample between two frames, to the resolution of the cache.
    pub fn extent(&self, from: i64, to: i64) -> (f32, f32) {
        if to <= 0 || from >= self.len() || self.peaks.is_empty() {
            return (0.0, 0.0);
        }
        let a = (from.max(0) as usize / PEAK_HOP).min(self.peaks.len() - 1);
        let b = ((to.max(1) as usize - 1) / PEAK_HOP).min(self.peaks.len() - 1).max(a);
        self.peaks[a..=b].iter().fold((0f32, 0f32), |(lo, hi), p| (lo.min(p.0), hi.max(p.1)))
    }
}

/// A take that has sound on one channel only (one microphone on a stereo input) is put on
/// both, so it sits in the middle. A real stereo take is returned untouched.
fn centred(mut data: SampleData) -> SampleData {
    let energy = |v: &[f32]| v.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>();
    let (el, er) = (energy(&data.l), energy(&data.r));
    // 34 dB apart or more: the quiet side is noise or crosstalk, not a second microphone.
    if el > 0.0 && er < el * 4e-4 {
        data.r.copy_from_slice(&data.l);
    } else if er > 0.0 && el < er * 4e-4 {
        data.l.copy_from_slice(&data.r);
    }
    data
}

/// A stretch of a take placed on the timeline.
#[derive(Clone)]
pub struct Clip {
    pub audio: Arc<TakeAudio>,
    /// The frame of `audio` heard at `start`. Slipping a clip changes only this.
    pub offset: i64,
    pub start: i64,
    pub len: i64,
    pub muted: bool,
}

impl Clip {
    pub fn end(&self) -> i64 {
        self.start + self.len
    }
}

impl PartialEq for Clip {
    fn eq(&self, o: &Clip) -> bool {
        Arc::ptr_eq(&self.audio, &o.audio) && (self.offset, self.start, self.len, self.muted) == (o.offset, o.start, o.len, o.muted)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum LaneKind {
    /// Heard only where the comp selects it.
    Take,
    /// Always heard, alongside the comp: doubles, stacks, harmonies.
    Thru,
}

#[derive(Clone, PartialEq)]
pub struct Lane {
    pub name: String,
    pub kind: LaneKind,
    pub clips: Vec<Clip>,
    pub gain: f32,
    pub pan: f32,
    /// Pitch offset in semitones, applied to play-through lanes.
    pub semis: f32,
    pub mute: bool,
}

/// From `at` until the next segment starts, the comp plays `lane` (or nothing).
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct CompSeg {
    pub at: i64,
    pub lane: Option<usize>,
}

#[derive(Clone, PartialEq, Default)]
pub struct VocalStack {
    pub lanes: Vec<Lane>,
    pub comp: Vec<CompSeg>,
    /// Length of the timeline in frames.
    pub len: i64,
    /// Until the comp is edited by hand, each new take becomes the whole comp.
    pub hand_comped: bool,
    /// Where another loop took over during a free pass: frame and loop number.
    pub markers: Vec<(i64, u32)>,
    /// Takes recorded so far, for naming the next.
    pub takes_made: u32,
}

impl VocalStack {
    pub fn is_empty(&self) -> bool {
        self.lanes.is_empty()
    }

    /// The comp as (from, to, lane) runs covering the whole timeline.
    pub fn comp_runs(&self) -> Vec<(i64, i64, Option<usize>)> {
        let mut runs = Vec::new();
        let mut at = 0;
        let mut lane = None;
        for seg in &self.comp {
            let next = seg.at.clamp(0, self.len);
            if next > at {
                runs.push((at, next, lane));
            }
            at = next;
            lane = seg.lane;
        }
        if self.len > at {
            runs.push((at, self.len, lane));
        }
        runs
    }

    pub fn comp_lane_at(&self, frame: i64) -> Option<usize> {
        self.comp.iter().take_while(|s| s.at <= frame).last().and_then(|s| s.lane)
    }

    /// Makes the comp play `lane` between two frames, leaving the rest as it was.
    pub fn set_comp(&mut self, from: i64, to: i64, lane: Option<usize>) {
        let (from, to) = (from.clamp(0, self.len), to.clamp(0, self.len));
        if to <= from {
            return;
        }
        let after = self.comp_lane_at(to);
        self.comp.retain(|s| s.at < from || s.at > to);
        self.comp.push(CompSeg { at: from, lane });
        if to < self.len {
            self.comp.push(CompSeg { at: to, lane: after });
        }
        self.tidy_comp();
    }

    /// Sorts the comp and drops boundaries that change nothing.
    fn tidy_comp(&mut self) {
        self.comp.sort_by_key(|s| s.at);
        let mut current = None;
        let mut kept: Vec<CompSeg> = Vec::new();
        for seg in self.comp.drain(..) {
            // A later segment at the same position wins.
            if kept.last().is_some_and(|k| k.at == seg.at) {
                kept.pop();
                current = kept.last().and_then(|k| k.lane);
            }
            if seg.lane != current {
                current = seg.lane;
                kept.push(seg);
            }
        }
        self.comp = kept;
    }

    /// Adds a recorded pass as a new take lane holding one clip, and returns the lane.
    /// `range` limits the clip to part of the timeline (a punch-in).
    pub fn add_take(&mut self, audio: Arc<TakeAudio>, range: Option<(i64, i64)>) -> usize {
        if self.lanes.is_empty() {
            self.len = self.len.max(audio.len());
        }
        self.takes_made += 1;
        let (from, to) = range.unwrap_or((0, audio.len()));
        let (from, to) = (from.clamp(0, self.len), to.clamp(0, audio.len().min(self.len)));
        let clip = Clip { audio, offset: from, start: from, len: (to - from).max(0), muted: false };
        // New takes go above the play-through lanes, which stay at the bottom.
        let at = self.lanes.iter().position(|l| l.kind == LaneKind::Thru).unwrap_or(self.lanes.len());
        self.shift_comp_lanes(at, 1);
        self.lanes.insert(
            at,
            Lane { name: format!("Take {}", self.takes_made), kind: LaneKind::Take, clips: vec![clip], gain: 1.0, pan: 0.0, semis: 0.0, mute: false },
        );
        if range.is_some() {
            self.set_comp(from, to, Some(at));
        } else if !self.hand_comped {
            self.comp = vec![CompSeg { at: 0, lane: Some(at) }];
        }
        at
    }

    /// Renumbers comp references for lanes at or after `at` moving by `by`.
    fn shift_comp_lanes(&mut self, at: usize, by: i64) {
        for seg in self.comp.iter_mut() {
            if let Some(l) = seg.lane.as_mut()
                && *l >= at
            {
                *l = (*l as i64 + by) as usize;
            }
        }
    }

    pub fn remove_lane(&mut self, lane: usize) {
        if lane >= self.lanes.len() {
            return;
        }
        self.lanes.remove(lane);
        for seg in self.comp.iter_mut().filter(|s| s.lane == Some(lane)) {
            seg.lane = None;
        }
        self.shift_comp_lanes(lane + 1, -1);
        self.tidy_comp();
    }

    /// A copy of a lane that always plays, placed at the bottom. Returns its index.
    pub fn duplicate_as_thru(&mut self, lane: usize) -> Option<usize> {
        let source = self.lanes.get(lane)?;
        let copies = self.lanes.iter().filter(|l| l.kind == LaneKind::Thru).count();
        let mut copy = source.clone();
        copy.kind = LaneKind::Thru;
        copy.name = format!("Double {}", copies + 1);
        // Alternate sides, a little apart in pitch, so a pair of doubles spreads by itself.
        copy.pan = if copies % 2 == 0 { -0.45 } else { 0.45 };
        copy.semis = if copies % 2 == 0 { -0.05 } else { 0.05 };
        copy.gain = 0.7;
        self.lanes.push(copy);
        Some(self.lanes.len() - 1)
    }

    /// Cuts a clip in two at a timeline frame. Returns the index of the second half.
    pub fn split(&mut self, lane: usize, clip: usize, at: i64) -> Option<usize> {
        let clips = &mut self.lanes.get_mut(lane)?.clips;
        let c = clips.get(clip)?.clone();
        if at <= c.start || at >= c.end() {
            return None;
        }
        clips[clip].len = at - c.start;
        clips.insert(clip + 1, Clip { offset: c.offset + (at - c.start), start: at, len: c.end() - at, ..c });
        Some(clip + 1)
    }

    /// Moves a clip to a new start, and to another lane if `to_lane` differs. Returns
    /// where it ended up.
    pub fn move_clip(&mut self, lane: usize, clip: usize, start: i64, to_lane: usize) -> Option<(usize, usize)> {
        if to_lane >= self.lanes.len() || clip >= self.lanes.get(lane)?.clips.len() {
            return None;
        }
        let limit = self.len;
        let c = &mut self.lanes[lane].clips[clip];
        c.start = start.clamp(0, (limit - c.len).max(0));
        if to_lane == lane {
            return Some((lane, clip));
        }
        let moved = self.lanes[lane].clips.remove(clip);
        self.lanes[to_lane].clips.push(moved);
        Some((to_lane, self.lanes[to_lane].clips.len() - 1))
    }

    /// Slides the audio inside a clip without moving its edges. Positive `by` hears the
    /// take later, so what was sung arrives earlier.
    pub fn slip(&mut self, lane: usize, clip: usize, offset: i64) {
        if let Some(c) = self.lanes.get_mut(lane).and_then(|l| l.clips.get_mut(clip)) {
            // Keep at least a sliver of the take inside the clip.
            c.offset = offset.clamp(1 - c.len, c.audio.len() - 1);
        }
    }

    /// Copies a clip to `start` on the same lane. Returns the copy's index.
    pub fn duplicate_clip(&mut self, lane: usize, clip: usize, start: i64) -> Option<usize> {
        let limit = self.len;
        let clips = &mut self.lanes.get_mut(lane)?.clips;
        let mut copy = clips.get(clip)?.clone();
        if start >= limit {
            return None;
        }
        copy.start = start.max(0);
        copy.len = copy.len.min(limit - copy.start);
        clips.push(copy);
        Some(clips.len() - 1)
    }

    /// Mixes the comp and the play-through lanes down to one buffer.
    ///
    /// `shifted` supplies a pitch-shifted copy of a take for lanes that ask for one; while
    /// it has none ready the take is used as recorded.
    pub fn render(&self, sr: f32, shifted: &Shifter) -> SampleData {
        let n = self.len.max(0) as usize;
        let (mut l, mut r) = (vec![0f32; n], vec![0f32; n]);
        let fade = ((FADE_SECS * sr) as i64).max(2);
        let mut lay = |lane: &Lane, clip: &Clip, from: i64, to: i64| {
            let source = if lane.kind == LaneKind::Thru && lane.semis.abs() > 0.001 {
                shifted(&clip.audio, lane.semis).unwrap_or_else(|| clip.audio.data.clone())
            } else {
                clip.audio.data.clone()
            };
            let angle = (lane.pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
            let gains = (angle.cos() * std::f32::consts::SQRT_2 * lane.gain, angle.sin() * std::f32::consts::SQRT_2 * lane.gain);
            lay_piece((&mut l, &mut r), &source, clip.offset - clip.start, from, to, gains, fade);
        };
        for (lane_index, lane) in self.lanes.iter().enumerate() {
            if lane.mute {
                continue;
            }
            for clip in lane.clips.iter().filter(|c| !c.muted && c.len > 0) {
                match lane.kind {
                    LaneKind::Thru => lay(lane, clip, clip.start, clip.end()),
                    LaneKind::Take => {
                        // Each run of the comp reaches half a fade into its neighbours, so
                        // where the take changes the two overlap and crossfade.
                        for (a, b, _) in self.comp_runs().into_iter().filter(|run| run.2 == Some(lane_index)) {
                            let (from, to) = ((a - fade / 2).max(clip.start), (b + fade / 2).min(clip.end()));
                            if to > from {
                                lay(lane, clip, from, to);
                            }
                        }
                    }
                }
            }
        }
        SampleData::new(l, r)
    }
}

/// Adds `source` into the mix between two timeline frames with an equal-power fade at each
/// end. `shift` is the source frame that falls on timeline frame zero.
fn lay_piece(out: (&mut [f32], &mut [f32]), source: &SampleData, shift: i64, from: i64, to: i64, gains: (f32, f32), fade: i64) {
    let (from, to) = (from.max(0), to.min(out.0.len() as i64));
    let fade = fade.min((to - from) / 2).max(1);
    for t in from..to {
        let s = t + shift;
        if s < 0 || s >= source.len() as i64 {
            continue;
        }
        let edge = (t - from).min(to - 1 - t);
        let g = if edge < fade { ((edge as f32 + 0.5) / fade as f32 * std::f32::consts::FRAC_PI_2).sin() } else { 1.0 };
        out.0[t as usize] += source.l[s as usize] * gains.0 * g;
        out.1[t as usize] += source.r[s as usize] * gains.1 * g;
    }
}

/// Splits a vocal into phrases at its silences: (start, end) frame pairs.
pub fn phrases(s: &SampleData, sr: f32) -> Vec<(usize, usize)> {
    let hop = (sr * 0.01) as usize;
    let peak = s.peak().max(1e-6);
    let floor = (peak * 0.03).max(0.002);
    let loud: Vec<bool> = (0..s.len().div_ceil(hop.max(1)))
        .map(|i| (i * hop..((i + 1) * hop).min(s.len())).any(|j| s.l[j].abs().max(s.r[j].abs()) > floor))
        .collect();
    // A gap has to last this long to end a phrase; shorter ones are breaths and consonants.
    let gap = (0.18 / 0.01) as usize;
    let lead = (0.02 / 0.01) as usize;
    let mut out = Vec::new();
    let mut start = None;
    let mut quiet = 0;
    for (i, &on) in loud.iter().enumerate() {
        if on {
            start.get_or_insert(i.saturating_sub(lead));
            quiet = 0;
        } else if let Some(from) = start {
            quiet += 1;
            if quiet >= gap {
                out.push((from * hop, ((i + 1 - quiet + lead + 3) * hop).min(s.len())));
                start = None;
            }
        }
    }
    if let Some(from) = start {
        out.push((from * hop, s.len()));
    }
    // Anything too short to be a word is noise.
    out.retain(|(a, b)| b - a > (sr * 0.08) as usize);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_one_sided_take_is_centred() {
        let voice: Vec<f32> = (0..2000).map(|i| (i as f32 * 0.1).sin() * 0.4).collect();
        let take = TakeAudio::new(SampleData::new(vec![0.0; 2000], voice.clone()));
        assert_eq!(take.data.l, voice);
        assert_eq!(take.data.r, voice);
        // Two different channels are a stereo recording and stay as they are.
        let other: Vec<f32> = voice.iter().map(|v| v * -0.5).collect();
        let take = TakeAudio::new(SampleData::new(voice.clone(), other.clone()));
        assert_eq!(take.data.r, other);
    }

    use super::*;

    const SR: f32 = 48000.0;

    fn steady(value: f32, frames: usize) -> Arc<TakeAudio> {
        TakeAudio::new(SampleData::new(vec![value; frames], vec![value; frames]))
    }

    fn no_shift(_: &Arc<TakeAudio>, _: f32) -> Option<Arc<SampleData>> {
        None
    }

    fn two_takes() -> VocalStack {
        let mut stack = VocalStack::default();
        stack.add_take(steady(0.25, 48000), None);
        stack.add_take(steady(0.5, 48000), None);
        stack
    }

    #[test]
    fn newest_take_is_the_comp_until_it_is_edited_by_hand() {
        let mut stack = two_takes();
        assert_eq!(stack.len, 48000);
        assert_eq!(stack.comp_runs(), vec![(0, 48000, Some(1))]);
        stack.hand_comped = true;
        stack.add_take(steady(0.75, 48000), None);
        assert_eq!(stack.comp_runs(), vec![(0, 48000, Some(1))], "a hand-made comp is left alone");
    }

    #[test]
    fn swiping_replaces_a_range_and_merges_neighbours() {
        let mut stack = two_takes();
        stack.set_comp(12000, 24000, Some(0));
        assert_eq!(stack.comp_runs(), vec![(0, 12000, Some(1)), (12000, 24000, Some(0)), (24000, 48000, Some(1))]);
        // Swiping the same take over the gap puts it back to one run.
        stack.set_comp(10000, 30000, Some(1));
        assert_eq!(stack.comp_runs(), vec![(0, 48000, Some(1))]);
        // A swipe that runs to the end leaves nothing dangling after it.
        stack.set_comp(40000, 60000, Some(0));
        assert_eq!(stack.comp_runs(), vec![(0, 40000, Some(1)), (40000, 48000, Some(0))]);
    }

    #[test]
    fn render_plays_the_comp_and_crossfades_where_it_changes() {
        let mut stack = two_takes();
        stack.set_comp(24000, 48000, Some(0));
        let out = stack.render(SR, &no_shift);
        assert!((out.l[12000] - 0.5).abs() < 1e-6 && (out.l[36000] - 0.25).abs() < 1e-6);
        // Across the change there is no gap and no jump: the level moves from one take to
        // the other over the two-millisecond fade.
        let across = &out.l[23900..24100];
        assert!(across.iter().all(|v| (0.2..0.6).contains(v)), "{:?}", across.iter().cloned().fold(f32::MAX, f32::min));
        assert!(across.windows(2).all(|w| (w[1] - w[0]).abs() < 0.02));
        // The very start and end of the timeline are faded, not clicked.
        assert!(out.l[0] < 0.01 && out.l[47999] < 0.01);
    }

    #[test]
    fn muted_clips_and_take_lanes_outside_the_comp_are_silent() {
        let mut stack = two_takes();
        // Lane 0 is not in the comp at all: only lane 1 (0.5) is heard.
        assert!((stack.render(SR, &no_shift).l[20000] - 0.5).abs() < 1e-6);
        stack.lanes[1].clips[0].muted = true;
        assert_eq!(stack.render(SR, &no_shift).l[20000], 0.0);
    }

    #[test]
    fn play_through_lanes_sound_with_the_comp() {
        let mut stack = two_takes();
        let double = stack.duplicate_as_thru(1).unwrap();
        stack.lanes[double].pan = 0.0;
        stack.lanes[double].gain = 1.0;
        stack.lanes[double].semis = 0.0;
        let out = stack.render(SR, &no_shift);
        assert!((out.l[20000] - 1.0).abs() < 1e-5, "comp 0.5 plus its double 0.5, got {}", out.l[20000]);
    }

    #[test]
    fn split_move_and_slip_keep_the_audio_where_it_should_be() {
        // A ramp, so every frame of the take is recognisable.
        let ramp: Vec<f32> = (0..48000).map(|i| i as f32 / 48000.0).collect();
        let mut stack = VocalStack::default();
        stack.add_take(TakeAudio::new(SampleData::new(ramp.clone(), ramp)), None);
        let second = stack.split(0, 0, 24000).unwrap();
        assert_eq!((stack.lanes[0].clips[0].len, stack.lanes[0].clips[second].start), (24000, 24000));
        // Unmoved, the two halves still play the take straight through.
        let out = stack.render(SR, &no_shift);
        assert!((out.l[30000] - 30000.0 / 48000.0).abs() < 1e-5);
        // Moving the second half earlier brings its audio with it.
        stack.move_clip(0, second, 12000, 0);
        let out = stack.render(SR, &no_shift);
        assert!((out.l[13000] - (25000.0 + 12000.0) / 48000.0).abs() < 0.3, "two clips overlap here");
        assert_eq!(out.l[40000], 0.0, "and leaves silence where it was");
        // Slipping the first half changes what is heard without moving its edges.
        stack.move_clip(0, second, 24000, 0);
        stack.slip(0, 0, 1000);
        let out = stack.render(SR, &no_shift);
        assert!((out.l[5000] - 6000.0 / 48000.0).abs() < 1e-5);
        assert_eq!(stack.lanes[0].clips[0].start, 0);
    }

    #[test]
    fn removing_a_lane_keeps_the_comp_pointing_at_the_right_takes() {
        let mut stack = two_takes();
        stack.add_take(steady(0.75, 48000), None);
        stack.set_comp(0, 16000, Some(0));
        stack.set_comp(16000, 32000, Some(1));
        stack.remove_lane(0);
        assert_eq!(stack.comp_runs(), vec![(0, 16000, None), (16000, 32000, Some(0)), (32000, 48000, Some(1))]);
        assert!((stack.render(SR, &no_shift).l[40000] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn a_punch_in_takes_over_just_its_range() {
        let mut stack = two_takes();
        let lane = stack.add_take(steady(0.75, 48000), Some((10000, 20000)));
        assert_eq!(stack.lanes[lane].clips[0].start, 10000);
        assert_eq!(stack.comp_runs(), vec![(0, 10000, Some(1)), (10000, 20000, Some(2)), (20000, 48000, Some(1))]);
    }

    #[test]
    fn phrases_are_found_between_silences() {
        let mut v = vec![0f32; 96000];
        for range in [4800..24000, 48000..62400] {
            for i in range {
                v[i] = (i as f32 * 0.05).sin() * 0.5;
            }
        }
        let found = phrases(&SampleData::new(v.clone(), v), SR);
        assert_eq!(found.len(), 2);
        assert!((found[0].0 as i64 - 4800).abs() < 1500 && (found[0].1 as i64 - 24000).abs() < 2500, "{found:?}");
        assert!((found[1].0 as i64 - 48000).abs() < 1500);
    }
}
