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
    /// A stack of vocal takes. To the engine it is an audio pad playing the comp; the takes
    /// and the edits live on the UI side.
    Vocal,
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

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum LfoShape {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
}

impl LfoShape {
    pub const ALL: [LfoShape; 4] = [LfoShape::Sine, LfoShape::Triangle, LfoShape::Saw, LfoShape::Square];

    /// The wave at `phase` (in cycles), from -1 to 1. Every shape starts its cycle at or
    /// heading away from zero in the same direction, so switching shape keeps the feel.
    pub fn at(self, phase: f64) -> f32 {
        let p = phase.rem_euclid(1.0) as f32;
        match self {
            LfoShape::Sine => (p * std::f32::consts::TAU).sin(),
            LfoShape::Triangle => 1.0 - 4.0 * ((p + 0.25).fract() - 0.5).abs(),
            LfoShape::Saw => 1.0 - 2.0 * p,
            LfoShape::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }
}

/// Where an LFO is in its cycle, given the engine's two clocks.
pub fn lfo_phase(cfg: &PadCfg, beat_ticks: f64, seconds: f64) -> f64 {
    match crate::music::lfo_sync_ticks(cfg.lfo_sync) {
        Some(ticks) => beat_ticks / ticks,
        None => seconds * cfg.lfo_rate as f64,
    }
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
    /// 0 = thru (straight to master), 1..=4 = bus, 5 = the vocal bus.
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
    #[serde(default)]
    pub lfo_shape: LfoShape,
    /// 0 = free, at `lfo_rate` Hz; otherwise a note value locked to the tempo. See
    /// [`crate::music::lfo_sync`].
    #[serde(default)]
    pub lfo_sync: u8,
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
            lfo_shape: LfoShape::Sine,
            lfo_sync: 0,
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
    /// The three knobs.
    pub params: [f32; 3],
    /// Two further parameters, shown as sliders by the effects that use them.
    #[serde(default)]
    pub sliders: [f32; 2],
    pub on: bool,
}

impl SlotCfg {
    /// A slot holding `kind` at its default settings.
    pub fn new(kind: FxKind, on: bool) -> SlotCfg {
        SlotCfg { kind, params: kind.defaults(), sliders: kind.slider_defaults(), on }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct BusCfg {
    pub level: f32,
    /// -1..1, a balance control: turning it one way brings the other side down.
    #[serde(default)]
    pub pan: f32,
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
            pan: 0.0,
            mute: false,
            out: 0,
            slots: [SlotCfg::new(FxKind::None, true); 3],
            sc_src: 0,
            sc_amount: -0.7,
            sc_release: default_sc_release(),
        }
    }
}

impl BusCfg {
    /// The vocal bus as a new project has it: tuning ready but switched off, levelling on,
    /// and the throw effect waiting with its send down.
    pub fn vocal() -> BusCfg {
        BusCfg {
            slots: [SlotCfg::new(FxKind::Tune, false), SlotCfg::new(FxKind::VocalComp, true), SlotCfg::new(FxKind::Throw, true)],
            ..BusCfg::default()
        }
    }

    /// The bus index keying this one, if sidechaining is in effect on bus `own`.
    pub fn sidechain_source(&self, own: usize) -> Option<usize> {
        let src = (self.sc_src as usize).checked_sub(1)?;
        (src < crate::shared::MASTER && src != own && self.sc_amount != 0.0).then_some(src)
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

/// The vocal bus's fixed front end and its hold on another bus.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct VocalFx {
    /// Gate threshold in dB; at or below -80 the gate is off.
    pub gate_db: f32,
    /// De-esser strength, 0..1.
    pub deess: f32,
    /// Bus (1-4) whose midrange makes room while the vocal sounds; 0 = none.
    pub duck_bus: u8,
}

impl Default for VocalFx {
    fn default() -> Self {
        VocalFx { gate_db: -80.0, deess: 0.0, duck_bus: 0 }
    }
}

/// The patterns of a loop that is waiting to take over from the one playing.
pub struct SceneSwap {
    pub index: u32,
    pub len_ticks: u32,
    /// Take over at the next bar line rather than when the playing loop ends.
    pub next_bar: bool,
    /// Every pad that has a pattern in either loop. After the switch this holds the
    /// outgoing patterns, and the whole thing goes back to the UI to be dropped.
    pub patterns: Vec<(u16, Option<Arc<Pattern>>)>,
}

pub enum Cmd {
    /// Length of the playing loop; the transport wraps here. 0 = never.
    SceneLen(u32),
    /// Line up the next loop, or cancel the one lined up.
    QueueScene(Option<Box<SceneSwap>>),
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
    /// Pitch classes of the project scale, a bit each, C first.
    SetScale(u16),
    SetVocalFx(VocalFx),
    Metronome(bool),
    /// 0 = master, 1..=4 = that bus post-FX, 5 = external input (pre-monitor), 6 = vocal bus.
    RingSource(u8),
    /// Silence one pad: its own voices and anything it triggered.
    StopPad { pad: u16 },
    /// Swap the external-input feed. `ratio` is input rate / engine rate.
    SetInput { rx: Option<Consumer<f32>>, ratio: f32 },
    /// Where the external input is monitored: 0 = off, 1 = thru, 2..=5 = Bus 1-4.
    InputCfg { dest: u8, gain: f32 },
    /// How the input's two channels are treated: 0 = auto (a source on one channel only is
    /// centred), 1 = stereo as it comes, 2 = always summed to mono.
    InputMode(u8),
    /// Note repeat; `ticks` is the repeat interval.
    Roll { on: bool, ticks: u32 },
    /// Silence everything now: voices, held notes and built-in effect tails.
    Panic,
}

/// Things the audio thread must not free itself; shipped back to the UI to drop.
#[allow(dead_code)] // The payloads exist only to be dropped on the other side.
pub enum Garbage {
    Sample(Arc<SampleData>),
    Pattern(Arc<Pattern>),
    Scene(Box<SceneSwap>),
    Input(Consumer<f32>),
    Plugin(Box<PluginRt>),
}

