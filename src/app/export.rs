//! Offline renders: the arrangement (or part of it) played by a second engine on a
//! background thread, as fast as it goes. Song export writes it to a WAV file;
//! consolidate and bake turn part of it into audio clips.
//!
//! The offline engine is the same engine the app plays through, so a render is what
//! playback sounds like. Hosted plugins can't be run by two engines at once, so the live
//! engine lets go of them for the length of the render and takes them back after.

use super::*;
use crate::engine::{Engine, SongCmd};
use rtrb::RingBuffer;
use std::path::Path;

/// Sample formats an export can be written in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Depth {
    Int16,
    Int24,
    Float32,
}

impl Depth {
    pub const ALL: [Depth; 3] = [Depth::Int24, Depth::Int16, Depth::Float32];

    pub fn name(self) -> &'static str {
        match self {
            Depth::Int16 => "16-bit",
            Depth::Int24 => "24-bit",
            Depth::Float32 => "32-bit float",
        }
    }
}

/// How long a render may ring on after the song ends, at most, in seconds.
const TAIL_SECS: f32 = 8.0;

/// What an offline render plays, and how.
pub(super) struct Offline {
    pub plan: crate::engine::ArrangementPlan,
    /// Song ticks.
    pub start: u64,
    pub end: u64,
    /// Ring out past the end until quiet.
    pub tail: bool,
    /// Through the buses and their effects (and the master's); otherwise each pad is heard
    /// dry, at full level and centred, as a pad's own sound.
    pub wet: bool,
    /// Only these pads are set up (all, if `None`).
    pub pads: Option<Vec<usize>>,
    /// The master's effects and level too (song export), or a plain master.
    pub master: bool,
}

/// What a finished render becomes: given the audio and where it began, makes the job result.
type Finish = Box<dyn FnOnce(Vec<f32>, Vec<f32>, Vec<PluginTarget>) -> JobResult + Send>;

impl App {
    /// Asks where to write the song, then exports it.
    pub fn export_song_dialog(&mut self) {
        if self.arrangement.is_empty() {
            self.status = "The arrangement is empty: press Chain to song or put loops in first".into();
            return;
        }
        let name = self
            .project_dir
            .as_ref()
            .and_then(|d| d.file_name())
            .map_or("song".to_string(), |n| n.to_string_lossy().into_owned());
        let Some(path) = rfd::FileDialog::new().set_title("Export song").set_file_name(format!("{name}.wav")).add_filter("WAV audio", &["wav"]).save_file() else {
            return;
        };
        self.export_song(&path, self.export_depth);
    }

    /// The stretch of song an export covers, in ticks: the loop brace if it is on, or
    /// the whole song.
    pub fn export_range(&self) -> (u64, u64) {
        match self.arrangement.brace {
            Some((s, e)) if self.arrangement.brace_on => (s, e),
            _ => (0, self.arrangement.end()),
        }
    }

    /// Renders the arrangement to `path` in the background.
    pub fn export_song(&mut self, path: &Path, depth: Depth) {
        let (start, end) = self.export_range();
        if end <= start {
            self.status = "Nothing to export: the song is empty".into();
            return;
        }
        let job = Offline { plan: self.song_plan(), start, end, tail: true, wet: true, pads: None, master: true };
        if self.export_to(job, path, depth) {
            self.status = format!("Exporting {:.1} s of song…", (end - start) as f64 / (self.bpm as f64 / 60.0 * PPQ as f64));
        }
    }

    /// Renders `job` to a WAV file at `path`.
    fn export_to(&mut self, job: Offline, path: &Path, depth: Depth) -> bool {
        let path = path.to_path_buf();
        let sr = self.shared.sample_rate;
        let finish: Finish = Box::new(move |l, r, detached| {
            let peak = l.iter().chain(&r).fold(0f32, |m, v| m.max(v.abs()));
            write_wav(&path, &l, &r, sr, depth)
                .map(|()| JobResult::Exported { path, secs: l.len() as f32 / sr as f32, peak, detached: detached.clone() })
                .unwrap_or_else(|e| JobResult::ExportFailed { error: e, detached })
        });
        self.render_offline(job, finish)
    }

    /// Asks for a folder, then renders stems into it: one per track (each pad that plays
    /// in the song, through its bus and effects), or one per bus (the pads routed to it,
    /// through it). Neither has the master's effects, so the stems add up to the mix
    /// before the master.
    pub fn export_stems_dialog(&mut self, by_bus: bool) {
        if self.arrangement.is_empty() {
            self.status = "The arrangement is empty: press Chain to song or put loops in first".into();
            return;
        }
        let Some(dir) = rfd::FileDialog::new().set_title("Export stems into").pick_folder() else { return };
        let (start, end) = self.export_range();
        let full = self.song_plan();
        let playing: Vec<usize> = full.tracks.iter().map(|t| t.pad as usize).collect();
        // (file name, pads).
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        if by_bus {
            for &pad in &playing {
                let bus = crate::shared::bus_index(self.pads[pad].cfg.bus);
                let name = if bus == crate::shared::MASTER { "Direct to master".to_string() } else { crate::shared::bus_label(bus).to_string() };
                match groups.iter_mut().find(|g| g.0 == name) {
                    Some(g) => g.1.push(pad),
                    None => groups.push((name, vec![pad])),
                }
            }
        } else {
            for &pad in &playing {
                let p = &self.pads[pad];
                let name = if p.name.is_empty() { pad_label(pad) } else { format!("{} {}", pad_label(pad), p.name) };
                groups.push((name, vec![pad]));
            }
        }
        if groups.is_empty() {
            self.status = "Nothing in the song plays a pad".into();
            return;
        }
        let project = self.project_dir.as_ref().and_then(|d| d.file_name()).map_or("song".to_string(), |n| n.to_string_lossy().into_owned());
        self.stem_queue.clear();
        for (k, (name, pads)) in groups.into_iter().enumerate() {
            let mut plan = self.song_plan();
            plan.tracks.retain(|t| pads.contains(&(t.pad as usize)));
            let safe: String = name.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
            let path = dir.join(format!("{project} - {:02} {}.wav", k + 1, safe.trim()));
            let job = Offline { plan, start, end, tail: true, wet: true, pads: Some(pads), master: false };
            self.stem_queue.push_back((job, path));
        }
        self.stems_total = self.stem_queue.len();
        self.next_stem();
    }

    /// Starts the next stem waiting, if any. True if one started.
    pub(super) fn next_stem(&mut self) -> bool {
        let Some((job, path)) = self.stem_queue.pop_front() else { return false };
        let depth = self.export_depth;
        let n = self.stems_total - self.stem_queue.len();
        if self.export_to(job, &path, depth) {
            self.status = format!("Exporting stem {n} of {}: {}", self.stems_total, path.file_name().map_or(String::new(), |f| f.to_string_lossy().into_owned()));
            true
        } else {
            self.stem_queue.clear();
            false
        }
    }

    /// Plays `job` on an offline engine in the background, then hands the audio to
    /// `finish` (on that thread) for the job result. False if a render is already going.
    pub(super) fn render_offline(&mut self, job: Offline, finish: Finish) -> bool {
        let Offline { plan, start, end, tail, wet, pads, master } = job;
        if self.exporting || end <= start {
            return false;
        }
        if self.playing() {
            self.toggle_play();
        }
        let wanted = |i: usize| pads.as_ref().is_none_or(|p| p.contains(&i));
        let sr = self.shared.sample_rate;
        let shared = Arc::new(crate::shared::Shared::new(sr, 10));
        let (mut tx, rx) = RingBuffer::new(1 << 16);
        let (_tx_midi, rx_midi) = RingBuffer::new(8);
        let (gtx, mut grx) = RingBuffer::new(1 << 14);
        let mut engine = Engine::new(shared, rx, rx_midi, gtx);
        let mut send = |cmd: Cmd| {
            let _ = tx.push(cmd);
        };

        // The project, as the live engine has it (or as much of it as the render needs).
        for (b, cfg) in self.buses.iter().enumerate() {
            let plain = !wet || (b == crate::shared::MASTER && !master);
            send(Cmd::SetBus { bus: b as u8, cfg: if plain { BusCfg::default() } else { *cfg } });
        }
        send(Cmd::SetBpm(self.bpm));
        send(Cmd::SetSwing(self.swing));
        send(Cmd::SetScale(self.key.mask()));
        send(Cmd::SetVocalFx(self.vocal_fx));
        for (i, p) in self.pads.iter().enumerate() {
            if (p.is_empty() && p.cfg == PadCfg::default()) || !wanted(i) {
                continue;
            }
            let cfg = if wet { PadCfg { mute: false, solo: false, ..p.cfg } } else { PadCfg { vol: 1.0, pan: 0.0, bus: 0, mute: false, solo: false, ..p.cfg } };
            send(Cmd::SetPadCfg { pad: i as u16, cfg });
            send(Cmd::SetSample { pad: i as u16, sample: p.sample().cloned() });
        }
        // Plugins move to the offline engine for the render.
        let mut detached = Vec::new();
        for i in 0..self.pads.len() {
            if let Some(slot) = self.pads[i].plugin.as_ref().filter(|_| wanted(i)) {
                send(Cmd::SetPadPlugin { pad: i as u16, plugin: Some(self.rt_for(slot)) });
                detached.push(PluginTarget::Pad(i));
            }
        }
        for i in 0..self.fx_plugins.len() {
            let on_master = i / 3 == crate::shared::MASTER;
            if let Some(slot) = self.fx_plugins[i].as_ref().filter(|_| wet && (master || !on_master)) {
                send(Cmd::SetFxPlugin { slot: i as u8, plugin: Some(self.rt_for(slot)) });
                detached.push(PluginTarget::Fx(i / 3, i % 3));
            }
        }
        send(Cmd::Song(SongCmd::Plan(Some(Arc::new(plan)))));
        send(Cmd::Song(SongCmd::Mode(true)));
        send(Cmd::Song(SongCmd::Locate(start as f64)));
        send(Cmd::Play(true));
        for &t in &detached {
            match t {
                PluginTarget::Pad(i) => self.send(Cmd::SetPadPlugin { pad: i as u16, plugin: None }),
                PluginTarget::Fx(b, s) => self.send(Cmd::SetFxPlugin { slot: (b * 3 + s) as u8, plugin: None }),
                PluginTarget::Analyzer(_) => {}
            }
        }

        let ticks_per_sec = self.bpm as f64 / 60.0 * PPQ as f64;
        let frames = ((end - start) as f64 / ticks_per_sec * sr as f64).round() as usize;
        let jobs = self.jobs_tx.clone();
        self.exporting = true;
        self.busy += 1;
        std::thread::spawn(move || {
            let mut l = Vec::with_capacity(frames + sr as usize);
            let mut r = Vec::with_capacity(frames + sr as usize);
            let mut block = vec![0f32; 512 * 2];
            let mut render = |engine: &mut Engine, n: usize, l: &mut Vec<f32>, r: &mut Vec<f32>| {
                engine.process(&mut block[..n * 2], 2);
                for [a, b] in block[..n * 2].as_chunks::<2>().0 {
                    l.push(*a);
                    r.push(*b);
                }
                while grx.pop().is_ok() {}
            };
            let mut done = 0;
            while done < frames {
                let n = (frames - done).min(512);
                render(&mut engine, n, &mut l, &mut r);
                done += n;
            }
            // Past the end nothing new starts; what is sounding rings out, until it has
            // been quiet for half a second.
            let _ = tx.push(Cmd::Song(SongCmd::Plan(None)));
            let (mut quiet, mut rung) = (0, 0);
            let max_tail = if tail { (TAIL_SECS * sr as f32) as usize } else { 0 };
            while rung < max_tail && quiet < sr as usize / 2 {
                let before = l.len();
                render(&mut engine, 512, &mut l, &mut r);
                rung += 512;
                let loud = l[before..].iter().chain(&r[before..]).any(|v| v.abs() > 1e-4);
                quiet = if loud { 0 } else { quiet + 512 };
            }
            // Trim the silence the tail ended on, keeping a moment of it.
            let keep = l.len().saturating_sub(quiet.saturating_sub(sr as usize / 10)).max(frames);
            l.truncate(keep);
            r.truncate(keep);
            drop(engine);
            let _ = jobs.send(finish(l, r, detached));
        });
        true
    }

    /// The export has finished (or failed): the live engine takes its plugins back.
    pub(super) fn export_done(&mut self, detached: &[PluginTarget]) {
        self.exporting = false;
        for &t in detached {
            match t {
                PluginTarget::Pad(i) => self.push_plugin(i),
                PluginTarget::Fx(b, s) => {
                    let plugin = self.fx_plugins[b * 3 + s].as_ref().map(|slot| self.rt_for(slot));
                    self.send(Cmd::SetFxPlugin { slot: (b * 3 + s) as u8, plugin });
                }
                PluginTarget::Analyzer(_) => {}
            }
        }
    }
}

/// Writes stereo audio as WAV. 16-bit is dithered (triangular, one bit), so quiet fades
/// don't break up into distortion; 24-bit and float are written as they are.
fn write_wav(path: &Path, l: &[f32], r: &[f32], rate: u32, depth: Depth) -> Result<(), String> {
    let (bits, format) = match depth {
        Depth::Int16 => (16, hound::SampleFormat::Int),
        Depth::Int24 => (24, hound::SampleFormat::Int),
        Depth::Float32 => (32, hound::SampleFormat::Float),
    };
    let spec = hound::WavSpec { channels: 2, sample_rate: rate, bits_per_sample: bits, sample_format: format };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    let mut seed = 0x1234_5678u32;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    for (a, b) in l.iter().zip(r) {
        for v in [*a, *b] {
            match depth {
                Depth::Float32 => w.write_sample(v),
                Depth::Int24 => w.write_sample((v.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32),
                Depth::Int16 => {
                    let d = noise() - noise();
                    w.write_sample((v.clamp(-1.0, 1.0) * 32767.0 + d).round().clamp(-32768.0, 32767.0) as i16)
                }
            }
            .map_err(|e| e.to_string())?;
        }
    }
    w.finalize().map_err(|e| e.to_string())
}
