//! The native drum synth: five percussion engines, each played through eight macros.
//!
//! A drum pad holds a [`DrumParams`] patch in its settings, so it travels to the engine,
//! into the undo history and into project files the same way every other pad setting does.
//! The audio thread plays it with a pool of [`DrumVoice`]s; the UI thread renders the very
//! same voice offline to bake a patch into a sample ([`render`]).
//!
//! Every hit is resolved from the patch first: the knob values, moved by velocity (each
//! macro has its own amount), by a little randomness (each has its own range) and by any
//! values locked to the step that fired it. The voice then runs with those fixed values.

use crate::fx::{Svf, svf_g};
use crate::music::{PPQ, TICKS_PER_BAR};
use crate::sample::SampleData;
use serde::{Deserialize, Serialize};
use std::f32::consts::{PI, TAU};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum DrumEngine {
    #[default]
    Kick,
    Snare,
    Metal,
    Fm,
    Modal,
}

impl DrumEngine {
    pub const ALL: [DrumEngine; 5] = [DrumEngine::Kick, DrumEngine::Snare, DrumEngine::Metal, DrumEngine::Fm, DrumEngine::Modal];

    pub fn name(self) -> &'static str {
        match self {
            DrumEngine::Kick => "Sub / Kick",
            DrumEngine::Snare => "Snare / Clap",
            DrumEngine::Metal => "Metallic / Hat",
            DrumEngine::Fm => "FM / Tom / Perc",
            DrumEngine::Modal => "Modal",
        }
    }

    /// A word for the pad face.
    pub fn short(self) -> &'static str {
        match self {
            DrumEngine::Kick => "KICK",
            DrumEngine::Snare => "SNARE",
            DrumEngine::Metal => "METAL",
            DrumEngine::Fm => "FM",
            DrumEngine::Modal => "MODAL",
        }
    }

    pub fn macros(self) -> [&'static str; 8] {
        match self {
            DrumEngine::Kick => ["Pitch", "Sweep", "Sweep Time", "Click", "Drive", "Sub", "Decay", "Tone"],
            DrumEngine::Snare => ["Body Pitch", "Wires", "Snap", "Clap Spread", "Density", "Box", "Decay", "Drive"],
            DrumEngine::Metal => ["Pitch", "Inharm", "FM", "Ring", "HPF", "Closed", "Open", "Crisp"],
            DrumEngine::Fm => ["Pitch", "Ratio", "FM Depth", "FM Decay", "Fold", "Feedback", "Decay", "LPG"],
            DrumEngine::Modal => ["Exciter", "Material", "Pitch", "Harmonics", "Damping", "Position", "Decay", "Width"],
        }
    }

    /// Macros behind the two handles on the envelope display: (x, y) for the body, and
    /// (x, y) for the attack shape. `None` where an engine has nothing to put there.
    pub fn handles(self) -> [Option<(usize, usize)>; 2] {
        match self {
            DrumEngine::Kick => [Some((6, 7)), Some((2, 1))],
            DrumEngine::Snare => [Some((6, 5)), Some((3, 1))],
            DrumEngine::Metal => [Some((6, 4)), Some((5, 7))],
            DrumEngine::Fm => [Some((6, 7)), Some((3, 2))],
            DrumEngine::Modal => [Some((6, 3)), Some((0, 4))],
        }
    }
}

/// Note values a drum's tail can be cut to, so it ends on the grid.
pub const SYNC_NAMES: [&str; 6] = ["Free", "1/16", "1/8", "1/4", "1/2", "1 bar"];

pub fn sync_ticks(sync: u8) -> Option<u32> {
    match sync {
        1 => Some(PPQ / 4),
        2 => Some(PPQ / 2),
        3 => Some(PPQ),
        4 => Some(PPQ * 2),
        5 => Some(TICKS_PER_BAR),
        _ => None,
    }
}

/// Per-step values for the eight macros, 0..=200 as 0..1, or [`NO_LOCK`].
pub type Locks = [u8; 8];
pub const NO_LOCK: u8 = 255;
pub const NO_LOCKS: Locks = [NO_LOCK; 8];

pub fn lock_value(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 200.0).round() as u8
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DrumParams {
    pub engine: DrumEngine,
    /// The eight macros, 0..1.
    pub m: [f32; 8],
    /// How far a hard hit moves each macro, -1..1. Normal velocity (100) moves nothing.
    pub vel: [f32; 8],
    /// How far each macro wanders at random on every hit, 0..1.
    pub rnd: [f32; 8],
    /// Metallic engine: ring out with the Open decay rather than the Closed one.
    pub open: bool,
    /// Pull the drum's pitch onto the project scale.
    pub key_lock: bool,
    /// Cut the tail after a note value at the project tempo; see [`SYNC_NAMES`].
    pub sync: u8,
}

impl Default for DrumParams {
    fn default() -> Self {
        patch(DrumEngine::Kick, [0.35, 0.35, 0.45, 0.15, 0.1, 0.3, 0.75, 0.45])
    }
}

fn patch(engine: DrumEngine, m: [f32; 8]) -> DrumParams {
    DrumParams { engine, m, vel: [0.0; 8], rnd: [0.0; 8], open: false, key_lock: false, sync: 0 }
}

impl DrumParams {
    /// The macros a hit plays with. `rand` gives values in -1..1.
    pub fn resolve(&self, vel: f32, locks: &Locks, rand: &mut impl FnMut() -> f32) -> [f32; 8] {
        let mut out = self.m;
        for k in 0..8 {
            out[k] = if locks[k] != NO_LOCK {
                locks[k] as f32 / 200.0
            } else {
                // Velocity 100 (0.79) is neutral; a full hit adds a quarter of the amount.
                let v = self.m[k] + self.vel[k] * (vel - 0.79) * 1.2;
                v + if self.rnd[k] > 0.0 { self.rnd[k] * rand() * 0.5 } else { 0.0 }
            }
            .clamp(0.0, 1.0);
        }
        out
    }

    /// The fundamental, in Hz, for a set of resolved macros.
    pub fn base_hz(&self, m: &[f32; 8]) -> f32 {
        let span = |lo: f32, hi: f32, t: f32| lo * (hi / lo).powf(t);
        match self.engine {
            DrumEngine::Kick => span(30.0, 130.0, m[0]),
            DrumEngine::Snare => span(120.0, 400.0, m[0]),
            DrumEngine::Metal => span(180.0, 720.0, m[0]),
            DrumEngine::Fm => span(50.0, 1200.0, m[0]),
            DrumEngine::Modal => span(60.0, 1200.0, m[2]),
        }
    }

    /// Seconds for the main body of the hit to fall 60 dB, from its decay knob.
    pub fn tail_secs(&self) -> f32 {
        let m = &self.m;
        match self.engine {
            DrumEngine::Kick => span(0.08, 3.0, m[6]),
            DrumEngine::Snare => span(0.1, 1.5, m[6]),
            DrumEngine::Metal if self.open => span(0.2, 4.0, m[6]),
            DrumEngine::Metal => span(0.03, 0.5, m[5]),
            DrumEngine::Fm => span(0.08, 4.0, m[6]),
            DrumEngine::Modal => span(0.1, 6.0, m[6]),
        }
    }

    /// A variation: every macro nudged by up to `amount` either way.
    pub fn mutate(&self, amount: f32, seed: u32) -> DrumParams {
        let mut rng = Rng(seed | 1);
        let mut out = *self;
        for v in out.m.iter_mut() {
            *v = (*v + rng.next() * amount).clamp(0.0, 1.0);
        }
        out
    }
}

/// A small, fast generator for noise and randomness; never zero.
#[derive(Clone, Copy, Default)]
pub struct Rng(pub u32);

impl Rng {
    /// The next value in -1..1.
    #[inline]
    pub fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }
}

/// The nearest frequency to `hz` that is a note of `scale` (a bit per pitch class, C first).
pub fn snap_hz(hz: f32, scale: u16) -> f32 {
    let scale = if scale & 0xfff == 0 { 0xfff } else { scale };
    let note = 69.0 + 12.0 * (hz / 440.0).log2();
    let base = note.round() as i32;
    let best = (-6..=6)
        .map(|d| base + d)
        .filter(|n| scale & (1 << n.rem_euclid(12)) != 0)
        .min_by(|a, b| (*a as f32 - note).abs().total_cmp(&(*b as f32 - note).abs()))
        .unwrap_or(base);
    440.0 * 2f32.powf((best as f32 - 69.0) / 12.0)
}

/// Ratios of the six square waves in the metallic engine, as on the TR-808.
const METAL_RATIOS: [f32; 6] = [1.0, 1.4826, 1.8, 2.546, 3.897, 2.63];
/// Mode ratios of a circular membrane (wood, skin) and of a free metal bar.
const MEMBRANE: [f32; 8] = [1.0, 1.594, 2.136, 2.296, 2.653, 2.918, 3.156, 3.501];
const BAR: [f32; 8] = [1.0, 2.756, 5.404, 8.933, 13.34, 18.64, 24.81, 31.87];

/// One drum hit sounding. Plain data, so a pool of them lives on the audio thread.
#[derive(Clone, Copy, Default)]
pub struct DrumVoice {
    pub active: bool,
    pub pad: u16,
    pub semis: f32,
    pub age: u64,
    /// Samples to wait inside the next block before sounding.
    pub offset: u32,
    /// Fading out quickly: choked, retriggered, stopped, or cut on the grid.
    pub killed: bool,
    fade: f32,
    engine: DrumEngine,
    m: [f32; 8],
    vel: f32,
    hz: f32,
    sr: f32,
    t: u32,
    /// Sample at which a synced tail is cut; 0 = never.
    cut_at: u32,
    rng: Rng,
    ph: [f32; 8],
    env: [f32; 6],
    /// Per-engine constants, worked out once when the hit starts.
    k: [f32; 12],
    svf: [Svf; 4],
    lp: f32,
    fb: f32,
    /// Clap bursts: how many, how many fired, samples apart.
    bursts: u32,
    fired: u32,
    spacing: u32,
    /// Modal resonators: state, coefficients (a1, a2, input gain) and pan.
    res: [[f32; 2]; 8],
    rc: [[f32; 3]; 8],
    pan: [f32; 8],
    exc_len: u32,
    level: f32,
}

#[inline]
fn decay(tau: f32, sr: f32) -> f32 {
    (-1.0 / (tau.max(1e-4) * sr)).exp()
}

#[inline]
fn span(lo: f32, hi: f32, t: f32) -> f32 {
    lo * (hi / lo).powf(t)
}

/// The time constant of a decay knob that runs from `lo` to `hi` seconds to fall 60 dB.
#[inline]
fn t60(lo: f32, hi: f32, t: f32) -> f32 {
    span(lo, hi, t) / 6.91
}

/// Leaves anything below 0.8 alone and bends the rest smoothly towards 1.
#[inline]
fn soft(x: f32) -> f32 {
    let a = x.abs();
    if a < 0.8 { x } else { x.signum() * (0.8 + 0.2 * ((a - 0.8) / 0.2).tanh()) }
}

/// `tanh` drive that leaves a full-scale input at full scale.
#[inline]
fn drive(x: f32, amount: f32) -> f32 {
    let g = 1.0 + amount * 9.0;
    (x * g).tanh() / g.tanh()
}

impl DrumVoice {
    /// Starts a hit. `m` are the resolved macros, `hz` the (possibly key-locked)
    /// fundamental, `cut_at` the synced tail length in samples (0 for none).
    #[allow(clippy::too_many_arguments)]
    pub fn start(&mut self, pad: u16, semis: f32, vel: f32, offset: u32, age: u64, p: &DrumParams, m: [f32; 8], hz: f32, sr: f32, cut_at: u32, seed: u32) {
        *self = DrumVoice {
            active: true,
            pad,
            semis,
            age,
            offset,
            fade: 1.0,
            engine: p.engine,
            m,
            vel: vel.clamp(0.0, 1.0),
            hz,
            sr,
            cut_at,
            rng: Rng(seed | 1),
            env: [1.0; 6],
            level: 1.0,
            ..Default::default()
        };
        let k = &mut self.k;
        match p.engine {
            DrumEngine::Kick => {
                k[0] = decay(t60(0.08, 3.0, m[6]), sr);
                k[1] = decay(span(0.004, 0.12, m[2]), sr);
                k[2] = decay(0.0015 + 0.003 * m[3], sr);
                k[3] = 1.0 - (-TAU * span(200.0, 16000.0, m[7]) / sr).exp();
                k[4] = 1.0 + m[1] * 7.0;
            }
            DrumEngine::Snare => {
                let tail = t60(0.1, 1.5, m[6]);
                k[0] = decay(tail * 0.55, sr);
                k[1] = decay(tail, sr);
                k[2] = decay(0.004, sr);
                k[3] = decay(0.0028, sr);
                k[4] = svf_g(1400.0, sr);
                k[5] = svf_g(1100.0, sr);
                k[6] = decay(0.006, sr);
                k[7] = svf_g(480.0, sr);
                if m[3] > 0.05 {
                    self.bursts = 1 + (m[4] * 5.0).round() as u32;
                    self.spacing = ((0.004 + m[3] * 0.012) * sr) as u32;
                }
            }
            DrumEngine::Metal => {
                let tau = if p.open { t60(0.2, 4.0, m[6]) } else { t60(0.03, 0.5, m[5]) };
                k[0] = decay(tau, sr);
                k[1] = svf_g((7000.0 * (0.6 + m[7] * 0.8)).min(sr * 0.45), sr);
                k[2] = 1.0 / (0.7 + m[3] * 10.0);
                k[3] = svf_g(span(1500.0, 12000.0, m[4]).min(sr * 0.45), sr);
                let inh = 0.5 + m[1] * 1.2;
                for (i, r) in METAL_RATIOS.iter().enumerate() {
                    self.rc[i][0] = (hz * (1.0 + (r - 1.0) * inh) / sr).min(0.45);
                }
            }
            DrumEngine::Fm => {
                k[0] = decay(t60(0.08, 4.0, m[6]), sr);
                k[1] = decay(span(0.005, 0.4, m[3]), sr);
                k[2] = decay(0.025, sr);
                k[3] = 0.5 + m[1] * 7.5;
            }
            DrumEngine::Modal => {
                let tau = t60(0.1, 6.0, m[6]);
                let pos = 0.08 + m[5] * 0.42;
                let mut total = 0.0;
                for i in 0..8 {
                    let ratio = (MEMBRANE[i].ln() * (1.0 - m[1]) + BAR[i].ln() * m[1]).exp();
                    let f = hz * ratio;
                    if f >= sr * 0.45 {
                        continue;
                    }
                    let w = TAU * f / sr;
                    let r = decay(tau / (1.0 + i as f32 * m[4] * 2.5), sr);
                    let amp = (1.0 / (i as f32 + 1.0)).powf(1.6 * (1.0 - m[3])) * (0.15 + 0.85 * (PI * (i as f32 + 1.0) * pos).sin().abs());
                    // Scaled so an impulse rings at `amp`: a two-pole's response peaks
                    // at 1 / sin(w).
                    self.rc[i] = [2.0 * r * w.cos(), -r * r, amp * w.sin()];
                    self.pan[i] = if i == 0 { 0.0 } else if i % 2 == 1 { -m[7] } else { m[7] };
                    total += amp;
                }
                k[0] = 1.0 / total.max(0.3);
                self.exc_len = ((0.0005 + m[0] * 0.03) * sr) as u32;
            }
        }
    }

    /// Adds this hit into `l` and `r`, scaled by `gl` and `gr`.
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32], gl: f32, gr: f32) {
        let n = l.len().min(r.len());
        let first = (self.offset as usize).min(n);
        self.offset -= first as u32;
        let kill = 1.0 / (0.004 * self.sr);
        let follow = (-1.0 / (0.05 * self.sr)).exp();
        let gain = self.vel.powf(1.3);
        for i in first..n {
            if self.cut_at > 0 && self.t >= self.cut_at {
                self.killed = true;
            }
            let (a, b) = self.tick();
            if self.killed {
                self.fade -= kill;
                if self.fade <= 0.0 {
                    self.active = false;
                    return;
                }
            }
            let g = gain * self.fade;
            l[i] += a * g * gl;
            r[i] += b * g * gr;
            self.level = (a.abs().max(b.abs())).max(self.level * follow);
            self.t += 1;
            // Done once it has been silent a while, or has rung for ten seconds.
            if (self.t as f32 > self.sr * 0.03 && self.level < 3e-5) || self.t as f32 > self.sr * 10.0 {
                self.active = false;
                return;
            }
        }
    }

    /// How loud the hit is now, for the pad's light.
    pub fn loudness(&self) -> f32 {
        self.level * self.vel * self.fade
    }

    #[inline]
    fn tick(&mut self) -> (f32, f32) {
        match self.engine {
            DrumEngine::Kick => {
                let y = self.kick();
                (y, y)
            }
            DrumEngine::Snare => {
                let y = self.snare();
                (y, y)
            }
            DrumEngine::Metal => {
                let y = self.metal();
                (y, y)
            }
            DrumEngine::Fm => {
                let y = self.fm();
                (y, y)
            }
            DrumEngine::Modal => self.modal(),
        }
    }

    fn kick(&mut self) -> f32 {
        let (m, sr) = (self.m, self.sr);
        let f = self.hz * (1.0 + (self.k[4] - 1.0) * self.env[1]);
        self.ph[0] = (self.ph[0] + f / sr).fract();
        self.ph[1] = (self.ph[1] + f * 0.5 / sr).fract();
        self.ph[2] = (self.ph[2] + 1800.0 / sr).fract();
        let sub = m[5];
        let body = (TAU * self.ph[0]).sin() * (1.0 - 0.6 * sub) + (TAU * self.ph[1]).sin() * sub * 0.9;
        let click = (self.rng.next() * 0.5 + (TAU * self.ph[2]).sin() * 0.5) * self.env[2] * m[3] * 0.9;
        let attack = (self.t as f32 / (0.0005 * sr)).min(1.0);
        let x = drive(body * self.env[0] * attack + click, m[4]);
        self.lp += self.k[3] * (x - self.lp);
        self.env[0] *= self.k[0];
        self.env[1] *= self.k[1];
        self.env[2] *= self.k[2];
        self.lp * 0.9
    }

    fn snare(&mut self) -> f32 {
        let (m, sr) = (self.m, self.sr);
        let bend = 1.0 + 0.4 * self.env[3];
        self.ph[0] = (self.ph[0] + self.hz * bend / sr).fract();
        self.ph[1] = (self.ph[1] + self.hz * 1.62 * bend / sr).fract();
        // From snare at no spread to all clap at full spread.
        let body_gain = 1.0 - m[3] * 0.9;
        let body = ((TAU * self.ph[0]).sin() * 0.65 + (TAU * self.ph[1]).sin() * 0.35) * self.env[0] * body_gain;
        let n = self.rng.next();
        let wires = self.svf[0].tick(n, self.k[4], 1.0).2 * self.env[1] * m[1];
        let snap = n * self.env[2] * m[2] * 0.9;
        let mut clap = 0.0;
        if self.bursts > 0 {
            if self.fired < self.bursts && self.t >= self.fired * self.spacing {
                self.env[4] = 1.0;
                self.fired += 1;
            }
            let tail = if self.fired >= self.bursts { self.env[1] * 0.5 } else { 0.0 };
            clap = self.svf[1].tick(n, self.k[5], 0.8).1 * (self.env[4] + tail) * m[3] * 2.2;
        }
        let mut x = body + wires + snap + clap;
        let boxy = self.svf[2].tick(x, self.k[7], 1.0).1;
        x += boxy * (m[5] - 0.35) * 2.0;
        self.env[0] *= self.k[0];
        self.env[1] *= self.k[1];
        self.env[2] *= self.k[2];
        self.env[3] *= self.k[6];
        self.env[4] *= self.k[3];
        drive(x * 0.8, m[7])
    }

    fn metal(&mut self) -> f32 {
        let m = self.m;
        let lead = if self.ph[0] < 0.5 { 1.0 } else { -1.0 };
        let mut s = 0.0;
        for i in 0..6 {
            let inc = self.rc[i][0] * (1.0 + m[2] * 0.9 * lead * (i > 0) as u8 as f32);
            self.ph[i] = (self.ph[i] + inc.max(0.0)).fract();
            s += if self.ph[i] < 0.5 { 1.0 } else { -1.0 };
        }
        s = s / 6.0 + self.rng.next() * m[7] * 0.5;
        let band = self.svf[0].tick(s, self.k[1], self.k[2]).1;
        let x = band + s * 0.3;
        let hp = self.svf[1].tick(x, self.k[3], 1.0).2;
        let hp = self.svf[2].tick(hp, self.k[3], 1.0).2;
        let attack = (self.t as f32 / (0.0003 * self.sr)).min(1.0);
        self.env[0] *= self.k[0];
        // A resonant band can ring up past full scale; round it off rather than clip.
        soft(hp * self.env[0] * attack * 1.3)
    }

    fn fm(&mut self) -> f32 {
        let (m, sr) = (self.m, self.sr);
        let drop = 1.0 + 0.25 * self.env[2];
        self.ph[1] = (self.ph[1] + self.hz * self.k[3] * drop / sr).fract();
        let modulator = (TAU * self.ph[1] + self.fb * m[5] * 1.3).sin();
        self.fb = modulator;
        self.ph[0] = (self.ph[0] + self.hz * drop / sr).fract();
        let mut x = (TAU * self.ph[0] + modulator * m[2] * 9.0 * self.env[1]).sin();
        if m[4] > 0.01 {
            x = (x * (1.0 + m[4] * 5.0) * PI * 0.5).sin();
        }
        // A low-pass gate: at full LPG the filter closes as the hit dies away.
        let cutoff = 18000.0 * (1.0 - m[7]) + m[7] * (150.0 + 9000.0 * self.env[0]);
        let y = self.svf[0].tick(x, svf_g(cutoff.min(sr * 0.45), sr), 1.0 / (0.7 + m[7] * 3.0)).0;
        let attack = (self.t as f32 / (0.0004 * sr)).min(1.0);
        self.env[0] *= self.k[0];
        self.env[1] *= self.k[1];
        self.env[2] *= self.k[2];
        soft(y * self.env[0] * attack * 0.9)
    }

    fn modal(&mut self) -> (f32, f32) {
        // A short burst of noise, shaped like a strike, sets the resonators ringing.
        let mut x = 0.0;
        if self.t < self.exc_len.max(1) {
            let p = self.t as f32 / self.exc_len.max(1) as f32;
            x = self.rng.next() * (PI * p).sin() * 3.0 / (self.exc_len.max(1) as f32).sqrt();
        }
        if self.t == 0 {
            x += 1.0;
        }
        let (mut l, mut r) = (0.0, 0.0);
        for i in 0..8 {
            let c = self.rc[i];
            if c[2] == 0.0 {
                continue;
            }
            let y = x * c[2] + c[0] * self.res[i][0] + c[1] * self.res[i][1];
            self.res[i][1] = self.res[i][0];
            self.res[i][0] = y;
            let p = self.pan[i];
            l += y * (1.0 - p).min(1.0);
            r += y * (1.0 + p).min(1.0);
        }
        (soft(l * self.k[0] * 1.3), soft(r * self.k[0] * 1.3))
    }
}

/// Renders one hit of a patch to a sample, to bake it onto a pad: from the strike until it
/// falls below -80 dB, or six seconds at most. Played at normal velocity with no
/// randomness, so the same patch always bakes the same.
pub fn render(p: &DrumParams, sr: f32, scale: u16, bpm: f32) -> SampleData {
    render_capped(p, sr, scale, bpm, 6.0)
}

/// [`render`], stopping after `max_secs`.
pub fn render_capped(p: &DrumParams, sr: f32, scale: u16, bpm: f32, max_secs: f32) -> SampleData {
    let m = p.resolve(0.79, &NO_LOCKS, &mut || 0.0);
    let mut hz = p.base_hz(&m);
    if p.key_lock {
        hz = snap_hz(hz, scale);
    }
    let cut = sync_ticks(p.sync).map_or(0, |t| (t as f32 / PPQ as f32 * 60.0 / bpm.max(20.0) * sr) as u32);
    let mut voice = DrumVoice::default();
    voice.start(0, 0.0, 0.79, 0, 0, p, m, hz, sr, cut, 0x1234_5679);
    let (mut l, mut r) = (Vec::new(), Vec::new());
    let block = 512;
    while voice.active && l.len() < (sr * max_secs) as usize {
        let at = l.len();
        l.resize(at + block, 0.0);
        r.resize(at + block, 0.0);
        voice.render(&mut l[at..], &mut r[at..], 1.0, 1.0);
    }
    // Trim to the last sound above -80 dB and fade the last few milliseconds.
    let last = l.iter().zip(&r).rposition(|(a, b)| a.abs().max(b.abs()) > 1e-4).map_or(1, |i| i + 1);
    l.truncate(last);
    r.truncate(last);
    let fade = ((sr * 0.003) as usize).min(last);
    for i in 0..fade {
        let g = i as f32 / fade as f32;
        l[last - 1 - i] *= g;
        r[last - 1 - i] *= g;
    }
    SampleData::new(l, r)
}

/// A factory sound: a name and a patch.
pub struct Preset {
    pub name: &'static str,
    pub params: DrumParams,
}

fn preset(name: &'static str, engine: DrumEngine, m: [f32; 8]) -> Preset {
    Preset { name, params: patch(engine, m) }
}

/// Every factory sound, by family.
pub fn presets() -> Vec<Preset> {
    use DrumEngine::*;
    let open = |mut p: Preset| {
        p.params.open = true;
        p
    };
    vec![
        preset("808 Kick", Kick, [0.35, 0.35, 0.45, 0.15, 0.1, 0.3, 0.75, 0.45]),
        preset("808 Boom", Kick, [0.28, 0.3, 0.5, 0.05, 0.15, 0.45, 0.95, 0.35]),
        preset("909 Kick", Kick, [0.45, 0.6, 0.3, 0.55, 0.35, 0.0, 0.45, 0.6]),
        preset("Boom Bap Kick", Kick, [0.38, 0.45, 0.35, 0.4, 0.25, 0.15, 0.5, 0.35]),
        preset("Techno Rumble", Kick, [0.3, 0.5, 0.4, 0.3, 0.8, 0.2, 0.65, 0.4]),
        preset("808 Snare", Snare, [0.4, 0.55, 0.3, 0.0, 0.0, 0.4, 0.35, 0.1]),
        preset("909 Snare", Snare, [0.5, 0.75, 0.6, 0.0, 0.0, 0.45, 0.4, 0.3]),
        preset("Boom Bap Snare", Snare, [0.35, 0.65, 0.5, 0.0, 0.0, 0.6, 0.45, 0.35]),
        preset("Rim", Snare, [0.85, 0.1, 0.9, 0.0, 0.0, 0.7, 0.05, 0.4]),
        preset("808 Clap", Snare, [0.4, 0.4, 0.2, 0.75, 0.6, 0.4, 0.45, 0.1]),
        preset("Tight Clap", Snare, [0.4, 0.3, 0.4, 0.95, 0.4, 0.5, 0.3, 0.3]),
        preset("808 Closed Hat", Metal, [0.5, 0.5, 0.2, 0.3, 0.55, 0.35, 0.5, 0.4]),
        open(preset("808 Open Hat", Metal, [0.5, 0.5, 0.2, 0.3, 0.55, 0.35, 0.55, 0.4])),
        preset("909 Closed Hat", Metal, [0.6, 0.7, 0.4, 0.2, 0.65, 0.3, 0.6, 0.8]),
        open(preset("909 Open Hat", Metal, [0.6, 0.7, 0.4, 0.2, 0.65, 0.3, 0.6, 0.8])),
        open(preset("Ride", Metal, [0.35, 0.8, 0.5, 0.6, 0.35, 0.5, 0.85, 0.5])),
        open(preset("Cowbell", Metal, [0.75, 0.15, 0.0, 0.7, 0.05, 0.6, 0.35, 0.0])),
        preset("Shaker", Metal, [0.9, 0.9, 0.8, 0.1, 0.7, 0.45, 0.4, 1.0]),
        preset("Low Tom", Fm, [0.185, 0.067, 0.15, 0.3, 0.0, 0.0, 0.5, 0.3]),
        preset("Mid Tom", Fm, [0.3, 0.067, 0.15, 0.3, 0.0, 0.0, 0.45, 0.3]),
        preset("High Tom", Fm, [0.42, 0.067, 0.15, 0.3, 0.0, 0.0, 0.4, 0.3]),
        preset("Conga", Fm, [0.52, 0.067, 0.2, 0.2, 0.05, 0.0, 0.45, 0.5]),
        preset("Bongo", Fm, [0.64, 0.133, 0.3, 0.15, 0.0, 0.1, 0.35, 0.6]),
        preset("Laser Zap", Fm, [0.6, 0.0, 0.9, 0.55, 0.3, 0.4, 0.4, 0.2]),
        preset("Wood Click", Fm, [0.85, 0.4, 0.5, 0.05, 0.2, 0.0, 0.1, 0.7]),
        preset("Glitch", Fm, [0.5, 0.9, 1.0, 0.3, 0.8, 0.8, 0.25, 0.3]),
        preset("Woodblock", Modal, [0.05, 0.0, 0.6, 0.3, 0.6, 0.3, 0.2, 0.2]),
        preset("Clave", Modal, [0.02, 0.15, 0.78, 0.2, 0.4, 0.2, 0.25, 0.1]),
        preset("Hand Drum", Modal, [0.25, 0.0, 0.35, 0.5, 0.7, 0.35, 0.35, 0.3]),
        preset("Tabla", Modal, [0.15, 0.1, 0.45, 0.6, 0.3, 0.12, 0.5, 0.2]),
        preset("Tuned Bar", Modal, [0.04, 0.7, 0.5, 0.3, 0.5, 0.15, 0.6, 0.3]),
        preset("Metal Pipe", Modal, [0.03, 1.0, 0.55, 0.6, 0.2, 0.25, 0.75, 0.5]),
    ]
}

pub fn preset_named(name: &str) -> Option<DrumParams> {
    presets().into_iter().find(|p| p.name == name).map(|p| p.params)
}

/// A bank of sixteen sounds. Pad 1 (index 0) is bottom left, as on the matrix. The hats
/// named in `choke` cut each other off.
pub struct Kit {
    pub name: &'static str,
    pub pads: [&'static str; 16],
    /// Turns the factory sounds into this kit's flavour.
    pub flavour: fn(&mut DrumParams),
}

fn plain(_: &mut DrumParams) {}

/// Darker, dirtier and a little loose: tape-worn drums.
fn lofi(p: &mut DrumParams) {
    match p.engine {
        DrumEngine::Kick => {
            p.m[7] *= 0.6;
            p.m[4] = (p.m[4] + 0.2).min(1.0);
        }
        DrumEngine::Snare => p.m[7] = (p.m[7] + 0.25).min(1.0),
        DrumEngine::Metal => {
            p.m[4] = (p.m[4] - 0.15).max(0.0);
            p.m[7] *= 0.5;
        }
        DrumEngine::Fm => p.m[7] = (p.m[7] + 0.3).min(1.0),
        DrumEngine::Modal => p.m[3] *= 0.6,
    }
    p.rnd = [0.06; 8];
}

/// Noisier and broken: more drive, more FM, and every hit a little different.
fn glitch(p: &mut DrumParams) {
    match p.engine {
        DrumEngine::Fm => p.m[5] = (p.m[5] + 0.3).min(1.0),
        DrumEngine::Metal => p.m[2] = (p.m[2] + 0.4).min(1.0),
        DrumEngine::Kick => p.m[4] = (p.m[4] + 0.4).min(1.0),
        _ => {}
    }
    p.rnd = [0.2; 8];
}

pub fn kits() -> Vec<Kit> {
    vec![
        Kit {
            name: "808 Analog",
            pads: [
                "808 Kick", "808 Snare", "808 Clap", "Rim", "808 Closed Hat", "808 Open Hat", "Cowbell", "Ride", "Low Tom", "Mid Tom", "High Tom", "Conga",
                "808 Boom", "Clave", "Shaker", "Laser Zap",
            ],
            flavour: plain,
        },
        Kit {
            name: "909 Analog",
            pads: [
                "909 Kick", "909 Snare", "Tight Clap", "Rim", "909 Closed Hat", "909 Open Hat", "Ride", "Cowbell", "Low Tom", "Mid Tom", "High Tom", "Bongo",
                "Techno Rumble", "808 Clap", "Shaker", "Wood Click",
            ],
            flavour: plain,
        },
        Kit {
            name: "Boom Bap",
            pads: [
                "Boom Bap Kick", "Boom Bap Snare", "808 Clap", "Rim", "808 Closed Hat", "808 Open Hat", "Shaker", "Ride", "Low Tom", "Mid Tom", "Conga", "Bongo",
                "808 Kick", "909 Snare", "Woodblock", "Tabla",
            ],
            flavour: plain,
        },
        Kit {
            name: "Lo-Fi Dust",
            pads: [
                "Boom Bap Kick", "Boom Bap Snare", "Tight Clap", "Rim", "808 Closed Hat", "808 Open Hat", "Shaker", "Cowbell", "Low Tom", "Hand Drum", "Conga",
                "Woodblock", "808 Boom", "808 Snare", "Clave", "Tuned Bar",
            ],
            flavour: lofi,
        },
        Kit {
            name: "FM Perc",
            pads: [
                "909 Kick", "808 Snare", "808 Clap", "Wood Click", "909 Closed Hat", "909 Open Hat", "Cowbell", "Laser Zap", "Low Tom", "Mid Tom", "High Tom", "Conga",
                "Bongo", "Glitch", "Shaker", "Rim",
            ],
            flavour: plain,
        },
        Kit {
            name: "Physical / Modal",
            pads: [
                "808 Kick", "Boom Bap Snare", "Tight Clap", "Clave", "808 Closed Hat", "808 Open Hat", "Metal Pipe", "Tuned Bar", "Hand Drum", "Tabla", "Woodblock",
                "Conga", "808 Boom", "Rim", "Shaker", "Ride",
            ],
            flavour: plain,
        },
        Kit {
            name: "Glitch / Noise",
            pads: [
                "Techno Rumble", "909 Snare", "Tight Clap", "Glitch", "909 Closed Hat", "909 Open Hat", "Laser Zap", "Metal Pipe", "Low Tom", "Wood Click", "Bongo",
                "Shaker", "808 Boom", "Rim", "Cowbell", "Tuned Bar",
            ],
            flavour: glitch,
        },
    ]
}

/// Whether a kit sound is one of a pair of hats that should choke each other.
pub fn is_hat(name: &str) -> bool {
    name.contains("Hat")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;

    fn peak(s: &SampleData) -> f32 {
        s.l.iter().chain(&s.r).fold(0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn every_preset_sounds_ends_and_stays_in_range() {
        for p in presets() {
            let s = render(&p.params, SR, 0xfff, 90.0);
            let level = peak(&s);
            let secs = s.len() as f32 / SR;
            assert!(level > 0.1 && level < 1.6, "{}: peak {level}", p.name);
            assert!(secs > 0.01 && secs < 6.0, "{}: lasts {secs} s", p.name);
            assert!(s.l.iter().chain(&s.r).all(|v| v.is_finite()), "{}: not finite", p.name);
        }
    }

    /// Not a pass/fail test: the level and length of every factory sound, for tuning them.
    /// `cargo test --release drum_levels -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn drum_levels() {
        for p in presets() {
            let s = render(&p.params, SR, 0xfff, 90.0);
            let rms = (s.l.iter().map(|v| v * v).sum::<f32>() / s.len().max(1) as f32).sqrt();
            println!("{:16} peak {:5.2}  rms {:5.3}  {:5.2} s", p.name, peak(&s), rms, s.len() as f32 / SR);
        }
    }

    #[test]
    fn decay_macro_lengthens_the_tail_and_sync_cuts_it() {
        for engine in DrumEngine::ALL {
            let mut p = patch(engine, [0.5; 8]);
            p.open = true;
            p.m[6] = 0.2;
            let short = render(&p, SR, 0xfff, 120.0).len();
            p.m[6] = 0.8;
            let long = render(&p, SR, 0xfff, 120.0).len();
            assert!(long > short, "{engine:?}: {short} then {long}");
            // A sixteenth at 120 BPM is 125 ms.
            p.sync = 1;
            let cut = render(&p, SR, 0xfff, 120.0).len() as f32 / SR;
            assert!(cut < 0.14, "{engine:?}: synced tail {cut} s");
        }
    }

    #[test]
    fn key_lock_lands_on_the_scale() {
        // A minor has no G#: just above G# goes to A, just below to G.
        let a_minor = 0b1010_1011_0101u16;
        assert!((snap_hz(425.0, a_minor) - 440.0).abs() < 0.01);
        assert!((snap_hz(395.0, a_minor) - 392.0).abs() < 0.5);
    }

    #[test]
    fn locks_and_velocity_move_the_macros() {
        let mut p = DrumParams::default();
        p.vel[3] = 1.0;
        let soft = p.resolve(0.4, &NO_LOCKS, &mut || 0.0);
        let hard = p.resolve(1.0, &NO_LOCKS, &mut || 0.0);
        assert!(hard[3] > p.m[3] && soft[3] < p.m[3]);
        let mut locks = NO_LOCKS;
        locks[6] = lock_value(0.1);
        assert!((p.resolve(0.79, &locks, &mut || 0.0)[6] - 0.1).abs() < 0.01);
    }

    #[test]
    fn every_kit_names_real_sounds() {
        for kit in kits() {
            for name in kit.pads {
                assert!(preset_named(name).is_some(), "{}: no sound called {name}", kit.name);
            }
        }
    }
}
