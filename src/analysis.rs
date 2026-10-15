//! Offline analysis: transient detection, tempo, key and monophonic pitch.

use crate::music::hz_to_midi;
use crate::sample::SampleData;
use rustfft::{FftPlanner, num_complex::Complex};
use std::f32::consts::PI;

#[derive(Clone, Debug, Default)]
pub struct Analysis {
    /// 0 when the material is too short or too sparse to carry a tempo.
    pub bpm: f32,
    pub bpm_conf: f32,
    pub key_root: u8,
    pub key_minor: bool,
    pub key_conf: f32,
    pub pcp: [f32; 12],
    /// Fractional MIDI note of a monophonic source, when one is clearly present.
    pub pitch: Option<f32>,
    pub onsets: Vec<u32>,
}

const ODF_N: usize = 1024;
const ODF_HOP: usize = 128;

fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / n as f32).cos()).collect()
}

/// Multi-band spectral flux. Each band is normalised by its own average so quiet
/// high-frequency hits (hats, plucks) register alongside kicks.
pub fn onset_envelope(mono: &[f32], sr: f32) -> Vec<f32> {
    if mono.len() < ODF_N {
        return Vec::new();
    }
    let fft = FftPlanner::<f32>::new().plan_fft_forward(ODF_N);
    let win = hann(ODF_N);
    let frames = (mono.len() - ODF_N) / ODF_HOP + 1;
    let edges_hz = [0.0, 200.0, 800.0, 3200.0, sr * 0.5];
    let edges: Vec<usize> =
        edges_hz.iter().map(|f| ((f / sr * ODF_N as f32) as usize).min(ODF_N / 2)).collect();
    let mut prev = vec![0f32; ODF_N / 2];
    let mut flux = vec![[0f32; 4]; frames];
    let mut buf = vec![Complex::new(0f32, 0f32); ODF_N];
    for (f, fl) in flux.iter_mut().enumerate() {
        let at = f * ODF_HOP;
        for i in 0..ODF_N {
            buf[i] = Complex::new(mono[at + i] * win[i], 0.0);
        }
        fft.process(&mut buf);
        for b in 0..4 {
            let mut acc = 0.0;
            for k in edges[b]..edges[b + 1] {
                let m = (1.0 + 100.0 * buf[k].norm()).ln();
                acc += (m - prev[k]).max(0.0);
                prev[k] = m;
            }
            fl[b] = acc;
        }
    }
    let mut mean = [1e-6f32; 4];
    for fl in &flux {
        for b in 0..4 {
            mean[b] += fl[b] / frames as f32;
        }
    }
    flux.iter().map(|fl| (0..4).map(|b| fl[b] / mean[b]).sum::<f32>() * 0.25).collect()
}

/// Transient positions in frames. `sensitivity` 0..1: higher finds more.
pub fn detect_onsets(mono: &[f32], sr: f32, sensitivity: f32) -> Vec<u32> {
    pick_onsets(&onset_envelope(mono, sr), mono, sr, sensitivity)
}

/// Peak-picks an envelope from [`onset_envelope`]. Split out because the envelope is the
/// expensive part and does not depend on the sensitivity, so callers can keep it around.
pub fn pick_onsets(odf: &[f32], mono: &[f32], sr: f32, sensitivity: f32) -> Vec<u32> {
    let n = odf.len();
    let mut out: Vec<u32> = Vec::new();
    let mult = 1.15 + (1.0 - sensitivity.clamp(0.0, 1.0)) * 2.6;
    let min_gap = (0.05 * sr) as u32;
    let wide = (0.12 * sr / ODF_HOP as f32) as usize;
    let near = (0.02 * sr / ODF_HOP as f32).max(1.0) as usize;
    for i in 0..n {
        let a = i.saturating_sub(wide);
        let b = (i + wide + 1).min(n);
        let local_mean = odf[a..b].iter().sum::<f32>() / (b - a) as f32;
        let na = i.saturating_sub(near);
        let nb = (i + near + 1).min(n);
        let is_max = odf[na..nb].iter().all(|&v| v <= odf[i]);
        if !is_max || odf[i] < local_mean * mult + 0.15 {
            continue;
        }
        let pos = refine_onset(mono, i * ODF_HOP);
        if out.last().is_none_or(|&last| pos > last + min_gap) {
            out.push(pos);
        }
    }
    out
}

/// The flux peak says "somewhere in this window"; find where the level actually jumps,
/// then back up to a zero crossing so the slice starts clean.
fn refine_onset(mono: &[f32], frame_start: usize) -> u32 {
    let a = frame_start + ODF_N / 2 - ODF_HOP;
    let b = (frame_start + ODF_N).min(mono.len());
    if a >= b {
        return frame_start.min(mono.len().saturating_sub(1)) as u32;
    }
    let step = 32;
    let energy = |at: usize| -> f32 {
        mono[at..(at + step).min(mono.len())].iter().map(|v| v * v).sum()
    };
    let mut best = a;
    let mut best_rise = 0.0;
    let mut prev = energy(a.saturating_sub(step));
    let mut at = a;
    while at < b {
        let e = energy(at);
        let rise = e - prev;
        if rise > best_rise {
            best_rise = rise;
            best = at;
        }
        prev = e;
        at += step;
    }
    let lo = best.saturating_sub(96);
    let mut pos = best;
    for i in (lo + 1..=best).rev() {
        if i < mono.len() && (mono[i] >= 0.0) != (mono[i - 1] >= 0.0) {
            pos = i;
            break;
        }
    }
    pos as u32
}

/// Tempo by comb-filtering the onset envelope's autocorrelation.
#[cfg(test)]
pub fn detect_bpm(mono: &[f32], sr: f32) -> (f32, f32) {
    bpm_from_envelope(&onset_envelope(mono, sr), mono.len(), sr)
}

/// Tempo from an existing onset envelope; `frames` is the length of the audio it describes.
/// Only the first 90 seconds are examined.
pub fn bpm_from_envelope(envelope: &[f32], frames: usize, sr: f32) -> (f32, f32) {
    let fps = sr / ODF_HOP as f32;
    let mut odf = envelope[..envelope.len().min((fps * 90.0) as usize)].to_vec();
    let n = odf.len();
    if n < (fps * 2.0) as usize {
        return (0.0, 0.0);
    }
    let mean = odf.iter().sum::<f32>() / n as f32;
    for v in odf.iter_mut() {
        *v = (*v - mean).max(0.0);
    }
    let max_lag = ((fps * 60.0 / 55.0) as usize * 4 + 2).min(n - 1);
    let ac: Vec<f32> = (0..=max_lag)
        .map(|lag| (0..n - lag).map(|i| odf[i] * odf[i + lag]).sum::<f32>() / (n - lag) as f32)
        .collect();
    if ac[0] <= 1e-9 {
        return (0.0, 0.0);
    }
    let at = |lag: f32| -> f32 {
        let i = lag as usize;
        if i + 1 >= ac.len() {
            return 0.0;
        }
        let t = lag - i as f32;
        ac[i] + (ac[i + 1] - ac[i]) * t
    };
    let mut best = (0f32, 0f32);
    let mut bpm = 60.0f32;
    while bpm <= 200.0 {
        let lag = fps * 60.0 / bpm;
        // Energy at half the lag (8ths) supports a beat; energy at a third of it means
        // the candidate is really 1.5x the true beat, the usual 2/3-tempo trap.
        let comb = at(lag) + 0.7 * at(lag * 2.0) + 0.5 * at(lag * 4.0) + 0.3 * at(lag * 3.0)
            + 0.4 * at(lag * 0.5)
            - 0.4 * at(lag / 3.0);
        // Mild preference for mid tempos to settle half/double-time ties.
        let prior = (-0.5 * ((bpm / 105.0).log2() / 0.85).powi(2)).exp();
        let score = comb * prior;
        if score > best.1 {
            best = (bpm, score);
        }
        bpm += 0.1;
    }
    let lag = fps * 60.0 / best.0;
    let conf = (at(lag) / ac[0] * 2.0).clamp(0.0, 1.0);
    let mut bpm = best.0;

    // A cleanly cut loop is a whole number of bars; if so its length gives the exact tempo.
    let secs = frames as f32 / sr;
    let beats = secs * bpm / 60.0;
    let whole = (beats / 4.0).round() * 4.0;
    if whole >= 4.0 && (beats - whole).abs() / whole < 0.025 {
        bpm = whole * 60.0 / secs;
    }
    (bpm, conf)
}

const KS_MAJOR: [f32; 12] =
    [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const KS_MINOR: [f32; 12] =
    [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

/// 12-bin pitch class profile. FFT bins are folded onto semitones with a window that is
/// constant in log-frequency, so every pitch gets the same relative bandwidth.
pub fn pitch_class_profile(mono: &[f32], sr: f32) -> [f32; 12] {
    const N: usize = 8192;
    let mut pcp = [0f32; 12];
    if mono.len() < N {
        return pcp;
    }
    let fft = FftPlanner::<f32>::new().plan_fft_forward(N);
    let win = hann(N);
    let mut buf = vec![Complex::new(0f32, 0f32); N];
    let lo = (65.0 / sr * N as f32) as usize;
    let hi = ((2100.0 / sr * N as f32) as usize).min(N / 2);
    // Per-bin pitch class and weight, computed once.
    let map: Vec<(usize, f32)> = (lo..hi)
        .map(|k| {
            let midi = hz_to_midi(k as f32 * sr / N as f32);
            let nearest = midi.round();
            let dev = midi - nearest;
            ((nearest as i32).rem_euclid(12) as usize, (-0.5 * (dev / 0.22).powi(2)).exp())
        })
        .collect();
    let mut at = 0;
    while at + N <= mono.len() {
        for i in 0..N {
            buf[i] = Complex::new(mono[at + i] * win[i], 0.0);
        }
        fft.process(&mut buf);
        for (j, k) in (lo..hi).enumerate() {
            let (pc, w) = map[j];
            pcp[pc] += buf[k].norm().sqrt() * w;
        }
        at += N / 2;
    }
    let sum: f32 = pcp.iter().sum();
    if sum > 0.0 {
        for v in pcp.iter_mut() {
            *v /= sum;
        }
    }
    pcp
}

fn pearson(a: &[f32; 12], b: &[f32; 12], rot: usize) -> f32 {
    let ma = a.iter().sum::<f32>() / 12.0;
    let mb = b.iter().sum::<f32>() / 12.0;
    let (mut num, mut da, mut db) = (0f32, 0f32, 0f32);
    for i in 0..12 {
        let x = a[(i + rot) % 12] - ma;
        let y = b[i] - mb;
        num += x * y;
        da += x * x;
        db += y * y;
    }
    if da <= 0.0 || db <= 0.0 { 0.0 } else { num / (da * db).sqrt() }
}

/// Krumhansl-Schmuckler: returns (root pitch class, is_minor, confidence 0..1).
pub fn detect_key(pcp: &[f32; 12]) -> (u8, bool, f32) {
    let mut scores: Vec<(f32, u8, bool)> = Vec::with_capacity(24);
    for root in 0..12 {
        scores.push((pearson(pcp, &KS_MAJOR, root), root as u8, false));
        scores.push((pearson(pcp, &KS_MINOR, root), root as u8, true));
    }
    scores.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (best, root, minor) = scores[0];
    let margin = ((best - scores[1].0) / 0.2).clamp(0.0, 1.0);
    (root, minor, (0.5 * best.max(0.0) + 0.5 * margin).clamp(0.0, 1.0))
}

/// Lowest and highest fundamentals looked for, in Hz: a low E on a five-string bass to
/// the top of a soprano and most leads.
const PITCH_LO: f32 = 30.0;
const PITCH_HI: f32 = 2000.0;

/// YIN's cumulative mean normalised difference for one frame, for every lag up to
/// `tau_max`. The difference function is computed exactly, through an FFT
/// autocorrelation, rather than on every other sample.
fn cmnd(x: &[f32], tau_max: usize, fft: &dyn rustfft::Fft<f32>, ifft: &dyn rustfft::Fft<f32>) -> Vec<f32> {
    let w = x.len() - tau_max;
    let size = fft.len();
    // r(tau) = sum over j < w of x[j] * x[j + tau], from the cross-spectrum of the first
    // w samples and the whole frame.
    let mut a: Vec<Complex<f32>> = (0..size).map(|i| Complex::new(if i < w { x[i] } else { 0.0 }, 0.0)).collect();
    let mut b: Vec<Complex<f32>> = (0..size).map(|i| Complex::new(*x.get(i).unwrap_or(&0.0), 0.0)).collect();
    fft.process(&mut a);
    fft.process(&mut b);
    for (p, q) in a.iter_mut().zip(&b) {
        *p = p.conj() * q;
    }
    ifft.process(&mut a);
    let r: Vec<f32> = a.iter().map(|c| c.re / size as f32).collect();
    // Energy of the window at each lag, by a running sum of squares.
    let sq: Vec<f64> = std::iter::once(0.0)
        .chain(x.iter().scan(0.0f64, |acc, v| {
            *acc += (*v as f64) * (*v as f64);
            Some(*acc)
        }))
        .collect();
    let e0 = sq[w];
    let mut out = vec![1f32; tau_max];
    let mut run = 0.0f64;
    for tau in 1..tau_max {
        let et = sq[tau + w] - sq[tau];
        let d = (e0 + et - 2.0 * r[tau] as f64).max(0.0);
        run += d;
        out[tau] = if run > 0.0 { (d * tau as f64 / run) as f32 } else { 1.0 };
    }
    out
}

/// Refines a dip of `c` at `tau` to a fractional lag.
fn refine(c: &[f32], tau: usize) -> f32 {
    if tau == 0 || tau + 1 >= c.len() {
        return tau as f32;
    }
    let (a, b, d) = (c[tau - 1], c[tau], c[tau + 1]);
    let denom = a - 2.0 * b + d;
    tau as f32 + if denom.abs() > 1e-9 { (0.5 * (a - d) / denom).clamp(-1.0, 1.0) } else { 0.0 }
}

/// The bottom of the dip of `c` nearest `tau`, searching a few lags either side.
fn dip_near(c: &[f32], tau: usize) -> usize {
    let reach = (tau / 20).max(2);
    let lo = tau.saturating_sub(reach).max(1);
    let hi = (tau + reach).min(c.len() - 1);
    (lo..=hi).min_by(|a, b| c[*a].total_cmp(&c[*b])).unwrap_or(tau)
}

/// One frame's pitch: (hz, aperiodicity 0..1, low is clearly pitched).
///
/// YIN's rule (the first dip below a threshold) with two guards against octave errors.
/// When nothing clears the threshold (a noisy or breathy note), the first dip nearly as
/// deep as the deepest is taken, since noise makes the dips at every multiple of the
/// period about equal. And a dip found first is checked against
/// the one at twice its lag: a missing fundamental makes the octave above look periodic
/// too, but its true period is deeper still.
fn yin_frame(x: &[f32], sr: f32, fft: &dyn rustfft::Fft<f32>, ifft: &dyn rustfft::Fft<f32>) -> Option<(f32, f32)> {
    let tau_max = ((sr / PITCH_LO) as usize).min(x.len() / 2);
    let tau_min = ((sr / PITCH_HI) as usize).max(2);
    if tau_max <= tau_min + 2 || x.len() - tau_max < 64 {
        return None;
    }
    let c = cmnd(x, tau_max, fft, ifft);
    let local_min = |t: usize| t > tau_min && t + 1 < tau_max && c[t] <= c[t - 1] && c[t] <= c[t + 1];
    const THRESHOLD: f32 = 0.12;
    let mut tau = None;
    let mut t = tau_min;
    while t + 1 < tau_max {
        if c[t] < THRESHOLD {
            while t + 1 < tau_max && c[t + 1] < c[t] {
                t += 1;
            }
            tau = Some(t);
            break;
        }
        t += 1;
    }
    let tau = match tau {
        Some(t) => {
            // The octave below: the true period if its dip is clearly the deeper one.
            let double = 2 * t;
            if double + 1 < tau_max {
                let d = dip_near(&c, double);
                if local_min(d) && c[t] > 0.04 && c[d] < c[t] * 0.5 { d } else { t }
            } else {
                t
            }
        }
        None => {
            let deepest = (tau_min..tau_max - 1).filter(|t| local_min(*t)).min_by(|a, b| c[*a].total_cmp(&c[*b]))?;
            if c[deepest] > 0.45 {
                return None;
            }
            // Noise makes the dips at every multiple of the period about equally deep, so
            // take the shortest period whose dip is nearly as deep as the deepest.
            let near = c[deepest] * 1.25 + 0.04;
            (tau_min..tau_max - 1).find(|t| local_min(*t) && c[*t] <= near).unwrap_or(deepest)
        }
    };
    Some((sr / refine(&c, tau), c[tau]))
}

/// The pitch of a monophonic source, as a fractional MIDI note: the note it sits on for
/// longest, weighted by how loud and how clearly pitched each moment is. `None` for
/// chords, drums and noise, which have no single pitch.
pub fn detect_pitch(mono: &[f32], sr: f32) -> Option<f32> {
    // Two of the longest periods looked for, and then some.
    let n = ((2.5 * sr / PITCH_LO) as usize).next_power_of_two().max(2048);
    if mono.len() < n {
        return None;
    }
    let peak = mono.iter().fold(0f32, |m, v| m.max(v.abs()));
    if peak < 1e-4 {
        return None;
    }
    let mut planner = FftPlanner::<f32>::new();
    let size = (2 * n).next_power_of_two();
    let (fft, ifft) = (planner.plan_fft_forward(size), planner.plan_fft_inverse(size));
    // Up to 120 frames spread over the whole sample, overlapping where it is short.
    let hop = ((mono.len() - n) / 120).max(n / 4);
    let mut frames = Vec::new();
    let mut sounding = 0.0f32;
    let mut at = 0;
    while at + n <= mono.len() {
        let frame = &mono[at..at + n];
        let rms = (frame.iter().map(|v| v * v).sum::<f32>() / n as f32).sqrt();
        if rms > peak * 0.03 {
            sounding += rms;
            // Breath or noise under a note raises its aperiodicity without hiding it.
            if let Some((hz, ap)) = yin_frame(frame, sr, fft.as_ref(), ifft.as_ref())
                && ap < 0.35
            {
                frames.push((hz_to_midi(hz), rms * (1.0 - ap)));
            }
        }
        at += hop;
    }
    let voiced: f32 = frames.iter().map(|f| f.1).sum();
    if frames.is_empty() || voiced < sounding * 0.35 {
        return None;
    }
    // A histogram in tenths of a semitone, smoothed over a semitone, finds the note it
    // dwells on; vibrato and slides around it don't move it.
    const BINS: usize = 1280;
    let mut hist = vec![0f32; BINS];
    for &(note, w) in &frames {
        let b = (note * 10.0).round();
        if (0.0..BINS as f32).contains(&b) {
            hist[b as usize] += w;
        }
    }
    let smooth: Vec<f32> = (0..BINS)
        .map(|i| (i.saturating_sub(5)..(i + 6).min(BINS)).map(|j| hist[j] * (6.0 - (j as f32 - i as f32).abs())).sum())
        .collect();
    let top = (0..BINS).max_by(|a, b| smooth[*a].total_cmp(&smooth[*b]))? as f32 / 10.0;
    // The weighted mean of the moments within half a semitone of it.
    let (sum, weight) = frames.iter().filter(|f| (f.0 - top).abs() <= 0.5).fold((0.0, 0.0), |(s, w), f| (s + f.0 * f.1, w + f.1));
    // A note it barely visits is no answer: that is a melody or a chord, not a pitch.
    if weight < voiced * 0.25 {
        return None;
    }
    Some(sum / weight)
}

pub fn analyze(s: &SampleData, sr: f32) -> Analysis {
    let mono = s.mono();
    let envelope = onset_envelope(&mono, sr);
    let onsets = pick_onsets(&envelope, &mono, sr, 0.5);
    let secs = mono.len() as f32 / sr;
    let (bpm, bpm_conf) = if secs >= 1.5 && onsets.len() >= 3 {
        bpm_from_envelope(&envelope, mono.len(), sr)
    } else {
        (0.0, 0.0)
    };
    let pcp = pitch_class_profile(&mono, sr);
    let (key_root, key_minor, key_conf) = detect_key(&pcp);
    Analysis { bpm, bpm_conf, key_root, key_minor, key_conf, pcp, pitch: detect_pitch(&mono, sr), onsets }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::midi_to_hz;

    const SR: f32 = 48000.0;

    fn click_track(bpm: f32, beats: usize) -> Vec<f32> {
        let step = (SR * 60.0 / bpm) as usize;
        let mut v = vec![0f32; step * beats];
        for b in 0..beats * 2 {
            let at = b * step / 2;
            let amp = if b % 2 == 0 { 0.9 } else { 0.4 };
            for i in 0..2000.min(v.len() - at) {
                let t = i as f32 / SR;
                v[at + i] += amp * (-t * 60.0).exp() * (2.0 * PI * 180.0 * t).sin();
            }
        }
        v
    }

    #[test]
    fn finds_tempo() {
        let (bpm, conf) = detect_bpm(&click_track(134.0, 16), SR);
        assert!((bpm - 134.0).abs() < 1.0, "bpm {bpm}");
        assert!(conf > 0.2, "conf {conf}");
    }

    #[test]
    fn finds_onsets() {
        let on = detect_onsets(&click_track(120.0, 8), SR, 0.5);
        assert!((14..=17).contains(&on.len()), "{} onsets", on.len());
        let step = SR as u32 / 4;
        for (i, o) in on.iter().enumerate().skip(1) {
            let expect = i as u32 * step;
            assert!(o.abs_diff(expect) < 600, "onset {i} at {o}, expected {expect}");
        }
    }

    #[test]
    fn finds_key_and_pitch() {
        // E major triad arpeggio plus the scale: should read as E major.
        let notes = [64, 68, 71, 64, 66, 68, 69, 71, 73, 75, 76, 71, 68, 64];
        let mut v = Vec::new();
        for &n in &notes {
            let hz = midi_to_hz(n as f32);
            for i in 0..(SR * 0.3) as usize {
                let t = i as f32 / SR;
                v.push(0.5 * (-t * 3.0).exp() * ((2.0 * PI * hz * t).sin() + 0.3 * (4.0 * PI * hz * t).sin()));
            }
        }
        let (root, minor, _) = detect_key(&pitch_class_profile(&v, SR));
        assert_eq!((root, minor), (4, false));

        let a: Vec<f32> = (0..SR as usize).map(|i| (2.0 * PI * 220.0 * i as f32 / SR).sin() * 0.5).collect();
        let p = detect_pitch(&a, SR).unwrap();
        assert!((p - 57.0).abs() < 0.05, "pitch {p}");
    }

    /// A note built from harmonics, with `fund` the level of the fundamental itself.
    fn note(hz: f32, secs: f32, fund: f32, harmonics: usize) -> Vec<f32> {
        (0..(SR * secs) as usize)
            .map(|i| {
                let t = i as f32 / SR;
                let mut v = fund * (2.0 * PI * hz * t).sin();
                for h in 2..=harmonics {
                    v += (2.0 * PI * hz * h as f32 * t).sin() / h as f32;
                }
                v * 0.3
            })
            .collect()
    }

    #[test]
    fn pitch_survives_a_missing_fundamental_and_low_notes() {
        // A bass note whose fundamental is almost gone: the ear still hears 55 Hz.
        let p = detect_pitch(&note(55.0, 1.0, 0.05, 8), SR).unwrap();
        assert!((p - 33.0).abs() < 0.1, "A1 with a weak fundamental read as {p}");
        // A low E on a bass guitar.
        let p = detect_pitch(&note(41.2, 1.5, 1.0, 6), SR).unwrap();
        assert!((p - 28.0).abs() < 0.1, "E1 read as {p}");
        // A high lead note.
        let p = detect_pitch(&note(1046.5, 0.5, 1.0, 4), SR).unwrap();
        assert!((p - 84.0).abs() < 0.1, "C6 read as {p}");
    }

    #[test]
    fn pitch_holds_through_vibrato_and_a_little_noise() {
        let mut seed = 7u32;
        let v: Vec<f32> = (0..SR as usize * 2)
            .map(|i| {
                let t = i as f32 / SR;
                // A 5.5 Hz vibrato of a third of a semitone around G3.
                let hz = 196.0 * 2f32.powf(0.33 / 12.0 * (2.0 * PI * 5.5 * t).sin());
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let noise = (seed as f32 / u32::MAX as f32 - 0.5) * 0.05;
                (2.0 * PI * 196.0 * t + (hz - 196.0) / 5.5 * -(2.0 * PI * 5.5 * t).cos()).sin() * 0.4 + noise
            })
            .collect();
        let p = detect_pitch(&v, SR).unwrap();
        assert!((p - 55.0).abs() < 0.15, "G3 with vibrato read as {p}");
    }

    #[test]
    fn a_melody_reads_as_the_note_it_dwells_on() {
        let mut v = Vec::new();
        for (midi, secs) in [(60.0, 0.6), (62.0, 0.2), (64.0, 0.2), (60.0, 0.6), (67.0, 0.2)] {
            v.extend(note(crate::music::midi_to_hz(midi), secs, 1.0, 5));
        }
        let p = detect_pitch(&v, SR).unwrap();
        assert!((p - 60.0).abs() < 0.15, "melody read as {p}");
    }

    #[test]
    fn a_breathy_note_still_has_a_pitch() {
        let mut seed = 11u32;
        let v: Vec<f32> = note(440.0, 1.0, 1.0, 3)
            .into_iter()
            .map(|x| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                x + (seed as f32 / u32::MAX as f32 - 0.5) * 0.4
            })
            .collect();
        let p = detect_pitch(&v, SR).expect("a note under breath noise");
        assert!((p - 69.0).abs() < 0.1, "A4 read as {p}");
    }

    #[test]
    fn chords_and_noise_have_no_pitch() {
        let chord: Vec<f32> = note(261.6, 1.0, 1.0, 4).iter().zip(note(329.6, 1.0, 1.0, 4)).zip(note(392.0, 1.0, 1.0, 4)).map(|((a, b), c)| a + b + c).collect();
        // A major triad's notes share a common period (a C two octaves down), so a reading
        // there is not wrong as such; what must not happen is calling it one of its notes.
        if let Some(p) = detect_pitch(&chord, SR) {
            assert!((p - 60.0).abs() > 1.0 && (p - 64.0).abs() > 1.0 && (p - 67.0).abs() > 1.0, "chord read as {p}");
        }
        let mut seed = 3u32;
        let noise: Vec<f32> = (0..SR as usize)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as f32 / u32::MAX as f32 - 0.5
            })
            .collect();
        assert_eq!(detect_pitch(&noise, SR), None);
    }
}
