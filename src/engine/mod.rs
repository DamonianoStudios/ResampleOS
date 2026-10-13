//! The real-time engine. Owned by the audio callback; the UI and MIDI threads talk to it
//! through lock-free command queues and read results back through `Shared`.
mod lufs;
mod plugin_rt;
mod render;
#[cfg(test)]
mod tests;
mod types;

pub use plugin_rt::PluginRt;
pub use types::*;

use crate::fx::{Fx, FxKind, Svf, svf_g};
use crate::music::{PPQ, TICKS_PER_BAR, swing_tick};
use crate::sample::{SampleData, hermite};
use crate::shared::{MASTER, MAX_PADS, NUM_BUSES, Shared, f32_load, f32_store};
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
const MAX_HELD: usize = 32;

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
    age: u64,
    any_solo: bool,

    buses: [BusCfg; NUM_BUSES],
    fx: Vec<Fx>,
    bufs: Vec<[Vec<f32>; 2]>,
    scratch: Vec<f32>,
    peaks: [f32; NUM_BUSES * 2],

    playing: bool,
    bpm: f32,
    swing: f32,
    /// Ticks since the playing loop started.
    tick: f64,
    scene_len: u32,
    queued: Option<Box<SceneSwap>>,
    /// How many loops have started since Play, so humanize differs on every pass.
    scene_pass: i64,
    /// Ticks since Play, never wrapped: what tempo-synced LFOs count from.
    song_tick: f64,
    /// Runs whether or not the transport does, so note repeat works when stopped.
    free_tick: f64,
    metronome: bool,
    click_phase: f32,
    click_env: f32,
    click_freq: f32,
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
    plug_notes: [Option<PlugNote>; 64],

    input: Option<Consumer<f32>>,
    in_ratio: f64,
    in_phase: f64,
    in_prev: [f32; 2],
    in_next: [f32; 2],
    in_dest: u8,
    in_gain: f32,
    in_peak: f32,
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
            age: 0,
            any_solo: false,
            buses: [BusCfg::default(); NUM_BUSES],
            fx: (0..NUM_BUSES * 3).map(|_| Fx::new(sr)).collect(),
            bufs: (0..NUM_BUSES).map(|_| [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]]).collect(),
            scratch: vec![0.0; MAX_BLOCK],
            peaks: [0.0; NUM_BUSES * 2],
            playing: false,
            bpm: 90.0,
            swing: 0.5,
            tick: 0.0,
            scene_len: 0,
            queued: None,
            scene_pass: 0,
            song_tick: 0.0,
            free_tick: 0.0,
            metronome: false,
            click_phase: 0.0,
            click_env: 0.0,
            click_freq: 1000.0,
            click_at: -1,
            roll_on: false,
            roll_ticks: PPQ / 4,
            held: [None; MAX_HELD],
            events: Vec::with_capacity(512),
            touched: Vec::with_capacity(NUM_VOICES + NUM_SYNTH_VOICES),
            samples_run: 0,
            ring_src: 0,
            ring_peak: 0.0,
            lufs: Lufs::new(sr),
            bus_idle: [0; NUM_BUSES],
            key_buf: (0..4).map(|_| vec![0.0; MAX_BLOCK]).collect(),
            sc_env: [0.0; NUM_BUSES],
            pad_plugins: (0..MAX_PADS).map(|_| None).collect(),
            plugin_pads: Vec::with_capacity(MAX_PADS),
            fx_plugins: (0..NUM_BUSES * 3).map(|_| None).collect(),
            plug_notes: [None; 64],
            input: None,
            in_ratio: 1.0,
            in_phase: 0.0,
            in_prev: [0.0; 2],
            in_next: [0.0; 2],
            in_dest: 1,
            in_gain: 1.0,
            in_peak: 0.0,
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
    }

    /// The playing loop has reached its end (or the bar line a queued loop was waiting
    /// for). Brings in the queued loop if there is one; otherwise the same loop goes round.
    pub(super) fn next_scene(&mut self) {
        self.scene_pass += 1;
        // Start clean: a long sample or a held synth note from the loop that just ended
        // would otherwise carry on over the next one, or double up with its own restart.
        // Short hits are left to ring out.
        for k in 0..self.seq_pads.len() {
            let pad = self.seq_pads[k];
            let p = &self.pads[pad as usize];
            let lingers = p.cfg.kind == PadKind::Midi || p.sample.as_ref().is_some_and(|s| s.len() as f32 > self.sr * 2.0);
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
                    if let Some(old) = old {
                        self.trash(Garbage::Plugin(old));
                    }
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
                if (slot as usize) < self.fx_plugins.len() {
                    let old = std::mem::replace(&mut self.fx_plugins[slot as usize], plugin);
                    if let Some(old) = old {
                        self.trash(Garbage::Plugin(old));
                    }
                }
            }
            Cmd::PadOn { pad, vel, semis, key } => {
                if (pad as usize) < MAX_PADS {
                    if let Some(slot) = self.held.iter_mut().find(|h| h.is_none()) {
                        *slot = Some(Held { pad, key, vel, semis });
                    }
                    self.trigger(Event { pad, vel, semis, key, offset: 0, gate: -1 }, true);
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
                }
            }
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
            Cmd::Play(on) => {
                if on && !self.playing {
                    self.tick = 0.0;
                    self.scene_pass = 0;
                    self.song_tick = 0.0;
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
                }
                self.playing = on;
                self.shared.playing.store(on, Ordering::Relaxed);
            }
            Cmd::SetBpm(b) => self.bpm = b.clamp(20.0, 300.0),
            Cmd::SetSwing(s) => self.swing = s.clamp(0.5, 0.75),
            Cmd::Metronome(on) => self.metronome = on,
            Cmd::RingSource(s) => self.ring_src = s.min(5),
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
                self.in_dest = dest.min(5);
                self.in_gain = gain;
            }
            Cmd::Roll { on, ticks } => {
                self.roll_on = on;
                self.roll_ticks = ticks.max(10);
            }
            Cmd::Panic => {
                for v in self.voices.iter_mut() {
                    v.killed = true;
                }
                for v in self.synths.iter_mut() {
                    v.released = true;
                }
                self.held = [None; MAX_HELD];
                self.plugin_notes_off(None);
            }
        }
    }

    fn trigger(&mut self, ev: Event, live: bool) {
        let cfg = self.pads[ev.pad as usize].cfg;
        if cfg.mute {
            return;
        }
        match cfg.kind {
            PadKind::Audio => self.start_voice(ev.pad, ev, 1.0, false, live),
            PadKind::Midi => match cfg.target {
                MidiTarget::Synth => {
                    self.age += 1;
                    let age = self.age;
                    let slot = pick(&mut self.synths, |v| v.active, |v| v.age);
                    let note = 60.0 + ev.semis + cfg.synth.octave as f32 * 12.0 + cfg.pitch;
                    slot.start(ev.pad, ev.key, note, ev.vel, ev.offset, ev.gate, age);
                }
                MidiTarget::Pad(p) => {
                    if (p as usize) < MAX_PADS && self.pads[p as usize].cfg.kind == PadKind::Audio {
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
        if cfg.choke > 0 {
            for v in self.voices.iter_mut() {
                if v.active && self.pads[v.pad as usize].cfg.choke == cfg.choke {
                    v.killed = true;
                }
            }
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
            rate: 2f64.powf(ev.semis as f64 / 12.0),
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

#[inline]
fn bus_index(bus: u8) -> usize {
    if (1..=4).contains(&bus) { bus as usize - 1 } else { MASTER }
}

/// Returns a free slot, or steals the oldest one.
fn pick<T>(pool: &mut [T], active: impl Fn(&T) -> bool, age: impl Fn(&T) -> u64) -> &mut T {
    let idx = pool
        .iter()
        .position(|v| !active(v))
        .unwrap_or_else(|| (0..pool.len()).min_by_key(|&i| age(&pool[i])).unwrap_or(0));
    &mut pool[idx]
}
