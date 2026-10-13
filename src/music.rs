//! Keys, scales and the tick grid shared by the sequencer and the piano roll.

use serde::{Deserialize, Serialize};

/// Sequencer resolution: ticks per quarter note.
pub const PPQ: u32 = 480;
pub const TICKS_PER_BAR: u32 = PPQ * 4;
pub const TICKS_PER_16TH: u32 = PPQ / 4;

pub const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Scale {
    Major,
    Minor,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    HarmonicMinor,
    PentMajor,
    PentMinor,
    Blues,
    Chromatic,
}

impl Scale {
    pub const ALL: [Scale; 11] = [
        Scale::Major,
        Scale::Minor,
        Scale::Dorian,
        Scale::Phrygian,
        Scale::Lydian,
        Scale::Mixolydian,
        Scale::HarmonicMinor,
        Scale::PentMajor,
        Scale::PentMinor,
        Scale::Blues,
        Scale::Chromatic,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Scale::Major => "Major",
            Scale::Minor => "Minor",
            Scale::Dorian => "Dorian",
            Scale::Phrygian => "Phrygian",
            Scale::Lydian => "Lydian",
            Scale::Mixolydian => "Mixolydian",
            Scale::HarmonicMinor => "Harm Minor",
            Scale::PentMajor => "Pent Major",
            Scale::PentMinor => "Pent Minor",
            Scale::Blues => "Blues",
            Scale::Chromatic => "Chromatic",
        }
    }

    pub fn intervals(self) -> &'static [i32] {
        match self {
            Scale::Major => &[0, 2, 4, 5, 7, 9, 11],
            Scale::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Scale::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Scale::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Scale::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            Scale::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Scale::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Scale::PentMajor => &[0, 2, 4, 7, 9],
            Scale::PentMinor => &[0, 3, 5, 7, 10],
            Scale::Blues => &[0, 3, 5, 6, 7, 10],
            Scale::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Key {
    /// Pitch class 0..12, C = 0.
    pub root: u8,
    pub scale: Scale,
}

impl Key {
    pub fn contains(self, note: i32) -> bool {
        let pc = (note - self.root as i32).rem_euclid(12);
        self.scale.intervals().contains(&pc)
    }

    /// Nearest in-scale note; ties resolve downward.
    pub fn snap(self, note: i32) -> i32 {
        for d in 0..7 {
            if self.contains(note - d) {
                return note - d;
            }
            if self.contains(note + d) {
                return note + d;
            }
        }
        note
    }

    /// Semitone offset of scale degree `idx` counted from the root (idx may be negative).
    pub fn degree_semis(self, idx: i32) -> i32 {
        let iv = self.scale.intervals();
        let n = iv.len() as i32;
        idx.div_euclid(n) * 12 + iv[idx.rem_euclid(n) as usize]
    }

    pub fn name(self) -> String {
        format!("{} {}", NOTE_NAMES[self.root as usize % 12], self.scale.name())
    }

    /// Smart Conform: given a detected pitch (fractional MIDI note) and a desired shift,
    /// returns the shift that lands the result on the nearest active scale degree.
    pub fn smart_shift(self, detected: f32, shift: f32) -> f32 {
        let target = detected + shift;
        let base = target.floor() as i32;
        let mut best = shift;
        let mut best_d = f32::MAX;
        for cand in (base - 6)..=(base + 7) {
            if !self.contains(cand) {
                continue;
            }
            let d = (target - cand as f32).abs();
            if d < best_d {
                best_d = d;
                best = cand as f32 - detected;
            }
        }
        best
    }
}

pub fn note_name(note: i32) -> String {
    format!("{}{}", NOTE_NAMES[note.rem_euclid(12) as usize], note.div_euclid(12) - 1)
}

pub fn is_black_key(note: i32) -> bool {
    matches!(note.rem_euclid(12), 1 | 3 | 6 | 8 | 10)
}

pub fn midi_to_hz(note: f32) -> f32 {
    440.0 * 2f32.powf((note - 69.0) / 12.0)
}

pub fn hz_to_midi(hz: f32) -> f32 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

/// MPC-style swing: warps straight tick time so every second 16th lands at `swing`
/// (0.5 = straight, 0.75 = hardest) of its 8th-note pair. Piecewise linear, so
/// microtimed notes between grid lines move proportionally.
pub fn swing_tick(t: f64, swing: f32) -> f64 {
    let pair = (TICKS_PER_16TH * 2) as f64;
    let half = TICKS_PER_16TH as f64;
    let s = swing.clamp(0.5, 0.75) as f64;
    let base = (t / pair).floor() * pair;
    let x = t - base;
    let w = if x < half {
        x * (s * pair) / half
    } else {
        s * pair + (x - half) * ((1.0 - s) * pair) / half
    };
    base + w
}

/// Bank label: A..Z, AA, AB, ...
pub fn bank_name(mut idx: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (idx % 26) as u8);
        if idx < 26 {
            break;
        }
        idx = idx / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap()
}

pub fn pad_label(pad: usize) -> String {
    format!("{}{:02}", bank_name(pad / 16), pad % 16 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bank_names() {
        assert_eq!(bank_name(0), "A");
        assert_eq!(bank_name(25), "Z");
        assert_eq!(bank_name(26), "AA");
        assert_eq!(bank_name(27), "AB");
        assert_eq!(pad_label(16), "B01");
    }

    #[test]
    fn swing_is_monotonic_and_anchored() {
        assert_eq!(swing_tick(0.0, 0.66), 0.0);
        assert_eq!(swing_tick(240.0, 0.66), 240.0);
        assert!((swing_tick(120.0, 0.75) - 180.0).abs() < 1e-9);
        assert_eq!(swing_tick(120.0, 0.5), 120.0);
    }

    #[test]
    fn smart_shift_lands_in_scale() {
        // E major root conformed to C minor: -4 lands E on C.
        let k = Key { root: 0, scale: Scale::Minor };
        assert_eq!(k.smart_shift(64.0, -4.0), -4.0);
        // E shifted by -3 would be C#, not in C minor; ties resolve to the lower degree, C.
        assert_eq!(k.smart_shift(64.0, -3.0), -4.0);
        assert_eq!(k.snap(61), 60);
    }
}
