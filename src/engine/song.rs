//! Song mode: the engine playing the arrangement.
//!
//! The song clock (`song_pos`, in ticks) runs whenever the transport does, in either mode,
//! so the arrangement can be picked up again where it would have been. In song mode it is
//! also the playhead: `tick` follows it, nothing wraps except at the loop brace, and the
//! pads play the clips on their tracks instead of their loop patterns.

use super::*;
use super::render::pattern_notes;

impl Engine {
    /// Sequences one block in song mode. `span` is its length in ticks.
    pub(super) fn sequence_song(&mut self, span: f64, tps: f64, events: &mut Vec<Event>) {
        let (mut t0, mut left, mut done) = (self.song_pos, span, 0.0);
        loop {
            let end = match self.brace {
                Some((_, e)) if t0 < e => e,
                _ => f64::INFINITY,
            };
            let part = left.min(end - t0);
            self.collect_song(t0, part, done, tps, events);
            self.click(t0, part, done, tps);
            t0 += part;
            left -= part;
            done += part;
            // Reaching the end of the loop brace goes back to its start, as a loop goes
            // round. (Only from inside it: a playhead put beyond it plays on.)
            if end.is_finite() && t0 >= end - 1e-9
                && let Some((s, _)) = self.brace
            {
                t0 = s;
                self.silence_lingering();
                self.scene_pass += 1;
            }
            if left <= 1e-9 {
                break;
            }
        }
        self.song_pos = t0;
        self.tick = t0;
    }

    /// Fires the clip notes that start in `[t0, t0 + span)`, and ends the pads whose clip
    /// ends there with nothing after it.
    fn collect_song(&mut self, t0: f64, span: f64, done: f64, tps: f64, events: &mut Vec<Event>) {
        let Some(plan) = self.song.clone() else { return };
        if span <= 0.0 {
            return;
        }
        let (t1, block_start) = (t0 + span, t0 - done);
        for track in &plan.tracks {
            let pad = track.pad;
            let Some(p) = self.pads.get(pad as usize) else { continue };
            if p.cfg.mute {
                continue;
            }
            let cfg = p.cfg;
            let clips = &track.clips;
            let first = clips.partition_point(|c| c.end() as f64 <= t0);
            let mut ended = false;
            for (k, c) in clips.iter().enumerate().skip(first) {
                let (start, end) = (c.start as f64, c.end() as f64);
                if start >= t1 {
                    break;
                }
                let (lo, hi) = (t0.max(start), t1.min(end));
                if hi > lo && c.pattern.len_ticks > 0 {
                    let before = events.len();
                    let local = lo - start + c.offset as f64;
                    let pass = (c.start / TICKS_PER_BAR as u64) as i64 + self.scene_pass * 7919;
                    pattern_notes(pad, &c.pattern, &cfg, self.swing, local, hi - lo, done + (lo - t0), pass, tps, events);
                    // Nothing sounds past the end of its clip.
                    let cut = ((end - block_start) / tps) as i64;
                    for ev in &mut events[before..] {
                        ev.gate = ev.gate.min((cut - ev.offset as i64).max(1));
                    }
                }
                let joined = clips.get(k + 1).is_some_and(|n| n.start == c.end());
                if end > t0 && end <= t1 && !joined {
                    ended = true;
                }
            }
            if ended && lingers(p, self.sr) {
                self.silence_pad(pad);
            }
        }
    }

    /// The automation of the section under the playhead, and where in its loop that is.
    pub(super) fn song_lanes(&self) -> Option<(Automation, f64, usize)> {
        let plan = self.song.as_ref()?;
        let pos = self.song_pos;
        let i = plan.sections.partition_point(|s| (s.start as f64) <= pos).checked_sub(1)?;
        let s = &plan.sections[i];
        if pos >= (s.start + s.len) as f64 || s.loop_len == 0 {
            return None;
        }
        let lanes = s.automation.clone()?;
        Some((lanes, (pos - s.start as f64).rem_euclid(s.loop_len as f64), i))
    }

    /// Applies the playing section's automation, letting go of whatever the last section
    /// moved when the playhead crosses into another.
    pub(super) fn apply_song_automation(&mut self) {
        let now = self.song_lanes();
        let section = now.as_ref().map(|n| n.2);
        if section != self.song_section {
            self.song_section = section;
            self.release_automation();
        }
        if let Some((lanes, at, _)) = now {
            self.apply_lanes(&lanes, at);
        }
    }

    /// Stops what would otherwise hang on across a jump: long samples and held synth or
    /// plugin notes. Short hits ring out.
    pub(super) fn silence_lingering(&mut self) {
        for pad in 0..self.pads.len() {
            let sequenced = self.pads[pad].pattern.is_some() || self.song_pads.contains(&(pad as u16));
            if sequenced && lingers(&self.pads[pad], self.sr) {
                self.silence_pad(pad as u16);
            }
        }
    }

    pub(super) fn handle_song(&mut self, cmd: SongCmd) {
        match cmd {
            SongCmd::Plan(plan) => {
                self.song_pads.clear();
                if let Some(p) = &plan {
                    for t in &p.tracks {
                        if self.song_pads.len() < self.song_pads.capacity() {
                            self.song_pads.push(t.pad);
                        }
                    }
                }
                if let Some(old) = std::mem::replace(&mut self.song, plan) {
                    self.trash(Garbage::Song(old));
                }
                self.song_section = None;
            }
            SongCmd::Mode(on) => {
                if on == self.song_mode {
                    return;
                }
                self.song_mode = on;
                self.silence_lingering();
                self.release_automation();
                self.song_section = None;
                // Into the song where it has got to; back to a loop from its top.
                self.tick = if on { self.song_pos } else { 0.0 };
            }
            SongCmd::Locate(t) => {
                self.song_pos = t.max(0.0);
                if self.song_mode {
                    self.tick = self.song_pos;
                    if self.playing {
                        self.silence_lingering();
                    }
                }
            }
            SongCmd::Brace(b) => self.brace = b.filter(|(s, e)| e > s),
        }
        self.shared.tick.store(self.tick.to_bits(), Ordering::Relaxed);
        self.shared.song_pos.store(self.song_pos.to_bits(), Ordering::Relaxed);
    }
}

/// Whether a pad's sound would carry on over a jump in time.
fn lingers(p: &PadRt, sr: f32) -> bool {
    let long = p.sample.as_ref().is_some_and(|s| s.len() as f32 > sr * 2.0);
    match p.cfg.kind {
        PadKind::Midi => true,
        PadKind::Audio => long,
        PadKind::Vocal => true,
        PadKind::Drum => false,
    }
}
