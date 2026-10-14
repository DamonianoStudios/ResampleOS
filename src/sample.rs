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

/// Zero crossings of the resampling kernel on each side of its centre.
const SINC_ZEROS: usize = 24;
/// Kernel values stored per zero crossing; read between them linearly.
const SINC_STEPS: usize = 512;

/// A Kaiser-windowed sinc from its centre out to [`SINC_ZEROS`] crossings, sampled
/// [`SINC_STEPS`] times per crossing. Beta 9 keeps the stopband about 90 dB down.
fn sinc_table() -> &'static [f32] {
    static TABLE: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        fn bessel_i0(x: f64) -> f64 {
            let (mut sum, mut term) = (1.0, 1.0);
            for k in 1..40 {
                term *= (x / (2.0 * k as f64)).powi(2);
                sum += term;
            }
            sum
        }
        let beta = 9.0;
        let len = SINC_ZEROS * SINC_STEPS;
        (0..=len + 1)
            .map(|i| {
                let t = i as f64 / SINC_STEPS as f64;
                if t >= SINC_ZEROS as f64 {
                    return 0.0;
                }
                let x = std::f64::consts::PI * t;
                let sinc = if i == 0 { 1.0 } else { x.sin() / x };
                let w = t / SINC_ZEROS as f64;
                (sinc * bessel_i0(beta * (1.0 - w * w).max(0.0).sqrt()) / bessel_i0(beta)) as f32
            })
            .collect()
    })
}

/// Reads `buf` at `speed` (>1 = shorter and higher) through a band-limited (windowed
/// sinc) interpolator, as tape or a varispeed does it but without its aliasing: reading
/// faster lowers the kernel's cutoff with the new Nyquist, so nothing folds back down.
pub fn resample_channel(buf: &[f32], speed: f64) -> Vec<f32> {
    resample_pair(buf, None, speed).0
}

/// [`resample_channel`] for one or two channels at once, working out each kernel value
/// once for both.
fn resample_pair(l: &[f32], r: Option<&[f32]>, speed: f64) -> (Vec<f32>, Vec<f32>) {
    if l.is_empty() || (speed - 1.0).abs() < 1e-9 {
        return (l.to_vec(), r.map_or_else(Vec::new, <[f32]>::to_vec));
    }
    let table = sinc_table();
    // Just inside the lower of the two Nyquists, so the kernel's roll-off sits above
    // what is kept rather than across it.
    let cutoff = 0.97 * (1.0 / speed).min(1.0);
    let reach = (SINC_ZEROS as f64 / cutoff).ceil() as isize;
    let steps = cutoff * SINC_STEPS as f64;
    let n = l.len() as isize;
    let out_len = (l.len() as f64 / speed).round() as usize;
    let (mut ol, mut or) = (Vec::with_capacity(out_len), Vec::with_capacity(if r.is_some() { out_len } else { 0 }));
    let last = table.len() - 1;
    for i in 0..out_len {
        let pos = i as f64 * speed;
        let centre = pos.floor() as isize;
        let (lo, hi) = ((centre - reach + 1).max(0) as usize, (centre + reach).min(n - 1) as usize);
        // Distance from each input sample to the read position, in kernel steps.
        let mut d = (pos - lo as f64) * steps;
        let (mut al, mut ar) = (0f32, 0f32);
        for j in lo..=hi {
            let a = d.abs();
            let k = a as usize;
            if k < last {
                let f = (a - k as f64) as f32;
                let h = table[k] + (table[k + 1] - table[k]) * f;
                al += l[j] * h;
                if let Some(r) = r {
                    ar += r[j] * h;
                }
            }
            d -= steps;
        }
        ol.push(al * cutoff as f32);
        if r.is_some() {
            or.push(ar * cutoff as f32);
        }
    }
    (ol, or)
}

pub fn resample(s: &SampleData, speed: f64) -> SampleData {
    let (l, r) = resample_pair(&s.l, Some(&s.r), speed);
    SampleData::new(l, r)
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

#[cfg(test)]
mod resample_tests {
    use super::*;

    fn sine(hz: f32, n: usize, sr: f32) -> Vec<f32> {
        (0..n).map(|i| (std::f32::consts::TAU * hz * i as f32 / sr).sin() * 0.5).collect()
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
    }

    /// Residual after removing the best-fitting sine at `hz`: what is left is distortion
    /// and noise.
    fn residual(v: &[f32], hz: f32, sr: f32) -> f32 {
        let (mut a, mut b) = (0.0f64, 0.0f64);
        for (i, x) in v.iter().enumerate() {
            let w = std::f64::consts::TAU * hz as f64 * i as f64 / sr as f64;
            a += *x as f64 * w.sin();
            b += *x as f64 * w.cos();
        }
        let (a, b) = (2.0 * a / v.len() as f64, 2.0 * b / v.len() as f64);
        let r: Vec<f32> = v
            .iter()
            .enumerate()
            .map(|(i, x)| {
                let w = std::f64::consts::TAU * hz as f64 * i as f64 / sr as f64;
                *x - (a * w.sin() + b * w.cos()) as f32
            })
            .collect();
        rms(&r)
    }

    #[test]
    fn varispeed_moves_a_tone_cleanly_and_keeps_its_level() {
        let sr = 48000.0;
        for (speed, hz) in [(1.5, 1000.0), (0.75, 3000.0), (2.0, 5000.0), (1.0594631, 440.0)] {
            let out = resample_channel(&sine(hz, 48000, sr), speed);
            let mid = &out[2000..out.len() - 2000];
            let moved = hz * speed as f32;
            let res = residual(mid, moved, sr);
            assert!(res < 0.5e-3, "speed {speed}: {hz} Hz to {moved} Hz leaves {res} of distortion");
            assert!((rms(mid) - 0.5 / 2f32.sqrt()).abs() < 0.01, "level changed: {}", rms(mid));
        }
    }

    #[test]
    fn reading_faster_does_not_fold_the_top_down() {
        // 18 kHz read at 1.5x would land at 27 kHz, above Nyquist: it must vanish rather
        // than alias down to 21 kHz.
        let sr = 48000.0;
        let out = resample_channel(&sine(18000.0, 48000, sr), 1.5);
        let mid = &out[2000..out.len() - 2000];
        assert!(rms(mid) < 0.002, "aliasing left {}", rms(mid));
    }
}
