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
    /// The native drum synth, playing the patch in [`PadCfg::drum`].
    Drum,
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
    /// The drum synth patch, used while the pad is a drum pad.
    #[serde(default)]
    pub drum: crate::drum::DrumParams,
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
            drum: crate::drum::DrumParams::default(),
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
    /// Drum synth macros locked to this step; see [`crate::drum::Locks`].
    #[serde(default = "no_locks", skip_serializing_if = "unlocked")]
    pub locks: crate::drum::Locks,
}

fn no_locks() -> crate::drum::Locks {
    crate::drum::NO_LOCKS
}

fn unlocked(l: &crate::drum::Locks) -> bool {
    *l == crate::drum::NO_LOCKS
}

impl Note {
    pub fn new(start: u32, len: u32, pitch: u8, vel: u8) -> Note {
        Note { start, len, pitch, vel, locks: crate::drum::NO_LOCKS }
    }

    pub fn has_locks(&self) -> bool {
        !unlocked(&self.locks)
    }
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

/// What an automation lane moves. Lane values are 0..1, spread over the control's range.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum AutoTarget {
    /// One of a bus effect's three knobs (0-2) or two sliders (3-4).
    BusParam { bus: u8, slot: u8, param: u8 },
    BusLevel(u8),
    BusPan(u8),
    PadVol(u16),
    PadPan(u16),
    PadCutoff(u16),
    /// One of a drum pad's eight knobs, [`crate::drum::DrumParams::m`].
    DrumMacro { pad: u16, m: u8 },
    /// A parameter of the plugin a pad plays, by the plugin's own id. Lane values are the
    /// parameter's normalized range.
    PadPlugin { pad: u16, id: u32 },
    /// A parameter of a bus effect plugin; `slot` is `bus * 3 + slot`.
    FxPlugin { slot: u8, id: u32 },
}

/// Highest level a bus or pad reaches; a lane value of 1 is this.
pub const LEVEL_MAX: f32 = 1.5;

impl AutoTarget {
    /// The control's own value for a lane value.
    pub fn scale_up(self, v: f32) -> f32 {
        match self {
            AutoTarget::BusLevel(_) | AutoTarget::PadVol(_) => v * LEVEL_MAX,
            AutoTarget::BusPan(_) | AutoTarget::PadPan(_) => v * 2.0 - 1.0,
            _ => v,
        }
    }

    /// The lane value for a control's own value.
    pub fn scale_down(self, v: f32) -> f32 {
        match self {
            AutoTarget::BusLevel(_) | AutoTarget::PadVol(_) => v / LEVEL_MAX,
            AutoTarget::BusPan(_) | AutoTarget::PadPan(_) => (v + 1.0) * 0.5,
            _ => v,
        }
        .clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct AutoPoint {
    /// Ticks from the start of the loop.
    pub tick: u32,
    pub value: f32,
    /// Bend of the segment that leaves this point, -1..1: 0 is a straight line, positive
    /// values ease in (slow start, fast finish) and negative ones ease out.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub curve: f32,
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

impl AutoPoint {
    pub fn new(tick: u32, value: f32) -> Self {
        AutoPoint { tick, value, curve: 0.0 }
    }
}

/// Where a segment with bend `curve` is at fraction `t` of the way along it, both 0..1.
pub fn bend(t: f32, curve: f32) -> f32 {
    if curve.abs() < 1e-3 {
        return t;
    }
    // A power curve whose exponent runs from 1/8 to 8; bent one way, then mirrored for
    // the other so equal and opposite bends look alike.
    let e = 8f32.powf(curve.abs());
    if curve > 0.0 { t.powf(e) } else { 1.0 - (1.0 - t).powf(e) }
}

/// One control's movement through a loop: lines (straight or bent) between points,
/// holding the first point's value before it and the last one's after it.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct AutoLane {
    pub target: AutoTarget,
    /// In order of tick.
    pub points: Vec<AutoPoint>,
}

impl AutoLane {
    pub fn value_at(&self, tick: f64) -> Option<f32> {
        let pts = &self.points;
        let first = pts.first()?;
        if tick <= first.tick as f64 {
            return Some(first.value);
        }
        let after = pts.iter().position(|p| p.tick as f64 > tick);
        Some(match after {
            None => pts[pts.len() - 1].value,
            Some(i) => {
                let (a, b) = (pts[i - 1], pts[i]);
                let t = ((tick - a.tick as f64) / (b.tick - a.tick).max(1) as f64) as f32;
                a.value + (b.value - a.value) * bend(t.clamp(0.0, 1.0), a.curve)
            }
        })
    }
}

/// The lanes of one loop, as the engine holds them.
pub type Automation = Arc<Vec<AutoLane>>;

/// The patterns of a loop that is waiting to take over from the one playing.
pub struct SceneSwap {
    pub index: u32,
    pub len_ticks: u32,
    /// Take over at the next bar line rather than when the playing loop ends.
    pub next_bar: bool,
    /// Every pad that has a pattern in either loop. After the switch this holds the
    /// outgoing patterns, and the whole thing goes back to the UI to be dropped.
    pub patterns: Vec<(u16, Option<Arc<Pattern>>)>,
    /// The incoming loop's automation; after the switch, the outgoing loop's.
    pub automation: Option<Automation>,
}

/// One clip as the engine plays it: a stretch of song time playing a pattern, repeated
/// to fill it, starting `offset` ticks into the pattern; or, for an audio clip, a stretch
/// of a recording, `offset` ticks into it (the pattern is then empty).
pub struct SongClip {
    pub start: u64,
    pub len: u64,
    pub offset: u64,
    pub pattern: Arc<Pattern>,
    pub audio: Option<SongAudio>,
}

/// What an audio clip plays: a recording at the engine's rate, locked to song time.
pub struct SongAudio {
    pub data: Arc<SampleData>,
    /// Linear gain.
    pub gain: f32,
    /// Fades, in ticks from each end of the clip.
    pub fade_in: u64,
    pub fade_out: u64,
    /// Crossfades with the audio clip before and after, in ticks: half of each is
    /// played past the clip's edge, under the other clip, with equal-power curves.
    pub xin: u64,
    pub xout: u64,
}

impl SongClip {
    pub fn end(&self) -> u64 {
        self.start + self.len
    }
}

/// A pad's clips, in order of start and never overlapping.
pub struct SongTrack {
    pub pad: u16,
    pub clips: Vec<SongClip>,
}

/// A stretch of the song that came from a loop, and that loop's automation, which plays
/// through it as the loop goes round (`loop_len` ticks).
pub struct SongSection {
    pub start: u64,
    pub len: u64,
    pub loop_len: u64,
    pub automation: Option<Automation>,
}

/// The arrangement, flattened for the engine: what each pad plays when.
#[derive(Default)]
pub struct ArrangementPlan {
    pub tracks: Vec<SongTrack>,
    /// In order of start.
    pub sections: Vec<SongSection>,
    /// Song-length lanes, in song ticks, over whatever the sections' loops automate.
    pub automation: Option<Automation>,
}

pub enum SongCmd {
    /// The arrangement to play in song mode.
    Plan(Option<Arc<ArrangementPlan>>),
    /// Song mode on (the pads play the arrangement) or off (they play their loop).
    Mode(bool),
    /// Move the song playhead, in ticks.
    Locate(f64),
    /// The loop brace, as (start, end) in ticks.
    Brace(Option<(f64, f64)>),
}

pub enum Cmd {
    Song(SongCmd),
    /// Length of the playing loop; the transport wraps here. 0 = never.
    SceneLen(u32),
    /// Line up the next loop, or cancel the one lined up.
    QueueScene(Option<Box<SceneSwap>>),
    /// Automation for the playing loop.
    SetAutomation(Option<Automation>),
    SetPadPlugin { pad: u16, plugin: Option<Box<PluginRt>> },
    /// `slot` is `bus * 3 + slot`.
    SetFxPlugin { slot: u8, plugin: Option<Box<PluginRt>> },
    /// An analyzer plugin, and the bus it watches (its output, after the fader). What the
    /// plugin puts out is thrown away: it only listens.
    SetAnalyzer { slot: u8, plugin: Option<Box<PluginRt>>, source: u8 },
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
    /// Play the latency test signal on the output. See [`crate::latency`].
    Ping,
    /// Audition a file from the sample browser (`None` stops), at this gain: played
    /// after the master, so it is neither processed nor captured.
    Preview(Option<Arc<SampleData>>, f32),
}

/// Things the audio thread must not free itself; shipped back to the UI to drop.
#[allow(dead_code)] // The payloads exist only to be dropped on the other side.
pub enum Garbage {
    Sample(Arc<SampleData>),
    Pattern(Arc<Pattern>),
    Scene(Box<SceneSwap>),
    Automation(Automation),
    Song(Arc<ArrangementPlan>),
    Input(Consumer<f32>),
    Plugin(Box<PluginRt>),
}

