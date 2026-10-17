//! Song mode: the engine playing the arrangement.
//!
//! The song clock (`song_pos`, in ticks) runs whenever the transport does, in either mode,
//! so the arrangement can be picked up again where it would have been. In song mode it is
//! also the playhead: `tick` follows it, nothing wraps except at the loop brace, and the
//! pads play the clips on their tracks instead of their loop patterns.

use super::*;
use super::render::{pan_gains, pattern_notes, report_level, send_out};

impl Engine {
    /// Sequences one block in song mode. `span` is its length in ticks.
    pub(super) fn sequence_song(&mut self, span: f64, tps: f64, events: &mut Vec<Event>) {
        let (mut t0, mut left, mut done) = (self.song_pos, span, 0.0);
        let mut jumped = std::mem::take(&mut self.song_jump);
        loop {
            let end = match self.brace {
                Some((_, e)) if t0 < e => e,
                _ => f64::INFINITY,
            };
            let part = left.min(end - t0);
            self.collect_song(t0, part, done, tps, events);
            self.click(t0, part, done, tps);
            if self.song_nsegs < self.song_segs.len() {
                let (f0, f1) = ((done / tps).round() as usize, ((done + part) / tps).round() as usize);
                self.song_segs[self.song_nsegs] = (t0, f0, f1.saturating_sub(f0), jumped);
                self.song_nsegs += 1;
            }
            jumped = false;
            t0 += part;
            left -= part;
            done += part;
            // Reaching the end of the loop brace goes back to its start, as a loop goes
            // round. (Only from inside it: a playhead put beyond it plays on.)
            if end.is_finite() && t0 >= end - 1e-9
                && let Some((s, _)) = self.brace
            {
                t0 = s;
                jumped = true;
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

    /// Plays the audio clips under this block, sample-locked to the song clock: each
    /// stretch the block covers (two when the brace wraps in it) reads its clips from
    /// where that stretch is in the song. On stop, what was playing fades out.
    pub(super) fn render_song_audio(&mut self, n: usize, tps: f64) {
        let segs = std::mem::take(&mut self.song_nsegs);
        let Some(plan) = self.song.clone() else { return };
        let step = 1.0 / (0.004 * self.sr);
        if !self.playing {
            // Stopped while the song played: a few milliseconds more, fading to nothing.
            if !std::mem::take(&mut self.song_fading) || !self.song_mode {
                return;
            }
            let frames = n.min((1.0 / step) as usize);
            self.song_audio(&plan, (self.song_pos, 0, frames), tps, |i| (1.0 - step * i as f32).max(0.0));
            self.song_ramp = 0.0;
            return;
        }
        for k in 0..segs {
            let (t0, f0, frames, jumped) = self.song_segs[k];
            if jumped {
                self.song_ramp = 0.0;
            }
            let ramp = self.song_ramp;
            self.song_audio(&plan, (t0, f0, frames.min(n - f0.min(n))), tps, |i| (ramp + step * (i + 1) as f32).min(1.0));
            self.song_ramp = (ramp + step * frames as f32).min(1.0);
        }
    }

    /// Mixes the audio clips under `frames` frames from song tick `t0` into the buses,
    /// from frame `f0` of the block, times `ramp(i)` at frame `i` of the stretch.
    fn song_audio(&mut self, plan: &ArrangementPlan, (t0, f0, frames): (f64, usize, usize), tps: f64, ramp: impl Fn(usize) -> f32) {
        let t1 = t0 + frames as f64 * tps;
        // At least a few milliseconds of fade at each end of a clip, so edges never click.
        let declick = 0.003 * self.sr as f64 * tps;
        let any_solo = self.any_solo;
        for track in &plan.tracks {
            let Some(cfg) = self.pads.get(track.pad as usize).map(|p| p.cfg) else { continue };
            if cfg.mute || (any_solo && !cfg.solo) {
                continue;
            }
            let clips = &track.clips;
            // A crossfade plays a clip a little past its end, so start one clip early.
            let first = clips.partition_point(|c| c.end() as f64 <= t0).saturating_sub(1);
            for c in &clips[first..] {
                let Some(a) = &c.audio else {
                    if c.start as f64 >= t1 {
                        break;
                    }
                    continue;
                };
                let (cs, ce) = (c.start as f64, c.end() as f64);
                let (xin, xout) = (a.xin as f64, a.xout as f64);
                // What this clip sounds over: its span, and half of each crossfade beyond.
                let (from, to) = (cs - xin * 0.5, ce + xout * 0.5);
                if from >= t1 {
                    break;
                }
                if to <= t0 {
                    continue;
                }
                let (gl, gr) = pan_gains(cfg.pan, cfg.vol * a.gain);
                let (fade_in, fade_out) = ((a.fade_in as f64).max(declick), (a.fade_out as f64).max(declick));
                let len = a.data.len() as f64;
                let lo = ((from - t0) / tps).ceil().max(0.0) as usize;
                let hi = (((to - t0) / tps).ceil().max(0.0) as usize).min(frames);
                let (lo, hi) = (lo.min(hi), hi);
                self.vscratch[0][f0 + lo..f0 + hi].fill(0.0);
                self.vscratch[1][f0 + lo..f0 + hi].fill(0.0);
                let out = &mut self.vscratch;
                let mut peak = 0f32;
                for i in lo..hi {
                    let t = t0 + i as f64 * tps;
                    let pos = (t - cs + c.offset as f64) / tps;
                    if pos >= len {
                        break;
                    }
                    if pos < 0.0 {
                        continue;
                    }
                    // Equal-power across a crossfade; otherwise the clip's own fades.
                    let head = if xin > 0.0 {
                        let x = ((t - from) / xin).clamp(0.0, 1.0);
                        (x * std::f64::consts::FRAC_PI_2).sin() as f32
                    } else {
                        let e = ((t - cs) / fade_in).clamp(0.0, 1.0) as f32;
                        e * e * (3.0 - 2.0 * e)
                    };
                    let tail = if xout > 0.0 {
                        let x = ((to - t) / xout).clamp(0.0, 1.0);
                        (x * std::f64::consts::FRAC_PI_2).sin() as f32
                    } else {
                        let e = ((ce - t) / fade_out).clamp(0.0, 1.0) as f32;
                        e * e * (3.0 - 2.0 * e)
                    };
                    let g = head.min(tail) * ramp(i);
                    let (l, r) = (hermite(&a.data.l, pos) * g, hermite(&a.data.r, pos) * g);
                    out[0][f0 + i] += l * gl;
                    out[1][f0 + i] += r * gr;
                    peak = peak.max(l.abs()).max(r.abs());
                }
                send_out(&mut self.bufs, &cfg, &self.vscratch, f0 + lo..f0 + hi);
                if peak > 0.0 {
                    report_level(&self.shared, &mut self.touched, track.pad, (peak * cfg.vol * a.gain).max(0.05));
                }
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

    /// Applies the playing section's automation, then the song's own lanes over it,
    /// letting go of whatever the last section moved when the playhead crosses into
    /// another.
    pub(super) fn apply_song_automation(&mut self) {
        let now = self.song_lanes();
        let section = now.as_ref().map(|n| n.2);
        if section != self.song_section {
            self.song_section = section;
            self.release_automation();
        }
        self.buses = self.bus_base;
        if let Some((lanes, at, _)) = now {
            self.layer_lanes(&lanes, at);
        }
        if let Some(lanes) = self.song.as_ref().and_then(|p| p.automation.clone()) {
            self.layer_lanes(&lanes, self.song_pos);
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
                // Lanes may have gone: let go, and whatever still automates takes hold again
                // this block.
                if self.song_mode {
                    self.release_automation();
                }
                self.song_section = None;
            }
            SongCmd::Mode(on) => {
                if on == self.song_mode {
                    return;
                }
                self.song_mode = on;
                self.song_jump = true;
                self.silence_lingering();
                self.release_automation();
                self.song_section = None;
                // Into the song where it has got to; back to a loop from its top.
                self.tick = if on { self.song_pos } else { 0.0 };
            }
            SongCmd::Locate(t) => {
                self.song_pos = t.max(0.0);
                self.song_jump = true;
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
