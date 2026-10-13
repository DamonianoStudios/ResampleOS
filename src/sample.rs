//! PCM buffers: decoding, WAV export, sample-rate conversion and the destructive edits
//! that feed a pad's take stack.

use std::path::Path;

pub struct SampleData {
    pub l: Vec<f32>,
    pub r: Vec<f32>,
}

impl SampleData {
    pub fn new(l: Vec<f32>, r: Vec<f32>) -> Self {
        debug_assert_eq!(l.len(), r.len());
        SampleData { l, r }
    }

    pub fn len(&self) -> usize {
        self.l.len()
    }

    pub fn mono(&self) -> Vec<f32> {
        self.l.iter().zip(&self.r).map(|(a, b)| (a + b) * 0.5).collect()
    }

    pub fn slice(&self, start: usize, end: usize) -> SampleData {
        let end = end.min(self.len());
        let start = start.min(end);
        SampleData::new(self.l[start..end].to_vec(), self.r[start..end].to_vec())
    }

    pub fn peak(&self) -> f32 {
        self.l.iter().chain(&self.r).fold(0f32, |m, v| m.max(v.abs()))
    }

    pub fn normalized(&self) -> SampleData {
        let p = self.peak();
        let g = if p > 1e-6 { 0.98 / p } else { 1.0 };
        SampleData::new(
            self.l.iter().map(|v| v * g).collect(),
            self.r.iter().map(|v| v * g).collect(),
        )
    }

    pub fn reversed(&self) -> SampleData {
        SampleData::new(
            self.l.iter().rev().copied().collect(),
            self.r.iter().rev().copied().collect(),
        )
    }

    /// Linear fade over `[start, end)`; `fade_in` ramps up, otherwise down.
    pub fn faded(&self, start: usize, end: usize, fade_in: bool) -> SampleData {
        let mut out = self.slice(0, self.len());
        let end = end.min(self.len());
        if end <= start {
            return out;
        }
        let n = (end - start) as f32;
        for i in start..end {
            let t = (i - start) as f32 / n;
            let g = if fade_in { t } else { 1.0 - t };
            out.l[i] *= g;
            out.r[i] *= g;
        }
        out
    }

    /// Min/max pairs for drawing, `n` columns across `[start, end)`.
    pub fn peaks(&self, start: usize, end: usize, n: usize) -> Vec<(f32, f32)> {
        let end = end.min(self.len());
        let start = start.min(end);
        let span = end - start;
        let mut out = Vec::with_capacity(n);
        for c in 0..n {
            let a = start + span * c / n.max(1);
            let b = (start + span * (c + 1) / n.max(1)).max(a + 1).min(end);
            let (mut lo, mut hi) = (0f32, 0f32);
            // Stride keeps huge buffers cheap to draw; the eye can't tell at this density.
            let step = ((b.saturating_sub(a)) / 64).max(1);
            let mut i = a;
            while i < b {
                let v = (self.l[i] + self.r[i]) * 0.5;
                lo = lo.min(v);
                hi = hi.max(v);
                i += step;
            }
            out.push((lo, hi));
        }
        out
    }
}

/// 4-point Hermite interpolation at fractional index `pos`.
#[inline]
pub fn hermite(buf: &[f32], pos: f64) -> f32 {
    let i = pos as isize;
    let t = (pos - i as f64) as f32;
    let n = buf.len() as isize;
    let at = |k: isize| -> f32 {
        if k < 0 || k >= n { 0.0 } else { buf[k as usize] }
    };
    let (y0, y1, y2, y3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

/// Reads `buf` at `speed` (>1 = shorter and higher). When speeding up, a one-pole
/// lowpass ahead of the read keeps the worst of the aliasing out.
pub fn resample_channel(buf: &[f32], speed: f64) -> Vec<f32> {
    if buf.is_empty() || (speed - 1.0).abs() < 1e-9 {
        return buf.to_vec();
    }
    let filtered;
    let src: &[f32] = if speed > 1.05 {
        let a = (1.0 - (-std::f64::consts::PI / speed).exp()) as f32;
        let mut f = Vec::with_capacity(buf.len());
        let (mut s1, mut s2) = (0f32, 0f32);
        for &x in buf {
            s1 += a * (x - s1);
            s2 += a * (s1 - s2);
            f.push(s2);
        }
        // Run it backwards too so the filter adds no phase shift to transients.
        let (mut s1, mut s2) = (0f32, 0f32);
        for x in f.iter_mut().rev() {
            s1 += a * (*x - s1);
            s2 += a * (s1 - s2);
            *x = s2;
        }
        filtered = f;
        &filtered
    } else {
        buf
    };
    let out_len = (buf.len() as f64 / speed).round() as usize;
    (0..out_len).map(|i| hermite(src, i as f64 * speed)).collect()
}

pub fn resample(s: &SampleData, speed: f64) -> SampleData {
    SampleData::new(resample_channel(&s.l, speed), resample_channel(&s.r, speed))
}

/// Decodes any supported file and converts it to stereo at `target_rate`.
pub fn load_file(path: &Path, target_rate: u32) -> Result<SampleData, String> {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::errors::Error;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| format!("unsupported file: {e}"))?;
    let track = format.default_track(TrackType::Audio).ok_or("no audio track")?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or("no audio codec parameters")?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| format!("unsupported codec: {e}"))?;
    let track_id = track.id;

    let mut l = Vec::new();
    let mut r = Vec::new();
    let mut rate = 0u32;
    let mut buf: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(_) => break,
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(audio) => {
                rate = audio.spec().rate();
                let ch = audio.spec().channels().count().max(1);
                buf.resize(audio.samples_interleaved(), 0.0);
                audio.copy_to_slice_interleaved(&mut buf[..]);
                for frame in buf.chunks_exact(ch) {
                    l.push(frame[0]);
                    r.push(if ch > 1 { frame[1] } else { frame[0] });
                }
            }
            Err(Error::DecodeError(_)) => (),
            Err(_) => break,
        }
    }
    if l.is_empty() || rate == 0 {
        return Err("file contains no audio".into());
    }
    let s = SampleData::new(l, r);
    Ok(if rate == target_rate { s } else { resample(&s, rate as f64 / target_rate as f64) })
}

pub fn save_wav(path: &Path, s: &SampleData, rate: u32) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for i in 0..s.len() {
        w.write_sample(s.l[i]).map_err(|e| e.to_string())?;
        w.write_sample(s.r[i]).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

/// Remembers the last waveform drawn, so a static view costs nothing per frame.
#[derive(Default)]
pub struct PeakCache {
    /// (buffer address, start, end, columns) the cached peaks were computed for.
    key: (usize, usize, usize, usize),
    peaks: Vec<(f32, f32)>,
}

impl PeakCache {
    pub fn get(&mut self, s: &std::sync::Arc<SampleData>, start: usize, end: usize, cols: usize) -> &[(f32, f32)] {
        let key = (std::sync::Arc::as_ptr(s) as usize, start, end, cols);
        if key != self.key || self.peaks.is_empty() {
            self.peaks = s.peaks(start, end, cols);
            self.key = key;
        }
        &self.peaks
    }
}
