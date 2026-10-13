//! Internal synth engines that MIDI pads can target.

use crate::fx::{Svf, svf_g};
use crate::music::midi_to_hz;
use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum SynthEngine {
    Sub808,
    Saw,
    Square,
    Keys,
}

impl SynthEngine {
    pub const ALL: [SynthEngine; 4] =
        [SynthEngine::Sub808, SynthEngine::Saw, SynthEngine::Square, SynthEngine::Keys];

    pub fn name(self) -> &'static str {
        match self {
            SynthEngine::Sub808 => "808 Sub",
            SynthEngine::Saw => "Saw Lead",
            SynthEngine::Square => "Square Pluck",
            SynthEngine::Keys => "FM Keys",
        }
    }

    /// Octave offset that puts the piano roll's middle rows in the engine's useful range.
    pub fn default_octave(self) -> i8 {
        match self {
            SynthEngine::Sub808 => -2,
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct SynthParams {
    pub engine: SynthEngine,
    pub octave: i8,
    /// 0..1: tail length.
    pub decay: f32,
    /// 0..1: brightness / pitch-drop amount.
    pub tone: f32,
    pub drive: f32,
}

impl Default for SynthParams {
    fn default() -> Self {
        SynthParams { engine: SynthEngine::Sub808, octave: -2, decay: 0.5, tone: 0.5, drive: 0.3 }
    }
}

#[derive(Clone, Copy, Default)]
pub struct SynthVoice {
    pub active: bool,
    pub pad: u16,
    pub key: u32,
    pub note: f32,
    pub vel: f32,
    /// Samples to wait inside the next block before sounding (sample-accurate sequencing).
    pub offset: u32,
    /// Samples until automatic note-off; negative means held by the player.
    pub gate_left: i64,
    pub released: bool,
    pub age: u64,
    phase: f32,
    phase2: f32,
    t: f32,
    env: f32,
    svf: Svf,
}

#[inline]
fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        2.0 * x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + 2.0 * x + 1.0
    } else {
        0.0
    }
}

impl SynthVoice {
    pub fn start(&mut self, pad: u16, key: u32, note: f32, vel: f32, offset: u32, gate: i64, age: u64) {
        *self = SynthVoice {
            active: true,
            pad,
            key,
            note,
            vel,
            offset,
            gate_left: gate,
            age,
            ..Default::default()
        };
    }

    pub fn level(&self) -> f32 {
        self.env * self.vel
    }

    /// Adds this voice (mono) into `out`.
    pub fn render(&mut self, p: &SynthParams, sr: f32, out: &mut [f32]) {
        let n = out.len();
        let start = (self.offset as usize).min(n);
        self.offset -= start as u32;
        let f0 = midi_to_hz(self.note);
        let attack = 1.0 / (0.002 * sr);
        // Per-sample multipliers for the held tail and the release.
        let (hold_t, rel_t) = match p.engine {
            SynthEngine::Sub808 => (0.25 + p.decay * 5.0, 0.04 + p.decay * 0.5),
            SynthEngine::Saw => (60.0, 0.03 + p.decay * 1.2),
            SynthEngine::Square => (0.15 + p.decay * 2.5, 0.03 + p.decay * 0.6),
            SynthEngine::Keys => (0.6 + p.decay * 5.0, 0.05 + p.decay * 0.8),
        };
        let hold_c = (-6.9 / (hold_t * sr)).exp();
        let rel_c = (-6.9 / (rel_t * sr)).exp();
        let drive = 1.0 + p.drive * 7.0;
        let drive_norm = 1.0 / drive.tanh();

        for o in out.iter_mut().take(n).skip(start) {
            if self.gate_left >= 0 {
                if self.gate_left == 0 {
                    self.released = true;
                }
                self.gate_left -= 1;
            }
            // Envelope: 2 ms linear attack, then exponential.
            let attacking = self.t < 0.002 * sr && !self.released;
            if attacking {
                self.env = (self.env + attack).min(1.0);
            } else if self.released {
                self.env *= rel_c;
            } else {
                self.env *= hold_c;
            }
            let secs = self.t / sr;
            self.t += 1.0;

            let s = match p.engine {
                SynthEngine::Sub808 => {
                    let f = f0 * (1.0 + (0.5 + p.tone * 5.0) * (-secs / 0.018).exp());
                    self.phase = (self.phase + f / sr).fract();
                    ((self.phase * TAU).sin() * drive).tanh() * drive_norm
                }
                SynthEngine::Saw | SynthEngine::Square => {
                    let dt = f0 / sr;
                    let dt2 = dt * 1.006;
                    self.phase = (self.phase + dt).fract();
                    self.phase2 = (self.phase2 + dt2).fract();
                    let raw = if p.engine == SynthEngine::Saw {
                        let a = 2.0 * self.phase - 1.0 - poly_blep(self.phase, dt);
                        let b = 2.0 * self.phase2 - 1.0 - poly_blep(self.phase2, dt2);
                        (a + b) * 0.5
                    } else {
                        let mut a = if self.phase < 0.5 { 1.0 } else { -1.0 };
                        a += poly_blep(self.phase, dt);
                        a -= poly_blep((self.phase + 0.5).fract(), dt);
                        a
                    };
                    let sweep = (-secs / (0.05 + p.decay * 0.4)).exp();
                    let fc = (120.0 * (100.0f32).powf(p.tone) * (1.0 + 3.0 * sweep)).min(sr * 0.45);
                    let (lo, _, _) = self.svf.tick(raw, svf_g(fc, sr), 0.6);
                    (lo * drive).tanh() * drive_norm * 0.8
                }
                SynthEngine::Keys => {
                    self.phase = (self.phase + f0 / sr).fract();
                    self.phase2 = (self.phase2 + f0 * 2.0 / sr).fract();
                    let index = (0.3 + p.tone * 3.5) * (-secs / 0.25).exp() * self.vel;
                    let m = (self.phase2 * TAU).sin() * index;
                    ((self.phase * TAU + m).sin() * drive).tanh() * drive_norm * 0.8
                }
            };
            *o += s * self.env * self.vel;

            if !attacking && self.env < 1e-4 {
                self.active = false;
                break;
            }
        }
    }
}
