//! Song export: the arrangement rendered to a WAV file by a second, offline engine.
//!
//! The offline engine is the same engine the app plays through, given the whole project
//! and the arrangement and run as fast as it can on a background thread, so the file is
//! what playback sounds like. Hosted plugins can't be run by two engines at once, so the
//! live engine lets go of them for the length of the export and takes them back after.

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
        if self.exporting {
            return;
        }
        if self.playing() {
            self.toggle_play();
        }
        let sr = self.shared.sample_rate;
        let shared = Arc::new(crate::shared::Shared::new(sr, 10));
        let (mut tx, rx) = RingBuffer::new(1 << 16);
        let (_tx_midi, rx_midi) = RingBuffer::new(8);
        let (gtx, mut grx) = RingBuffer::new(1 << 14);
        let mut engine = Engine::new(shared, rx, rx_midi, gtx);
        let mut send = |cmd: Cmd| {
            let _ = tx.push(cmd);
        };

        // The whole project, as the live engine has it.
        for (b, cfg) in self.buses.iter().enumerate() {
            send(Cmd::SetBus { bus: b as u8, cfg: *cfg });
        }
        send(Cmd::SetBpm(self.bpm));
        send(Cmd::SetSwing(self.swing));
        send(Cmd::SetScale(self.key.mask()));
        send(Cmd::SetVocalFx(self.vocal_fx));
        for (i, p) in self.pads.iter().enumerate() {
            if p.is_empty() && p.cfg == PadCfg::default() {
                continue;
            }
            send(Cmd::SetPadCfg { pad: i as u16, cfg: p.cfg });
            send(Cmd::SetSample { pad: i as u16, sample: p.sample().cloned() });
        }
        // Plugins move to the offline engine for the export.
        let mut detached = Vec::new();
        for i in 0..self.pads.len() {
            if let Some(slot) = &self.pads[i].plugin {
                send(Cmd::SetPadPlugin { pad: i as u16, plugin: Some(self.rt_for(slot)) });
                detached.push(PluginTarget::Pad(i));
            }
        }
        for i in 0..self.fx_plugins.len() {
            if let Some(slot) = &self.fx_plugins[i] {
                send(Cmd::SetFxPlugin { slot: i as u8, plugin: Some(self.rt_for(slot)) });
                detached.push(PluginTarget::Fx(i / 3, i % 3));
            }
        }
        send(Cmd::Song(SongCmd::Plan(Some(Arc::new(self.song_plan())))));
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
        let path = path.to_path_buf();
        let jobs = self.jobs_tx.clone();
        self.exporting = true;
        self.busy += 1;
        self.status = format!("Exporting {:.1} s of song…", frames as f32 / sr as f32);
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
            let (mut quiet, mut tail) = (0, 0);
            let max_tail = (TAIL_SECS * sr as f32) as usize;
            while tail < max_tail && quiet < sr as usize / 2 {
                let before = l.len();
                render(&mut engine, 512, &mut l, &mut r);
                tail += 512;
                let loud = l[before..].iter().chain(&r[before..]).any(|v| v.abs() > 1e-4);
                quiet = if loud { 0 } else { quiet + 512 };
            }
            // Trim the silence the tail ended on, keeping a moment of it.
            let keep = l.len().saturating_sub(quiet.saturating_sub(sr as usize / 10)).max(frames);
            l.truncate(keep);
            r.truncate(keep);
            drop(engine);
            let peak = l.iter().chain(&r).fold(0f32, |m, v| m.max(v.abs()));
            let result = write_wav(&path, &l, &r, sr, depth)
                .map(|()| JobResult::Exported { path, secs: l.len() as f32 / sr as f32, peak, detached: detached.clone() })
                .unwrap_or_else(|e| JobResult::ExportFailed { error: e, detached });
            let _ = jobs.send(result);
        });
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
