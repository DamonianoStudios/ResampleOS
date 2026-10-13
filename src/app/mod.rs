//! Application state and every action the UI, keyboard and controllers can perform.
mod capture;
mod conform;
mod history;
mod input;
mod pads;
mod plugins;

pub use history::History;
pub use pads::PadAction;

use crate::analysis::{self, Analysis};
use crate::audio_io;
use crate::engine::{BusCfg, Cmd, Garbage, MidiTarget, Note, PadCfg, PadKind, PlayMode, PluginRt, SlotCfg};
use crate::fx::{Fx, FxKind};
use crate::midi::{self, MidiEvent};
use crate::model::{PadModel, PluginSlot};
use crate::music::{Key, PPQ, Scale, TICKS_PER_BAR, pad_label};
use crate::plugins::Catalog;
use crate::prefs::Prefs;
use crate::sample::{self, SampleData};
use crate::shared::{MASTER, MAX_BANKS, NUM_BUSES, Shared};
use crate::stretch;
use eframe::egui;
use rtrb::{Consumer, Producer};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{Receiver, Sender, channel};

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
}

#[derive(Clone, Copy)]
pub struct RollDrag {
    pub idx: usize,
    pub resize: bool,
    pub grab_tick: i64,
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

struct PendingNote {
    key: u32,
    pad: usize,
    pitch: u8,
    vel: u8,
    start: f64,
}

pub struct App {
    pub shared: Arc<Shared>,
    tx: Producer<Cmd>,
    garbage: Consumer<Garbage>,
    _stream: Option<cpal::Stream>,
    _midi: midi::Midi,
    midi_rx: Receiver<MidiEvent>,
    pub midi_ports: Vec<String>,
    pub device_name: String,
    pub audio_error: Option<String>,
    /// Preferences as the running engine was started with.
    pub prefs: Prefs,
    /// The copy being edited in the preferences window.
    pub prefs_edit: Prefs,
    pub show_prefs: bool,
    pub history: History,
    pub outputs: Vec<audio_io::OutputInfo>,
    _input: Option<cpal::Stream>,
    pub input_names: Vec<String>,
    pub input_sel: Option<usize>,
    /// Monitor destination: 0 = off, 1 = thru, 2..=5 = Bus 1-4.
    pub input_dest: u8,
    pub input_gain: f32,
    host: Option<vst3_host::Vst3Host>,
    pub catalog: Catalog,
    pub browser: Browser,
    /// Open plugin editor windows, keyed by `PluginSlot::id`.
    editors: Vec<(usize, vst3_host::PluginWindow)>,
    /// Bus effect plugins, indexed `bus * 3 + slot`.
    pub fx_plugins: Vec<Option<PluginSlot>>,
    synced_transport: (f32, bool),
    jobs_tx: Sender<JobResult>,
    jobs_rx: Receiver<JobResult>,
    pub busy: usize,

    // Project.
    pub pads: Vec<PadModel>,
    pub banks: usize,
    pub bpm: f32,
    pub swing: f32,
    pub key: Key,
    pub buses: [BusCfg; NUM_BUSES],
    pub conform_mode: ConformMode,
    pub auto_conform: bool,
    pub preserve_transients: bool,
    pub project_dir: Option<PathBuf>,

    // Session state.
    pub bank: usize,
    pub selected: usize,
    pub zoom: u8,
    pub tab: Tab,
    pub focus_bus: usize,
    pub focus_slot: usize,
    pub chromatic: bool,
    pub octave: i32,
    pub roll_on: bool,
    pub roll_div: usize,
    pub ring_src: u8,
    pub metronome: bool,
    pub resample: Resample,
    pub skip: SkipUi,
    pub harm: Harmonizer,
    pub roll: RollUi,
    pub show_choke: bool,
    pub show_help: bool,
    pub sensitivity: f32,
    pub wave_view: (f64, f64),
    pub wave_peaks: sample::PeakCache,
    pub wave_anchor: Option<usize>,
    pub warp_semis: f32,
    pub warp_stretch: f32,
    pub status: String,
    pub clipboard: Option<usize>,
    /// Pad rectangles from the last frame, for drops and drags that end on a pad.
    pub pad_rects: Vec<(usize, egui::Rect)>,

    held_keys: Vec<(egui::Key, u16, u32)>,
    pub mouse_held: Option<(u16, u32)>,
    pending: Vec<PendingNote>,
    pub m_down: bool,
    pub q_down: bool,
    /// Step sequencer: ticks per step, and what a drag is painting.
    pub step_res: u32,
    pub step_bars: u32,
    pub step_paint: Option<bool>,
}

impl App {
    pub fn new(prefs: Prefs) -> Self {
        let audio = audio_io::start(&prefs);
        let audio_note = audio.note.clone();
        let (mtx, midi_rx) = channel();
        let midi = midi::start(audio.shared.clone(), audio.tx_midi, mtx);
        let midi_ports = midi.port_names.clone();
        let (jobs_tx, jobs_rx) = channel();
        let banks = 4;
        let mut app = App {
            shared: audio.shared,
            tx: audio.tx,
            garbage: audio.garbage,
            _stream: audio.stream,
            _midi: midi,
            midi_rx,
            midi_ports,
            device_name: audio.device_name,
            audio_error: audio.error,
            prefs_edit: prefs.clone(),
            prefs,
            show_prefs: false,
            history: History::default(),
            outputs: Vec::new(),
            _input: None,
            input_names: audio_io::input_names(),
            input_sel: None,
            input_dest: 1,
            input_gain: 1.0,
            host: None,
            catalog: Catalog::load(),
            browser: Browser { open: false, query: String::new(), filter: 0, target: PluginTarget::Pad(0) },
            editors: Vec::new(),
            fx_plugins: (0..NUM_BUSES * 3).map(|_| None).collect(),
            synced_transport: (0.0, false),
            jobs_tx,
            jobs_rx,
            busy: 0,
            pads: (0..banks * 16).map(|_| PadModel::default()).collect(),
            banks,
            bpm: 88.0,
            swing: 0.5,
            key: Key { root: 0, scale: Scale::Minor },
            buses: [BusCfg::default(); NUM_BUSES],
            conform_mode: ConformMode::Strict,
            auto_conform: false,
            preserve_transients: true,
            project_dir: None,
            bank: 0,
            selected: 0,
            zoom: 0,
            tab: Tab::Wave,
            focus_bus: 0,
            focus_slot: 0,
            chromatic: false,
            octave: 0,
            roll_on: false,
            roll_div: 2,
            ring_src: 0,
            metronome: false,
            resample: Resample::Idle,
            skip: SkipUi { window_secs: 30.0, sel: None, frozen_end: None, mark_in: None, dragging_out: false },
            harm: Harmonizer { open: false, bpm: 120.0, root: 0, minor: false, tempo: true, key: true },
            roll: RollUi { snap: 3, scale_lock: true, ghosts: true, rec: false, top_f: 72.0, last_len: 120, drag: None },
            show_choke: false,
            show_help: false,
            sensitivity: 0.5,
            wave_view: (0.0, 1.0),
            wave_peaks: sample::PeakCache::default(),
            wave_anchor: None,
            warp_semis: 0.0,
            warp_stretch: 100.0,
            status: String::new(),
            clipboard: None,
            pad_rects: Vec::new(),
            held_keys: Vec::new(),
            mouse_held: None,
            pending: Vec::new(),
            m_down: false,
            q_down: false,
            step_res: 120,
            step_bars: 1,
            step_paint: None,
        };
        // The spec's default performance chain on Bus 1.
        app.buses[0].slots = [
            SlotCfg { kind: FxKind::Vinyl, params: FxKind::Vinyl.defaults(), on: true },
            SlotCfg { kind: FxKind::Cassette, params: FxKind::Cassette.defaults(), on: false },
            SlotCfg { kind: FxKind::Isolator, params: FxKind::Isolator.defaults(), on: true },
        ];
        app.push_all();
        // Files passed on the command line (or via "Open with") fill pads from A01.
        for (i, path) in std::env::args_os().skip(1).take(16).enumerate() {
            app.load_file(i, PathBuf::from(path));
        }
        let loading = app.busy > 0;
        app.status = match &app.audio_error {
            Some(e) => format!("Audio unavailable: {e}"),
            None if loading => app.status.clone(),
            None if audio_note.is_some() => audio_note.unwrap_or_default(),
            None => format!("{} @ {} Hz", app.device_name, app.shared.sample_rate),
        };
        app
    }

    pub fn sr(&self) -> f32 {
        self.shared.sample_rate as f32
    }

    pub fn send(&mut self, cmd: Cmd) {
        if self.tx.push(cmd).is_err() {
            self.status = "Engine queue full; command dropped".into();
        }
    }

    pub fn push_pad(&mut self, idx: usize) {
        let cfg = self.pads[idx].cfg;
        self.send(Cmd::SetPadCfg { pad: idx as u16, cfg });
    }

    pub fn push_sample(&mut self, idx: usize) {
        let sample = self.pads[idx].sample().cloned();
        self.send(Cmd::SetSample { pad: idx as u16, sample });
    }

    pub fn push_pattern(&mut self, idx: usize) {
        let p = &self.pads[idx];
        // An audio pad with no steps has nothing to sequence.
        let pattern = (p.cfg.kind == PadKind::Midi || !p.pattern.notes.is_empty())
            .then(|| Arc::new(p.pattern.clone()));
        self.send(Cmd::SetPattern { pad: idx as u16, pattern });
    }

    pub fn push_bus(&mut self, bus: usize) {
        let cfg = self.buses[bus];
        self.send(Cmd::SetBus { bus: bus as u8, cfg });
    }

    pub fn push_transport(&mut self) {
        self.send(Cmd::SetBpm(self.bpm));
        self.send(Cmd::SetSwing(self.swing));
        self.send(Cmd::Metronome(self.metronome));
        self.send(Cmd::RingSource(self.ring_src));
        self.send(Cmd::InputCfg { dest: self.input_dest, gain: self.input_gain });
        self.send(Cmd::Roll { on: self.roll_on, ticks: ROLL_DIVS[self.roll_div].1 });
    }

    /// Full resync, used at startup and after loading a project.
    pub fn push_all(&mut self) {
        for b in 0..NUM_BUSES {
            self.push_bus(b);
        }
        self.push_transport();
        for i in 0..self.pads.len() {
            if self.pads[i].is_empty() && self.pads[i].cfg == PadCfg::default() {
                continue;
            }
            self.resync_pad(i);
        }
    }

    /// Sends everything the engine holds for one pad.
    pub fn resync_pad(&mut self, i: usize) {
        self.push_pad(i);
        self.push_sample(i);
        self.push_pattern(i);
        self.push_plugin(i);
    }

    pub fn playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    pub fn toggle_play(&mut self) {
        let on = !self.playing();
        if on
            && let Resample::Recording { target, bar_aligned: false, .. } = self.resample {
                // Starting the transport mid-take restarts the take on the downbeat.
                self.resample =
                    Resample::Recording { target, start: self.shared.ring.written(), bar_aligned: true };
            }
        self.send(Cmd::Play(on));
    }

    /// Semitone offset for the i-th key of a chromatic spread.
    pub fn spread_semis(&self, i: i32) -> f32 {
        let s = match self.conform_mode {
            ConformMode::Strict => i,
            ConformMode::Smart => self.key.degree_semis(i),
        };
        (s + self.octave * 12) as f32
    }

    /// A hit on pad slot `i` (0..16) of the visible bank, from keyboard or mouse.
    pub fn slot_hit(&mut self, i: usize, key: u32) -> (u16, u32) {
        if self.chromatic {
            let semis = self.spread_semis(i as i32);
            self.note_on(self.selected, semis, 0.9, key);
            (self.selected as u16, key)
        } else {
            let pad = self.bank * 16 + i;
            self.pad_hit(pad, key);
            (pad as u16, key)
        }
    }

    pub fn pad_hit(&mut self, pad: usize, key: u32) {
        if pad >= self.pads.len() {
            return;
        }
        if self.m_down {
            self.pads[pad].cfg.mute ^= true;
            self.push_pad(pad);
            return;
        }
        if self.q_down {
            self.stop_pad(pad);
            return;
        }
        if self.resample != Resample::Armed {
            self.select(pad);
        }
        let semis = if self.pads[pad].cfg.kind == PadKind::Midi { (self.octave * 12) as f32 } else { 0.0 };
        self.note_on(pad, semis, 0.9, key);
    }

    pub fn note_on(&mut self, pad: usize, semis: f32, vel: f32, key: u32) {
        if self.resample == Resample::Armed {
            self.begin_recording(pad);
            return;
        }
        self.send(Cmd::PadOn { pad: pad as u16, vel, semis, key });
        self.record_on(pad, semis, vel, key);
    }

    pub fn note_off(&mut self, pad: u16, key: u32) {
        self.send(Cmd::PadOff { pad, key });
        self.record_off(key);
    }

    pub fn select(&mut self, pad: usize) {
        if pad < self.pads.len() && pad != self.selected {
            self.selected = pad;
            self.wave_view = (0.0, 1.0);
            self.roll.drag = None;
        }
    }

    fn record_on(&mut self, pad: usize, semis: f32, vel: f32, key: u32) {
        if !self.roll.rec || !self.playing() {
            return;
        }
        // Audio pads record as steps at their own pitch; MIDI pads record the note played.
        let semis = if self.pads[pad].cfg.kind == PadKind::Midi { semis } else { 0.0 };
        let pitch = (60.0 + semis).round().clamp(0.0, 127.0) as u8;
        self.pending.push(PendingNote {
            key,
            pad,
            pitch,
            vel: (vel * 127.0) as u8,
            start: self.shared.tick(),
        });
    }

    fn record_off(&mut self, key: u32) {
        let Some(at) = self.pending.iter().position(|p| p.key == key) else { return };
        let p = self.pending.swap_remove(at);
        let snap = SNAPS[self.roll.snap].1 as f64;
        let len_ticks = self.pads[p.pad].pattern.len_ticks.max(1) as f64;
        let len = (self.shared.tick() - p.start).max(30.0);
        let start = ((p.start / snap).round() * snap).rem_euclid(len_ticks);
        self.pads[p.pad].pattern.notes.push(Note {
            start: start as u32,
            len: len as u32,
            pitch: p.pitch,
            vel: p.vel.max(1),
        });
        self.push_pattern(p.pad);
    }

    fn poll(&mut self) {
        // Buffers and plugins the engine has let go of are freed here, off the audio thread.
        while self.garbage.pop().is_ok() {}
        self.service_plugins();

        while let Ok(job) = self.jobs_rx.try_recv() {
            self.busy = self.busy.saturating_sub(1);
            match job {
                JobResult::Loaded { pad, name, sample, analysis } => {
                    if pad >= self.pads.len() {
                        continue;
                    }
                    let secs = sample.len() as f32 / self.sr();
                    let summary = if analysis.bpm > 0.0 {
                        format!(
                            "{:.1} BPM, {} {} ({:.0}%)",
                            analysis.bpm,
                            crate::music::NOTE_NAMES[analysis.key_root as usize],
                            if analysis.key_minor { "minor" } else { "major" },
                            analysis.key_conf * 100.0
                        )
                    } else {
                        "one-shot".to_string()
                    };
                    // Long material loops by default; hits stay one-shots.
                    self.pads[pad].cfg.mode = PlayMode::OneShot;
                    self.set_sample(pad, sample, Some(name.clone()), Some(analysis));
                    self.status = format!("{}: loaded {name} ({secs:.1}s, {summary})", pad_label(pad));
                    if self.auto_conform && self.pads[pad].analysis.as_ref().is_some_and(|a| a.bpm > 0.0) {
                        let was = self.selected;
                        self.selected = pad;
                        self.open_harmonizer();
                        self.harm.open = false;
                        self.conform(pad);
                        self.selected = was;
                    }
                }
                JobResult::Conformed { pad, sample, note } => {
                    if pad < self.pads.len() {
                        self.set_sample(pad, sample, None, None);
                        self.pads[pad].analysis = None;
                        self.status = format!("{} {note}", pad_label(pad));
                    }
                }
                JobResult::Failed(e) => self.status = e,
            }
        }

        while let Ok(ev) = self.midi_rx.try_recv() {
            match ev {
                MidiEvent::NoteOn { pad, semis, vel, key, is_pad } => {
                    let pad = pad as usize;
                    if self.resample == Resample::Armed {
                        self.begin_recording(pad);
                    } else {
                        if is_pad && !self.chromatic {
                            self.select(pad);
                        }
                        if pad < self.pads.len() {
                            self.record_on(pad, semis, vel, key);
                        }
                    }
                }
                MidiEvent::NoteOff { key } => self.record_off(key),
                MidiEvent::Cc { cc, value } => self.midi_cc(cc, value),
            }
        }

        let s = &self.shared;
        s.ui_bank.store(self.bank as u32, Ordering::Relaxed);
        s.ui_selected.store(self.selected as u32, Ordering::Relaxed);
        s.ui_chromatic.store(self.chromatic, Ordering::Relaxed);
        s.ui_octave.store(self.octave as u32, Ordering::Relaxed);
        s.ui_capture_pads.store(self.resample == Resample::Armed, Ordering::Relaxed);
        let mask = match self.conform_mode {
            ConformMode::Strict => 0xfff,
            ConformMode::Smart => self.key.scale.intervals().iter().fold(0u32, |m, &i| m | 1 << i),
        };
        s.ui_scale_mask.store(mask, Ordering::Relaxed);
    }
}

impl App {
    pub fn open_prefs(&mut self) {
        self.prefs_edit = self.prefs.clone();
        self.outputs = audio_io::output_devices();
        self.show_prefs = true;
    }

    /// Saves the edited preferences. Interface scale applies at once; anything the audio
    /// engine reads only at startup waits for the next launch and is left in `prefs_edit`.
    pub fn apply_prefs(&mut self, ctx: &egui::Context) {
        ctx.set_zoom_factor(self.prefs_edit.ui_scale);
        self.status = match self.prefs_edit.save() {
            Ok(()) if self.prefs_edit.needs_restart(&self.prefs) => "Preferences saved; audio changes apply after a restart".into(),
            Ok(()) => "Preferences saved".into(),
            Err(e) => format!("Could not save preferences: {e}"),
        };
        // Keep the engine-level fields as they are running, so the window keeps saying
        // a restart is pending until one happens.
        self.prefs.ui_scale = self.prefs_edit.ui_scale;
        self.prefs.vst3_dirs = self.prefs_edit.vst3_dirs.clone();
        self.show_prefs = false;
    }

    /// True while the screen has something to animate: sound, transport, a job, a drag.
    fn is_animating(&self, ctx: &egui::Context) -> bool {
        let level = |i: usize| crate::shared::f32_load(&self.shared.bus_peak[i]);
        self.playing()
            || self.busy > 0
            || self.catalog.scanning
            || self.resample != Resample::Idle
            || !self.editors.is_empty()
            || self.input_sel.is_some()
            || level(MASTER * 2).max(level(MASTER * 2 + 1)) > 1e-4
            || ctx.input(|i| i.pointer.any_down())
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll();
        self.keys(&ctx);
        crate::ui::draw(self, ui);
        self.track_history(&ctx);
        // Full frame rate while something is moving; otherwise a slow tick is enough to
        // keep the skip-back display scrolling. Input wakes the UI immediately either way.
        if self.is_animating(&ctx) {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(66));
        }
    }
}
