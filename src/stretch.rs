//! Transient-locked elastic stretch. Transient heads are copied through untouched and
//! only the decay between them is stretched, by waveform-similarity overlap-add.

use crate::sample::{SampleData, resample, resample_channel};

const GRAIN: usize = 2048;
const TOLERANCE: usize = 512;

/// Stretches `(l, r)` to exactly `out_len` frames. The first and last grains are pinned
/// to the input's own start and end, so the result splices seamlessly to its neighbours.
fn wsola(l: &[f32], r: &[f32], out_len: usize) -> (Vec<f32>, Vec<f32>) {
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
    let grain = GRAIN.min(n / 2).min(out_len / 2) & !1;
    let hop = grain / 2;
    let mono: Vec<f32> = l.iter().zip(r).map(|(a, b)| a + b).collect();

    let mut ol = vec![0f32; out_len];
    let mut or = vec![0f32; out_len];
    let mut norm = vec![0f32; out_len];
    let count = (out_len - grain).div_ceil(hop) + 1;
    let scale = (n - grain) as f64 / (out_len - grain).max(1) as f64;
    let mut prev = 0usize;

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
                let lo = ideal.saturating_sub(TOLERANCE);
                let hi = (ideal + TOLERANCE).min(n - grain);
                let mut best = ideal.min(n - grain);
                let mut best_score = f32::MIN;
                let mut cand = lo;
                while cand <= hi {
                    let (mut dot, mut energy) = (0f32, 1e-9f32);
                    let mut j = 0;
                    while j < hop {
                        let c = mono[cand + j];
                        dot += c * mono[natural + j];
                        energy += c * c;
                        j += 4;
                    }
                    let score = dot / energy.sqrt();
                    if score > best_score {
                        best_score = score;
                        best = cand;
                    }
                    cand += 2;
                }
                best
            }
        };
        for j in 0..grain {
            let phase = j as f32 / grain as f32;
            let mut w = 0.5 - 0.5 * (std::f32::consts::TAU * phase).cos();
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

/// Stretches to `ratio` times the original length (>1 = longer). With `onsets`, each
/// transient's first few milliseconds pass through at original speed.
pub fn stretch(s: &SampleData, onsets: &[u32], ratio: f64, sr: f32) -> SampleData {
    let n = s.len();
    if n == 0 || (ratio - 1.0).abs() < 1e-6 {
        return s.slice(0, n);
    }
    let head = (0.015 * sr) as usize;
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
        let (tl, tr) = wsola(&s.l[a + keep..b], &s.r[a + keep..b], out_len - keep);
        ol.extend_from_slice(&tl);
        or.extend_from_slice(&tr);
        // An empty tail can't supply its share; pad so the next transient stays on the grid.
        ol.resize(target_end, 0.0);
        or.resize(target_end, 0.0);
    }
    SampleData::new(ol, or)
}

/// Independent tempo and pitch change: `ratio` scales length, `semis` shifts pitch.
pub fn conform(s: &SampleData, onsets: &[u32], ratio: f64, semis: f32, sr: f32) -> SampleData {
    let pf = 2f64.powf(semis as f64 / 12.0);
    // Stretch so that the pitch-shifting resample lands on the wanted length.
    let stretched = stretch(s, onsets, ratio * pf, sr);
    if (pf - 1.0).abs() < 1e-6 { stretched } else { resample(&stretched, pf) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_and_transients_hold() {
        let sr = 48000.0;
        let n = 48000;
        let mut l = vec![0f32; n];
        let onsets = [0u32, 12000, 24000, 36000];
        for &o in &onsets {
            for i in 0..8000 {
                let t = i as f32 / sr;
                l[o as usize + i] = (-t * 25.0).exp() * (std::f32::consts::TAU * 200.0 * t).sin();
            }
            l[o as usize] = 1.0;
        }
        let s = SampleData::new(l.clone(), l);
        for ratio in [134.0 / 88.0, 0.7] {
            let out = stretch(&s, &onsets, ratio, sr);
            assert_eq!(out.len(), (n as f64 * ratio).round() as usize);
            for &o in &onsets[1..] {
                let at = (o as f64 * ratio).round() as usize;
                assert!((out.l[at] - 1.0).abs() < 1e-3, "transient moved at ratio {ratio}");
            }
            assert!(out.peak() < 1.5);
        }
        let shifted = conform(&s, &onsets, 1.0, -4.0, sr);
        assert!(shifted.len().abs_diff(n) < 4);
    }
}
