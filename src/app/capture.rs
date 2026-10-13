//! Getting audio in: external input, the skip-back buffer and resample-to-pad.

use super::*;

impl App {
    /// Opens (or with `None`, closes) the external input device.
    pub fn set_input(&mut self, sel: Option<usize>) {
        self._input = None;
        self.input_sel = None;
        let Some(i) = sel else {
            self.send(Cmd::SetInput { rx: None, ratio: 1.0 });
            self.status = "External input off".into();
            return;
        };
        match audio_io::open_input(i, self.shared.sample_rate) {
            Ok(h) => {
                self.send(Cmd::SetInput { rx: Some(h.rx), ratio: h.ratio });
                self.send(Cmd::InputCfg { dest: self.input_dest, gain: self.input_gain });
                self._input = Some(h.stream);
                self.input_sel = Some(i);
                self.status = format!("Input: {}", self.input_names.get(i).cloned().unwrap_or_default());
            }
            Err(e) => {
                self.send(Cmd::SetInput { rx: None, ratio: 1.0 });
                self.status = format!("Input failed: {e}");
            }
        }
    }

    pub fn frames_per_bar(&self) -> f64 {
        4.0 * 60.0 / self.bpm as f64 * self.sr() as f64
    }

    pub fn capture(&mut self, start: u64, end: u64, dest: Dest) {
        let (l, r) = self.shared.ring.read(start, end);
        if l.len() < 64 {
            self.status = "Skip-back: nothing in that range".into();
            return;
        }
        let s = SampleData::new(l, r);
        let secs = s.len() as f32 / self.sr();
        match dest {
            Dest::Slice => {
                // Park the capture on a pad, then chop it across the next empty bank.
                if let Some(idx) = self.next_empty_pad() {
                    self.set_sample(idx, s, Some("Skip-back".into()), None);
                    self.chop(idx);
                }
            }
            Dest::Pad(idx) => {
                self.set_sample(idx, s, Some("Skip-back".into()), None);
                self.select(idx);
                self.status = format!("Captured {secs:.1}s to {}", pad_label(idx));
            }
            Dest::NextEmpty => {
                if let Some(idx) = self.next_empty_pad() {
                    self.set_sample(idx, s, Some("Skip-back".into()), None);
                    self.select(idx);
                    self.bank = idx / 16;
                    self.status = format!("Captured {secs:.1}s to {}", pad_label(idx));
                }
            }
        }
    }

    /// `S`: the last 16 bars of output, onto the selected pad. A pad that already holds
    /// something is left alone and the capture goes to the next empty one instead.
    pub fn capture_last_bars(&mut self, bars: f64) {
        let end = self.shared.ring.written();
        let frames = (self.frames_per_bar() * bars) as u64;
        let start = end.saturating_sub(frames).max(self.shared.ring.oldest());
        let dest = if self.pads[self.selected].is_empty() { Some(self.selected) } else { self.next_empty_pad() };
        let Some(dest) = dest else {
            self.status = "Skip-back: no empty pad left".into();
            return;
        };
        self.capture(start, end, Dest::Pad(dest));
        if self.pads[dest].sample().is_some() {
            self.bank = dest / 16;
            self.pads[dest].cfg.mode = PlayMode::Loop;
            self.push_pad(dest);
        }
    }

    /// `R`: arm, cancel, or finish depending on where we are.
    pub fn resample_key(&mut self) {
        match self.resample {
            Resample::Idle => {
                self.resample = Resample::Armed;
                self.status = "Resample armed: hit the destination pad".into();
            }
            Resample::Armed => {
                self.resample = Resample::Idle;
                self.status = "Resample cancelled".into();
            }
            Resample::Recording { .. } => self.finish_recording(),
        }
    }

    pub(super) fn begin_recording(&mut self, target: usize) {
        if target >= self.pads.len() {
            return;
        }
        let now = self.shared.ring.written();
        let playing = self.playing();
        let start = if playing {
            // Already rolling: start on the next downbeat so the take is a clean loop.
            let bar = TICKS_PER_BAR as f64;
            let tick = self.shared.tick();
            let ticks_left = (tick / bar).ceil() * bar - tick;
            let tps = self.bpm as f64 / 60.0 * PPQ as f64 / self.sr() as f64;
            now + (ticks_left / tps) as u64
        } else {
            now
        };
        self.resample = Resample::Recording { target, start, bar_aligned: playing };
        self.status = format!("Recording to {}: press R to bake", pad_label(target));
    }

    pub(super) fn finish_recording(&mut self) {
        let Resample::Recording { target, start, bar_aligned } = self.resample else { return };
        self.resample = Resample::Idle;
        let end = self.shared.ring.written();
        if end <= start + 64 {
            self.status = "Resample: nothing recorded".into();
            return;
        }
        let (mut l, mut r) = self.shared.ring.read(start, end);
        if bar_aligned {
            let bar = self.frames_per_bar();
            let bars = (l.len() as f64 / bar).round().max(1.0);
            let want = (bars * bar).round() as usize;
            l.resize(want, 0.0);
            r.resize(want, 0.0);
        } else {
            // Free-time take: drop the silence before the first hit and after the tail.
            let loud = |i: usize| l[i].abs() > 0.001 || r[i].abs() > 0.001;
            let Some(first) = (0..l.len()).find(|&i| loud(i)) else {
                self.status = "Resample: only silence recorded".into();
                return;
            };
            let last = (0..l.len()).rev().find(|&i| loud(i)).unwrap_or(l.len() - 1);
            let a = first.saturating_sub(32);
            let b = (last + (0.15 * self.sr()) as usize).min(l.len());
            l = l[a..b].to_vec();
            r = r[a..b].to_vec();
        }
        let secs = l.len() as f32 / self.sr();
        self.set_sample(target, SampleData::new(l, r), Some("Resample".into()), None);
        let p = &mut self.pads[target];
        p.cfg.mode = if bar_aligned { PlayMode::Loop } else { PlayMode::OneShot };
        // The bake already contains its bus colour; send it clean from here on.
        p.cfg.bus = 0;
        self.push_pad(target);
        self.select(target);
        self.bank = target / 16;
        self.status = format!("Baked {secs:.1}s into {}", pad_label(target));
    }
}
