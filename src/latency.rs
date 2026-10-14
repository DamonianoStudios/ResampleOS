//! Measuring how late recorded input arrives.
//!
//! The engine plays a few short chirps on the output (see [`ping`]) and notes where in the
//! input ring the first one went out. With the output reaching the input (a loopback
//! cable, or a microphone near a speaker), the chirps come back that much later than
//! they left: the round trip through the interface, which is exactly how far vocal takes
//! have to be moved earlier to line up with what was playing. [`measure`] finds them.

/// When each chirp starts, in seconds. The gaps all differ, so the train lines up with
/// itself only at the true delay: shifted by any gap, at most one chirp matches.
const AT_SECS: [f32; 4] = [0.0, 0.29, 0.66, 0.89];
const CHIRP_SECS: f32 = 0.006;
/// Longest round trip looked for.
pub const MAX_SECS: f32 = 0.5;
/// How long after the first chirp to wait before looking for them all.
pub const LISTEN_SECS: f32 = 0.89 + CHIRP_SECS + MAX_SECS + 0.2;

/// The test signal: chirps rising from 500 Hz to 8 kHz, each shaped by a Hann window so
/// it has no edges, at the times in [`AT_SECS`].
pub fn ping(sr: f32) -> Vec<f32> {
    let chirp = chirp(sr);
    let starts = starts(sr);
    let mut out = vec![0.0; starts[starts.len() - 1] + chirp.len()];
    for at in starts {
        out[at..at + chirp.len()].copy_from_slice(&chirp);
    }
    out
}

fn starts(sr: f32) -> [usize; 4] {
    AT_SECS.map(|t| (t * sr) as usize)
}

fn chirp(sr: f32) -> Vec<f32> {
    let n = (CHIRP_SECS * sr) as usize;
    let (f0, f1) = (500.0f32, 8000.0f32.min(sr * 0.4));
    let dur = n as f32 / sr;
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            let phase = std::f32::consts::TAU * (f0 * t + (f1 - f0) * t * t / (2.0 * dur));
            let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (n - 1) as f32).cos();
            phase.sin() * w * 0.5
        })
        .collect()
}

/// What a measurement found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Found {
    /// Round trip, in samples.
    pub samples: usize,
    /// How far the best match stands above the rest, 0..1. Below about 0.5 the result is
    /// not to be trusted.
    pub confidence: f32,
}

/// Looks for the chirps in `input`, recorded from the moment the first one went out.
/// Every lag up to [`MAX_SECS`] is scored by how well each chirp matches there, averaged,
/// so only a lag where all of them match scores near 1.
pub fn measure(input: &[f32], sr: f32) -> Option<Found> {
    let chirp = chirp(sr);
    let starts = starts(sr);
    let max_lag = (MAX_SECS * sr) as usize;
    let energy: f32 = chirp.iter().map(|v| v * v).sum();
    let mut scores = Vec::with_capacity(max_lag);
    for lag in 0..max_lag {
        let mut total = 0.0f32;
        for at in starts {
            let seg = input.get(lag + at..lag + at + chirp.len())?;
            let dot: f32 = seg.iter().zip(&chirp).map(|(a, b)| a * b).sum();
            let norm: f32 = seg.iter().map(|v| v * v).sum();
            // Normalised: 1 where the input holds the chirp exactly, at any level.
            if norm > 1e-9 {
                total += dot / (norm * energy).sqrt();
            }
        }
        scores.push(total / starts.len() as f32);
    }
    let (best, &peak) = scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    if peak < 0.2 {
        return None;
    }
    // The next best score away from the peak itself (and its own ringing).
    let guard = chirp.len();
    let rival = scores
        .iter()
        .enumerate()
        .filter(|(i, _)| i.abs_diff(best) > guard)
        .map(|(_, v)| *v)
        .fold(0.0f32, f32::max);
    Some(Found { samples: best, confidence: ((peak - rival.max(0.0)) / peak).clamp(0.0, 1.0) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_round_trip_through_noise_and_level_change() {
        let sr = 48000.0;
        let sig = ping(sr);
        for delay in [0usize, 137, 4410, 11000] {
            let len = (LISTEN_SECS * sr) as usize;
            let mut input = vec![0.0f32; len];
            let mut seed = 12345u32 + delay as u32;
            for (i, v) in input.iter_mut().enumerate() {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let noise = (seed as f32 / u32::MAX as f32 - 0.5) * 0.05;
                let s = i.checked_sub(delay).and_then(|j| sig.get(j)).copied().unwrap_or(0.0);
                // Quieter on the way back, as through a speaker and a microphone.
                *v = s * 0.2 + noise;
            }
            let found = measure(&input, sr).expect("found");
            assert_eq!(found.samples, delay, "{found:?}");
            assert!(found.confidence > 0.5, "{found:?}");
        }
    }

    #[test]
    fn silence_and_noise_find_nothing() {
        let sr = 48000.0;
        let len = (LISTEN_SECS * sr) as usize;
        assert_eq!(measure(&vec![0.0; len], sr), None);
        let mut seed = 99u32;
        let noise: Vec<f32> = (0..len)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as f32 / u32::MAX as f32 - 0.5
            })
            .collect();
        assert!(measure(&noise, sr).is_none_or(|f| f.confidence < 0.5));
    }
}
