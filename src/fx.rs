//! Performance bus effects. Each slot owns all of its state up front so switching
//! effect type on the audio thread never allocates.

use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum FxKind {
    None,
    Vinyl,
    Cassette,
    Isolator,
    Filter,
    Crush,
    Comp,
    Delay,
    Reverb,
    /// A hosted VST3 effect; the instance lives in the engine's plugin slots.
    Vst,
    /// Low shelf, mid bell, high shelf. Listed last so projects saved before it existed
    /// still read back the same effects.
    Eq,
}

impl FxKind {
    pub const ALL: [FxKind; 10] = [
        FxKind::None,
        FxKind::Vinyl,
        FxKind::Cassette,
        FxKind::Eq,
        FxKind::Isolator,
        FxKind::Filter,
        FxKind::Crush,
        FxKind::Comp,
        FxKind::Delay,
        FxKind::Reverb,
    ];

    pub fn name(self) -> &'static str {
        match self {
            FxKind::None => "—",
            FxKind::Vinyl => "Vinyl Sim",
            FxKind::Cassette => "Cassette",
            FxKind::Isolator => "DJ Isolator",
            FxKind::Filter => "DJ Filter",
            FxKind::Crush => "12-Bit Crush",
            FxKind::Comp => "FET Comp",
            FxKind::Delay => "Tape Delay",
            FxKind::Reverb => "Reverb",
            FxKind::Vst => "VST3",
            FxKind::Eq => "3-Band EQ",
        }
    }

    pub fn param_names(self) -> [&'static str; 3] {
        match self {
            FxKind::None => ["", "", ""],
            FxKind::Vinyl => ["Warble", "Dust", "Age"],
            FxKind::Cassette => ["Drive", "Flutter", "Tone"],
            FxKind::Isolator => ["Low", "Mid", "High"],
            FxKind::Filter => ["Cutoff", "Reso", "Mix"],
            FxKind::Crush => ["Rate", "Bits", "Mix"],
            FxKind::Comp => ["Thresh", "Ratio", "Mix"],
            FxKind::Delay => ["Time", "Feedbk", "Mix"],
            FxKind::Reverb => ["Size", "Damp", "Mix"],
            FxKind::Vst => ["Mix", "", ""],
            FxKind::Eq => ["Low", "Mid", "High"],
        }
    }

    /// Names of the two slider parameters, for the effects that have them.
    pub fn slider_names(self) -> Option<[&'static str; 2]> {
        (self == FxKind::Eq).then_some(["Lo", "Hi"])
    }

    pub fn slider_defaults(self) -> [f32; 2] {
        match self {
            // About 200 Hz and 5 kHz.
            FxKind::Eq => [0.6, 0.5],
            _ => [0.0; 2],
        }
    }

    pub fn defaults(self) -> [f32; 3] {
        match self {
            FxKind::None => [0.0; 3],
            FxKind::Vinyl => [0.35, 0.3, 0.4],
            FxKind::Cassette => [0.4, 0.3, 0.7],
            FxKind::Isolator => [0.5, 0.5, 0.5],
            FxKind::Filter => [0.5, 0.2, 1.0],
            FxKind::Crush => [0.3, 0.33, 1.0],
            FxKind::Comp => [0.5, 0.5, 0.5],
            FxKind::Delay => [0.6, 0.4, 0.3],
            FxKind::Reverb => [0.5, 0.5, 0.25],
            FxKind::Vst => [1.0, 0.0, 0.0],
            FxKind::Eq => [0.5, 0.5, 0.5],
        }
    }
}

/// How far each EQ band can cut or boost, in dB.
pub const EQ_RANGE_DB: f32 = 15.0;

/// Corner of the EQ's low shelf for a slider position: 40 Hz to 600 Hz.
pub fn eq_low_hz(v: f32) -> f32 {
    40.0 * 15f32.powf(v.clamp(0.0, 1.0))
}

/// Corner of the EQ's high shelf for a slider position: 1.5 kHz to 16 kHz.
pub fn eq_high_hz(v: f32) -> f32 {
    1500.0 * (16000.0f32 / 1500.0).powf(v.clamp(0.0, 1.0))
}

/// The EQ's mid bell sits halfway (in octaves) between the two shelves and is as wide
/// as the gap between them, within reason.
pub fn eq_mid(low_hz: f32, high_hz: f32) -> (f32, f32) {
    let octaves = (high_hz / low_hz).log2();
    ((low_hz * high_hz).sqrt(), (2.4 / octaves).clamp(0.4, 1.4))
}

/// One second-order section, transposed direct form II. Coefficients are normalised by a0.
#[derive(Clone, Copy, Default)]
pub struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [[f32; 2]; 2],
}

impl Biquad {
    fn set(&mut self, b: [f32; 3], a: [f32; 3]) {
        self.b = [b[0] / a[0], b[1] / a[0], b[2] / a[0]];
        self.a = [a[1] / a[0], a[2] / a[0]];
    }

    /// A shelf at `hz` with a gentle (Butterworth-like) slope: low if `high` is false.
    pub fn shelf(&mut self, high: bool, hz: f32, db: f32, sr: f32) {
        let a = 10f32.powf(db / 40.0);
        let w = 2.0 * PI * (hz / sr).min(0.49);
        let (sin, cos) = w.sin_cos();
        let beta = 2.0 * a.sqrt() * sin * std::f32::consts::FRAC_1_SQRT_2;
        // The two shelves are mirror images: flip the sign of every cosine term.
        let c = if high { -cos } else { cos };
        self.set(
            [a * ((a + 1.0) - (a - 1.0) * c + beta), 2.0 * a * ((a - 1.0) - (a + 1.0) * c), a * ((a + 1.0) - (a - 1.0) * c - beta)],
            [(a + 1.0) + (a - 1.0) * c + beta, -2.0 * ((a - 1.0) + (a + 1.0) * c), (a + 1.0) + (a - 1.0) * c - beta],
        );
        if high {
            self.b[1] = -self.b[1];
            self.a[0] = -self.a[0];
        }
    }

    pub fn bell(&mut self, hz: f32, q: f32, db: f32, sr: f32) {
        let a = 10f32.powf(db / 40.0);
        let w = 2.0 * PI * (hz / sr).min(0.49);
        let alpha = w.sin() / (2.0 * q);
        let cos = w.cos();
        self.set([1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a], [1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a]);
    }

    #[inline]
    fn tick(&mut self, ch: usize, x: f32) -> f32 {
        let z = &mut self.z[ch];
        let y = self.b[0] * x + z[0];
        z[0] = self.b[1] * x - self.a[0] * y + z[1];
        z[1] = self.b[2] * x - self.a[1] * y;
        y
    }

    /// Gain in dB at `hz`, worked out from the coefficients.
    #[cfg(test)]
    fn db_at(&self, hz: f32, sr: f32) -> f32 {
        let w = 2.0 * PI * hz / sr;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let mag = |k0: f32, k1: f32, k2: f32| ((k0 + k1 * c1 + k2 * c2).powi(2) + (k1 * s1 + k2 * s2).powi(2)).sqrt();
        20.0 * (mag(self.b[0], self.b[1], self.b[2]) / mag(1.0, self.a[0], self.a[1]).max(1e-9)).max(1e-9).log10()
    }
}

/// The three sections of the EQ for a slot's settings.
pub fn eq_bands(params: [f32; 3], sliders: [f32; 2], sr: f32) -> [Biquad; 3] {
    let db = |v: f32| (v - 0.5) * 2.0 * EQ_RANGE_DB;
    let (low, high) = (eq_low_hz(sliders[0]), eq_high_hz(sliders[1]));
    let (mid, q) = eq_mid(low, high);
    let mut bands = [Biquad::default(); 3];
    bands[0].shelf(false, low, db(params[0]), sr);
    bands[1].bell(mid, q, db(params[1]), sr);
    bands[2].shelf(true, high, db(params[2]), sr);
    bands
}

/// Topology-preserving state variable filter, one channel.
#[derive(Clone, Copy, Default)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    /// Returns (low, band, high). `g = tan(pi * fc / sr)`, `k = 1 / Q`.
    #[inline]
    pub fn tick(&mut self, x: f32, g: f32, k: f32) -> (f32, f32, f32) {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, v1, x - k * v1 - v2)
    }
}

#[inline]
pub fn svf_g(fc: f32, sr: f32) -> f32 {
    (PI * (fc / sr).min(0.49)).tan()
}

struct Rng(u32);

impl Rng {
    #[inline]
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1 << 24) as f32
    }
    #[inline]
    fn bipolar(&mut self) -> f32 {
        self.next() * 2.0 - 1.0
    }
}

const MOD_LEN: usize = 8192;
const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;

struct Comb {
    buf: Vec<f32>,
    pos: usize,
    store: f32,
}

impl Comb {
    #[inline]
    fn tick(&mut self, x: f32, fb: f32, damp: f32) -> f32 {
        let out = self.buf[self.pos];
        self.store = out * (1.0 - damp) + self.store * damp;
        self.buf[self.pos] = x + self.store * fb;
        self.pos = (self.pos + 1) % self.buf.len();
        out
    }
}

struct Allpass {
    buf: Vec<f32>,
    pos: usize,
}

impl Allpass {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let b = self.buf[self.pos];
        self.buf[self.pos] = x + b * 0.5;
        self.pos = (self.pos + 1) % self.buf.len();
        b - x
    }
}

pub struct Fx {
    sr: f32,
    last_kind: FxKind,
    /// Smoothed parameters; they chase the targets once per block.
    p: [f32; 3],
    sliders: [f32; 2],
    eq: [Biquad; 3],
    rng: Rng,

    // Short modulated delay for wow / flutter.
    modl: Vec<f32>,
    modr: Vec<f32>,
    modw: usize,
    lfo1: f32,
    lfo2: f32,
    drift: f32,

    svf: [Svf; 4],
    lp: [f32; 4],
    crackle: f32,
    crackle_lp: f32,
    /// Opens the dust/hiss while audio is passing, so an idle bus stays silent.
    noise_gate: f32,

    hold_l: f32,
    hold_r: f32,
    hold_count: f32,

    comp_env: f32,

    dl: Vec<f32>,
    dr: Vec<f32>,
    dw: usize,
    delay_cur: f32,
    fb_lp: [f32; 2],

    combs: [Vec<Comb>; 2],
    allpasses: [Vec<Allpass>; 2],
}

impl Fx {
    pub fn new(sr: f32) -> Self {
        let scale = sr / 44100.0;
        let mk = |spread: usize| -> (Vec<Comb>, Vec<Allpass>) {
            (
                COMB_TUNING
                    .iter()
                    .map(|&n| Comb {
                        buf: vec![0.0; ((n + spread) as f32 * scale) as usize],
                        pos: 0,
                        store: 0.0,
                    })
                    .collect(),
                ALLPASS_TUNING
                    .iter()
                    .map(|&n| Allpass { buf: vec![0.0; ((n + spread) as f32 * scale) as usize], pos: 0 })
                    .collect(),
            )
        };
        let (cl, al) = mk(0);
        let (cr, ar) = mk(STEREO_SPREAD);
        let dlen = (sr * 2.0) as usize + 4;
        Fx {
            sr,
            last_kind: FxKind::None,
            p: [0.0; 3],
            sliders: [0.0; 2],
            eq: [Biquad::default(); 3],
            rng: Rng(0x9E3779B9),
            modl: vec![0.0; MOD_LEN],
            modr: vec![0.0; MOD_LEN],
            modw: 0,
            lfo1: 0.0,
            lfo2: 0.0,
            drift: 0.0,
            svf: [Svf::default(); 4],
            lp: [0.0; 4],
            crackle: 0.0,
            crackle_lp: 0.0,
            noise_gate: 0.0,
            hold_l: 0.0,
            hold_r: 0.0,
            hold_count: 0.0,
            comp_env: 0.0,
            dl: vec![0.0; dlen],
            dr: vec![0.0; dlen],
            dw: 0,
            delay_cur: 0.0,
            fb_lp: [0.0; 2],
            combs: [cl, cr],
            allpasses: [al, ar],
        }
    }

    fn reset(&mut self) {
        self.modl.fill(0.0);
        self.modr.fill(0.0);
        self.dl.fill(0.0);
        self.dr.fill(0.0);
        for ch in 0..2 {
            for c in &mut self.combs[ch] {
                c.buf.fill(0.0);
                c.store = 0.0;
            }
            for a in &mut self.allpasses[ch] {
                a.buf.fill(0.0);
            }
        }
        self.svf = [Svf::default(); 4];
        self.eq = [Biquad::default(); 3];
        self.lp = [0.0; 4];
        self.fb_lp = [0.0; 2];
        self.comp_env = 0.0;
        self.crackle = 0.0;
        self.delay_cur = 0.0;
    }

    /// Writes into the mod delay and reads `delay` samples back, linearly interpolated.
    #[inline]
    fn mod_delay(&mut self, l: f32, r: f32, delay: f32) -> (f32, f32) {
        self.modl[self.modw] = l;
        self.modr[self.modw] = r;
        let d = delay.clamp(1.0, (MOD_LEN - 2) as f32);
        let rp = self.modw as f32 + MOD_LEN as f32 - d;
        let i = rp as usize;
        let t = rp - i as f32;
        let (a, b) = (i % MOD_LEN, (i + 1) % MOD_LEN);
        let out = (
            self.modl[a] + (self.modl[b] - self.modl[a]) * t,
            self.modr[a] + (self.modr[b] - self.modr[a]) * t,
        );
        self.modw = (self.modw + 1) % MOD_LEN;
        out
    }

    /// Processes one block in place. `target` is the slot's three knobs and `sliders` its
    /// two slider parameters, all 0..1.
    pub fn process(&mut self, kind: FxKind, target: [f32; 3], sliders: [f32; 2], bpm: f32, l: &mut [f32], r: &mut [f32]) {
        if kind != self.last_kind {
            // The fill is a one-off cost on a type change; no allocation involved.
            self.reset();
            self.last_kind = kind;
            self.p = target;
            self.sliders = sliders;
        }
        for i in 0..3 {
            self.p[i] += (target[i] - self.p[i]) * 0.35;
        }
        for i in 0..2 {
            self.sliders[i] += (sliders[i] - self.sliders[i]) * 0.35;
        }
        let p = self.p;
        let sr = self.sr;
        let n = l.len();
        match kind {
            // Hosted plugins are processed by the engine, not here.
            FxKind::None | FxKind::Vst => {}

            FxKind::Vinyl => {
                let (warble, dust, age) = (p[0], p[1], p[2]);
                let lp_a = 1.0 - (-2.0 * PI * (16000.0 * (1.0 - age) + 2800.0 * age) / sr).exp();
                let hp_a = 1.0 - (-2.0 * PI * (20.0 + 180.0 * age) / sr).exp();
                let inc1 = 0.5556 / sr; // 33 1/3 rpm
                let inc2 = 3.1 / sr;
                let pop_rate = dust * dust * 18.0 / sr;
                let hiss = dust * 0.0035;
                // 12-bit converter character comes in with age.
                let q = 2048.0f32;
                let crush = (age * 1.6).min(1.0);
                let gate_release = (-1.0 / (0.6 * sr)).exp();
                for i in 0..n {
                    self.lfo1 = (self.lfo1 + inc1).fract();
                    self.lfo2 = (self.lfo2 + inc2).fract();
                    let m = (self.lfo1 * 2.0 * PI).sin() * 0.8 + (self.lfo2 * 2.0 * PI).sin() * 0.2;
                    let d = sr * 0.006 + m * warble * sr * 0.0035;
                    let (mut a, mut b) = self.mod_delay(l[i], r[i], d);

                    if self.rng.next() < pop_rate {
                        self.crackle = self.rng.bipolar() * (0.15 + 0.5 * self.rng.next());
                    }
                    self.crackle *= 0.86;
                    self.crackle_lp += 0.5 * (self.crackle - self.crackle_lp);
                    let level = l[i].abs().max(r[i].abs());
                    if level > 0.003 {
                        self.noise_gate = 1.0;
                    } else {
                        self.noise_gate *= gate_release;
                    }
                    let noise = (self.crackle_lp * dust + self.rng.bipolar() * hiss) * self.noise_gate;
                    a += noise;
                    b += noise * 0.9;

                    let (qa, qb) = ((a * q).round() / q, (b * q).round() / q);
                    a += (qa - a) * crush;
                    b += (qb - b) * crush;

                    self.lp[0] += lp_a * (a - self.lp[0]);
                    self.lp[1] += lp_a * (b - self.lp[1]);
                    self.lp[2] += hp_a * (self.lp[0] - self.lp[2]);
                    self.lp[3] += hp_a * (self.lp[1] - self.lp[3]);
                    l[i] = self.lp[0] - self.lp[2];
                    r[i] = self.lp[1] - self.lp[3];
                }
            }

            FxKind::Cassette => {
                let (drive, flutter, tone) = (p[0], p[1], p[2]);
                let gain = 1.0 + drive * 7.0;
                let comp = 1.0 / gain.tanh().max(0.3) * (1.0 - drive * 0.35);
                let lp_a = 1.0 - (-2.0 * PI * (1800.0 * (9.0f32).powf(tone)) / sr).exp();
                let inc1 = 0.8 / sr;
                let inc2 = 6.3 / sr;
                for i in 0..n {
                    self.lfo1 = (self.lfo1 + inc1).fract();
                    self.lfo2 = (self.lfo2 + inc2).fract();
                    self.drift += 0.0004 * (self.rng.bipolar() - self.drift);
                    let m = (self.lfo1 * 2.0 * PI).sin() * 0.6
                        + (self.lfo2 * 2.0 * PI).sin() * 0.25
                        + self.drift * 40.0;
                    let d = sr * 0.004 + m * flutter * sr * 0.0012;
                    let (a, b) = self.mod_delay(l[i], r[i], d);
                    let a = (a * gain).tanh() * comp;
                    let b = (b * gain).tanh() * comp;
                    self.lp[0] += lp_a * (a - self.lp[0]);
                    self.lp[1] += lp_a * (b - self.lp[1]);
                    l[i] = self.lp[0];
                    r[i] = self.lp[1];
                }
            }

            FxKind::Eq => {
                // New coefficients each block, on the smoothed settings; the filter state
                // carries over, so a moving control glides rather than clicks.
                let fresh = eq_bands(p, self.sliders, sr);
                for (band, new) in self.eq.iter_mut().zip(fresh) {
                    band.b = new.b;
                    band.a = new.a;
                }
                for i in 0..n {
                    let (mut a, mut b) = (l[i], r[i]);
                    for band in self.eq.iter_mut() {
                        a = band.tick(0, a);
                        b = band.tick(1, b);
                    }
                    l[i] = a;
                    r[i] = b;
                }
            }

            FxKind::Isolator => {
                // 0 kills the band, 0.5 is unity, 1 is +12 dB.
                let gain = |v: f32| (v * 2.0) * (v * 2.0);
                let (gl, gm, gh) = (gain(p[0]), gain(p[1]), gain(p[2]));
                let g1 = svf_g(300.0, sr);
                let g2 = svf_g(4000.0, sr);
                let k = 1.414;
                for i in 0..n {
                    let (lo, _, _) = self.svf[0].tick(l[i], g1, k);
                    let rest = l[i] - lo;
                    let (_, _, hi) = self.svf[1].tick(rest, g2, k);
                    l[i] = lo * gl + (rest - hi) * gm + hi * gh;

                    let (lo, _, _) = self.svf[2].tick(r[i], g1, k);
                    let rest = r[i] - lo;
                    let (_, _, hi) = self.svf[3].tick(rest, g2, k);
                    r[i] = lo * gl + (rest - hi) * gm + hi * gh;
                }
            }

            FxKind::Filter => {
                // One knob: left of centre sweeps a lowpass down, right sweeps a highpass up.
                let (c, reso, mix) = (p[0], p[1], p[2]);
                let k = 1.0 / (0.7 + reso * 9.0);
                let (fc, high) = if c < 0.48 {
                    (60.0 * (18000.0f32 / 60.0).powf(c / 0.48), false)
                } else if c > 0.52 {
                    (20.0 * (9000.0f32 / 20.0).powf((c - 0.52) / 0.48), true)
                } else {
                    (0.0, false)
                };
                if fc > 0.0 {
                    let g = svf_g(fc, sr);
                    for i in 0..n {
                        let (lo, _, hi) = self.svf[0].tick(l[i], g, k);
                        let w = if high { hi } else { lo };
                        l[i] += (w - l[i]) * mix;
                        let (lo, _, hi) = self.svf[1].tick(r[i], g, k);
                        let w = if high { hi } else { lo };
                        r[i] += (w - r[i]) * mix;
                    }
                }
            }

            FxKind::Crush => {
                let (rate, bits, mix) = (p[0], p[1], p[2]);
                let factor = 1.0 + rate * rate * 47.0;
                let q = 2f32.powf(15.0 - bits * 12.0);
                for i in 0..n {
                    self.hold_count += 1.0;
                    if self.hold_count >= factor {
                        self.hold_count -= factor;
                        self.hold_l = (l[i] * q).round() / q;
                        self.hold_r = (r[i] * q).round() / q;
                    }
                    l[i] += (self.hold_l - l[i]) * mix;
                    r[i] += (self.hold_r - r[i]) * mix;
                }
            }

            FxKind::Comp => {
                let (thresh, ratio, mix) = (p[0], p[1], p[2]);
                let th_db = -thresh * 40.0;
                let ratio = 2.0 + ratio * 18.0;
                let att = 1.0 - (-1.0 / (0.0004 * sr)).exp();
                let rel = 1.0 - (-1.0 / (0.09 * sr)).exp();
                let makeup = 10f32.powf(-th_db * (1.0 - 1.0 / ratio) * 0.5 / 20.0);
                for i in 0..n {
                    let level = l[i].abs().max(r[i].abs());
                    let coef = if level > self.comp_env { att } else { rel };
                    self.comp_env += coef * (level - self.comp_env);
                    let db = 20.0 * self.comp_env.max(1e-6).log10();
                    let over = (db - th_db).max(0.0);
                    let g = 10f32.powf(-over * (1.0 - 1.0 / ratio) / 20.0) * makeup;
                    // A little odd-harmonic bite on the squashed signal, FET style.
                    let a = (l[i] * g * 1.2).tanh() / 1.2f32.tanh() * 0.83;
                    let b = (r[i] * g * 1.2).tanh() / 1.2f32.tanh() * 0.83;
                    l[i] += (a - l[i]) * mix;
                    r[i] += (b - r[i]) * mix;
                }
            }

            FxKind::Delay => {
                let (time, fb, mix) = (p[0], p[1], p[2]);
                // Tempo-synced divisions, in beats.
                const DIVS: [f32; 8] = [0.125, 1.0 / 6.0, 0.25, 1.0 / 3.0, 0.5, 0.75, 1.0, 2.0];
                let idx = ((time * 7.999) as usize).min(7);
                let len = self.dl.len();
                let want = (DIVS[idx] * 60.0 / bpm.max(20.0) * sr).min((len - 4) as f32);
                if self.delay_cur < 1.0 {
                    self.delay_cur = want;
                }
                let fb = fb * 0.95;
                let lp_a = 0.35;
                for i in 0..n {
                    // Slewing the delay time gives the tape-style pitch glide on changes.
                    self.delay_cur += (want - self.delay_cur) * 0.0004;
                    let rp = self.dw as f32 + len as f32 - self.delay_cur;
                    let j = rp as usize;
                    let t = rp - j as f32;
                    let (a, b) = (j % len, (j + 1) % len);
                    let wl = self.dl[a] + (self.dl[b] - self.dl[a]) * t;
                    let wr = self.dr[a] + (self.dr[b] - self.dr[a]) * t;
                    self.fb_lp[0] += lp_a * (wl - self.fb_lp[0]);
                    self.fb_lp[1] += lp_a * (wr - self.fb_lp[1]);
                    self.dl[self.dw] = l[i] + (self.fb_lp[0] * fb).tanh();
                    self.dr[self.dw] = r[i] + (self.fb_lp[1] * fb).tanh();
                    self.dw = (self.dw + 1) % len;
                    l[i] += wl * mix;
                    r[i] += wr * mix;
                }
            }

            FxKind::Reverb => {
                let (size, damp, mix) = (p[0], p[1], p[2]);
                let fb = 0.7 + size * 0.28;
                let damp = damp * 0.4;
                for i in 0..n {
                    let input = (l[i] + r[i]) * 0.015;
                    let mut out = [0f32; 2];
                    for ch in 0..2 {
                        let mut acc = 0.0;
                        for c in &mut self.combs[ch] {
                            acc += c.tick(input, fb, damp);
                        }
                        for a in &mut self.allpasses[ch] {
                            acc = a.tick(acc);
                        }
                        out[ch] = acc;
                    }
                    l[i] = l[i] * (1.0 - mix * 0.5) + out[0] * mix * 2.0;
                    r[i] = r[i] * (1.0 - mix * 0.5) + out[1] * mix * 2.0;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Level in dB of a sine at `hz` after the EQ, relative to the input.
    fn eq_gain(hz: f32, params: [f32; 3], sliders: [f32; 2]) -> f32 {
        let sr = 48000.0;
        let mut fx = Fx::new(sr);
        // Compared by energy: the peaks of a sampled sine depend on where the samples fall.
        let (mut dry, mut wet) = (0f64, 0f64);
        for block in 0..200 {
            let mut l: Vec<f32> = (0..256).map(|i| ((block * 256 + i) as f32 * 2.0 * PI * hz / sr).sin()).collect();
            let mut r = l.clone();
            let before: f64 = l.iter().map(|v| (*v as f64).powi(2)).sum();
            fx.process(FxKind::Eq, params, sliders, 120.0, &mut l, &mut r);
            if block > 150 {
                dry += before;
                wet += r.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
            }
        }
        (10.0 * (wet / dry).log10()) as f32
    }

    #[test]
    fn eq_is_flat_with_the_knobs_centred() {
        for hz in [50.0, 300.0, 1000.0, 5000.0, 12000.0] {
            assert!(eq_gain(hz, [0.5; 3], [0.6, 0.5]).abs() < 0.05, "{hz} Hz");
        }
    }

    #[test]
    fn eq_bands_act_where_they_should() {
        let sliders = [0.6, 0.5]; // about 200 Hz and 5 kHz
        // Low shelf up: the bottom rises by the full amount, the top is left alone.
        assert!((eq_gain(40.0, [1.0, 0.5, 0.5], sliders) - EQ_RANGE_DB).abs() < 1.0);
        assert!(eq_gain(8000.0, [1.0, 0.5, 0.5], sliders).abs() < 0.5);
        // High shelf down: the reverse.
        assert!((eq_gain(15000.0, [0.5, 0.5, 0.0], sliders) + EQ_RANGE_DB).abs() < 1.0);
        assert!(eq_gain(60.0, [0.5, 0.5, 0.0], sliders).abs() < 0.5);
        // The bell peaks between the two corners and falls away either side.
        let (mid, _) = eq_mid(eq_low_hz(sliders[0]), eq_high_hz(sliders[1]));
        let at = |hz: f32| eq_gain(hz, [0.5, 1.0, 0.5], sliders);
        assert!((at(mid) - EQ_RANGE_DB).abs() < 0.5);
        assert!(at(40.0) < 3.0 && at(15000.0) < 3.0);
    }

    #[test]
    fn eq_response_curve_matches_the_filter() {
        let (params, sliders, sr) = ([0.8, 0.3, 0.9], [0.4, 0.7], 48000.0);
        let bands = eq_bands(params, sliders, sr);
        for hz in [80.0, 700.0, 9000.0] {
            let drawn: f32 = bands.iter().map(|b| b.db_at(hz, sr)).sum();
            assert!((drawn - eq_gain(hz, params, sliders)).abs() < 0.2, "{hz} Hz");
        }
    }

    #[test]
    fn moving_the_cutoffs_moves_the_shelves() {
        // 300 Hz is inside a low shelf set to 600 Hz and outside one set to 40 Hz.
        assert!(eq_gain(300.0, [1.0, 0.5, 0.5], [1.0, 0.5]) > 9.0);
        assert!(eq_gain(300.0, [1.0, 0.5, 0.5], [0.0, 0.5]) < 2.0);
    }
}
