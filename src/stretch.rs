//! Time stretching and pitch shifting, three ways.
//!
//! - [`WarpMode::Elastic`]: a phase vocoder with identity phase locking. Every spectral
//!   peak keeps its own frequency and the bins around it move with it, so chords, pads
//!   and voices stretch without the warble of a grain-based method. Transients are kept
//!   by mapping time so that each hit and the moment before it play at their own speed,
//!   with the phases reset there, and the stretch is taken up in between. Pitch shifting
//!   can keep the formants where they were.
//! - [`WarpMode::Beats`]: waveform-similarity overlap-add between transients. Drum loops
//!   keep their punch and grain; tonal material warbles a little.
//! - [`WarpMode::Varispeed`]: the whole sound read faster or slower, as tape: pitch and
//!   length change together, with no processing artefacts at all.

use crate::sample::{SampleData, resample, resample_channel};
use rustfft::{Fft, FftPlanner, num_complex::Complex};
use serde::{Deserialize, Serialize};
use std::f32::consts::{PI, TAU};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum WarpMode {
    /// Phase vocoder: the all-rounder, and the one for anything tonal.
    #[default]
    Elastic,
    /// Grain splicing between hits: for drums.
    Beats,
    /// Tape speed: pitch and length move together.
    Varispeed,
}

impl WarpMode {
    pub const ALL: [WarpMode; 3] = [WarpMode::Elastic, WarpMode::Beats, WarpMode::Varispeed];

    pub fn name(self) -> &'static str {
        match self {
            WarpMode::Elastic => "Elastic",
            WarpMode::Beats => "Beats",
            WarpMode::Varispeed => "Varispeed",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            WarpMode::Elastic => "Pitch and length change independently, smoothly. Best for anything tonal, and fine for most loops",
            WarpMode::Beats => "Splices the sound between hits. Keeps drums crisp and gritty; tonal parts may warble",
            WarpMode::Varispeed => "Like changing tape speed: faster is higher, slower is lower. No artefacts, but pitch and length are tied",
        }
    }
}

/// Settings for one warp.
#[derive(Clone, Copy, Debug)]
pub struct Warp {
    pub mode: WarpMode,
    /// Length multiplier (>1 = longer).
    pub ratio: f64,
    pub semis: f32,
    /// Keep the formants (the vowel and body of a voice or instrument) where they were
    /// when shifting pitch. Elastic only.
    pub formants: bool,
}

/// Semitones a varispeed moves pitch by for a length multiplier.
pub fn varispeed_semis(ratio: f64) -> f32 {
    (-12.0 * ratio.max(1e-6).log2()) as f32
}

/// Length multiplier a varispeed gives for a pitch shift in semitones.
pub fn varispeed_ratio(semis: f32) -> f64 {
    2f64.powf(-semis as f64 / 12.0)
}

/// Applies a warp. `onsets` (transient positions in frames) are kept intact by the
/// Elastic and Beats modes; Varispeed has no use for them.
pub fn warp(s: &SampleData, onsets: &[u32], w: Warp, sr: f32) -> SampleData {
    match w.mode {
        WarpMode::Varispeed => resample(s, 1.0 / w.ratio.max(1e-6)),
        WarpMode::Beats => conform(s, onsets, w.ratio, w.semis, sr),
        WarpMode::Elastic => {
            let pf = 2f64.powf(w.semis as f64 / 12.0);
            let shift = (pf - 1.0).abs() > 1e-6;
            // Stretch so that the pitch-shifting resample lands on the wanted length.
            let formant = (shift && w.formants).then_some(pf);
            let stretched = elastic(s, onsets, w.ratio * pf, formant, sr);
            if shift { resample(&stretched, pf) } else { stretched }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Elastic: phase vocoder.

/// Where in the input each point of the output reads from: straight lines between
/// breakpoints. `locked` stretches play at the input's own speed and carry the input's
/// own phases, so what they cover comes through exactly as it was.
struct TimeMap {
    /// (output frame, input frame, locked until the next breakpoint)
    points: Vec<(f64, f64, bool)>,
}

impl TimeMap {
    /// Maps `n` input frames onto `total` output frames. Around each onset, `pre` frames
    /// before it and `head` frames after it are locked; the rest of each gap between
    /// onsets takes up the change of length.
    fn new(n: usize, total: usize, onsets: &[u32], head: usize, pre: usize) -> TimeMap {
        let ratio = total as f64 / n as f64;
        let mut marks: Vec<usize> = vec![0];
        for &o in onsets {
            let o = o as usize;
            if o > *marks.last().unwrap() + head && o + head < n {
                marks.push(o);
            }
        }
        marks.push(n);
        let mut points = Vec::new();
        for (k, w) in marks.windows(2).enumerate() {
            let (a, b) = (w[0], w[1]);
            let out_a = if k == 0 { 0.0 } else { (a as f64 * ratio).round() };
            let out_b = if b == n { total as f64 } else { (b as f64 * ratio).round() };
            // Locked stretches can't take more than most of either side's room.
            let has_head = k > 0 || onsets.first() == Some(&0);
            let has_pre = b < n;
            let want = (if has_head { head } else { 0 } + if has_pre { pre } else { 0 }) as f64;
            let room = ((b - a) as f64).min(out_b - out_a) * 0.8;
            let shrink = if want > room && want > 0.0 { room / want } else { 1.0 };
            let h = if has_head { (head as f64 * shrink).floor() } else { 0.0 };
            let p = if has_pre { (pre as f64 * shrink).floor() } else { 0.0 };
            if h > 0.0 {
                points.push((out_a, a as f64, true));
            }
            points.push((out_a + h, a as f64 + h, false));
            if p > 0.0 {
                points.push((out_b - p, b as f64 - p, true));
            }
        }
        points.push((total as f64, n as f64, false));
        TimeMap { points }
    }

    /// The input position for output position `t`, and whether it is locked.
    fn at(&self, t: f64) -> (f64, bool) {
        let pts = &self.points;
        let i = pts.partition_point(|p| p.0 <= t).clamp(1, pts.len() - 1);
        let (a, b) = (pts[i - 1], pts[i]);
        let span = (b.0 - a.0).max(1e-9);
        let f = ((t - a.0) / span).clamp(0.0, 1.0);
        let pos = if t < a.0 { a.1 - (a.0 - t) } else if t > b.0 { b.1 + (t - b.0) } else { a.1 + (b.1 - a.1) * f };
        (pos, a.2 && t >= a.0 && t < b.0)
    }
}

/// FFT size for the phase vocoder: about 43 ms, a compromise between holding low notes
/// steady and keeping attacks tight.
fn pv_size(sr: f32) -> usize {
    ((sr * 0.043) as usize).next_power_of_two().clamp(512, 8192)
}

struct Pv {
    n: usize,
    fwd: Arc<dyn Fft<f32>>,
    inv: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    buf: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl Pv {
    fn new(n: usize) -> Pv {
        let mut planner = FftPlanner::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let scratch_len = fwd.get_inplace_scratch_len().max(inv.get_inplace_scratch_len());
        Pv {
            n,
            fwd,
            inv,
            window: (0..n).map(|i| 0.5 - 0.5 * (TAU * i as f32 / n as f32).cos()).collect(),
            buf: vec![Complex::default(); n],
            scratch: vec![Complex::default(); scratch_len],
        }
    }

    /// Windowed spectrum of `x` around `centre` (zeros beyond its ends).
    fn spectrum(&mut self, x: &[f32], centre: isize, out: &mut [Complex<f32>]) {
        let start = centre - self.n as isize / 2;
        for (i, b) in self.buf.iter_mut().enumerate() {
            let j = start + i as isize;
            let v = if j >= 0 && (j as usize) < x.len() { x[j as usize] } else { 0.0 };
            *b = Complex::new(v * self.window[i], 0.0);
        }
        self.fwd.process_with_scratch(&mut self.buf, &mut self.scratch);
        out.copy_from_slice(&self.buf[..out.len()]);
    }

    /// Windowed frame from the positive-frequency half `half`, added into `out` at `centre`.
    fn synthesise(&mut self, half: &[Complex<f32>], out: &mut [f32], centre: isize) {
        let n = self.n;
        for k in 0..n {
            self.buf[k] = if k < half.len() {
                half[k]
            } else {
                half[n - k].conj()
            };
        }
        self.inv.process_with_scratch(&mut self.buf, &mut self.scratch);
        let start = centre - n as isize / 2;
        let scale = 1.0 / n as f32;
        for i in 0..n {
            let j = start + i as isize;
            if j >= 0 && (j as usize) < out.len() {
                out[j as usize] += self.buf[i].re * scale * self.window[i];
            }
        }
    }
}

fn princarg(x: f32) -> f32 {
    x - TAU * ((x + PI) / TAU).floor()
}

/// The spectral envelope of a magnitude spectrum, by cepstral smoothing: the log
/// spectrum with its fine (harmonic) detail lifted out.
fn envelope(mags: &[f32], n: usize, lifter: usize, fft: &Arc<dyn Fft<f32>>, scratch: &mut [Complex<f32>]) -> Vec<f32> {
    let half = mags.len();
    let mut c: Vec<Complex<f32>> = (0..n)
        .map(|k| {
            let m = if k < half { mags[k] } else { mags[n - k] };
            Complex::new((m + 1e-9).ln(), 0.0)
        })
        .collect();
    fft.process_with_scratch(&mut c, scratch);
    // Real, even cepstrum: keep the low quefrencies (with a gentle taper) on both sides.
    for (q, v) in c.iter_mut().enumerate() {
        let d = q.min(n - q);
        let w = if d < lifter { 1.0 } else if d < lifter * 2 { 0.5 + 0.5 * (PI * (d - lifter) as f32 / lifter as f32).cos() } else { 0.0 };
        *v *= w / n as f32;
    }
    fft.process_with_scratch(&mut c, scratch);
    c[..half].iter().map(|v| v.re.exp()).collect()
}

/// Phase-vocoder stretch of the whole sample to `ratio` times its length. With
/// `formant = Some(pf)`, the spectral envelope is pre-warped so that resampling by `pf`
/// afterwards (the pitch shift) leaves the formants where they were.
fn elastic(s: &SampleData, onsets: &[u32], ratio: f64, formant: Option<f64>, sr: f32) -> SampleData {
    let len = s.len();
    let total = (len as f64 * ratio).round() as usize;
    if len == 0 || total == 0 {
        return SampleData::new(vec![0.0; total], vec![0.0; total]);
    }
    if (ratio - 1.0).abs() < 1e-9 && formant.is_none() {
        return s.slice(0, len);
    }
    let n = pv_size(sr);
    let hop = n / 4;
    let bins = n / 2 + 1;
    let head = (0.015 * sr) as usize;
    let map = TimeMap::new(len, total, onsets, head, n / 2);
    let mid: Vec<f32> = s.l.iter().zip(&s.r).map(|(a, b)| (a + b) * 0.5).collect();
    let same = s.l == s.r;

    let mut pv = Pv::new(n);
    let env_fft = FftPlanner::new().plan_fft_forward(n);
    let mut env_scratch = vec![Complex::default(); env_fft.get_inplace_scratch_len()];
    let lifter = ((0.0012 * sr) as usize).clamp(8, n / 8);

    let (mut ol, mut or) = (vec![0f32; total], vec![0f32; total]);
    let mut norm = vec![0f32; total];
    let (mut a, mut b) = (vec![Complex::default(); bins], vec![Complex::default(); bins]);
    let (mut xl, mut xr) = (vec![Complex::default(); bins], vec![Complex::default(); bins]);
    let mut phase_out = vec![0f32; bins];
    let mut next_phase = vec![0f32; bins];
    let mut fresh = true;
    let mut mags = vec![0f32; bins];
    let mut peaks: Vec<usize> = Vec::with_capacity(bins / 2);
    let mut owner = vec![0usize; bins];
    let mut rot = vec![Complex::default(); bins];
    let win_sq: Vec<f32> = pv.window.iter().map(|w| w * w).collect();
    let bin_w: Vec<f32> = (0..bins).map(|k| TAU * k as f32 / n as f32).collect();

    let frames = total / hop + 2;
    for m in 0..frames {
        let c_out = (m * hop) as f64;
        let (pos, locked) = map.at(c_out);
        let c_in = pos.round() as isize;
        pv.spectrum(&mid, c_in, &mut a);
        for k in 0..bins {
            mags[k] = a[k].norm();
        }
        if locked || fresh {
            // Phases straight from the input: a locked stretch comes out as it went in,
            // and the first frame after it starts the stretch from the input's phases.
            for k in 0..bins {
                phase_out[k] = a[k].arg();
            }
            fresh = locked;
        } else {
            // Each peak's true frequency, from how its phase moved over one hop.
            pv.spectrum(&mid, c_in - hop as isize, &mut b);
            peaks.clear();
            for k in 2..bins.saturating_sub(2) {
                let v = mags[k];
                if v > mags[k - 1] && v >= mags[k + 1] && v > mags[k - 2] && v >= mags[k + 2] && v > 1e-7 {
                    peaks.push(k);
                }
            }
            if peaks.is_empty() {
                peaks.push(mags.iter().enumerate().max_by(|x, y| x.1.total_cmp(y.1)).map_or(0, |(k, _)| k));
            }
            // Every bin follows the peak nearest to it.
            let mut p = 0;
            for (k, o) in owner.iter_mut().enumerate() {
                while p + 1 < peaks.len() && (peaks[p + 1] as isize - k as isize).abs() <= (peaks[p] as isize - k as isize).abs() {
                    p += 1;
                }
                *o = peaks[p];
            }
            for &k in &peaks {
                let (pa, pb) = (a[k].arg(), b[k].arg());
                let dev = princarg(pa - pb - bin_w[k] * hop as f32);
                let omega = bin_w[k] + dev / hop as f32;
                next_phase[k] = phase_out[k] + omega * hop as f32;
            }
            for k in 0..bins {
                let o = owner[k];
                if o != k {
                    next_phase[k] = next_phase[o] + a[k].arg() - a[o].arg();
                }
            }
            std::mem::swap(&mut phase_out, &mut next_phase);
        }
        // The rotation that takes the input's phases to the output's, shared by both
        // channels so the stereo image holds.
        for k in 0..bins {
            let r = phase_out[k] - a[k].arg();
            rot[k] = Complex::from_polar(1.0, r);
        }
        if let Some(pf) = formant {
            let env = envelope(&mags, n, lifter, &env_fft, &mut env_scratch);
            for k in 0..bins {
                // The envelope this bin must carry so that, once resampling moves it to
                // k * pf, it sits under the original envelope there.
                let at = (k as f64 * pf).min((bins - 1) as f64);
                let (i, f) = (at as usize, (at - at.floor()) as f32);
                let target = env[i] + (env[(i + 1).min(bins - 1)] - env[i]) * f;
                rot[k] *= (target / env[k].max(1e-9)).clamp(0.05, 20.0);
            }
        }
        let centre = c_out as isize;
        if same {
            for k in 0..bins {
                xl[k] = a[k] * rot[k];
            }
            pv.synthesise(&xl, &mut ol, centre);
        } else {
            pv.spectrum(&s.l, c_in, &mut xl);
            pv.spectrum(&s.r, c_in, &mut xr);
            for k in 0..bins {
                xl[k] *= rot[k];
                xr[k] *= rot[k];
            }
            pv.synthesise(&xl, &mut ol, centre);
            pv.synthesise(&xr, &mut or, centre);
        }
        let start = centre - n as isize / 2;
        for (i, w) in win_sq.iter().enumerate() {
            let j = start + i as isize;
            if j >= 0 && (j as usize) < total {
                norm[j as usize] += w;
            }
        }
    }
    for i in 0..total {
        let g = 1.0 / norm[i].max(0.25);
        ol[i] *= g;
        or[i] *= g;
    }
    if same {
        or.copy_from_slice(&ol);
    }
    SampleData::new(ol, or)
}

// ---------------------------------------------------------------------------------------
// Beats: waveform-similarity overlap-add between transients.

/// Stretches `(l, r)` to exactly `out_len` frames. The first and last grains are pinned
/// to the input's own start and end, so the result splices seamlessly to its neighbours.
fn wsola(l: &[f32], r: &[f32], out_len: usize, grain: usize) -> (Vec<f32>, Vec<f32>) {
    let n = l.len();
    if out_len == 0 || n == 0 {
        return (Vec::new(), Vec::new());
    }
    if n == out_len {
        return (l.to_vec(), r.to_vec());
    }
    // Too short to hold two grains: plain resampling is inaudible at this size.
    if n < 512 || out_len < 512 {
        let speed = n as f64 / out_len as f64;
        let fix = |mut v: Vec<f32>| {
            v.resize(out_len, 0.0);
            v
        };
        return (fix(resample_channel(l, speed)), fix(resample_channel(r, speed)));
    }
    let grain = grain.min(n / 2).min(out_len / 2) & !1;
    let hop = grain / 2;
    let tolerance = grain / 4;
    let mono: Vec<f32> = l.iter().zip(r).map(|(a, b)| a + b).collect();
    // Normalised cross-correlation of what a candidate would play against what should
    // come next: both energies count, so a loud candidate doesn't win on level alone.
    let score = |cand: usize, natural: usize, step: usize| {
        let (mut dot, mut ec, mut en) = (0f32, 1e-9f32, 1e-9f32);
        let mut j = 0;
        while j < hop {
            let (c, t) = (mono[cand + j], mono[natural + j]);
            dot += c * t;
            ec += c * c;
            en += t * t;
            j += step;
        }
        dot / (ec * en).sqrt()
    };

    let mut ol = vec![0f32; out_len];
    let mut or = vec![0f32; out_len];
    let mut norm = vec![0f32; out_len];
    let count = (out_len - grain).div_ceil(hop) + 1;
    let scale = (n - grain) as f64 / (out_len - grain).max(1) as f64;
    let mut prev = 0usize;
    let window: Vec<f32> = (0..grain).map(|j| 0.5 - 0.5 * (TAU * j as f32 / grain as f32).cos()).collect();

    for k in 0..count {
        let last = k == count - 1;
        let out_at = if last { out_len - grain } else { k * hop };
        let ideal = (out_at as f64 * scale).round() as usize;
        let pos = if k == 0 || last {
            ideal
        } else {
            // Pick the grain near `ideal` that best continues what the previous one was playing.
            let natural = prev + hop;
            if natural + hop > n {
                ideal
            } else {
                let lo = ideal.saturating_sub(tolerance);
                let hi = (ideal + tolerance).min(n - grain);
                // A coarse pass, then every position around its winner.
                let mut best = ideal.min(n - grain);
                let mut best_score = f32::MIN;
                let mut cand = lo;
                while cand <= hi {
                    let sc = score(cand, natural, 2);
                    if sc > best_score {
                        best_score = sc;
                        best = cand;
                    }
                    cand += 4;
                }
                let (lo2, hi2) = (best.saturating_sub(3).max(lo), (best + 3).min(hi));
                let mut fine = (best, f32::MIN);
                for cand in lo2..=hi2 {
                    let sc = score(cand, natural, 1);
                    if sc > fine.1 {
                        fine = (cand, sc);
                    }
                }
                fine.0
            }
        };
        for j in 0..grain {
            let mut w = window[j];
            // Outer edges stay at full level; there's no neighbour to crossfade with.
            if (k == 0 && j < hop) || (last && j >= hop) {
                w = 1.0;
            }
            let w = w.max(1e-4);
            ol[out_at + j] += l[pos + j] * w;
            or[out_at + j] += r[pos + j] * w;
            norm[out_at + j] += w;
        }
        prev = pos;
    }
    for i in 0..out_len {
        if norm[i] > 0.0 {
            ol[i] /= norm[i];
            or[i] /= norm[i];
        }
    }
    (ol, or)
}

/// Stretches to `ratio` times the original length (>1 = longer) by WSOLA. With `onsets`,
/// each transient's first few milliseconds pass through at original speed.
pub fn stretch(s: &SampleData, onsets: &[u32], ratio: f64, sr: f32) -> SampleData {
    let n = s.len();
    if n == 0 || (ratio - 1.0).abs() < 1e-6 {
        return s.slice(0, n);
    }
    let head = (0.015 * sr) as usize;
    let grain = ((0.043 * sr) as usize).max(256);
    let mut marks: Vec<usize> = vec![0];
    for &o in onsets {
        let o = o as usize;
        if o > *marks.last().unwrap() + head * 2 && o < n {
            marks.push(o);
        }
    }
    marks.push(n);

    let total = (n as f64 * ratio).round() as usize;
    let mut ol = Vec::with_capacity(total + 16);
    let mut or = Vec::with_capacity(total + 16);
    for w in marks.windows(2) {
        let (a, b) = (w[0], w[1]);
        // Target the cumulative position, so rounding never drifts the grid.
        let target_end = if b == n { total } else { (b as f64 * ratio).round() as usize };
        let out_len = target_end.saturating_sub(ol.len());
        let seg = b - a;
        // The very first segment has no transient unless the sample starts on one.
        let keep = if onsets.is_empty() { 0 } else { head.min(seg).min(out_len) };
        ol.extend_from_slice(&s.l[a..a + keep]);
        or.extend_from_slice(&s.r[a..a + keep]);
        let (tl, tr) = wsola(&s.l[a + keep..b], &s.r[a + keep..b], out_len - keep, grain);
        ol.extend_from_slice(&tl);
        or.extend_from_slice(&tr);
        // An empty tail can't supply its share; pad so the next transient stays on the grid.
        ol.resize(target_end, 0.0);
        or.resize(target_end, 0.0);
    }
    SampleData::new(ol, or)
}

/// Independent tempo and pitch change by WSOLA and resampling: `ratio` scales length,
/// `semis` shifts pitch.
pub fn conform(s: &SampleData, onsets: &[u32], ratio: f64, semis: f32, sr: f32) -> SampleData {
    let pf = 2f64.powf(semis as f64 / 12.0);
    // Stretch so that the pitch-shifting resample lands on the wanted length.
    let stretched = stretch(s, onsets, ratio * pf, sr);
    if (pf - 1.0).abs() < 1e-6 { stretched } else { resample(&stretched, pf) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;

    fn hits(n: usize, onsets: &[u32]) -> SampleData {
        let mut l = vec![0f32; n];
        for &o in onsets {
            for i in 0..8000 {
                let t = i as f32 / SR;
                l[o as usize + i] = (-t * 25.0).exp() * (TAU * 200.0 * t).sin();
            }
            l[o as usize] = 1.0;
        }
        SampleData::new(l.clone(), l)
    }

    /// Strongest frequency in `x` between `lo` and `hi` Hz, by a fine DFT scan.
    fn pitch_of(x: &[f32], lo: f32, hi: f32) -> f32 {
        let mut best = (0.0, 0.0);
        let mut f = lo;
        while f < hi {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, v) in x.iter().enumerate() {
                let w = TAU * f * i as f32 / SR;
                re += v * w.cos();
                im += v * w.sin();
            }
            let m = re * re + im * im;
            if m > best.1 {
                best = (f, m);
            }
            f += 0.5;
        }
        best.0
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn length_and_transients_hold_in_every_mode() {
        let n = 48000;
        let onsets = [0u32, 12000, 24000, 36000];
        let s = hits(n, &onsets);
        for mode in [WarpMode::Beats, WarpMode::Elastic] {
            for ratio in [134.0 / 88.0, 0.7] {
                let out = warp(&s, &onsets, Warp { mode, ratio, semis: 0.0, formants: false }, SR);
                assert_eq!(out.len(), (n as f64 * ratio).round() as usize, "{mode:?}");
                for &o in &onsets[1..] {
                    let at = (o as f64 * ratio).round() as usize;
                    // The click at the onset survives at its new place.
                    let near = out.l[at.saturating_sub(2)..at + 3].iter().fold(0f32, |m, v| m.max(*v));
                    assert!(near > 0.9, "{mode:?} at ratio {ratio}: transient at {at} came out {near}");
                }
                assert!(out.peak() < 1.5);
            }
        }
        let shifted = conform(&s, &onsets, 1.0, -4.0, SR);
        assert!(shifted.len().abs_diff(n) < 4);
    }

    #[test]
    fn elastic_stretch_keeps_a_chord_in_tune_and_steady() {
        // A two-note chord (A3 and E4), stretched half as long again: the notes must keep
        // their frequencies and the level must not pump.
        let n = 48000;
        let v: Vec<f32> = (0..n).map(|i| {
            let t = i as f32 / SR;
            0.3 * (TAU * 220.0 * t).sin() + 0.3 * (TAU * 329.63 * t).sin()
        }).collect();
        let s = SampleData::new(v.clone(), v);
        let out = warp(&s, &[], Warp { mode: WarpMode::Elastic, ratio: 1.5, semis: 0.0, formants: false }, SR);
        assert_eq!(out.len(), 72000);
        let mid = &out.l[12000..60000];
        assert!((pitch_of(mid, 200.0, 260.0) - 220.0).abs() < 1.0);
        assert!((pitch_of(mid, 300.0, 360.0) - 329.6).abs() < 1.0);
        // Level steady to within a dB or so across the middle, in 50 ms slices.
        let levels: Vec<f32> = mid.chunks(2400).map(rms).collect();
        let (lo, hi) = levels.iter().fold((f32::MAX, 0f32), |(a, b), v| (a.min(*v), b.max(*v)));
        assert!(hi / lo < 1.15, "level pumps between {lo} and {hi}");
    }

    #[test]
    fn pitch_shift_moves_the_note_and_keeps_the_length() {
        let n = 48000;
        let v: Vec<f32> = (0..n).map(|i| 0.5 * (TAU * 220.0 * i as f32 / SR).sin()).collect();
        let s = SampleData::new(v.clone(), v);
        for formants in [false, true] {
            let out = warp(&s, &[], Warp { mode: WarpMode::Elastic, ratio: 1.0, semis: 7.0, formants }, SR);
            assert!(out.len().abs_diff(n) < 4, "length {}", out.len());
            let p = pitch_of(&out.l[8000..40000], 300.0, 360.0);
            assert!((p - 329.6).abs() < 1.5, "a fifth up should be 329.6 Hz, got {p}");
        }
    }

    #[test]
    fn varispeed_ties_pitch_to_length() {
        let n = 48000;
        let v: Vec<f32> = (0..n).map(|i| 0.5 * (TAU * 440.0 * i as f32 / SR).sin()).collect();
        let s = SampleData::new(v.clone(), v);
        // Twice as long is an octave down.
        let out = warp(&s, &[], Warp { mode: WarpMode::Varispeed, ratio: 2.0, semis: 0.0, formants: false }, SR);
        assert_eq!(out.len(), 96000);
        assert!((pitch_of(&out.l[10000..80000], 200.0, 240.0) - 220.0).abs() < 0.5);
        assert!((varispeed_semis(2.0) + 12.0).abs() < 1e-4);
        assert!((varispeed_ratio(12.0) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn keeping_formants_holds_the_spectral_envelope() {
        // A buzzy 150 Hz tone through a resonance at 1 kHz, shifted up a fifth. Without
        // formant keeping the resonance moves up with it; with it, it stays near 1 kHz.
        let n = 48000;
        let mut buzz: Vec<f32> = (0..n).map(|i| {
            let t = i as f32 / SR;
            (1..40).map(|h| (TAU * 150.0 * h as f32 * t).sin() / h as f32).sum::<f32>() * 0.2
        }).collect();
        // A two-pole resonator at 1 kHz.
        let (f0, q) = (1000.0f32, 6.0f32);
        let w = TAU * f0 / SR;
        let alpha = w.sin() / (2.0 * q);
        let (b0, a1, a2) = (alpha, -2.0 * w.cos(), 1.0 - alpha);
        let a0 = 1.0 + alpha;
        let (mut x1, mut x2, mut y1, mut y2) = (0f32, 0f32, 0f32, 0f32);
        for v in buzz.iter_mut() {
            let x = *v;
            let y = (b0 * x - b0 * x2 - a1 * y1 - a2 * y2) / a0;
            (x2, x1, y2, y1) = (x1, x, y1, y);
            *v = y;
        }
        let s = SampleData::new(buzz.clone(), buzz);
        // Where the spectrum, smoothed over 300 Hz so single harmonics don't count, peaks.
        let centroid_peak = |x: &[f32]| {
            let n = 32768;
            let mut buf: Vec<Complex<f32>> = (0..n).map(|i| Complex::new(*x.get(i).unwrap_or(&0.0), 0.0)).collect();
            FftPlanner::new().plan_fft_forward(n).process(&mut buf);
            let hz = SR / n as f32;
            let mags: Vec<f32> = buf[..n / 2].iter().map(|c| c.norm_sqr()).collect();
            let half = (150.0 / hz) as usize;
            let (lo, hi) = ((600.0 / hz) as usize, (2500.0 / hz) as usize);
            (lo..hi)
                .map(|k| (k, mags[k - half..k + half].iter().sum::<f32>()))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map_or(0.0, |(k, _)| k as f32 * hz)
        };
        let plain = warp(&s, &[], Warp { mode: WarpMode::Elastic, ratio: 1.0, semis: 7.0, formants: false }, SR);
        let kept = warp(&s, &[], Warp { mode: WarpMode::Elastic, ratio: 1.0, semis: 7.0, formants: true }, SR);
        let (p, k) = (centroid_peak(&plain.l[8000..40000]), centroid_peak(&kept.l[8000..40000]));
        assert!(p > 1300.0, "without formant keeping the resonance should rise: {p}");
        assert!((k - 1000.0).abs() < 200.0, "with it, the resonance should stay near 1 kHz: {k}");
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// Not a pass/fail test: times each mode on a minute of stereo.
    #[test]
    #[ignore]
    fn warp_speed() {
        let sr = 48000.0;
        let n = 48000 * 60;
        let l: Vec<f32> = (0..n).map(|i| ((i as f32 * 0.013).sin() + (i as f32 * 0.0071).sin()) * 0.3).collect();
        let r: Vec<f32> = l.iter().map(|v| v * 0.9).collect();
        let s = SampleData::new(l, r);
        let onsets: Vec<u32> = (0..n as u32).step_by(12000).collect();
        for (mode, formants) in [(WarpMode::Elastic, false), (WarpMode::Elastic, true), (WarpMode::Beats, false), (WarpMode::Varispeed, false)] {
            let t = std::time::Instant::now();
            let out = warp(&s, &onsets, Warp { mode, ratio: 1.2, semis: 3.0, formants }, sr);
            println!("SPEED {mode:?} formants {formants}: {:.2} s for a minute ({} frames)", t.elapsed().as_secs_f32(), out.len());
        }
        let t = std::time::Instant::now();
        let _ = crate::sample::resample(&s, 44100.0 / 48000.0);
        println!("SPEED rate conversion: {:.2} s for a minute", t.elapsed().as_secs_f32());
    }
}
