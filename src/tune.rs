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
/// Detections without a pitch (about 5 ms each) before a note is taken to have ended.
const MISSES_TO_LET_GO: u32 = 4;
/// Detections an octave (or more) away from the tracked pitch before the leap is believed.
const DOUBTS_TO_BELIEVE: u32 = 5;

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
    /// Detections in a row that found no pitch.
    misses: u32,
    /// Detections in a row that put the period at a multiple of what is being tracked.
    doubts: u32,
    /// The period first read from a sound that has not yet been confirmed as a voice.
    candidate: f32,
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
            misses: 0,
            doubts: 0,
            candidate: 0.0,
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
                let onset = self.track();
                self.retarget(retune, transpose, scale, onset);
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

    /// Looks for the period of the last stretch of input and updates what is being tracked.
    /// Returns true when a voice has just started after not being there.
    fn track(&mut self) -> bool {
        match self.detect() {
            Some(period) => {
                let onset = !self.voiced;
                self.misses = 0;
                // The first reading of a new sound is taken while it is still arriving
                // and is often wild. Wait for a second that agrees with it.
                if onset && !(period / std::mem::replace(&mut self.candidate, period)).log2().abs().lt(&0.08) {
                    return false;
                }
                // A sudden move to a whole multiple of the period is nearly always the
                // detector latching on to every second or third repeat for a moment, not
                // the singer dropping an exact octave or more inside a note. Believe it
                // only once it has lasted. (A move the other way is taken at once: tracking
                // a multiple is the mistake, and leaving it is the cure.)
                let ratio = period / self.period;
                if !onset && ratio > 1.8 && (ratio - ratio.round()).abs() < 0.08 * ratio {
                    self.doubts += 1;
                    if self.doubts < DOUBTS_TO_BELIEVE {
                        return false;
                    }
                }
                self.doubts = 0;
                self.voiced = true;
                self.period = period;
                self.note = 69.0 + 12.0 * (self.sr / period / 440.0).log2();
                onset
            }
            None => {
                // One doubtful frame in the middle of a note is not the end of the note:
                // letting go and catching on again is far more audible than holding on.
                self.misses += 1;
                self.candidate = 0.0;
                if self.misses >= MISSES_TO_LET_GO {
                    self.voiced = false;
                }
                false
            }
        }
    }

    /// The period of the last stretch of input in samples, if it has one.
    fn detect(&mut self) -> Option<f32> {
        let (lo, hi) = ((self.sr / HIGHEST) as usize / DECIMATE, self.max_period / DECIMATE);
        let need = COMPARE + hi + 1;
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
            return None;
        }
        // Cumulative-mean-normalised difference, as in YIN: low where the signal repeats.
        let mut running = 0.0;
        self.diff[0] = 1.0;
        for lag in 1..=hi {
            let d: f32 = (0..COMPARE).map(|j| (self.scratch[j] - self.scratch[j + lag]).powi(2)).sum();
            running += d;
            self.diff[lag] = if running > 0.0 { d * lag as f32 / running } else { 1.0 };
        }
        let diff = &self.diff;
        let lowest_in = |a: usize, b: usize| (a.max(lo)..=b.min(hi)).min_by(|x, y| diff[*x].total_cmp(&diff[*y]));
        let is_dip = |l: usize| diff[l] <= diff[l.max(2) - 1] && diff[l] <= diff[(l + 1).min(hi)];
        // How low a dip really goes. The detector sees every fourth sample, so a period
        // rarely lands on a whole lag; judged by the nearest lag alone, a high voice would
        // look less periodic at its true period than at twice it.
        let depth = |l: usize| {
            let (a, b, c) = (diff[l.max(2) - 1], diff[l], diff[(l + 1).min(hi)]);
            let bend = a - 2.0 * b + c;
            if is_dip(l) && bend > 1e-9 { (b - 0.125 * (a - c) * (a - c) / bend).max(0.0) } else { b }
        };

        // A voice moves smoothly, so look first near where it was. Searching the whole range
        // afresh every time is what lets the answer hop an octave for a frame or two, on a
        // breathy or creaky stretch, and each hop is heard as a yelp.
        let near = (self.voiced)
            .then(|| self.period / DECIMATE as f32)
            .and_then(|was| lowest_in((was * 0.89) as usize, (was * 1.12).ceil() as usize))
            .filter(|lag| depth(*lag) < 0.3);
        let mut lag = match near {
            Some(lag) => lag,
            None => {
                // The first clear dip. Failing that, the first dip about as good as the best
                // there is: every multiple of the period repeats as well as the period does,
                // and the deepest of them is as likely as not the wrong one.
                let dips = || (lo..=hi).filter(|l| is_dip(*l));
                match dips().find(|l| depth(*l) < 0.15) {
                    Some(l) => l,
                    None => {
                        let floor = dips().map(depth).fold(f32::MAX, f32::min);
                        if floor >= 0.3 {
                            return None;
                        }
                        dips().find(|l| depth(*l) <= floor + 0.05)?
                    }
                }
            }
        };
        // Staying near the last answer must not mean staying wrong. If half this lag
        // repeats cleanly too, the real period is the shorter one...
        while let Some(half) = lowest_in(lag / 2 - 1, lag / 2 + 1).filter(|h| *h + 1 < lag && depth(*h) < 0.12) {
            lag = half;
        }
        // ...and if twice the lag repeats far better, this one is only a strong harmonic.
        if let Some(double) = lowest_in(lag * 2 - 1, lag * 2 + 1).filter(|d| *d > lag + 1)
            && depth(lag) > 0.08
            && depth(double) < depth(lag) * 0.35
        {
            lag = double;
        }
        // Parabolic fit through the neighbours for a finer estimate than one lag.
        let (a, b, c) = (diff[lag.max(2) - 1], diff[lag], diff[(lag + 1).min(hi)]);
        let bend = a - 2.0 * b + c;
        let fine = if bend.abs() > 1e-9 { (0.5 * (a - c) / bend).clamp(-0.5, 0.5) } else { 0.0 };
        Some(((lag as f32 + fine) * DECIMATE as f32).clamp(self.sr / HIGHEST, self.max_period as f32))
    }

    /// Moves the correction toward the nearest allowed note.
    fn retarget(&mut self, retune: f32, transpose: i32, scale: u16, onset: bool) {
        if !self.voiced {
            return;
        }
        // Stay with the current note until the voice is clearly nearer another, so a
        // pitch sitting between two notes does not flip back and forth.
        if onset || (self.note - self.target).abs() > 0.65 || !allowed(self.target.round() as i32, scale) {
            self.target = nearest_allowed(self.note, scale);
        }
        let wanted = self.target + transpose as f32 - self.note;
        if onset {
            // A new phrase has nothing to glide from; starting it at the last phrase's
            // correction would bend its first few milliseconds.
            self.correction = wanted;
            return;
        }
        let seconds = 0.25 * (1.0 - retune.clamp(0.0, 1.0)).powi(2);
        let step = DETECT_EVERY as f32 / self.sr;
        let k = if seconds < 1e-4 { 1.0 } else { 1.0 - (-step / seconds).exp() };
        self.correction += (wanted - self.correction) * k;
    }

    /// The place near `guess` where the input looks most like it does around `reference`.
    ///
    /// Grains are meant to be whole periods apart, but the period is only an estimate and
    /// a voice is never exactly periodic. Laid down a few samples out of step, successive
    /// grains partly cancel, which is heard as a rough, metallic edge. Matching each grain
    /// to the one before keeps them in step whatever the estimate says.
    fn align(&self, guess: f64, reference: f64, period: f32) -> f64 {
        let reach = ((period * 0.12) as i64).clamp(2, 48);
        let width = (period as i64).min(480);
        let (g, r) = (guess.round() as i64, reference.round() as i64);
        let newest = self.written as i64 - 2;
        let oldest = self.written as i64 - RING as i64 + 2;
        if g + reach + width > newest || g.min(r) - reach - width < oldest {
            return guess;
        }
        let at = |i: i64| self.input[i.rem_euclid(RING as i64) as usize];
        let score = |shift: i64| {
            let (mut dot, mut power) = (0f32, 1e-9f32);
            let mut k = -width;
            while k <= width {
                let x = at(g + shift + k);
                dot += x * at(r + k);
                power += x * x;
                k += 2;
            }
            // All else equal, stay where the estimate pointed.
            dot / power.sqrt() * (1.0 - 0.03 * shift.abs() as f32 / reach as f32)
        };
        let mut best = (0i64, 0f32);
        for shift in -reach..=reach {
            let s = score(shift);
            if s > best.1 {
                best = (shift, s);
            }
        }
        if best.1 <= 0.0 {
            return guess;
        }
        // The best match usually lies between two samples; a whole-sample answer would
        // leave every grain up to half a sample out.
        let (a, b, c) = (score(best.0 - 1), best.1, score(best.0 + 1));
        let bend = a - 2.0 * b + c;
        let fine = if bend < -1e-9 { (0.5 * (a - c) / bend).clamp(-0.5, 0.5) } else { 0.0 };
        reference + (g + best.0 - r) as f64 + fine as f64
    }

    /// Lays down one grain and schedules the next.
    fn grain(&mut self, rate: f32) {
        let (period_in, ratio) = if self.voiced { (self.period, 2f32.powf(self.correction / 12.0)) } else { (IDLE_PERIOD, 1.0) };
        let period_out = (period_in / ratio).clamp(16.0, self.max_period as f32);
        let newest = self.written as f64 - 2.0;
        let reach = period_in as f64;
        let mut from = if self.voiced {
            // Take the grain from a whole number of periods after the last one, as near as
            // possible to where it is going, so successive grains stay in phase.
            let steps = ((self.next_out - self.last_in) / period_in as f64).round().max(0.0);
            let guess = self.last_in + steps * period_in as f64;
            if steps > 0.0 { self.align(guess, self.last_in, period_in) } else { guess }
        } else {
            // Nothing to keep in phase: the sound goes through exactly where it was.
            self.next_out
        };
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
    fn holds_one_octave_through_vibrato_and_a_weak_fundamental() {
        // A high note with vibrato whose second harmonic is far stronger than its first,
        // and a breathy patch in the middle: the kind of voice that used to send the
        // tracker hopping between octaves, each hop a yelp in the output.
        let mut seed = 0x2468_ace1u32;
        let mut phase = 0f32;
        let input: Vec<f32> = (0..96000)
            .map(|i| {
                let t = i as f32 / SR;
                let hz = 440.0 * 2f32.powf(0.4 / 12.0 * (t * 5.5 * std::f32::consts::TAU).sin());
                phase += hz / SR;
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let noise = (seed >> 8) as f32 / (1 << 23) as f32 - 1.0;
                let breath = if (0.9..1.0).contains(&t) { 0.12 } else { 0.01 };
                let tone: f32 = [0.15, 1.0, 0.3, 0.5, 0.1].iter().enumerate().map(|(h, a)| a * (phase * (h + 1) as f32 * std::f32::consts::TAU).sin()).sum();
                tone * 0.25 + noise * breath
            })
            .collect();
        let mut tuner = Tuner::new(SR);
        let mut periods = Vec::new();
        for block in input.chunks(256) {
            let (mut l, mut r) = (block.to_vec(), block.to_vec());
            tuner.process(0.6, 0.0, 0, 0xfff, &mut l, &mut r);
            if tuner.voiced {
                periods.push(tuner.period);
            }
        }
        assert!(periods.len() > 300, "lost the voice: {} voiced blocks", periods.len());
        let expected = SR / 440.0;
        let off: Vec<f32> = periods.iter().copied().filter(|p| (p / expected).log2().abs() > 0.1).collect();
        assert!(off.is_empty(), "left the octave {} times, e.g. {:?}", off.len(), &off[..off.len().min(5)]);
    }

    /// YIN on one frame: (period in samples, how far from periodic it is, 0 = perfectly).
    fn frame_pitch(x: &[f32]) -> (f32, f32) {
        let (w, lo, hi) = (1024, 60, 640);
        let mut running = 0.0;
        let mut best = (0usize, f32::MAX);
        let mut norms = vec![1.0f32; hi + 1];
        for lag in 1..=hi {
            let d: f32 = (0..w).map(|j| (x[j] - x[j + lag]).powi(2)).sum();
            running += d;
            norms[lag] = if running > 0.0 { d * lag as f32 / running } else { 1.0 };
        }
        for lag in lo..hi {
            if norms[lag] < 0.12 && norms[lag] <= norms[lag + 1] {
                return (lag as f32, norms[lag]);
            }
            if norms[lag] < best.1 {
                best = (lag, norms[lag]);
            }
        }
        (best.0 as f32, best.1)
    }

    /// Not a pass/fail test: runs a recording through the tuner and prints how clean the
    /// result is next to the original. `TUNE_WAV=path cargo test tune_report -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn tune_report() {
        let path = std::env::var("TUNE_WAV").expect("set TUNE_WAV");
        let data = crate::sample::load_file(std::path::Path::new(&path), SR as u32).unwrap();
        let input: Vec<f32> = data.l.iter().zip(&data.r).map(|(a, b)| (a + b) * 0.5).collect();
        let scale = std::env::var("TUNE_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(0xfff);
        let shift = std::env::var("TUNE_SHIFT").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        let retune = std::env::var("TUNE_RETUNE").ok().and_then(|s| s.parse().ok()).unwrap_or(0.6);
        let mut tuner = Tuner::new(SR);
        let late = tuner.latency();
        let mut out = Vec::new();
        let mut log = Vec::new();
        for block in input.chunks(256) {
            let (mut l, mut r) = (block.to_vec(), block.to_vec());
            tuner.process(retune, 0.0, shift, scale, &mut l, &mut r);
            out.extend(l);
            log.push((tuner.voiced, tuner.period, tuner.correction));
        }
        let (mut flips, mut leaps, mut voiced_blocks) = (0, 0, 0);
        for w in log.windows(2) {
            if w[0].0 != w[1].0 {
                flips += 1;
            }
            if w[0].0 && w[1].0 {
                voiced_blocks += 1;
                if (w[1].1 / w[0].1).log2().abs() > 0.2 {
                    leaps += 1;
                    if std::env::var("TUNE_LEAPS").is_ok() {
                        print!("{:.0}>{:.0} ", w[0].1, w[1].1);
                    }
                }
            }
        }
        println!("tracker: {voiced_blocks} voiced blocks, {flips} voiced/unvoiced flips, {leaps} period leaps");
        let mut bad = Vec::new();
        let (mut frames, mut rough_in, mut rough_out, mut jumps, mut level) = (0, 0.0, 0.0, 0, 0.0);
        let mut last: Option<f32> = None;
        let mut at = 0;
        while at + late + 2048 < input.len() {
            let a = &input[at..at + 2048];
            let b = &out[at + late..at + late + 2048];
            let (pa, na) = frame_pitch(a);
            if rms(&a[..1024]) > 0.02 && na < 0.1 {
                let (pb, nb) = frame_pitch(b);
                frames += 1;
                rough_in += na;
                rough_out += nb;
                level += (rms(&b[..1024]) / rms(&a[..1024])).ln().abs();
                if nb > na + 0.1 {
                    bad.push(at);
                }
                let cents = 1200.0 * (pa / pb).log2() - shift as f32 * 100.0;
                if let Some(prev) = last {
                    if (cents - prev).abs() > 150.0 {
                        jumps += 1;
                    }
                }
                last = Some(cents);
            } else {
                last = None;
            }
            at += 512;
        }
        let n = frames as f32;
        println!("voiced frames {frames}: roughness in {:.4} out {:.4}, level wander {:.3}, pitch jumps {jumps}", rough_in / n, rough_out / n, level / n);
        println!("{} frames made clearly rougher; first at {:?}", bad.len(), bad.iter().take(12).map(|a| *a as f32 / SR).collect::<Vec<_>>());
        if let Ok(to) = std::env::var("TUNE_OUT") {
            let _ = crate::sample::save_wav(std::path::Path::new(&to), &crate::sample::SampleData::new(out.clone(), out), SR as u32);
        }
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
