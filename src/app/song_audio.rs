//! Audio clips: recordings placed on the song timeline, played locked to it.
//!
//! The recordings live in the arrangement's pool (`Arrangement::audio`), so a recording
//! used by several clips is held once. They come from files (dropped on the timeline or
//! imported) or from a pad's own sample.

use super::*;
use super::arrange::{AudioSource, MIN_CLIP};
use super::export::Offline;
use std::path::PathBuf;

/// Where rendered clips go back.
#[derive(Debug)]
pub enum BounceTo {
    /// Consolidate: in place of `clips` on `pad`'s track, from `start`.
    Replace { pad: usize, start: u64, clips: Vec<Clip> },
    /// Bake: on a new track named `name`, from `start`, with `clips` (by pad) taken away.
    NewTrack { start: u64, name: String, clips: Vec<(usize, Clip)> },
    /// Freeze: `pad`'s whole track, `clips`, kept to unfreeze.
    Freeze { pad: usize, start: u64, clips: Vec<Clip> },
}

impl App {
    /// Adds a recording to the pool, or finds it there already. Returns its index.
    pub fn add_audio_source(&mut self, name: &str, data: Arc<SampleData>) -> usize {
        let pool = &mut self.arrangement.audio;
        if let Some(i) = pool.iter().position(|a| a.data.as_ref().is_some_and(|d| Arc::ptr_eq(d, &data))) {
            return i;
        }
        let safe: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(40).collect();
        let file = format!("{:03}_{safe}.wav", pool.len() + 1);
        pool.push(AudioSource::new(file, name.to_string(), data, self.bpm));
        pool.len() - 1
    }

    /// Puts recording `source` on `pad`'s track at `at`, as long as the recording.
    pub fn put_audio(&mut self, pad: usize, at: u64, source: usize) {
        let frames = self.arrangement.audio.get(source).map_or(0, AudioSource::frames);
        let len = ((frames as f64 * self.source_ticks_per_frame(source)).round() as u64).max(MIN_CLIP);
        self.arrangement.place(pad, Clip::audio(at, len, source));
        self.arr.selected = self.arrangement.clips(pad).iter().position(|c| c.start == at).map(|i| vec![(pad, i)]).unwrap_or_default();
        self.arr.sections.clear();
    }

    /// Puts the pad's own sample (its active take) on its track at `at`.
    pub fn put_pad_sample(&mut self, pad: usize, at: u64) {
        let Some(data) = self.pads.get(pad).and_then(|p| p.sample().cloned()) else {
            self.status = format!("{} has no sample to put on the timeline", pad_label(pad));
            return;
        };
        let name = if self.pads[pad].name.is_empty() { pad_label(pad) } else { self.pads[pad].name.clone() };
        let source = self.add_audio_source(&name, data);
        self.put_audio(pad, at, source);
        self.status = format!("{name} put on the timeline at bar {}", at / TICKS_PER_BAR as u64 + 1);
    }

    /// A pad with nothing on it and no track, to be the track of new audio.
    pub(super) fn free_track_pad(&mut self) -> Option<usize> {
        let unused = |app: &App, i: usize| {
            let p = &app.pads[i];
            p.is_empty() && p.pattern.notes.is_empty() && p.name.is_empty() && app.arrangement.clips(i).is_empty() && !app.scenes.iter().any(|s| s.patterns.iter().any(|(k, pat)| *k == i && !pat.notes.is_empty()))
        };
        if let Some(i) = (0..self.pads.len()).find(|&i| unused(self, i)) {
            return Some(i);
        }
        self.add_bank().then(|| self.pads.len() - 16)
    }

    /// Asks for audio files and puts them on the timeline at the insert marker: on
    /// `pad`'s track, or each on a new track.
    pub fn import_audio_dialog(&mut self, pad: Option<usize>) {
        let Some(paths) = rfd::FileDialog::new().set_title("Import audio").add_filter("Audio", &["wav", "mp3", "flac", "ogg", "aif", "aiff"]).pick_files() else {
            return;
        };
        let at = self.arr.cursor;
        for path in paths {
            self.load_song_audio(path, pad, at);
        }
    }

    /// Decodes a file in the background, to land on the timeline at `at` (see
    /// `song_audio_loaded`).
    pub fn load_song_audio(&mut self, path: PathBuf, pad: Option<usize>, at: u64) {
        let tx = self.jobs_tx.clone();
        let rate = self.shared.sample_rate;
        self.busy += 1;
        self.status = format!("Loading {}…", path.display());
        std::thread::spawn(move || {
            let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let res = match sample::load_file(&path, rate) {
                Ok(sample) => JobResult::SongAudio { pad, at, name, sample },
                Err(e) => JobResult::Failed(e),
            };
            let _ = tx.send(res);
        });
    }

    /// A file has been decoded: onto `pad`'s track, or a new track named after it.
    pub(super) fn song_audio_loaded(&mut self, pad: Option<usize>, at: u64, name: String, sample: SampleData) {
        let pad = match pad.filter(|&p| p < self.pads.len()) {
            Some(p) => p,
            None => {
                let Some(p) = self.free_track_pad() else {
                    self.status = "No free pad for a new track".into();
                    return;
                };
                self.pads[p].name = name.clone();
                p
            }
        };
        let secs = sample.len() as f32 / self.sr();
        let source = self.add_audio_source(&name, Arc::new(sample));
        self.put_audio(pad, at, source);
        self.view = View::Arrange;
        self.status = format!("{name} ({secs:.1} s) on {} at bar {}", pad_label(pad), at / TICKS_PER_BAR as u64 + 1);
    }

    /// Files dropped on the timeline: on the track under the pointer, or new tracks,
    /// one after another from `at`.
    pub fn drop_song_audio(&mut self, paths: Vec<PathBuf>, pad: Option<usize>, at: u64) {
        // The first goes on the track it was dropped on; any more get tracks of their own.
        for (k, path) in paths.into_iter().enumerate() {
            self.load_song_audio(path, pad.filter(|_| k == 0), at);
        }
    }

    /// How many ticks one frame of recording `source` lasts, as it is heard: at the tempo
    /// it was recorded at when it follows the project's, otherwise at the project's.
    pub fn source_ticks_per_frame(&self, source: usize) -> f64 {
        let bpm = self.arrangement.audio.get(source).filter(|a| a.warp && a.bpm > 0.0).map_or(self.bpm, |a| a.bpm);
        bpm as f64 / 60.0 * PPQ as f64 / self.shared.sample_rate as f64
    }

    /// Stretches the recordings that follow the tempo to it, in the background, once the
    /// tempo has been still for a moment. Until a stretch is ready the original plays.
    pub(super) fn poll_warps(&mut self) {
        let bpm = self.bpm;
        if (self.arr.tempo_since.0 - bpm).abs() > 1e-4 {
            self.arr.tempo_since = (bpm, Some(std::time::Instant::now()));
            return;
        }
        if self.arr.tempo_since.1.is_some_and(|t| t.elapsed().as_secs_f32() < 0.4) {
            return;
        }
        let used: Vec<usize> = self.arrangement.tracks.iter().flat_map(|(_, c)| c.iter().filter_map(|c| c.audio)).collect();
        let sr = self.sr();
        for source in used {
            let Some(a) = self.arrangement.audio.get(source) else { continue };
            if !a.needs_warp(bpm) || matches!(&a.warped, Some((t, m, _)) if (*t - bpm).abs() < 1e-3 && *m == a.mode) {
                continue;
            }
            if self.arr.warping.iter().any(|w| w.0 == source && (w.1 - bpm).abs() < 1e-3 && w.2 == a.mode) {
                continue;
            }
            let Some(data) = a.data.clone() else { continue };
            let (mode, file) = (a.mode, a.file.clone());
            // Longer when the project is slower than the recording.
            let w = crate::stretch::Warp { mode, ratio: a.bpm as f64 / bpm as f64, semis: 0.0, formants: false };
            self.arr.warping.push((source, bpm, mode));
            self.busy += 1;
            let tx = self.jobs_tx.clone();
            std::thread::spawn(move || {
                let onsets = crate::analysis::detect_onsets(&data.mono(), sr, 0.5);
                let out = crate::stretch::warp(&data, &onsets, w, sr);
                let _ = tx.send(JobResult::Warped { source, file, bpm, mode, data: out });
            });
        }
    }

    /// A recording has been stretched: it plays at the new tempo from the next block.
    pub(super) fn warp_done(&mut self, source: usize, file: String, bpm: f32, mode: crate::stretch::WarpMode, data: SampleData) {
        self.arr.warping.retain(|w| !(w.0 == source && (w.1 - bpm).abs() < 1e-3 && w.2 == mode));
        if let Some(a) = self.arrangement.audio.get_mut(source).filter(|a| a.file == file && a.mode == mode) {
            a.warped = Some((bpm, mode, Arc::new(data)));
            self.arr.dirty = true;
        }
    }

    /// The selected clips' part of the song, for rendering: only those clips play.
    fn selection_plan(&self, picked: &[(usize, Clip)]) -> crate::engine::ArrangementPlan {
        let mut plan = self.song_plan();
        for t in plan.tracks.iter_mut() {
            let pad = t.pad as usize;
            t.clips.retain(|c| picked.iter().any(|(p, k)| *p == pad && k.start == c.start && k.len == c.len));
        }
        plan.tracks.retain(|t| !t.clips.is_empty());
        plan
    }

    /// Consolidate (`Ctrl+J`): the selected clips of one track rendered into one audio
    /// clip in their place, as the pad sounds on its own (before its bus), so the clip
    /// plays back through the pad just as they did.
    pub fn consolidate(&mut self) {
        let picked = self.selected_clips();
        let Some(&(pad, _)) = picked.first() else {
            self.status = "Select the clips to consolidate first".into();
            return;
        };
        if picked.iter().any(|(p, _)| *p != pad) {
            self.status = "Consolidate works on one track at a time; Bake renders several tracks into one".into();
            return;
        }
        if picked.len() == 1 && picked[0].1.is_audio() && picked[0].1.gain_db == 0.0 {
            self.status = "That is already one audio clip".into();
            return;
        }
        let start = picked.iter().map(|(_, c)| c.start).min().unwrap_or(0);
        let end = picked.iter().map(|(_, c)| c.end()).max().unwrap_or(0);
        let job = Offline { plan: self.selection_plan(&picked), start, end, tail: false, wet: false, pads: Some(vec![pad]), master: false };
        let to = BounceTo::Replace { pad, start, clips: picked.iter().map(|(_, c)| *c).collect() };
        if self.render_offline(job, Box::new(move |l, r, detached| JobResult::Bounced { to, l, r, detached })) {
            self.status = format!("Consolidating {} clip{} on {}…", picked.len(), if picked.len() == 1 { "" } else { "s" }, pad_label(pad));
        }
    }

    /// Bake: the selected clips, from any tracks, rendered through their buses and
    /// effects (not the master's) into one audio clip on a new track, in their place.
    pub fn bake_selection(&mut self) {
        let picked = self.selected_clips();
        if picked.is_empty() {
            self.status = "Select the clips to bake first".into();
            return;
        }
        let start = picked.iter().map(|(_, c)| c.start).min().unwrap_or(0);
        let end = picked.iter().map(|(_, c)| c.end()).max().unwrap_or(0);
        let mut pads: Vec<usize> = picked.iter().map(|(p, _)| *p).collect();
        pads.sort_unstable();
        pads.dedup();
        let name = match pads.as_slice() {
            [p] => format!("{} baked", if self.pads[*p].name.is_empty() { pad_label(*p) } else { self.pads[*p].name.clone() }),
            _ => format!("Bake of {} tracks", pads.len()),
        };
        let job = Offline { plan: self.selection_plan(&picked), start, end, tail: true, wet: true, pads: Some(pads), master: false };
        let to = BounceTo::NewTrack { start, name, clips: picked.clone() };
        if self.render_offline(job, Box::new(move |l, r, detached| JobResult::Bounced { to, l, r, detached })) {
            self.status = format!("Baking {} clip{}…", picked.len(), if picked.len() == 1 { "" } else { "s" });
        }
    }

    /// Freeze: a track's clips rendered into audio (as the pad sounds on its own, tails and
    /// all), and its instrument plugin switched off to spare the processor. The clips are
    /// kept, and come back with Unfreeze.
    pub fn freeze_track(&mut self, pad: usize) {
        if self.arrangement.is_frozen(pad) {
            return;
        }
        let clips: Vec<Clip> = self.arrangement.clips(pad).to_vec();
        if clips.is_empty() {
            self.status = format!("{} has nothing on the timeline to freeze", pad_label(pad));
            return;
        }
        let picked: Vec<(usize, Clip)> = clips.iter().map(|c| (pad, *c)).collect();
        let start = clips.iter().map(|c| c.start).min().unwrap_or(0);
        let end = clips.iter().map(|c| c.end()).max().unwrap_or(0);
        let job = Offline { plan: self.selection_plan(&picked), start, end, tail: true, wet: false, pads: Some(vec![pad]), master: false };
        let to = BounceTo::Freeze { pad, start, clips };
        if self.render_offline(job, Box::new(move |l, r, detached| JobResult::Bounced { to, l, r, detached })) {
            self.status = format!("Freezing {}…", pad_label(pad));
        }
    }

    /// Puts a frozen track's clips back and switches its plugin on again.
    pub fn unfreeze_track(&mut self, pad: usize) {
        let Some(i) = self.arrangement.frozen.iter().position(|(p, _)| *p == pad) else { return };
        let (_, clips) = self.arrangement.frozen.remove(i);
        let track = self.arrangement.track_mut(pad);
        *track = clips;
        self.arr.deselect();
        self.push_plugin(pad);
        self.status = format!("{} unfrozen: its clips and its plugin are back", pad_label(pad));
    }

    /// A render for consolidate, bake or freeze is back: the clips go, the audio comes in.
    pub(super) fn bounce_done(&mut self, to: BounceTo, l: Vec<f32>, r: Vec<f32>) {
        if l.is_empty() {
            self.status = "The render came back empty".into();
            return;
        }
        let take = |arr: &mut Arrangement, pad: usize, clips: &[Clip]| {
            let picks: Vec<(usize, usize)> = clips.iter().filter_map(|c| Some((pad, arr.clips(pad).iter().position(|x| x == c)?))).collect();
            arr.take(picks);
        };
        let data = Arc::new(SampleData::new(l, r));
        match to {
            BounceTo::Replace { pad, start, clips } => {
                let name = format!("{} consolidated", if self.pads[pad].name.is_empty() { pad_label(pad) } else { self.pads[pad].name.clone() });
                take(&mut self.arrangement, pad, &clips);
                let source = self.add_audio_source(&name, data);
                self.put_audio(pad, start, source);
                self.status = format!("Consolidated {} clip{} into one on {}", clips.len(), if clips.len() == 1 { "" } else { "s" }, pad_label(pad));
            }
            BounceTo::Freeze { pad, start, clips } => {
                let name = format!("{} frozen", if self.pads[pad].name.is_empty() { pad_label(pad) } else { self.pads[pad].name.clone() });
                self.arrangement.track_mut(pad).clear();
                self.arrangement.frozen.push((pad, clips));
                let source = self.add_audio_source(&name, data);
                self.put_audio(pad, start, source);
                self.push_plugin(pad);
                self.status = format!("{} frozen: it plays as audio{}. Unfreeze to edit it again.", pad_label(pad), if self.pads[pad].plugin.is_some() { ", its plugin switched off" } else { "" });
            }
            BounceTo::NewTrack { start, name, clips } => {
                let Some(track) = self.free_track_pad() else {
                    self.status = "No free pad for the baked track".into();
                    return;
                };
                for (pad, c) in &clips {
                    take(&mut self.arrangement, *pad, std::slice::from_ref(c));
                }
                self.pads[track].name = name.clone();
                self.pads[track].cfg.vol = 1.0;
                self.push_pad(track);
                let source = self.add_audio_source(&name, data);
                self.put_audio(track, start, source);
                self.status = format!("Baked into {} on {}", name, pad_label(track));
            }
        }
    }

    /// Sets whether recording `source` follows the tempo, the tempo it is at, and how it is
    /// stretched.
    pub fn set_source_warp(&mut self, source: usize, warp: bool, bpm: f32, mode: crate::stretch::WarpMode) {
        if let Some(a) = self.arrangement.audio.get_mut(source) {
            a.warp = warp;
            a.bpm = bpm.clamp(20.0, 300.0);
            a.mode = mode;
        }
    }

    /// Sets an audio clip's gain and fades.
    pub fn set_clip_audio(&mut self, pad: usize, index: usize, gain_db: f32, fade_in: u64, fade_out: u64) {
        let track = self.arrangement.track_mut(pad);
        if let Some(c) = track.get_mut(index).filter(|c| c.is_audio()) {
            c.gain_db = gain_db.clamp(-60.0, 12.0);
            c.fade_in = fade_in.min(c.len);
            c.fade_out = fade_out.min(c.len - c.fade_in.min(c.len));
        }
    }
}
