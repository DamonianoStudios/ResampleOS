//! The small types that describe what the app is showing and doing: tabs, modes, the
//! state of each editor. Project data lives in `model.rs`; the engine's types in `engine`.

use super::*;

/// Computer-keyboard pad grid, bottom row first (pads 1-4), matching the 4x4 on screen.
pub const PAD_KEYS: [egui::Key; 16] = {
    use egui::Key::*;
    [N, Comma, Period, Slash, H, J, K, L, Y, U, I, O, Num6, Num7, Num8, Num9]
};
pub const PAD_KEY_LABELS: [&str; 16] =
    ["N", ",", ".", "/", "H", "J", "K", "L", "Y", "U", "I", "O", "6", "7", "8", "9"];

pub const ROLL_DIVS: [(&str, u32); 6] =
    [("1/8", 240), ("1/8T", 160), ("1/16", 120), ("1/16T", 80), ("1/32", 60), ("1/32T", 40)];
pub const SNAPS: [(&str, u32); 7] =
    [("Off", 1), ("1/4", 480), ("1/8", 240), ("1/16", 120), ("1/32", 60), ("1/8T", 160), ("1/16T", 80)];

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Wave,
    Roll,
    Steps,
    Mod,
    Vocal,
    Drum,
}

#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConformMode {
    Strict,
    Smart,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Resample {
    Idle,
    /// Waiting for a pad hit to choose the destination.
    Armed,
    Recording { target: usize, start: u64, bar_aligned: bool },
}

#[derive(Clone, Copy, PartialEq)]
pub enum PluginTarget {
    /// Instrument on a pad.
    Pad(usize),
    /// Effect in (bus, slot).
    Fx(usize, usize),
}

pub struct Browser {
    pub open: bool,
    pub query: String,
    /// 0 = all, 1 = instruments, 2 = effects, 3 = failed.
    pub filter: u8,
    pub target: PluginTarget,
}

pub enum Dest {
    Pad(usize),
    NextEmpty,
    Slice,
}

pub enum JobResult {
    Loaded { pad: usize, name: String, sample: SampleData, analysis: Analysis },
    Conformed { pad: usize, sample: SampleData, note: String },
    /// A pitch-shifted copy of a vocal take, for a play-through lane on `pad`.
    Shifted { key: (usize, i32), pad: usize, sample: SampleData },
    Failed(String),
}

/// What the Key/BPM Harmonizer believes about the focused pad; editable before conforming.
#[derive(Clone, Copy)]
pub struct Harmonizer {
    pub open: bool,
    pub bpm: f32,
    pub root: u8,
    pub minor: bool,
    pub tempo: bool,
    pub key: bool,
}

pub struct RollUi {
    pub snap: usize,
    pub scale_lock: bool,
    pub ghosts: bool,
    pub rec: bool,
    /// Highest pitch currently visible (fractional so smooth scrolling accumulates).
    pub top_f: f32,
    pub last_len: u32,
    pub drag: Option<RollDrag>,
    /// Selected notes, as indices into the pattern of pad `sel_pad`.
    pub sel: Vec<usize>,
    pub sel_pad: usize,
    /// Where a rubber-band selection started.
    pub marquee: Option<egui::Pos2>,
    /// How much of the pattern is in view, 1 = all of it.
    pub zoom: f32,
    /// First tick in view when zoomed in.
    pub scroll: f32,
    /// Keep the playhead in view while playing. Shared with the step sequencer's pages.
    pub follow: bool,
}

pub struct RollDrag {
    /// The note under the pointer; the rest of the selection follows it.
    pub idx: usize,
    pub resize: bool,
    pub grab_tick: i64,
    pub grab_pitch: i32,
    /// Every note being dragged, as it was when the drag began.
    pub orig: Vec<(usize, Note)>,
}

pub struct SkipUi {
    pub window_secs: f32,
    /// Selection in absolute ring frames.
    pub sel: Option<(u64, u64)>,
    /// While set, the display stops scrolling so the selection can be made.
    pub frozen_end: Option<u64>,
    pub mark_in: Option<u64>,
    pub dragging_out: bool,
}

impl SkipUi {
    /// The selection in time order, or `None` if there isn't one worth the name.
    pub fn selection(&self) -> Option<(u64, u64)> {
        let (a, b) = self.sel?;
        (a.abs_diff(b) >= 64).then(|| (a.min(b), a.max(b)))
    }

    /// Drops any selection and lets the display follow the audio again. The display is
    /// only ever held still for a selection, so clearing one must always release it.
    pub fn resume(&mut self) {
        self.sel = None;
        self.frozen_end = None;
        self.mark_in = None;
        self.dragging_out = false;
    }
}
