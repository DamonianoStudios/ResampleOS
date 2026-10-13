//! Real-time pitch correction for one voice.
//!
//! The pitch is tracked with a YIN-style detector on a decimated copy of the input. The
//! output is rebuilt from two-period grains of the input laid down at the corrected period
//! (pitch-synchronous overlap-add). Moving whole grains changes the pitch and leaves each
//! grain's spectrum, and so the vowel, as it was; reading a grain faster or slower moves
//! the vowel without the pitch. Anything unvoiced passes through, delayed like the rest.

const RING: usize = 8192;
const MASK: usize = RING - 1;
/// Pitch is looked for between these, in Hz.
const LOWEST: f32 = 75.0;
const HIGHEST: f32 = 800.0;
/// The detector runs on every fourth sample.
const DECIMATE: usize = 4;
const DETECT_EVERY: usize = 256;
/// Decimated samples compared at each lag.
const COMPARE: usize = 352;
/// Grain spacing used while no pitch is found.
const IDLE_PERIOD: f32 = 256.0;

pub struct Tuner {
    sr: f32,
    /// Longest period handled, in samples; also half the widest grain.
    max_period: usize,
    input: Vec<f32>,
    output: Vec<f32>,
    /// Samples taken in so far.
    written: usize,
    until_detect: usize,
    scratch: Vec<f32>,
    diff: Vec<f32>,

    voiced: bool,
    period: f32,
    /// Detected pitch as a (fractional) MIDI note, and the scale note it is being pulled to.
    note: f32,
    target: f32,
    /// Correction being applied, in semitones.
    correction: f32,

    /// Where the next grain goes in the output, and where the last one came from.
    next_out: f64,
    last_in: f64,
}

impl Tuner {
    pub fn new(sr: f32) -> Self {
        let max_period = (sr / LOWEST) as usize;
        Tuner {
            sr,
            max_period,
            input: vec![0.0; RING],
            output: vec![0.0; RING],
            written: 0,
            until_detect: DETECT_EVERY,
            scratch: vec![0.0; COMPARE + max_period / DECIMATE + 2],
            diff: vec![0.0; max_period / DECIMATE + 2],
            voiced: false,
            period: IDLE_PERIOD,
            note: 60.0,
            target: 60.0,
            correction: 0.0,
            next_out: 0.0,
            last_in: 0.0,
        }
    }

    /// Samples between a sound going in and coming out.
    pub fn latency(&self) -> usize {
        self.max_period * 2 + 8
    }

    pub fn reset(&mut self) {
        self.input.fill(0.0);
        self.output.fill(0.0);
        self.voiced = false;
        self.correction = 0.0;
        self.next_out = self.written as f64;
        self.last_in = self.written as f64;
    }

    /// Corrects one block in place (the two channels are summed: this is for a single
    /// voice). `retune` is 0 for slow and natural to 1 for instant; `formant` and
    /// `transpose` are in semitones; `scale` has a bit set for each pitch class allowed.
    pub fn process(&mut self, retune: f32, formant: f32, transpose: i32, scale: u16, l: &mut [f32], r: &mut [f32]) {
        let latency = self.latency();
        let grain_rate = 2f32.powf(formant / 12.0);
        for i in 0..l.len() {
            self.input[self.written & MASK] = (l[i] + r[i]) * 0.5;
            self.written += 1;
            self.until_detect -= 1;
            if self.until_detect == 0 {
                self.until_detect = DETECT_EVERY;
                self.detect();
                self.retarget(retune, transpose, scale);
            }
            // The sample about to be read is `latency` behind the input; every grain that
            // reaches it must be down before it goes out.
            let read = self.written as i64 - 1 - latency as i64;
            while self.next_out <= (read + self.max_period as i64) as f64 {
                self.grain(grain_rate);
            }
            let slot = read.rem_euclid(RING as i64) as usize;
            let y = std::mem::take(&mut self.output[slot]);
            l[i] = y;
            r[i] = y;
        }
    }

    /// Finds the period of the last stretch of input, if it has one.
    fn detect(&mut self) {
        let lags = (self.sr / HIGHEST) as usize / DECIMATE..=self.max_period / DECIMATE;
        let need = COMPARE + lags.end() + 1;
        let start = self.written as i64 - (need * DECIMATE) as i64;
        let mut energy = 0.0;
        for (j, slot) in self.scratch[..need].iter_mut().enumerate() {
            let at = start + (j * DECIMATE) as i64;
            // A short average stands in for a proper low-pass before decimating.
            let sum: f32 = (0..DECIMATE as i64).map(|k| self.input[(at + k).rem_euclid(RING as i64) as usize]).sum();
            *slot = sum / DECIMATE as f32;
            energy += *slot * *slot;
        }
        if energy / (need as f32) < 1e-7 {
            self.voiced = false;
            return;
        }
        // Cumulative-mean-normalised difference, as in YIN: low where the signal repeats.
        let mut running = 0.0;
        let mut best = (0usize, f32::MAX);
        let mut found = None;
        for lag in 1..=*lags.end() {
            let d: f32 = (0..COMPARE).map(|j| (self.scratch[j] - self.scratch[j + lag]).powi(2)).sum();
            running += d;
            let norm = if running > 0.0 { d * lag as f32 / running } else { 1.0 };
            self.diff[lag] = norm;
            if lag < *lags.start() {
                continue;
            }
            if found.is_none() && norm < 0.15 {
                found = Some(lag);
            }
            if let Some(first) = found {
                // Walk on to the bottom of the dip that crossed the threshold.
                if norm < self.diff[first] {
                    found = Some(lag);
                } else if lag > first + 2 {
                    break;
                }
            }
            if norm < best.1 {
                best = (lag, norm);
            }
        }
        let lag = match found {
            Some(lag) => lag,
            None if best.1 < 0.3 => best.0,
            None => {
                self.voiced = false;
                return;
            }
        };
        // Parabolic fit through the neighbours for a finer estimate than one lag.
        let (a, b, c) = (self.diff[lag.max(2) - 1], self.diff[lag], self.diff[(lag + 1).min(*lags.end())]);
        let bend = a - 2.0 * b + c;
        let fine = if bend.abs() > 1e-9 { (0.5 * (a - c) / bend).clamp(-0.5, 0.5) } else { 0.0 };
        self.period = ((lag as f32 + fine) * DECIMATE as f32).clamp(self.sr / HIGHEST, self.max_period as f32);
        self.note = 69.0 + 12.0 * (self.sr / self.period / 440.0).log2();
        self.voiced = true;
    }

    /// Moves the correction toward the nearest allowed note.
    fn retarget(&mut self, retune: f32, transpose: i32, scale: u16) {
        if !self.voiced {
            return;
        }
        // Stay with the current note until the voice is clearly nearer another, so a
        // pitch sitting between two notes does not flip back and forth.
        if (self.note - self.target).abs() > 0.65 || !allowed(self.target.round() as i32, scale) {
            self.target = nearest_allowed(self.note, scale);
        }
        let wanted = self.target + transpose as f32 - self.note;
        let seconds = 0.25 * (1.0 - retune.clamp(0.0, 1.0)).powi(2);
        let step = DETECT_EVERY as f32 / self.sr;
        let k = if seconds < 1e-4 { 1.0 } else { 1.0 - (-step / seconds).exp() };
        self.correction += (wanted - self.correction) * k;
    }

    /// Lays down one grain and schedules the next.
    fn grain(&mut self, rate: f32) {
        let (period_in, ratio) = if self.voiced { (self.period, 2f32.powf(self.correction / 12.0)) } else { (IDLE_PERIOD, 1.0) };
        let period_out = (period_in / ratio).clamp(16.0, self.max_period as f32);
        // Take the grain from a whole number of periods after the last one, as near as
        // possible to where it is going, so successive grains stay in phase.
        let steps = ((self.next_out - self.last_in) / period_in as f64).round().max(0.0);
        let mut from = self.last_in + steps * period_in as f64;
        let newest = self.written as f64 - 2.0;
        let reach = period_in as f64;
        while from + reach > newest {
            from -= period_in as f64;
        }
        let oldest = self.written as f64 - RING as f64 + 2.0;
        if from - reach < oldest {
            from = self.next_out.min(newest - reach);
        }
        let half = (period_in / rate).min(self.max_period as f32);
        // Hann windows a fixed distance apart add up to a constant only when that distance
        // is half their width; otherwise scale to keep the level.
        let gain = period_out / half;
        let centre = self.next_out.round();
        let frac = (self.next_out - centre) as f32;
        let reach_out = half.floor() as i64;
        for j in -reach_out..=reach_out {
            let x = j as f32 - frac;
            if x.abs() >= half {
                continue;
            }
            let window = 0.5 * (1.0 + (std::f32::consts::PI * x / half).cos());
            let at = from + (x * rate) as f64;
            let i = at.floor();
            let t = (at - i) as f32;
            let a = self.input[(i as i64).rem_euclid(RING as i64) as usize];
            let b = self.input[(i as i64 + 1).rem_euclid(RING as i64) as usize];
            let slot = (centre as i64 + j).rem_euclid(RING as i64) as usize;
            self.output[slot] += (a + (b - a) * t) * window * gain;
        }
        self.last_in = from;
        self.next_out += period_out as f64;
    }
}

fn allowed(note: i32, scale: u16) -> bool {
    let scale = if scale & 0xfff == 0 { 0xfff } else { scale };
    scale & (1 << note.rem_euclid(12)) != 0
}

/// The allowed note closest to `note`.
fn nearest_allowed(note: f32, scale: u16) -> f32 {
    let base = note.round() as i32;
    (-6..=6)
        .map(|d| base + d)
        .filter(|n| allowed(*n, scale))
        .min_by(|a, b| (*a as f32 - note).abs().total_cmp(&(*b as f32 - note).abs()))
        .unwrap_or(base) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;
    /// A minor: A B C D E F G.
    const A_MINOR: u16 = 0b1010_1011_0101;

    /// A buzzy tone with a strong fundamental, like a sung vowel.
    fn voice(hz: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| {
                let t = i as f32 / SR;
                (1..=8).map(|h| (t * hz * h as f32 * std::f32::consts::TAU).sin() / h as f32).sum::<f32>() * 0.3
            })
            .collect()
    }

    fn run(tuner: &mut Tuner, input: &[f32], retune: f32, formant: f32, transpose: i32, scale: u16) -> Vec<f32> {
        let mut out = Vec::new();
        for block in input.chunks(256) {
            let (mut l, mut r) = (block.to_vec(), block.to_vec());
            tuner.process(retune, formant, transpose, scale, &mut l, &mut r);
            out.extend(l);
        }
        out
    }

    /// Fundamental of a steady signal, from the lag at which it best repeats.
    fn pitch_of(x: &[f32]) -> f32 {
        let n = 8192;
        let x = &x[x.len() - n - 2000..];
        let score = |lag: usize| (0..n).map(|i| x[i] * x[i + lag]).sum::<f32>();
        let lag = (60..700).max_by(|a, b| score(*a).total_cmp(&score(*b))).unwrap();
        let (a, b, c) = (score(lag - 1), score(lag), score(lag + 1));
        SR / (lag as f32 + 0.5 * (a - c) / (a - 2.0 * b + c))
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn pulls_a_flat_note_onto_the_scale() {
        // 226 Hz is 47 cents above A3 (220 Hz); in A minor, A is the nearest note.
        let input = voice(226.0, 48000);
        let out = run(&mut Tuner::new(SR), &input, 1.0, 0.0, 0, A_MINOR);
        let hz = pitch_of(&out);
        assert!((hz - 220.0).abs() < 1.0, "came out at {hz} Hz");
        let level = rms(&out[24000..]) / rms(&input[24000..]);
        assert!((0.8..1.25).contains(&level), "level changed by {level}");
    }

    #[test]
    fn leaves_an_in_tune_note_alone_and_transposes_on_request() {
        let input = voice(220.0, 48000);
        let same = pitch_of(&run(&mut Tuner::new(SR), &input, 1.0, 0.0, 0, A_MINOR));
        assert!((same - 220.0).abs() < 0.6, "{same}");
        let up = pitch_of(&run(&mut Tuner::new(SR), &input, 1.0, 0.0, 12, A_MINOR));
        assert!((up - 440.0).abs() < 3.0, "{up}");
        let down = pitch_of(&run(&mut Tuner::new(SR), &input, 1.0, 0.0, -12, A_MINOR));
        assert!((down - 110.0).abs() < 1.0, "{down}");
    }

    #[test]
    fn formant_shift_keeps_the_pitch() {
        let input = voice(220.0, 48000);
        let hz = pitch_of(&run(&mut Tuner::new(SR), &input, 1.0, 5.0, 0, A_MINOR));
        assert!((hz - 220.0).abs() < 1.0, "{hz}");
    }

    #[test]
    fn unpitched_sound_passes_through_delayed() {
        // Noise from a fixed sequence: nothing to track, so the output is the input, late.
        let mut seed = 0x1234_5678u32;
        let input: Vec<f32> = (0..24000)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed >> 8) as f32 / (1 << 23) as f32 - 1.0
            })
            .collect();
        let mut tuner = Tuner::new(SR);
        let late = tuner.latency();
        let out = run(&mut tuner, &input, 1.0, 0.0, 0, A_MINOR);
        let worst = (4000..20000).map(|i| (out[i + late] - input[i]).abs()).fold(0f32, f32::max);
        assert!(worst < 1e-3, "differs by {worst}");
    }

    #[test]
    fn snaps_to_the_nearest_note_in_the_scale() {
        // G#3 (56) is not in A minor; a pitch just under it goes to G, just over to A.
        assert_eq!(nearest_allowed(55.9, A_MINOR), 55.0);
        assert_eq!(nearest_allowed(56.2, A_MINOR), 57.0);
        // No scale at all means every semitone.
        assert_eq!(nearest_allowed(56.2, 0), 56.0);
    }
}
