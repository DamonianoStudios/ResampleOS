//! Application state and every action the UI, keyboard and controllers can perform.
mod arrange;
mod automation;
mod capture;
mod conform;
mod drum;
mod export;
mod history;
mod input;
mod midi_io;
mod pads;
mod play;
mod plugins;
mod roll;
mod samples;
mod scenes;
mod song_audio;
mod song_rec;
mod state;
mod vocal;

pub use arrange::{ARR_SNAPS, ArrDrag, ArrangeUi, Arrangement, Clip, OVERVIEW_STEP, View, db_to_gain, pos_text};
pub use export::Depth;
pub use midi_io::is_midi;
pub use samples::{SampleBrowser, SampleEntry, SampleFolder, SampleInfo, SampleList, folder_view as samples_folder_view};
pub use song_audio::BounceTo;
pub use automation::{AUTO_SNAPS, AutoScope, AutoUi};
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
    pub(crate) _midi: midi::Midi,
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
    pub song_rec: song_rec::SongRec,
    pub samples: SampleBrowser,
    /// Stems still to render, each after the last, and how many there were.
    stem_queue: std::collections::VecDeque<(export::Offline, std::path::PathBuf)>,
    stems_total: usize,
    /// Bus effect plugins, indexed `bus * 3 + slot`.
    pub fx_plugins: Vec<Option<PluginSlot>>,
    /// Analyzer plugins and the bus each watches.
    pub analyzers: Vec<(Option<PluginSlot>, usize)>,
    pub show_analyzers: bool,
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
    /// The song timeline.
    pub arrangement: Arrangement,
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
    /// How warps and conforms are done.
    pub warp_mode: stretch::WarpMode,
    /// Keep formants when an Elastic warp shifts pitch.
    pub warp_formants: bool,
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
    /// The menu of a pad's name in the step sequencer: the pad, and where it opened.
    pub step_menu: Option<(usize, egui::Pos2)>,
    /// Choke matrix: whether a drag is assigning or clearing.
    pub choke_paint: Option<bool>,

    pub vocal: VocalUi,
    /// Which of the two main views is on screen.
    pub view: View,
    pub arr: ArrangeUi,
    /// The engine is playing the arrangement rather than a loop.
    pub song_mode: bool,
    /// A song export is running; the live engine has lent it the plugins.
    pub exporting: bool,
    pub export_depth: export::Depth,
    pub drum: DrumUi,
    pub auto: AutoUi,
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
    /// File > New was asked for while the project has unsaved changes.
    pub confirm_new: bool,
    /// The window was closed while the project has unsaved changes.
    pub confirm_quit: bool,
    /// Close without asking (a restart carries the unsaved project across; or it was
    /// just saved).
    pub quit_ok: bool,
    /// A latency measurement under way: when it started, and the input monitoring to put
    /// back after it.
    pub latency_probe: Option<(std::time::Instant, u8)>,
}

impl App {
    pub fn new(prefs: Prefs) -> Self {
        let audio = audio_io::start(&prefs);
        let audio_note = audio.note.clone();
        let (mtx, midi_rx) = channel();
        let midi = midi::start(audio.shared.clone(), audio.tx_midi, mtx, prefs.pad_lights);
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
            song_rec: Default::default(),
            samples: Default::default(),
            stem_queue: Default::default(),
            stems_total: 0,
            fx_plugins: (0..NUM_BUSES * 3).map(|_| None).collect(),
            analyzers: (0..crate::shared::NUM_ANALYZERS).map(|_| (None, MASTER)).collect(),
            show_analyzers: false,
            synced_transport: (0.0, false),
            jobs_tx,
            jobs_rx,
            busy: 0,
            pads: (0..banks * 16).map(|_| PadModel::default()).collect(),
            banks,
            bpm: 88.0,
            swing: 0.5,
            key: Key { root: 0, scale: Scale::Minor },
            buses: App::starting_buses(),
            conform_mode: ConformMode::Strict,
            auto_conform: false,
            preserve_transients: true,
            project_dir: None,
            scenes: vec![Scene::new(2)],
            scene: 0,
            chain: Vec::new(),
            arrangement: Arrangement::default(),
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
            roll: RollUi { snap: 3, scale_lock: true, ghosts: true, rec: false, top_f: 72.0, last_len: 120, drag: None, sel: Vec::new(), sel_pad: 0, marquee: None, zoom: 1.0, scroll: 0.0, follow: true, vel_drag: None, vel_random: 0.2 },
            show_choke: false,
            show_help: false,
            sensitivity: 0.5,
            wave_view: (0.0, 1.0),
            wave_peaks: sample::PeakCache::default(),
            wave_anchor: None,
            warp_semis: 0.0,
            warp_stretch: 100.0,
            warp_mode: stretch::WarpMode::Elastic,
            warp_formants: false,
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
            step_menu: None,
            choke_paint: None,
            vocal: VocalUi::default(),
            view: View::Pads,
            arr: ArrangeUi::default(),
            song_mode: false,
            exporting: false,
            export_depth: export::Depth::Int24,
            drum: DrumUi::default(),
            auto: AutoUi { snap: 3, ..AutoUi::default() },
            vocal_work: Default::default(),
            sent_scale: 0,
            perform: false,
            queued_scene: None,
            chain_pos: 0,
            chain_drag: None,
            launch_on_bar: false,
            seen_starts: 0,
            confirm_new: false,
            confirm_quit: false,
            quit_ok: false,
            latency_probe: None,
        };
        app.push_all();
        // A project folder on the command line opens that project; audio files (also via
        // "Open with") fill pads from A01.
        let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
        let mut opened = None;
        // A project carried across a restart names the folder it really belongs to.
        let was = args.iter().position(|a| a.as_os_str() == crate::project::WAS_FLAG).map(|i| args.get(i + 1).cloned().unwrap_or_default());
        if let Some(dir) = args.first().filter(|p| p.join("project.json").is_file()) {
            opened = Some(match crate::project::load(&mut app, dir) {
                Ok(n) => match was {
                    Some(was) => {
                        app.project_dir = (!was.as_os_str().is_empty()).then_some(was);
                        app.history.forget_saved();
                        format!("Restarted at {} Hz with your project as it was", app.shared.sample_rate)
                    }
                    None => {
                        app.project_dir = Some(dir.clone());
                        format!("Opened {} ({n} pads)", dir.display())
                    }
                },
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

    /// The buses of a new project: the vocal bus set up for a voice, and a performance
    /// chain on Bus 1.
    pub fn starting_buses() -> [BusCfg; NUM_BUSES] {
        let mut buses = [BusCfg::default(); NUM_BUSES];
        buses[VOCAL] = BusCfg::vocal();
        buses[0].slots = [SlotCfg::new(FxKind::Vinyl, true), SlotCfg::new(FxKind::Cassette, false), SlotCfg::new(FxKind::Isolator, true)];
        buses
    }

    /// File > New: starts an empty project, first asking if there are unsaved changes.
    pub fn request_new(&mut self) {
        if self.has_unsaved_changes() {
            self.confirm_new = true;
        } else {
            crate::project::new_project(self);
        }
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
        self.arr.dirty = true;
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
        self.push_automation();
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
        self.latency_poll();
        self.poll_song_rec();
        self.poll_warps();
        self.poll_arrangement();
        // Effects that follow the project key hear about a change of key.
        if self.key.mask() != self.sent_scale {
            self.sent_scale = self.key.mask();
            self.send(Cmd::SetScale(self.sent_scale));
        }

        while let Ok(job) = self.jobs_rx.try_recv() {
            // Browser listings and auditions run quietly, without the busy indicator.
            if !matches!(job, JobResult::SamplesListed { .. } | JobResult::Previewed { .. } | JobResult::SamplesAnalysed { .. }) {
                self.busy = self.busy.saturating_sub(1);
            }
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
                JobResult::SongAudio { pad, at, name, sample } => self.song_audio_loaded(pad, at, name, sample),
                JobResult::Warped { source, file, bpm, mode, data } => self.warp_done(source, file, bpm, mode, data),
                JobResult::SamplesListed { root, entries } => self.samples_listed(root, entries),
                JobResult::Previewed { seq, data, sync } => self.previewed(seq, data, sync),
                JobResult::SamplesAnalysed { root, items, left } => self.samples_analysed(root, items, left),
                JobResult::Bounced { to, l, r, detached } => {
                    self.export_done(&detached);
                    self.bounce_done(to, l, r);
                }
                JobResult::Failed(e) => self.status = e,
                JobResult::Exported { path, secs, peak, detached } => {
                    self.export_done(&detached);
                    let db = 20.0 * peak.max(1e-6).log10();
                    let clip = if peak >= 0.999 { " It clips: bring the master down and export again." } else { "" };
                    if self.stems_total > 0 && !self.next_stem() {
                        let folder = path.parent().map_or(String::new(), |d| d.display().to_string());
                        self.status = format!("Exported {} stems to {folder}", self.stems_total);
                        self.stems_total = 0;
                    } else if self.stems_total == 0 {
                        self.status = format!("Exported {secs:.1} s to {} (peak {db:.1} dB).{clip}", path.display());
                    }
                }
                JobResult::ExportFailed { error, detached } => {
                    self.export_done(&detached);
                    self.stem_queue.clear();
                    self.stems_total = 0;
                    self.status = format!("Export failed: {error}");
                }
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

        self.pad_lights();
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

    /// True when the edited preferences change the sample rate, which only a restart of
    /// the app can bring in (every take and plugin is at the running rate).
    pub fn prefs_need_relaunch(&self) -> bool {
        self.prefs_edit.needs_restart(&self.prefs) && audio_io::planned_rate(&self.prefs_edit).is_some_and(|r| r != self.shared.sample_rate)
    }

    /// Saves the edited preferences. Interface scale applies at once, and a new device,
    /// buffer size or skip-back length restarts the audio in place. A new sample rate
    /// waits for the app to be restarted and is left in `prefs_edit`.
    pub fn apply_prefs(&mut self, ctx: &egui::Context) {
        ctx.set_zoom_factor(self.prefs_edit.ui_scale);
        let relaunch = self.prefs_need_relaunch();
        self.status = match self.prefs_edit.save() {
            Ok(()) if relaunch => "Preferences saved; the new sample rate applies after a restart".into(),
            Ok(()) => "Preferences saved".into(),
            Err(e) => format!("Could not save preferences: {e}"),
        };
        if self.prefs_edit.needs_restart(&self.prefs) && !relaunch {
            let running = self.prefs.clone();
            self.prefs.output_device = self.prefs_edit.output_device.clone();
            self.prefs.sample_rate = self.prefs_edit.sample_rate;
            self.prefs.buffer_frames = self.prefs_edit.buffer_frames;
            self.prefs.skipback_minutes = self.prefs_edit.skipback_minutes;
            if !self.restart_audio() {
                // Go back to what was running.
                self.prefs = running;
                self.restart_audio();
            }
        }
        // Keep the engine-level fields as they are running, so the window keeps saying
        // a restart is pending until one happens.
        self.prefs.ui_scale = self.prefs_edit.ui_scale;
        if self.prefs.pad_lights != self.prefs_edit.pad_lights {
            self.prefs.pad_lights = self.prefs_edit.pad_lights;
            if self.prefs.pad_lights {
                // MIDI ports are opened with or without their light outputs.
                self.restart_midi();
            } else {
                self._midi.leds.close();
            }
        }
        if self.prefs.vst3_dirs != self.prefs_edit.vst3_dirs {
            self.prefs.vst3_dirs = self.prefs_edit.vst3_dirs.clone();
            self.catalog.start_scan(false, &self.prefs.vst3_dirs);
        }
        self.show_prefs = false;
    }

    /// Stops the audio engine and starts a new one with the running preferences, then
    /// sends it the whole project again. The sample rate must stay the same (see
    /// [`App::prefs_need_relaunch`]). Skip-back starts empty. False if no device opened.
    pub fn restart_audio(&mut self) -> bool {
        self.send(Cmd::Play(false));
        self.vocal_disarm();
        self.latency_probe = None;
        let input = self.input_sel;
        self.set_input(None);
        // The old engine goes with its stream; MIDI ports are let go of before reopening.
        self._stream = None;
        self._midi = midi::Midi::none();
        while self.garbage.pop().is_ok() {}
        let audio = audio_io::start(&self.prefs);
        self.shared = audio.shared;
        self.open_midi(audio.tx_midi);
        self.tx = audio.tx;
        self.garbage = audio.garbage;
        self._stream = audio.stream;
        self.device_name = audio.device_name;
        self.audio_error = audio.error;
        // Positions in the old rings mean nothing in the new ones.
        self.skip.sel = None;
        self.skip.frozen_end = None;
        self.skip.mark_in = None;
        self.queued_scene = None;
        self.chain_pos = 0;
        self.sent_scale = 0;
        self.synced_transport = (0.0, false);
        self.shared.scene_index.store(self.scene as u32, Ordering::Relaxed);
        self.push_all();
        for i in 0..self.fx_plugins.len() {
            let plugin = self.fx_plugins[i].as_ref().map(|slot| self.rt_for(slot));
            if plugin.is_some() {
                self.send(Cmd::SetFxPlugin { slot: i as u8, plugin });
            }
        }
        for k in 0..self.analyzers.len() {
            self.push_analyzer(k);
        }
        self.push_scene_len();
        if input.is_some() {
            self.set_input(input);
        }
        self.status = match &self.audio_error {
            Some(e) => format!("Audio unavailable: {e}"),
            None => format!("Audio restarted: {} @ {} Hz", self.device_name, self.shared.sample_rate),
        };
        self.audio_error.is_none()
    }

    /// Opens the MIDI inputs (and the pad lights, if wanted) feeding the engine queue `tx`.
    fn open_midi(&mut self, tx: Producer<Cmd>) {
        let (mtx, midi_rx) = channel();
        self._midi = midi::start(self.shared.clone(), tx, mtx, self.prefs.pad_lights);
        self.midi_rx = midi_rx;
        self.midi_ports = self._midi.port_names.clone();
    }

    /// Reopens the MIDI ports, as when pad lights are turned on. The engine's MIDI queue
    /// cannot be handed over twice, so this restarts the audio, which makes a new one.
    fn restart_midi(&mut self) {
        self.restart_audio();
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
            || self.latency_probe.is_some()
            || level(MASTER * 2).max(level(MASTER * 2 + 1)) > 1e-4
            || ctx.input(|i| i.pointer.any_down())
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Closing with unsaved changes asks first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quit_ok && self.has_unsaved_changes() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_quit = true;
        }
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
