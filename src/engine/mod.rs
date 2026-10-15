//! The real-time engine. Owned by the audio callback; the UI and MIDI threads talk to it
//! through lock-free command queues and read results back through `Shared`.
mod lufs;
mod plugin_rt;
mod render;
mod song;
#[cfg(test)]
mod tests;
mod types;

pub use plugin_rt::PluginRt;
pub use types::*;

use crate::fx::{Fx, FxCtx, FxKind, Svf, svf_g};
use crate::music::{PPQ, TICKS_PER_BAR, swing_tick};
use crate::sample::{SampleData, hermite};
use crate::shared::{MASTER, MAX_PADS, NUM_BUSES, Shared, VOCAL, bus_index, f32_load, f32_store};
use crate::drum::{Ctx, DrumVoice, Hit, Locks, NO_LOCKS, Rng};
use crate::synth::SynthVoice;
use lufs::Lufs;
use plugin_rt::{PlugEv, PlugNote};
use rtrb::{Consumer, Producer};
use std::f32::consts::{FRAC_PI_4, SQRT_2, TAU};
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub const MAX_BLOCK: usize = 512;
const NUM_VOICES: usize = 64;
const NUM_SYNTH_VOICES: usize = 24;
const NUM_DRUM_VOICES: usize = 48;
const MAX_HELD: usize = 32;

/// A sample region longer than this is cut by a new hit of the same note, not layered.
const RETRIGGER_SECS: f32 = 1.0;

#[derive(Default)]
struct PadRt {
    cfg: PadCfg,
    sample: Option<Arc<SampleData>>,
    pattern: Option<Arc<Pattern>>,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    active: bool,
    /// Pad whose sample and settings this voice plays.
    pad: u16,
    /// Pad that triggered it (differs when a MIDI pad plays another pad).
    owner: u16,
    key: u32,
    pos: f64,
    rate: f64,
    gain: f32,
    env: f32,
    gated: bool,
    releasing: bool,
    killed: bool,
    gate_left: i64,
    offset: u32,
    age: u64,
    svf: [Svf; 2],
}

#[derive(Clone, Copy)]
struct Event {
    pad: u16,
    vel: f32,
    semis: f32,
    key: u32,
    offset: u32,
    gate: i64,
    /// Drum macros locked to the step that fired this.
    locks: Locks,
}

#[derive(Clone, Copy, Default)]
struct Held {
    pad: u16,
    key: u32,
    vel: f32,
    semis: f32,
}

pub struct Engine {
    sr: f32,
    shared: Arc<Shared>,
    rx: Consumer<Cmd>,
    rx_midi: Consumer<Cmd>,
    garbage: Producer<Garbage>,

    pads: Vec<PadRt>,
    seq_pads: Vec<u16>,
    voices: [Voice; NUM_VOICES],
    synths: [SynthVoice; NUM_SYNTH_VOICES],
    drums: [DrumVoice; NUM_DRUM_VOICES],
    /// Randomness for drum hits.
    drum_seed: u32,
    age: u64,
    any_solo: bool,

    buses: [BusCfg; NUM_BUSES],
    /// The buses as set by hand; `buses` is this with automation on top.
    bus_base: [BusCfg; NUM_BUSES],
    automation: Option<Automation>,
    /// Every pad's settings as set by hand, so pads automation has moved can be put
    /// back once it stops.
    pad_base: Vec<PadCfg>,
    auto_pads: Vec<u16>,
    fx: Vec<Fx>,
    bufs: Vec<[Vec<f32>; 2]>,
    scratch: Vec<f32>,
    peaks: [f32; NUM_BUSES * 2],

    playing: bool,
    bpm: f32,
    swing: f32,
    /// Pitch classes of the project scale, for effects that follow it.
    scale: u16,
    vocal_fx: VocalFx,
    vox: VocalState,
    /// Ticks since the playing loop started.
    tick: f64,
    scene_len: u32,
    queued: Option<Box<SceneSwap>>,
    /// How many loops have started since Play, so humanize differs on every pass.
    scene_pass: i64,
    /// Ticks since Play, never wrapped: what tempo-synced LFOs count from.
    song_tick: f64,
    /// The arrangement, and whether the pads are playing it rather than their loops.
    song: Option<Arc<ArrangementPlan>>,
    song_mode: bool,
    /// The song playhead in ticks. Runs in both modes, so the song can be picked up again.
    song_pos: f64,
    /// The loop brace: (start, end) in ticks.
    brace: Option<(f64, f64)>,
    /// The section whose automation is applied, to let go of it on leaving.
    song_section: Option<usize>,
    /// Pads with a track in the arrangement.
    song_pads: Vec<u16>,
    /// The stretches of song this block covers (more than one when the loop brace wraps
    /// in it): (tick, first frame, frames, follows a jump).
    song_segs: [(f64, usize, usize, bool); 4],
    song_nsegs: usize,
    /// Audio clips fade in over a few milliseconds after a jump (a locate, the brace
    /// wrapping, play), so they don't click in mid-waveform; and out again on stop.
    song_ramp: f32,
    song_jump: bool,
    song_fading: bool,
    /// Runs whether or not the transport does, so note repeat works when stopped.
    free_tick: f64,
    metronome: bool,
    click_phase: f32,
    click_env: f32,
    click_freq: f32,
    /// The latency test signal, and how much of it has played (`usize::MAX`: idle).
    ping: Vec<f32>,
    ping_pos: usize,
    /// A file auditioned from the sample browser, how far it has played, and its gain.
    preview: Option<Arc<SampleData>>,
    preview_pos: usize,
    preview_gain: f32,
    click_at: i64,

    roll_on: bool,
    roll_ticks: u32,
    held: [Option<Held>; MAX_HELD],

    events: Vec<Event>,
    touched: Vec<u16>,
    samples_run: u64,
    ring_src: u8,
    ring_peak: f32,
    lufs: Lufs,
    /// Samples of continuous silence at each bus input.
    bus_idle: [u32; NUM_BUSES],
    /// Level of what was sent into Bus 1-4 this block, for buses that key off it.
    key_buf: Vec<Vec<f32>>,
    /// Sidechain envelope follower state per keyed bus.
    sc_env: [f32; NUM_BUSES],

    pad_plugins: Vec<Option<Box<PluginRt>>>,
    plugin_pads: Vec<u16>,
    fx_plugins: Vec<Option<Box<PluginRt>>>,
    /// Analyzer plugins and the bus each watches.
    analyzers: Vec<(Option<Box<PluginRt>>, u8)>,
    plug_notes: [Option<PlugNote>; 64],

    input: Option<Consumer<f32>>,
    in_ratio: f64,
    in_phase: f64,
    in_prev: [f32; 2],
    in_next: [f32; 2],
    in_dest: u8,
    in_gain: f32,
    in_peak: f32,
    /// Counts down to emptying the effects after a panic; 0 when none is pending.
    flush_in: i32,
    in_mode: u8,
    /// Slow level of each input channel, for telling a one-sided source from a stereo one.
    in_env: [f32; 2],
    in_one_sided: bool,
    /// How far the two input channels are folded into each other, 0..1.
    in_fold: f32,
    in_ring_peak: f32,
    vox_key: Vec<f32>,
    in_buf: [Vec<f32>; 2],
}

impl Engine {
    pub fn new(
        shared: Arc<Shared>,
        rx: Consumer<Cmd>,
        rx_midi: Consumer<Cmd>,
        garbage: Producer<Garbage>,
    ) -> Self {
        let sr = shared.sample_rate as f32;
        Engine {
            sr,
            shared,
            rx,
            rx_midi,
            garbage,
            pads: (0..MAX_PADS).map(|_| PadRt::default()).collect(),
            seq_pads: Vec::with_capacity(MAX_PADS),
            voices: [Voice::default(); NUM_VOICES],
            synths: [SynthVoice::default(); NUM_SYNTH_VOICES],
            drums: [DrumVoice::default(); NUM_DRUM_VOICES],
            drum_seed: 0x2545_f491,
            age: 0,
            any_solo: false,
            buses: [BusCfg::default(); NUM_BUSES],
            bus_base: [BusCfg::default(); NUM_BUSES],
            automation: None,
            pad_base: vec![PadCfg::default(); MAX_PADS],
            auto_pads: Vec::with_capacity(MAX_PADS),
            fx: (0..NUM_BUSES * 3).map(|_| Fx::new(sr)).collect(),
            bufs: (0..NUM_BUSES).map(|_| [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]]).collect(),
            scratch: vec![0.0; MAX_BLOCK],
            peaks: [0.0; NUM_BUSES * 2],
            playing: false,
            bpm: 90.0,
            swing: 0.5,
            scale: 0xfff,
            vocal_fx: VocalFx::default(),
            vox: VocalState::default(),
            tick: 0.0,
            scene_len: 0,
            queued: None,
            scene_pass: 0,
            song_tick: 0.0,
            song: None,
            song_mode: false,
            song_pos: 0.0,
            brace: None,
            song_section: None,
            song_pads: Vec::with_capacity(MAX_PADS),
            song_segs: [(0.0, 0, 0, false); 4],
            song_nsegs: 0,
            song_ramp: 0.0,
            song_jump: true,
            song_fading: false,
            free_tick: 0.0,
            metronome: false,
            click_phase: 0.0,
            click_env: 0.0,
            click_freq: 1000.0,
            ping: crate::latency::ping(sr),
            ping_pos: usize::MAX,
            preview: None,
            preview_pos: 0,
            preview_gain: 1.0,
            click_at: -1,
            roll_on: false,
            roll_ticks: PPQ / 4,
            held: [None; MAX_HELD],
            events: Vec::with_capacity(512),
            touched: Vec::with_capacity(NUM_VOICES + NUM_SYNTH_VOICES + NUM_DRUM_VOICES),
            samples_run: 0,
            ring_src: 0,
            ring_peak: 0.0,
            lufs: Lufs::new(sr),
            bus_idle: [0; NUM_BUSES],
            key_buf: (0..MASTER).map(|_| vec![0.0; MAX_BLOCK]).collect(),
            sc_env: [0.0; NUM_BUSES],
            pad_plugins: (0..MAX_PADS).map(|_| None).collect(),
            plugin_pads: Vec::with_capacity(MAX_PADS),
            fx_plugins: (0..NUM_BUSES * 3).map(|_| None).collect(),
            analyzers: (0..crate::shared::NUM_ANALYZERS).map(|_| (None, MASTER as u8)).collect(),
            plug_notes: [None; 64],
            input: None,
            in_ratio: 1.0,
            in_phase: 0.0,
            in_prev: [0.0; 2],
            in_next: [0.0; 2],
            in_dest: 1,
            in_gain: 1.0,
            in_peak: 0.0,
            flush_in: 0,
            in_mode: 0,
            in_env: [0.0; 2],
            in_one_sided: false,
            in_fold: 0.0,
            in_ring_peak: 0.0,
            vox_key: vec![0.0; MAX_BLOCK],
            in_buf: [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]],
        }
    }

    fn trash(&mut self, g: Garbage) {
        // If the queue is full the drop happens here; rare, and better than leaking.
        let _ = self.garbage.push(g);
    }

    fn silence_pad(&mut self, pad: u16) {
        for v in self.voices.iter_mut() {
            if v.active && (v.pad == pad || v.owner == pad) {
                v.killed = true;
            }
        }
        for v in self.synths.iter_mut() {
            if v.active && v.pad == pad {
                v.released = true;
            }
        }
        for v in self.drums.iter_mut() {
            if v.active && v.pad == pad {
                v.killed = true;
            }
        }
    }

    /// Puts every control automation has moved back to where it was set by hand. The
    /// next block applies whatever automation is current again.
    fn release_automation(&mut self) {
        self.buses = self.bus_base;
        for k in 0..self.auto_pads.len() {
            let pad = self.auto_pads[k] as usize;
            self.pads[pad].cfg = self.pad_base[pad];
        }
        self.auto_pads.clear();
        // Plugin parameters are let go of by the plugins themselves on their next block,
        // when nothing has automated them in it.
    }

    /// Moves the automated controls to where the playing loop's lanes are at the playhead.
    fn apply_automation(&mut self) {
        if let Some(lanes) = self.automation.clone() {
            self.apply_lanes(&lanes, self.tick);
        }
    }

    /// Moves the controls `lanes` automate to where the lanes are at tick `at` of their loop.
    fn apply_lanes(&mut self, lanes: &Automation, at: f64) {
        self.buses = self.bus_base;
        self.layer_lanes(lanes, at);
    }

    /// As `apply_lanes`, on top of what other lanes have already moved this block (the
    /// buses are not put back first).
    fn layer_lanes(&mut self, lanes: &Automation, at: f64) {
        let Engine { buses, pads, auto_pads, pad_plugins, fx_plugins, .. } = self;
        for lane in lanes.iter() {
            let Some(v) = lane.value_at(at) else { continue };
            let value = lane.target.scale_up(v);
            match lane.target {
                AutoTarget::BusParam { bus, slot, param } => {
                    if let Some(slot) = buses.get_mut(bus as usize).and_then(|b| b.slots.get_mut(slot as usize)) {
                        match param {
                            0..=2 => slot.params[param as usize] = value,
                            3 | 4 => slot.sliders[param as usize - 3] = value,
                            _ => {}
                        }
                    }
                }
                AutoTarget::BusLevel(b) => {
                    if let Some(bus) = buses.get_mut(b as usize) {
                        bus.level = value;
                    }
                }
                AutoTarget::BusPan(b) => {
                    if let Some(bus) = buses.get_mut(b as usize) {
                        bus.pan = value;
                    }
                }
                AutoTarget::PadVol(p) | AutoTarget::PadPan(p) | AutoTarget::PadCutoff(p) | AutoTarget::DrumMacro { pad: p, .. } => {
                    let Some(pad) = pads.get_mut(p as usize) else { continue };
                    if !auto_pads.contains(&p) {
                        auto_pads.push(p);
                    }
                    match lane.target {
                        AutoTarget::PadVol(_) => pad.cfg.vol = value,
                        AutoTarget::PadPan(_) => pad.cfg.pan = value,
                        AutoTarget::DrumMacro { m, .. } => {
                            if let Some(k) = pad.cfg.drum.m.get_mut(m as usize) {
                                *k = value;
                            }
                        }
                        _ => pad.cfg.cutoff = value,
                    }
                }
                AutoTarget::PadPlugin { pad, id } => {
                    if let Some(rt) = pad_plugins.get_mut(pad as usize).and_then(|p| p.as_mut()) {
                        rt.automate(id, value);
                    }
                }
                AutoTarget::FxPlugin { slot, id } => {
                    if let Some(rt) = fx_plugins.get_mut(slot as usize).and_then(|p| p.as_mut()) {
                        rt.automate(id, value);
                    }
                }
            }
        }
    }

    /// Sends a plugin the engine has let go of back to the UI to drop, telling it first
    /// that processing has ended, unless `same` (the same instance took its place, as
    /// when the UI resends everything).
    fn retire_plugin(&mut self, old: Option<Box<PluginRt>>, same: bool) {
        let Some(mut old) = old else { return };
        if !same {
            old.stop();
        }
        self.trash(Garbage::Plugin(old));
    }

    /// Cuts everything sounding in choke group `group`: samples and drum hits alike.
    fn choke(&mut self, group: u8) {
        for v in self.voices.iter_mut() {
            if v.active && self.pads[v.pad as usize].cfg.choke == group {
                v.killed = true;
            }
        }
        for v in self.drums.iter_mut() {
            if v.active && self.pads[v.pad as usize].cfg.choke == group {
                v.killed = true;
            }
        }
    }

    /// The playing loop has reached its end (or the bar line a queued loop was waiting
    /// for). Brings in the queued loop if there is one; otherwise the same loop goes round.
    ///
    /// `frame` is where in the input ring the new loop begins, for lining up vocal takes.
    pub(super) fn next_scene(&mut self, frame: u64) {
        self.scene_pass += 1;
        self.shared.loop_frame.store(frame, Ordering::Relaxed);
        // Start clean: a long sample or a held synth note from the loop that just ended
        // would otherwise carry on over the next one, or double up with its own restart.
        // Short hits are left to ring out.
        for k in 0..self.seq_pads.len() {
            let pad = self.seq_pads[k];
            let p = &self.pads[pad as usize];
            // A vocal pad is left alone: its comp is as long as its loop, and a free take
            // is meant to run on across loops.
            let long = p.sample.as_ref().is_some_and(|s| s.len() as f32 > self.sr * 2.0);
            let lingers = match p.cfg.kind {
                PadKind::Midi => true,
                PadKind::Audio => long,
                PadKind::Vocal | PadKind::Drum => false,
            };
            if lingers {
                self.silence_pad(pad);
            }
        }
        if let Some(mut q) = self.queued.take() {
            for k in 0..q.patterns.len() {
                let pad = q.patterns[k].0;
                if (pad as usize) < MAX_PADS {
                    std::mem::swap(&mut self.pads[pad as usize].pattern, &mut q.patterns[k].1);
                    self.refresh_seq(pad);
                }
            }
            std::mem::swap(&mut self.automation, &mut q.automation);
            self.release_automation();
            self.scene_len = q.len_ticks;
            self.shared.scene_index.store(q.index, Ordering::Relaxed);
            self.trash(Garbage::Scene(q));
        }
        self.shared.scene_starts.fetch_add(1, Ordering::Relaxed);
    }

    fn refresh_seq(&mut self, pad: u16) {
        let p = &self.pads[pad as usize];
        // Audio pads carry patterns too: that is the step sequencer.
        let want = p.pattern.is_some();
        let at = self.seq_pads.iter().position(|&x| x == pad);
        match (want, at) {
            (true, None) => self.seq_pads.push(pad),
            (false, Some(i)) => {
                self.seq_pads.swap_remove(i);
            }
            _ => {}
        }
    }

    /// Ends hosted-plugin notes: every one, or just those a given pad started.
    fn plugin_notes_off(&mut self, pad: Option<u16>) {
        for i in 0..self.plug_notes.len() {
            let Some(pn) = self.plug_notes[i] else { continue };
            if pad.is_none_or(|p| p == pn.pad) {
                if let Some(rt) = self.pad_plugins[pn.pad as usize].as_mut() {
                    rt.push(PlugEv { on: false, note: pn.note, vel: 0, offset: 0 });
                }
                self.plug_notes[i] = None;
            }
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::SetPadPlugin { pad, plugin } => {
                if (pad as usize) < MAX_PADS {
                    for n in self.plug_notes.iter_mut() {
                        if matches!(n, Some(x) if x.pad == pad) {
                            *n = None;
                        }
                    }
                    let has = plugin.is_some();
                    let old = std::mem::replace(&mut self.pad_plugins[pad as usize], plugin);
                    let same = same_instance(&old, &self.pad_plugins[pad as usize]);
                    self.retire_plugin(old, same);
                    let at = self.plugin_pads.iter().position(|&x| x == pad);
                    match (has, at) {
                        (true, None) => self.plugin_pads.push(pad),
                        (false, Some(i)) => {
                            self.plugin_pads.swap_remove(i);
                        }
                        _ => {}
                    }
                }
            }
            Cmd::SetFxPlugin { slot, plugin } => {
                if let Some(at) = self.fx_plugins.get_mut(slot as usize) {
                    let old = std::mem::replace(at, plugin);
                    let same = same_instance(&old, at);
                    self.retire_plugin(old, same);
                }
            }
            Cmd::SetAnalyzer { slot, plugin, source } => {
                if let Some(at) = self.analyzers.get_mut(slot as usize) {
                    at.1 = source.min(MASTER as u8);
                    let old = std::mem::replace(&mut at.0, plugin);
                    let same = same_instance(&old, &at.0);
                    self.retire_plugin(old, same);
                }
            }
            Cmd::PadOn { pad, vel, semis, key } => {
                if (pad as usize) < MAX_PADS {
                    if let Some(slot) = self.held.iter_mut().find(|h| h.is_none()) {
                        *slot = Some(Held { pad, key, vel, semis });
                    }
                    self.trigger(Event { pad, vel, semis, key, offset: 0, gate: -1, locks: NO_LOCKS }, true);
                }
            }
            Cmd::PadOff { pad, key } => {
                for h in self.held.iter_mut() {
                    if matches!(h, Some(x) if x.pad == pad && x.key == key) {
                        *h = None;
                    }
                }
                for v in self.voices.iter_mut() {
                    if v.active && v.owner == pad && v.key == key && v.gated {
                        v.releasing = true;
                    }
                }
                for v in self.synths.iter_mut() {
                    if v.active && v.pad == pad && v.key == key {
                        v.released = true;
                    }
                }
                for i in 0..self.plug_notes.len() {
                    let Some(pn) = self.plug_notes[i] else { continue };
                    if pn.pad == pad && pn.key == key {
                        if let Some(rt) = self.pad_plugins[pad as usize].as_mut() {
                            rt.push(PlugEv { on: false, note: pn.note, vel: 0, offset: 0 });
                        }
                        self.plug_notes[i] = None;
                    }
                }
            }
            Cmd::SetPadCfg { pad, cfg } => {
                if (pad as usize) < MAX_PADS {
                    let solo_changed = self.pads[pad as usize].cfg.solo != cfg.solo;
                    self.pads[pad as usize].cfg = cfg;
                    self.pad_base[pad as usize] = cfg;
                    if solo_changed {
                        self.any_solo = self.pads.iter().any(|p| p.cfg.solo);
                    }
                    self.refresh_seq(pad);
                }
            }
            Cmd::SetSample { pad, sample } => {
                if (pad as usize) < MAX_PADS {
                    for v in self.voices.iter_mut() {
                        if v.pad == pad {
                            v.active = false;
                        }
                    }
                    let old = std::mem::replace(&mut self.pads[pad as usize].sample, sample);
                    if let Some(old) = old {
                        self.trash(Garbage::Sample(old));
                    }
                }
            }
            Cmd::SetPattern { pad, pattern } => {
                if (pad as usize) < MAX_PADS {
                    let old = std::mem::replace(&mut self.pads[pad as usize].pattern, pattern);
                    if let Some(old) = old {
                        self.trash(Garbage::Pattern(old));
                    }
                    self.refresh_seq(pad);
                }
            }
            Cmd::SetBus { bus, cfg } => {
                if (bus as usize) < NUM_BUSES {
                    self.buses[bus as usize] = cfg;
                    self.bus_base[bus as usize] = cfg;
                }
            }
            Cmd::SetAutomation(lanes) => {
                if let Some(old) = std::mem::replace(&mut self.automation, lanes) {
                    self.trash(Garbage::Automation(old));
                }
                self.release_automation();
            }
            Cmd::SetScale(mask) => self.scale = mask,
            Cmd::SetVocalFx(fx) => self.vocal_fx = fx,
            Cmd::SceneLen(len) => {
                self.scene_len = len;
                if len > 0 {
                    self.tick = self.tick.rem_euclid(len as f64);
                }
            }
            Cmd::QueueScene(scene) => {
                if let Some(old) = std::mem::replace(&mut self.queued, scene) {
                    self.trash(Garbage::Scene(old));
                }
            }
            Cmd::Song(cmd) => self.handle_song(cmd),
            Cmd::Play(on) => {
                if on && !self.playing {
                    // A loop plays from its top; the song from wherever its playhead is.
                    self.tick = if self.song_mode { self.song_pos } else { 0.0 };
                    self.song_jump = true;
                    self.song_fading = false;
                    self.scene_pass = 0;
                    self.song_tick = 0.0;
                    self.shared.loop_frame.store(self.shared.input_ring.written(), Ordering::Relaxed);
                    self.shared.play_starts.fetch_add(1, Ordering::Relaxed);
                }
                if !on {
                    // Stop also ends sequenced notes and free-running loops.
                    for v in self.voices.iter_mut() {
                        if v.active && (v.gate_left >= 0 || self.pads[v.pad as usize].cfg.mode == PlayMode::Loop) {
                            v.killed = true;
                        }
                    }
                    for v in self.synths.iter_mut() {
                        v.released = true;
                    }
                    self.plugin_notes_off(None);
                    // Automated controls go back to where they were set by hand.
                    self.release_automation();
                    self.song_fading = self.playing && self.song_mode;
                }
                self.playing = on;
                self.shared.playing.store(on, Ordering::Relaxed);
            }
            Cmd::SetBpm(b) => self.bpm = b.clamp(20.0, 300.0),
            Cmd::SetSwing(s) => self.swing = s.clamp(0.5, 0.75),
            Cmd::Metronome(on) => self.metronome = on,
            Cmd::RingSource(s) => self.ring_src = s.min(6),
            Cmd::StopPad { pad } => {
                self.silence_pad(pad);
                for h in self.held.iter_mut() {
                    if matches!(h, Some(x) if x.pad == pad) {
                        *h = None;
                    }
                }
                self.plugin_notes_off(Some(pad));
            }
            Cmd::SetInput { rx, ratio } => {
                let old = std::mem::replace(&mut self.input, rx);
                if let Some(old) = old {
                    self.trash(Garbage::Input(old));
                }
                self.in_ratio = ratio as f64;
                self.in_phase = 0.0;
                self.in_prev = [0.0; 2];
                self.in_next = [0.0; 2];
            }
            Cmd::InputCfg { dest, gain } => {
                self.in_dest = dest.min(MASTER as u8 + 1);
                self.in_gain = gain;
            }
            Cmd::InputMode(mode) => self.in_mode = mode.min(2),
            Cmd::Roll { on, ticks } => {
                self.roll_on = on;
                self.roll_ticks = ticks.max(10);
            }
            Cmd::Ping => self.ping_pos = 0,
            Cmd::Preview(s, gain) => {
                if let Some(old) = std::mem::replace(&mut self.preview, s) {
                    self.trash(Garbage::Sample(old));
                }
                self.preview_pos = 0;
                self.preview_gain = gain;
            }
            Cmd::Panic => {
                for v in self.voices.iter_mut() {
                    v.killed = true;
                }
                for v in self.drums.iter_mut() {
                    v.killed = true;
                }
                for v in self.synths.iter_mut() {
                    v.released = true;
                }
                self.held = [None; MAX_HELD];
                self.plugin_notes_off(None);
                // The voices take a few milliseconds to fade; the effects are emptied once
                // they have, or the fade itself would be left echoing in them.
                self.flush_in = (self.sr * 0.006) as i32;
            }
        }
    }

    fn trigger(&mut self, ev: Event, live: bool) {
        let cfg = self.pads[ev.pad as usize].cfg;
        if cfg.mute {
            return;
        }
        match cfg.kind {
            PadKind::Audio | PadKind::Vocal => self.start_voice(ev.pad, ev, 1.0, false, live),
            PadKind::Drum => self.start_drum(ev, &cfg),
            PadKind::Midi => match cfg.target {
                MidiTarget::Synth => {
                    self.age += 1;
                    let age = self.age;
                    let slot = pick(&mut self.synths, |v| v.active, |v| v.age);
                    let note = 60.0 + ev.semis + cfg.synth.octave as f32 * 12.0 + cfg.pitch;
                    slot.start(ev.pad, ev.key, note, ev.vel, ev.offset, ev.gate, age);
                }
                MidiTarget::Pad(p) => {
                    if (p as usize) < MAX_PADS && self.pads[p as usize].cfg.kind != PadKind::Midi {
                        self.start_voice(p, ev, cfg.vol, true, live);
                    }
                }
                MidiTarget::Plugin => {
                    if let Some(rt) = self.pad_plugins[ev.pad as usize].as_mut() {
                        let note = (60.0 + ev.semis + cfg.pitch).round().clamp(0.0, 127.0) as u8;
                        let vel = ((ev.vel * 127.0) as u8).clamp(1, 127);
                        rt.push(PlugEv { on: true, note, vel, offset: ev.offset });
                        if let Some(slot) = self.plug_notes.iter_mut().find(|s| s.is_none()) {
                            let left = if ev.gate >= 0 { ev.offset as i64 + ev.gate.max(1) } else { -1 };
                            *slot = Some(PlugNote { pad: ev.pad, key: ev.key, note, left });
                        }
                    }
                }
            },
        }
    }

    fn start_voice(&mut self, pad: u16, ev: Event, gain: f32, via_midi: bool, live: bool) {
        let p = &self.pads[pad as usize];
        let cfg = p.cfg;
        let Some(sample) = &p.sample else { return };
        let len = sample.len();
        let end = if cfg.end == 0 || cfg.end as usize > len { len } else { cfg.end as usize };
        let start = (cfg.start as usize).min(end);

        // Loop pads latch: a second hit stops the loop instead of stacking another copy.
        if live && !via_midi && cfg.mode == PlayMode::Loop {
            let mut stopped = false;
            for v in self.voices.iter_mut() {
                if v.active && v.pad == pad && !v.releasing && !v.killed {
                    v.releasing = true;
                    stopped = true;
                }
            }
            if stopped {
                return;
            }
        }
        // A sequenced loop restarts rather than piling up copies of itself.
        if !live && !via_midi && cfg.mode == PlayMode::Loop {
            for v in self.voices.iter_mut() {
                if v.active && v.pad == pad {
                    v.killed = true;
                }
            }
        }
        // Hitting a long sample again restarts it instead of piling another copy on top;
        // only the same note is cut, so chords played from one pad still stack. Short hits
        // are left to overlap, which is what makes a fast roll sound natural.
        let rate = 2f64.powf(ev.semis as f64 / 12.0);
        if cfg.mode != PlayMode::Loop && (end - start) as f32 > self.sr * RETRIGGER_SECS {
            for v in self.voices.iter_mut() {
                if v.active && v.pad == pad && v.owner == ev.pad && v.rate == rate {
                    v.killed = true;
                }
            }
        }
        if cfg.choke > 0 {
            self.choke(cfg.choke);
        }
        self.age += 1;
        let age = self.age;
        let slot = pick(&mut self.voices, |v| v.active, |v| v.age);
        *slot = Voice {
            active: true,
            pad,
            owner: ev.pad,
            key: ev.key,
            pos: if cfg.reverse { end as f64 - 1.0 } else { start as f64 },
            rate,
            gain: ev.vel * gain,
            env: 0.0,
            gated: via_midi || cfg.mode == PlayMode::Gate,
            releasing: false,
            killed: false,
            gate_left: if via_midi || cfg.mode == PlayMode::Gate { ev.gate } else { -1 },
            offset: ev.offset,
            age,
            svf: [Svf::default(); 2],
        };
    }

    /// Starts a drum synth hit: the patch resolved for this velocity and step, tuned by the
    /// note played, and held to the project scale if the patch asks for it.
    fn start_drum(&mut self, ev: Event, cfg: &PadCfg) {
        if cfg.choke > 0 {
            self.choke(cfg.choke);
        }
        // A pad hit again cuts its own last hit at the same pitch, as a drum machine does.
        for v in self.drums.iter_mut() {
            if v.active && v.pad == ev.pad && v.semis == ev.semis {
                v.killed = true;
            }
        }
        self.age += 1;
        let hit = Hit { pad: ev.pad, note: ev.semis, tune: ev.semis + cfg.pitch, vel: ev.vel, offset: ev.offset, age: self.age, locks: ev.locks };
        let ctx = Ctx { sr: self.sr, bpm: self.bpm, scale: self.scale };
        let mut rng = Rng(self.drum_seed | 1);
        pick(&mut self.drums, |v| v.active, |v| v.age).start(&cfg.drum, &hit, ctx, &mut rng);
        self.drum_seed = rng.0;
    }

    /// Fills `out` (interleaved, `channels` wide) with the next block of audio.
    pub fn process(&mut self, out: &mut [f32], channels: usize) {
        let t0 = std::time::Instant::now();
        while let Ok(cmd) = self.rx.pop() {
            self.handle(cmd);
        }
        while let Ok(cmd) = self.rx_midi.pop() {
            self.handle(cmd);
        }
        let frames = out.len() / channels.max(1);
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(MAX_BLOCK);
            self.render_block(n);
            let (l, r) = (&self.bufs[MASTER][0], &self.bufs[MASTER][1]);
            for i in 0..n {
                let f = &mut out[(done + i) * channels..(done + i + 1) * channels];
                f.fill(0.0);
                if channels == 1 {
                    f[0] = ((l[i] + r[i]) * 0.5).clamp(-1.0, 1.0);
                } else {
                    f[0] = l[i].clamp(-1.0, 1.0);
                    f[1] = r[i].clamp(-1.0, 1.0);
                }
            }
            done += n;
        }
        let budget = frames as f32 / self.sr;
        if budget > 0.0 {
            f32_store(&self.shared.load, t0.elapsed().as_secs_f32() / budget);
        }
    }
}

/// Filter and envelope state for the vocal bus's gate, de-esser and midrange ducking.
#[derive(Default)]
pub(super) struct VocalState {
    pub gate_env: f32,
    pub gate_gain: f32,
    pub gate_open: bool,
    pub ess: [Svf; 2],
    pub ess_hi: f32,
    pub ess_all: f32,
    /// How strongly the vocal is present, 0..1, for the bus that makes room for it.
    pub presence: f32,
    pub duck: [Svf; 4],
}

/// Returns a free slot, or steals the oldest one.
fn pick<T>(pool: &mut [T], active: impl Fn(&T) -> bool, age: impl Fn(&T) -> u64) -> &mut T {
    let idx = pool
        .iter()
        .position(|v| !active(v))
        .unwrap_or_else(|| (0..pool.len()).min_by_key(|&i| age(&pool[i])).unwrap_or(0));
    &mut pool[idx]
}

/// True when a slot's old and new plugin handles are the same instance.
fn same_instance(old: &Option<Box<PluginRt>>, new: &Option<Box<PluginRt>>) -> bool {
    matches!((old, new), (Some(a), Some(b)) if a.same_plugin(b))
}
