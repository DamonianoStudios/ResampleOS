//! Key/BPM harmonizer, warping, and printing bus effects into a pad.

use super::*;

impl App {
    pub fn open_harmonizer(&mut self) {
        let Some(s) = self.pads[self.selected].sample().cloned() else {
            self.status = "Harmonizer needs an audio pad".into();
            return;
        };
        if self.pads[self.selected].analysis.is_none() {
            self.pads[self.selected].analysis = Some(analysis::analyze(&s, self.sr()));
        }
        let a = self.pads[self.selected].analysis.as_ref().unwrap();
        self.harm = Harmonizer {
            open: true,
            bpm: if a.bpm > 0.0 { a.bpm } else { self.bpm },
            root: a.key_root,
            minor: a.key_minor,
            tempo: a.bpm > 0.0,
            key: true,
        };
    }

    /// Semitone shift that brings the harmonizer's source key onto the project key.
    pub fn conform_semis(&self, idx: usize) -> f32 {
        if !self.harm.key {
            return 0.0;
        }
        // Root to root by the shorter direction.
        let raw = ((self.key.root as i32 - self.harm.root as i32 + 6).rem_euclid(12) - 6) as f32;
        if self.conform_mode == ConformMode::Strict {
            return raw;
        }
        let Some(a) = &self.pads[idx].analysis else { return raw };
        if let Some(p) = a.pitch {
            return self.key.smart_shift(p, raw);
        }
        // Polyphonic: choose the shift that puts the most spectral weight on scale degrees.
        let mut best = (f32::MIN, raw);
        for shift in -6..=6 {
            let fit: f32 = (0..12)
                .filter(|&pc| self.key.contains(pc + shift))
                .map(|pc| a.pcp[pc as usize])
                .sum();
            let score = fit - 0.01 * (shift as f32 - raw).abs();
            if score > best.0 {
                best = (score, shift as f32);
            }
        }
        best.1
    }

    pub fn conform_ratio(&self) -> f64 {
        if self.harm.tempo && self.harm.bpm > 1.0 { self.harm.bpm as f64 / self.bpm as f64 } else { 1.0 }
    }

    pub fn conform(&mut self, idx: usize) {
        let ratio = self.conform_ratio();
        let semis = self.conform_semis(idx);
        let note = format!(
            "conformed {:.1} to {:.1} BPM, {:+.0} st",
            if self.harm.tempo { self.harm.bpm } else { self.bpm },
            self.bpm,
            semis
        );
        self.warp(idx, ratio, semis, note);
    }

    /// Time-stretch and pitch-shift a pad in the background, landing as a new take.
    pub fn warp(&mut self, idx: usize, ratio: f64, semis: f32, note: String) {
        let Some(src) = self.pads[idx].sample().cloned() else { return };
        if (ratio - 1.0).abs() < 1e-4 && semis.abs() < 1e-3 {
            self.status = "Already conformed".into();
            return;
        }
        let onsets = if self.preserve_transients { self.pads[idx].transients.clone() } else { Vec::new() };
        let sr = self.sr();
        let tx = self.jobs_tx.clone();
        self.busy += 1;
        self.status = "Warping…".into();
        std::thread::spawn(move || {
            let out = stretch::conform(&src, &onsets, ratio, semis, sr);
            let _ = tx.send(JobResult::Conformed { pad: idx, sample: out, note });
        });
    }

    /// Runs the pad's bus effects into the buffer itself.
    pub fn bake_fx(&mut self, idx: usize) {
        let bus = match self.pads[idx].cfg.bus {
            0 => MASTER,
            b => b as usize - 1,
        };
        let slots = self.buses[bus].slots;
        if !slots.iter().any(|s| s.on && s.kind != FxKind::None) {
            self.status = "That bus has no active effects to bake".into();
            return;
        }
        let sr = self.sr();
        let bpm = self.bpm;
        let tail = slots.iter().any(|s| s.on && matches!(s.kind, FxKind::Delay | FxKind::Reverb));
        self.bake(idx, "bus FX baked", |src, _| {
            let mut l = src.l.clone();
            let mut r = src.r.clone();
            if tail {
                let extra = (sr * 1.5) as usize;
                l.resize(l.len() + extra, 0.0);
                r.resize(r.len() + extra, 0.0);
            }
            let mut fx: Vec<Fx> = (0..3).map(|_| Fx::new(sr)).collect();
            let n = l.len();
            let mut at = 0;
            while at < n {
                let to = (at + 256).min(n);
                for (i, slot) in slots.iter().enumerate() {
                    if slot.on && slot.kind != FxKind::None {
                        fx[i].process(slot.kind, slot.params, bpm, &mut l[at..to], &mut r[at..to]);
                    }
                }
                at = to;
            }
            SampleData::new(l, r)
        });
        // Otherwise the pad would run through the same effects a second time.
        if self.pads[idx].cfg.bus != 0 {
            self.pads[idx].cfg.bus = 0;
            self.push_pad(idx);
        }
    }
}
