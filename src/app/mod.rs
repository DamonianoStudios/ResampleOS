//! Application state and every action the UI, keyboard and controllers can perform.
mod capture;
mod conform;
mod drum;
mod history;
mod input;
mod pads;
mod play;
mod plugins;
mod roll;
mod scenes;
mod state;
mod vocal;

pub use drum::{DrumUi, MacroLayer};
pub use history::History;
pub use pads::PadAction;
pub use scenes::{MAX_CHAIN, MAX_SCENES, Scene};
pub use state::*;
pub use vocal::{RecState, VOCAL_SNAPS, VocalDrag, VocalTool, VocalUi};

use play::PendingNote;

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
use crate::shared::{MASTER, MAX_BANKS, NUM_BUSES, Shared, VOCAL};
use crate::stretch;
use eframe::egui;
use rtrb::{Consumer, Producer};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{Receiver, Sender, channel};

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
    editors: Vec<plugins::EditorWin>,
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
    /// Loops. The pads hold the patterns of `scenes[scene]`.
    pub scenes: Vec<Scene>,
    pub scene: usize,
    /// Loops to play one after another, by index into `scenes`.
    pub chain: Vec<usize>,
    pub chain_on: bool,
    /// The vocal bus's gate, de-esser and ducking.
    pub vocal_fx: crate::engine::VocalFx,

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
    /// Which four bars of a long pattern the step sequencer shows.
    pub step_page: u32,
    pub step_paint: Option<bool>,
    /// Choke matrix: whether a drag is assigning or clearing.
    pub choke_paint: Option<bool>,

    pub vocal: VocalUi,
    pub drum: DrumUi,
    vocal_work: vocal::VocalWork,
    sent_scale: u16,
    /// Perform mode: the pad matrix launches loops.
    pub perform: bool,
    /// The loop the engine has been asked to bring in next.
    pub queued_scene: Option<usize>,
    /// Which chain entry is playing.
    pub chain_pos: usize,
    /// The chain entry being dragged to a new place, if any.
    pub chain_drag: Option<usize>,
    /// Launch on the next bar line rather than at the end of the playing loop.
    pub launch_on_bar: bool,
    seen_starts: u32,
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
            buses: {
                let mut buses = [BusCfg::default(); NUM_BUSES];
                buses[VOCAL] = BusCfg::vocal();
                buses
            },
            conform_mode: ConformMode::Strict,
            auto_conform: false,
            preserve_transients: true,
            project_dir: None,
            scenes: vec![Scene::new(2)],
            scene: 0,
            chain: Vec::new(),
            chain_on: false,
            vocal_fx: Default::default(),
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
            roll: RollUi { snap: 3, scale_lock: true, ghosts: true, rec: false, top_f: 72.0, last_len: 120, drag: None, sel: Vec::new(), sel_pad: 0, marquee: None, zoom: 1.0, scroll: 0.0, follow: true },
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
            step_page: 0,
            step_paint: None,
            choke_paint: None,
            vocal: VocalUi::default(),
            drum: DrumUi::default(),
            vocal_work: Default::default(),
            sent_scale: 0,
            perform: false,
            queued_scene: None,
            chain_pos: 0,
            chain_drag: None,
            launch_on_bar: false,
            seen_starts: 0,
        };
        // The spec's default performance chain on Bus 1.
        app.buses[0].slots = [
            SlotCfg::new(FxKind::Vinyl, true),
            SlotCfg::new(FxKind::Cassette, false),
            SlotCfg::new(FxKind::Isolator, true),
        ];
        app.push_all();
        // A project folder on the command line opens that project; audio files (also via
        // "Open with") fill pads from A01.
        let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
        let mut opened = None;
        if let Some(dir) = args.first().filter(|p| p.join("project.json").is_file()) {
            opened = Some(match crate::project::load(&mut app, dir) {
                Ok(n) => {
                    app.project_dir = Some(dir.clone());
                    format!("Opened {} ({n} pads)", dir.display())
                }
                Err(e) => format!("Open failed: {e}"),
            });
        } else {
            for (i, path) in args.into_iter().take(16).enumerate() {
                app.load_file(i, path);
            }
        }
        let loading = app.busy > 0;
        app.status = match &app.audio_error {
            Some(e) => format!("Audio unavailable: {e}"),
            None if loading => app.status.clone(),
            None if opened.is_some() => opened.unwrap_or_default(),
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
        let ticks = if p.pattern.notes.is_empty() { 0 } else { p.pattern.len_ticks };
        self.send(Cmd::SetPattern { pad: idx as u16, pattern });
        self.fit_scene_to(ticks);
    }

    pub fn push_bus(&mut self, bus: usize) {
        let cfg = self.buses[bus];
        self.send(Cmd::SetBus { bus: bus as u8, cfg });
    }

    pub fn push_transport(&mut self) {
        self.send(Cmd::SetBpm(self.bpm));
        self.send(Cmd::SetSwing(self.swing));
        self.send(Cmd::SetScale(self.key.mask()));
        self.sent_scale = self.key.mask();
        self.send(Cmd::SetVocalFx(self.vocal_fx));
        self.send(Cmd::Metronome(self.metronome));
        self.send(Cmd::RingSource(self.ring_src));
        self.send(Cmd::InputCfg { dest: self.input_dest, gain: self.input_gain });
        self.send(Cmd::InputMode(self.prefs.input_mode));
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

    fn poll(&mut self) {
        // Buffers and plugins the engine has let go of are freed here, off the audio thread.
        while self.garbage.pop().is_ok() {}
        self.service_plugins();
        self.poll_scenes();
        self.vocal_poll();
        // Effects that follow the project key hear about a change of key.
        if self.key.mask() != self.sent_scale {
            self.sent_scale = self.key.mask();
            self.send(Cmd::SetScale(self.sent_scale));
        }

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
                JobResult::Shifted { key, pad, sample } => self.vocal_shift_done(key, pad, sample),
                JobResult::Failed(e) => self.status = e,
            }
        }

        while let Ok(ev) = self.midi_rx.try_recv() {
            match ev {
                MidiEvent::NoteOn { pad, semis, vel, key, is_pad } => {
                    let pad = pad as usize;
                    if self.perform && is_pad {
                        self.launch_scene(pad % 16);
                    } else if self.resample == Resample::Armed {
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
        if self.prefs.vst3_dirs != self.prefs_edit.vst3_dirs {
            self.prefs.vst3_dirs = self.prefs_edit.vst3_dirs.clone();
            self.catalog.start_scan(false, &self.prefs.vst3_dirs);
        }
        self.show_prefs = false;
    }

    /// True while the screen has something to animate: sound, transport, a job, a drag.
    fn is_animating(&self, ctx: &egui::Context) -> bool {
        let level = |i: usize| crate::shared::f32_load(&self.shared.bus_peak[i]);
        self.playing()
            || self.busy > 0
            || self.catalog.scanning
            || self.resample != Resample::Idle
            || self.vocal_armed()
            || !self.editors.is_empty()
            // The LFO scope follows the engine.
            || (self.tab == Tab::Mod && self.pads[self.selected].cfg.lfo_target != crate::engine::LfoTarget::Off)
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
        crate::ui::theme::finish(&ctx);
        self.vocal_commit(&ctx);
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
