//! Pads and banks: samples, the take stack, copying, clearing and transient slicing.

use super::*;

/// Something the inspector asks to have done to a pad once its settings are committed.
#[derive(Clone, Copy, PartialEq)]
pub enum PadAction {
    SetKind(PadKind),
    Browse,
    MakeMidi,
    Chop,
    Crop,
    Normalize,
    Reverse,
    FadeIn,
    FadeOut,
    BakePitch,
    BakeFx,
    StepTake(i32),
    /// Bring back the drum synth that baked the active take.
    Resynth,
}

impl App {
    pub fn pad_action(&mut self, idx: usize, action: PadAction) {
        // A vocal pad's sample is rebuilt from its takes, so an edit to it would not last.
        let rewrites = !matches!(action, PadAction::SetKind(_) | PadAction::Chop | PadAction::MakeMidi | PadAction::Resynth);
        if rewrites && self.is_vocal(idx) {
            self.status = "A vocal pad plays its comp. Bake it to a pad (Vocal tab) to edit the audio.".into();
            return;
        }
        match action {
            PadAction::SetKind(kind) => self.set_kind(idx, kind),
            PadAction::Browse => self.browse_sample(idx),
            PadAction::MakeMidi => {
                self.set_kind(idx, PadKind::Midi);
                self.tab = Tab::Roll;
            }
            PadAction::Chop => self.chop(idx),
            PadAction::Crop => self.bake(idx, "cropped", |s, p| {
                let (a, b) = p.region();
                s.slice(a, b)
            }),
            PadAction::Normalize => self.bake(idx, "normalized", |s, _| s.normalized()),
            PadAction::Reverse => self.bake(idx, "reversed", |s, _| s.reversed()),
            PadAction::FadeIn | PadAction::FadeOut => {
                let fade_in = action == PadAction::FadeIn;
                self.bake(idx, if fade_in { "faded in" } else { "faded out" }, |s, p| {
                    let (a, b) = p.region();
                    s.faded(a, b, fade_in)
                });
            }
            PadAction::BakePitch => {
                // Varispeed: prints the pad's pitch into the buffer and zeroes the knob.
                let semis = self.pads[idx].cfg.pitch;
                if semis.abs() > 1e-3 && self.pads[idx].sample().is_some() {
                    self.bake(idx, "pitch baked", |s, _| sample::resample(s, 2f64.powf(semis as f64 / 12.0)));
                    self.pads[idx].cfg.pitch = 0.0;
                    self.push_pad(idx);
                }
            }
            PadAction::BakeFx => self.bake_fx(idx),
            PadAction::Resynth => self.drum_resynth(idx),
            PadAction::StepTake(delta) => self.step_take(idx, delta),
        }
    }

    pub fn stop_pad(&mut self, pad: usize) {
        self.send(Cmd::StopPad { pad: pad as u16 });
    }

    /// Returns the engine's copy of pad `i` to an empty default, whatever the model holds.
    pub fn reset_engine_pad(&mut self, i: usize) {
        let pad = i as u16;
        self.send(Cmd::SetPadCfg { pad, cfg: PadCfg::default() });
        self.send(Cmd::SetSample { pad, sample: None });
        self.send(Cmd::SetPattern { pad, pattern: None });
        self.send(Cmd::SetPadPlugin { pad, plugin: None });
    }

    /// Removes a bank and closes the gap. Pads in later banks move down by 16, and MIDI
    /// pads that were playing a pad in the removed bank fall back to the internal synth.
    pub fn remove_bank(&mut self, bank: usize) {
        if self.banks <= 1 || bank >= self.banks {
            return;
        }
        let old_len = self.pads.len();
        self.send(Cmd::Panic);
        self.pads.drain(bank * 16..bank * 16 + 16);
        self.banks -= 1;
        let (lo, hi) = ((bank * 16) as u16, (bank * 16 + 16) as u16);
        for p in self.pads.iter_mut() {
            if let MidiTarget::Pad(t) = p.cfg.target {
                p.cfg.target = if t >= hi {
                    MidiTarget::Pad(t - 16)
                } else if t >= lo {
                    MidiTarget::Synth
                } else {
                    MidiTarget::Pad(t)
                };
            }
        }
        // Targets may have changed anywhere; everything from the removed bank up has moved.
        for i in 0..self.pads.len() {
            if i >= bank * 16 {
                self.resync_pad(i);
            } else if self.pads[i].cfg.kind == PadKind::Midi {
                self.push_pad(i);
            }
        }
        for i in self.pads.len()..old_len {
            self.reset_engine_pad(i);
        }
        if self.selected >= bank * 16 + 16 {
            self.selected -= 16;
        }
        self.selected = self.selected.min(self.pads.len() - 1);
        if self.bank > bank {
            self.bank -= 1;
        }
        self.bank = self.bank.min(self.banks - 1);
        self.clipboard = None;
        self.resample = Resample::Idle;
        self.status = format!("Removed a bank; {} left", self.banks);
    }

    pub fn bank_is_empty(&self, bank: usize) -> bool {
        (0..16).all(|i| {
            let p = &self.pads[bank * 16 + i];
            p.is_empty() && p.pattern.notes.is_empty()
        })
    }

    /// Drops every empty bank (always keeping at least one).
    pub fn trim_banks(&mut self) {
        let mut removed = 0;
        let mut b = self.banks;
        while b > 0 {
            b -= 1;
            if self.banks > 1 && self.bank_is_empty(b) {
                self.remove_bank(b);
                removed += 1;
            }
        }
        self.status = format!("Removed {removed} empty bank(s)");
    }


    /// Puts a new buffer on a pad as a fresh take. Transient markers come from `analysis`
    /// when there is one, otherwise they are detected.
    pub fn set_sample(&mut self, idx: usize, s: SampleData, name: Option<String>, analysis: Option<Analysis>) {
        let markers = analysis.as_ref().map(|a| a.onsets.clone());
        if analysis.is_some() {
            self.pads[idx].analysis = analysis;
        }
        self.install_take(idx, s, name, markers);
    }

    /// The single path by which a pad gets a new buffer. `markers: None` means detect them.
    pub(super) fn install_take(&mut self, idx: usize, s: SampleData, name: Option<String>, markers: Option<Vec<u32>>) {
        let (sr, sens) = (self.sr(), self.sensitivity);
        let p = &mut self.pads[idx];
        p.cfg.kind = PadKind::Audio;
        if let Some(n) = name {
            p.name = n;
        }
        p.push_take(Arc::new(s));
        match markers {
            Some(m) => p.transients = m,
            None => p.detect_transients(sr, sens),
        }
        self.push_pad(idx);
        self.push_sample(idx);
    }

    /// Destructive edit of the pad's active buffer: the result becomes a new take.
    pub fn bake(&mut self, idx: usize, what: &str, f: impl FnOnce(&SampleData, &PadModel) -> SampleData) {
        let Some(src) = self.pads[idx].sample().cloned() else { return };
        let out = f(&src, &self.pads[idx]);
        if out.len() < 2 {
            self.status = format!("{what}: nothing left to keep");
            return;
        }
        self.set_sample(idx, out, None, None);
        let p = &self.pads[idx];
        self.status = format!("{} {what}, now take {}/{}", pad_label(idx), p.take_idx + 1, p.takes.len());
    }

    /// Ctrl+Shift+Z / Y: walk this pad's revisions without touching the rest of the project.
    pub fn step_take(&mut self, idx: usize, delta: i32) {
        let p = &mut self.pads[idx];
        if p.takes.is_empty() {
            return;
        }
        // A drum pad's live synth sits before its first take: stepping back from there
        // brings it back, and stepping on from the synth goes to the take it was on.
        if p.cfg.kind == PadKind::Drum {
            if delta > 0 {
                let at = p.take_idx;
                self.drum_to_take(idx, at);
            }
            return;
        }
        if delta < 0 && p.take_idx == 0 && self.take_patch(idx).is_some() {
            self.drum_resynth(idx);
            return;
        }
        let p = &self.pads[idx];
        let to = (p.take_idx as i32 + delta).clamp(0, p.takes.len() as i32 - 1) as usize;
        if to == p.take_idx {
            self.status = format!("{}: no {} take", pad_label(idx), if delta < 0 { "earlier" } else { "later" });
            return;
        }
        self.show_take(idx, to);
        self.status = format!("{} take {}/{}", pad_label(idx), to + 1, self.pads[idx].takes.len());
    }

    /// Makes take `take` the one the pad plays, with the whole of it as the region.
    pub(super) fn show_take(&mut self, idx: usize, take: usize) {
        let (sr, sens) = (self.sr(), self.sensitivity);
        let p = &mut self.pads[idx];
        if take >= p.takes.len() {
            return;
        }
        p.take_idx = take;
        p.cfg.start = 0;
        p.cfg.end = 0;
        p.detect_transients(sr, sens);
        p.refresh_mini();
        self.push_pad(idx);
        self.push_sample(idx);
    }

    pub fn clear_pad(&mut self, idx: usize) {
        self.pads[idx] = PadModel::default();
        self.resync_pad(idx);
    }

    pub fn copy_pad(&mut self, from: usize, to: usize) {
        if from == to || from >= self.pads.len() || to >= self.pads.len() {
            return;
        }
        let copy = self.pads[from].duplicate();
        self.pads[to] = copy;
        self.resync_pad(to);
    }

    pub fn set_kind(&mut self, idx: usize, kind: PadKind) {
        if kind == PadKind::Vocal {
            self.make_vocal(idx);
            return;
        }
        if kind == PadKind::Drum {
            self.make_drum(idx, None);
            return;
        }
        if self.is_vocal(idx) {
            // Leaving vocal: the comp stays as the pad's sample, the takes go.
            if self.vocal_armed() {
                self.vocal_disarm();
            }
            self.pads[idx].vocal = None;
        }
        let p = &mut self.pads[idx];
        if p.cfg.kind == kind {
            return;
        }
        p.cfg.kind = kind;
        if kind == PadKind::Midi && p.name.is_empty() {
            p.name = p.cfg.synth.engine.name().into();
        }
        self.push_pad(idx);
        self.push_pattern(idx);
    }

    pub fn add_bank(&mut self) -> bool {
        if self.banks >= MAX_BANKS {
            return false;
        }
        self.banks += 1;
        self.pads.extend((0..16).map(|_| PadModel::default()));
        true
    }

    pub(super) fn next_empty_pad(&mut self) -> Option<usize> {
        let from = self.bank * 16;
        // A pad with only audio clips on its track is a track, not a free pad.
        if let Some(i) = (from..self.pads.len()).find(|&i| self.pads[i].is_empty() && self.arrangement.clips(i).is_empty()) {
            return Some(i);
        }
        self.add_bank().then(|| self.pads.len() - 16)
    }

    pub(super) fn lowest_empty_bank(&mut self) -> Option<usize> {
        let found = (0..self.banks).find(|b| (0..16).all(|i| self.pads[b * 16 + i].is_empty()));
        found.or_else(|| self.add_bank().then(|| self.banks - 1))
    }

    pub fn load_file(&mut self, pad: usize, path: PathBuf) {
        let tx = self.jobs_tx.clone();
        let rate = self.shared.sample_rate;
        self.busy += 1;
        self.status = format!("Loading {}…", path.display());
        std::thread::spawn(move || {
            let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let res = match sample::load_file(&path, rate) {
                Ok(sample) => {
                    let analysis = analysis::analyze(&sample, rate as f32);
                    JobResult::Loaded { pad, name, sample, analysis }
                }
                Err(e) => JobResult::Failed(e),
            };
            let _ = tx.send(res);
        });
    }

    pub fn browse_sample(&mut self, pad: usize) {
        let picked = rfd::FileDialog::new()
            .add_filter("Audio", &["wav", "mp3", "flac", "ogg", "aif", "aiff"])
            .pick_file();
        if let Some(path) = picked {
            self.load_file(pad, path);
        }
    }

    pub fn export_pad(&mut self, idx: usize) {
        let Some(s) = self.pads[idx].sample().cloned() else { return };
        let picked = rfd::FileDialog::new()
            .add_filter("WAV", &["wav"])
            .set_file_name(format!("{}.wav", pad_label(idx)))
            .save_file();
        if let Some(path) = picked {
            self.status = match sample::save_wav(&path, &s, self.shared.sample_rate) {
                Ok(()) => format!("Exported {}", path.display()),
                Err(e) => format!("Export failed: {e}"),
            };
        }
    }

    pub fn redetect(&mut self, idx: usize) {
        let (sr, sens) = (self.sr(), self.sensitivity);
        self.pads[idx].detect_transients(sr, sens);
        self.status = format!("{} transients", self.pads[idx].transients.len());
    }

    /// `C`: spread the focused pad's transient slices across the lowest empty bank.
    pub fn chop(&mut self, idx: usize) {
        let Some(src) = self.pads[idx].sample().cloned() else {
            self.status = "Nothing to chop on this pad".into();
            return;
        };
        let (start, end) = self.pads[idx].region();
        let sr = self.sr();
        let mut marks: Vec<usize> = self.pads[idx]
            .transients
            .iter()
            .map(|&t| t as usize)
            .filter(|&t| t > start + (0.01 * sr) as usize && t < end)
            .collect();
        // More than a bank's worth: keep the hardest hits.
        if marks.len() > 15 {
            let win = (0.02 * sr) as usize;
            let mut ranked: Vec<(f32, usize)> = marks
                .iter()
                .map(|&m| {
                    let hi = (m + win).min(src.len());
                    (src.l[m..hi].iter().fold(0f32, |a, v| a.max(v.abs())), m)
                })
                .collect();
            ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
            marks = ranked[..15].iter().map(|r| r.1).collect();
            marks.sort_unstable();
        }
        marks.insert(0, start);
        marks.push(end);
        if marks.len() < 3 {
            self.status = "No transients found; raise the sensitivity".into();
            return;
        }
        let Some(bank) = self.lowest_empty_bank() else {
            self.status = "No empty bank left".into();
            return;
        };
        let base_cfg = PadCfg { start: 0, end: 0, mode: PlayMode::OneShot, ..self.pads[idx].cfg };
        let count = marks.len() - 1;
        for (i, w) in marks.windows(2).enumerate() {
            let dest = bank * 16 + i;
            self.pads[dest].cfg = base_cfg;
            // A slice is one hit; it needs no markers of its own.
            self.install_take(dest, src.slice(w[0], w[1]), Some(format!("{}·{}", pad_label(idx), i + 1)), Some(Vec::new()));
        }
        self.bank = bank;
        self.status = format!("Chopped {} into {count} slices on bank {}", pad_label(idx), crate::music::bank_name(bank));
    }
}
