//! The audio thread's side of a hosted plugin, VST3 or CLAP.

use super::MAX_BLOCK;
use crate::hosted::PluginHandle;
use vst3_host::audio::AudioBuffers;
use vst3_host::midi::{MidiChannel, MidiEvent};

/// How long the audio thread retries a contended plugin lock: tens of microseconds.
const LOCK_SPINS: u32 = 4000;
/// Parameters of one plugin that automation can move at once.
const MAX_AUTO: usize = 32;

#[derive(Clone, Copy)]
pub(super) struct PlugEv {
    pub on: bool,
    pub note: u8,
    pub vel: u8,
    pub offset: u32,
}

/// A parameter automation is moving: where the lane has it, what was last sent, and
/// where it was before automation took it, to put it back after.
#[derive(Clone, Copy, Default)]
struct AutoParam {
    id: u32,
    value: f64,
    sent: f64,
    base: Option<f64>,
    /// Moved by a lane in the current block.
    seen: bool,
}

/// The audio thread's handle on a hosted plugin. The plugin itself sits behind a mutex
/// shared with the UI (editor, state, parameters); the audio thread never parks on it. It
/// retries for a bounded moment, which outlasts the UI's per-frame servicing, and gives the
/// block up if the UI is doing something slower.
pub struct PluginRt {
    handle: PluginHandle,
    pub(super) buf: AudioBuffers,
    pending: [PlugEv; 64],
    n_pending: usize,
    auto: [AutoParam; MAX_AUTO],
    n_auto: usize,
    /// Frames an effect has put out nothing but zeros while being fed a clear signal.
    silent_frames: u64,
}

/// Input above this (-40 dBFS) counts as a clear signal for the silence watchdog.
const WATCH_INPUT: f32 = 0.01;
/// How long an effect may put out pure silence for a clear signal before it is passed by.
const WATCH_SECS: f64 = 3.0;

impl PluginRt {
    pub fn new(handle: PluginHandle, sample_rate: f64) -> Box<Self> {
        Box::new(PluginRt {
            handle,
            // Sized to the largest block up front; later resizes stay within capacity.
            buf: AudioBuffers::new(2, 2, MAX_BLOCK, sample_rate),
            pending: [PlugEv { on: false, note: 0, vel: 0, offset: 0 }; 64],
            n_pending: 0,
            auto: [AutoParam::default(); MAX_AUTO],
            n_auto: 0,
            silent_frames: 0,
        })
    }

    pub(super) fn push(&mut self, ev: PlugEv) {
        if self.n_pending < self.pending.len() {
            self.pending[self.n_pending] = ev;
            self.n_pending += 1;
        }
    }

    /// A lane has parameter `id` at normalized `value` in this block.
    pub(super) fn automate(&mut self, id: u32, value: f32) {
        let value = value.clamp(0.0, 1.0) as f64;
        let k = match self.auto[..self.n_auto].iter().position(|a| a.id == id) {
            Some(k) => k,
            None if self.n_auto < MAX_AUTO => {
                self.auto[self.n_auto] = AutoParam { id, value, sent: -1.0, base: None, seen: true };
                self.n_auto += 1;
                return;
            }
            None => return,
        };
        self.auto[k].value = value;
        self.auto[k].seen = true;
    }

    /// Watches an effect for passing no audio at all, as a few plugins do when hosted here:
    /// call after [`PluginRt::run`] with the peak of what went in. True once it has put out
    /// pure silence for a clear signal for a few seconds; the engine then passes the bus
    /// dry rather than muting it. It is still run, and is heard again as soon as it makes
    /// any sound.
    pub(super) fn gone_silent(&mut self, n: usize, input_peak: f32, sample_rate: f32) -> bool {
        let out_silent = self.buf.outputs.iter().all(|ch| ch[..n.min(ch.len())].iter().all(|v| *v == 0.0));
        if !out_silent {
            self.silent_frames = 0;
        } else if input_peak > WATCH_INPUT {
            self.silent_frames += n as u64;
        }
        self.silent_frames as f64 > WATCH_SECS * sample_rate as f64
    }

    /// Same plugin instance as `other`?
    pub(super) fn same_plugin(&self, other: &PluginRt) -> bool {
        self.handle.id() == other.handle.id()
    }

    /// Tells a CLAP plugin that processing has ended, from the audio thread as CLAP asks,
    /// before the engine lets go of it.
    pub(super) fn stop(&mut self) {
        if let PluginHandle::Clap(h) = &self.handle
            && let Some(mut p) = lock(h)
        {
            p.stop_processing();
        }
    }

    /// Sizes the buffers for an `n`-frame block and zeroes them.
    pub(super) fn prepare(&mut self, n: usize) {
        for ch in self.buf.inputs.iter_mut().chain(self.buf.outputs.iter_mut()) {
            ch.clear();
            ch.resize(n, 0.0);
        }
    }

    /// Delivers queued notes and automation, and renders one block. False if the plugin
    /// was busy or failed; queued notes are kept for the next block in that case.
    pub(super) fn run(&mut self, n: usize) -> bool {
        match &self.handle {
            PluginHandle::Vst3(h) => {
                let mut p = match lock(h) {
                    Some(p) => p,
                    None => return false,
                };
                for ev in &self.pending[..self.n_pending] {
                    let (channel, note) = (MidiChannel::Ch1, ev.note);
                    let m = if ev.on {
                        MidiEvent::NoteOn { channel, note, velocity: ev.vel }
                    } else {
                        MidiEvent::NoteOff { channel, note, velocity: 0 }
                    };
                    let _ = p.send_midi_event_at(m, (ev.offset as usize).min(n.saturating_sub(1)) as i32);
                }
                sync_automation(&mut self.auto, &mut self.n_auto, &mut *p, |p, id| p.get_parameter(id).ok(), |p, id, v| {
                    let _ = p.set_parameter_at(id, v, 0);
                });
                self.n_pending = 0;
                p.process_audio(&mut self.buf).is_ok()
            }
            PluginHandle::Clap(h) => {
                let mut p = match lock(h) {
                    Some(p) => p,
                    None => return false,
                };
                for ev in &self.pending[..self.n_pending] {
                    p.queue_note(ev.on, ev.note, ev.vel as f64 / 127.0, (ev.offset as usize).min(n.saturating_sub(1)) as u32);
                }
                sync_automation(&mut self.auto, &mut self.n_auto, &mut *p, |p, id| p.get_parameter(id), |p, id, v| p.queue_parameter(id, v));
                self.n_pending = 0;
                let AudioBuffers { inputs, outputs, .. } = &mut self.buf;
                p.process(n, inputs, outputs)
            }
        }
    }
}

fn lock<T>(m: &std::sync::Mutex<T>) -> Option<std::sync::MutexGuard<'_, T>> {
    let mut tries = 0;
    loop {
        match m.try_lock() {
            Ok(p) => return Some(p),
            Err(_) if tries < LOCK_SPINS => {
                tries += 1;
                std::hint::spin_loop();
            }
            Err(_) => return None,
        }
    }
}

/// Sends the plugin what automation has changed since the last block, and hands back the
/// parameters no lane moved this block, at the values they had before automation.
fn sync_automation<P>(auto: &mut [AutoParam; MAX_AUTO], n: &mut usize, p: &mut P, get: impl Fn(&P, u32) -> Option<f64>, set: impl Fn(&mut P, u32, f64)) {
    let mut k = 0;
    while k < *n {
        let a = &mut auto[k];
        if a.seen {
            if a.base.is_none() {
                a.base = Some(get(p, a.id).unwrap_or(a.value));
            }
            if (a.value - a.sent).abs() > 1e-6 {
                set(p, a.id, a.value);
                a.sent = a.value;
            }
            a.seen = false;
            k += 1;
        } else {
            if let Some(base) = a.base {
                set(p, a.id, base);
            }
            auto[k] = auto[*n - 1];
            *n -= 1;
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct PlugNote {
    pub pad: u16,
    pub key: u32,
    pub note: u8,
    /// Samples until note-off; negative while held by the player.
    pub left: i64,
}
