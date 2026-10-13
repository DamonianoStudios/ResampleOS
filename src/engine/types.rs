//! Plain data the UI and engine exchange: pad and bus settings, patterns, commands.

use super::PluginRt;
use crate::fx::FxKind;
use crate::music::TICKS_PER_BAR;
use crate::sample::SampleData;
use crate::synth::SynthParams;
use rtrb::Consumer;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum PadKind {
    Audio,
    Midi,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum PlayMode {
    OneShot,
    Gate,
    Loop,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum LfoTarget {
    Off,
    Pitch,
    Volume,
    Pan,
    Filter,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum MidiTarget {
    Synth,
    /// Plays another pad's sample chromatically; note 60 is the original pitch.
    Pad(u16),
    /// The VST3 instrument loaded on this pad.
    Plugin,
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct PadCfg {
    pub kind: PadKind,
    pub vol: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    /// 0 = off, 1..=32.
    pub choke: u8,
    /// 0 = thru (straight to master), 1..=4 = bus.
    pub bus: u8,
    /// Semitones, coarse + fine.
    pub pitch: f32,
    pub mode: PlayMode,
    pub start: u32,
    /// 0 = end of sample.
    pub end: u32,
    pub attack: f32,
    pub release: f32,
    pub reverse: bool,
    pub cutoff: f32,
    pub reso: f32,
    pub lfo_rate: f32,
    pub lfo_depth: f32,
    pub lfo_target: LfoTarget,
    pub target: MidiTarget,
    pub synth: SynthParams,
    /// Humanize, 0..1: how far sequenced notes may drift off the grid, and how much their
    /// velocity varies. Applied at playback, differently on every pass; the pattern itself
    /// is not changed.
    #[serde(default)]
    pub human_time: f32,
    #[serde(default)]
    pub human_vel: f32,
}

impl Default for PadCfg {
    fn default() -> Self {
        PadCfg {
            kind: PadKind::Audio,
            vol: 0.8,
            pan: 0.0,
            mute: false,
            solo: false,
            choke: 0,
            bus: 0,
            pitch: 0.0,
            mode: PlayMode::OneShot,
            start: 0,
            end: 0,
            attack: 0.0,
            release: 0.02,
            reverse: false,
            cutoff: 1.0,
            reso: 0.0,
            lfo_rate: 2.0,
            lfo_depth: 0.0,
            lfo_target: LfoTarget::Off,
            target: MidiTarget::Synth,
            synth: SynthParams::default(),
            human_time: 0.0,
            human_vel: 0.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct SlotCfg {
    pub kind: FxKind,
    pub params: [f32; 3],
    pub on: bool,
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct BusCfg {
    pub level: f32,
    pub mute: bool,
    /// 0 = master, otherwise the 1-based number of a later bus.
    pub out: u8,
    pub slots: [SlotCfg; 3],
    /// Sidechain key: 0 = off, 1..=4 = what is being sent into that bus.
    #[serde(default)]
    pub sc_src: u8,
    /// -1..1. Negative ducks this bus when the key sounds; positive holds it down and
    /// lets it swell up with the key.
    #[serde(default)]
    pub sc_amount: f32,
    /// Seconds for the effect to let go after the key stops.
    #[serde(default = "default_sc_release")]
    pub sc_release: f32,
}

fn default_sc_release() -> f32 {
    0.18
}

impl Default for BusCfg {
    fn default() -> Self {
        BusCfg {
            level: 1.0,
            mute: false,
            out: 0,
            slots: [SlotCfg { kind: FxKind::None, params: [0.0; 3], on: true }; 3],
            sc_src: 0,
            sc_amount: -0.7,
            sc_release: default_sc_release(),
        }
    }
}

impl BusCfg {
    /// The bus index keying this one, if sidechaining is in effect on bus `own`.
    pub fn sidechain_source(&self, own: usize) -> Option<usize> {
        let src = (self.sc_src as usize).checked_sub(1)?;
        (src < 4 && src != own && self.sc_amount != 0.0).then_some(src)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Note {
    pub start: u32,
    pub len: u32,
    pub pitch: u8,
    pub vel: u8,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Pattern {
    pub len_ticks: u32,
    pub notes: Vec<Note>,
}

impl Default for Pattern {
    fn default() -> Self {
        Pattern { len_ticks: TICKS_PER_BAR * 2, notes: Vec::new() }
    }
}

pub enum Cmd {
    SetPadPlugin { pad: u16, plugin: Option<Box<PluginRt>> },
    /// `slot` is `bus * 3 + slot`.
    SetFxPlugin { slot: u8, plugin: Option<Box<PluginRt>> },
    PadOn { pad: u16, vel: f32, semis: f32, key: u32 },
    PadOff { pad: u16, key: u32 },
    SetPadCfg { pad: u16, cfg: PadCfg },
    SetSample { pad: u16, sample: Option<Arc<SampleData>> },
    SetPattern { pad: u16, pattern: Option<Arc<Pattern>> },
    SetBus { bus: u8, cfg: BusCfg },
    Play(bool),
    SetBpm(f32),
    SetSwing(f32),
    Metronome(bool),
    /// 0 = master, 1..=4 = that bus post-FX, 5 = external input (pre-monitor).
    RingSource(u8),
    /// Silence one pad: its own voices and anything it triggered.
    StopPad { pad: u16 },
    /// Swap the external-input feed. `ratio` is input rate / engine rate.
    SetInput { rx: Option<Consumer<f32>>, ratio: f32 },
    /// Where the external input is monitored: 0 = off, 1 = thru, 2..=5 = Bus 1-4.
    InputCfg { dest: u8, gain: f32 },
    /// Note repeat; `ticks` is the repeat interval.
    Roll { on: bool, ticks: u32 },
    Panic,
}

/// Things the audio thread must not free itself; shipped back to the UI to drop.
#[allow(dead_code)] // The payloads exist only to be dropped on the other side.
pub enum Garbage {
    Sample(Arc<SampleData>),
    Pattern(Arc<Pattern>),
    Input(Consumer<f32>),
    Plugin(Box<PluginRt>),
}

