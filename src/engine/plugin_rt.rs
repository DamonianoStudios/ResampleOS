//! The audio thread's side of a hosted VST3 plugin.

use super::MAX_BLOCK;
use std::sync::{Arc, Mutex};
use vst3_host::Plugin;
use vst3_host::audio::AudioBuffers;
use vst3_host::midi::{MidiChannel, MidiEvent};

#[derive(Clone, Copy)]
pub(super) struct PlugEv {
    pub on: bool,
    pub note: u8,
    pub vel: u8,
    pub offset: u32,
}

/// The audio thread's handle on a hosted plugin. The plugin itself sits behind a mutex
/// shared with the UI (editor, state, parameters); the audio thread only ever `try_lock`s,
/// so a busy UI costs one silent block rather than a stall.
pub struct PluginRt {
    handle: Arc<Mutex<Plugin>>,
    pub(super) buf: AudioBuffers,
    pending: [PlugEv; 64],
    n_pending: usize,
}

impl PluginRt {
    pub fn new(handle: Arc<Mutex<Plugin>>, sample_rate: f64) -> Box<Self> {
        Box::new(PluginRt {
            handle,
            // Sized to the largest block up front; later resizes stay within capacity.
            buf: AudioBuffers::new(2, 2, MAX_BLOCK, sample_rate),
            pending: [PlugEv { on: false, note: 0, vel: 0, offset: 0 }; 64],
            n_pending: 0,
        })
    }

    pub(super) fn push(&mut self, ev: PlugEv) {
        if self.n_pending < self.pending.len() {
            self.pending[self.n_pending] = ev;
            self.n_pending += 1;
        }
    }

    /// Sizes the buffers for an `n`-frame block and zeroes them.
    pub(super) fn prepare(&mut self, n: usize) {
        for ch in self.buf.inputs.iter_mut().chain(self.buf.outputs.iter_mut()) {
            ch.clear();
            ch.resize(n, 0.0);
        }
    }

    /// Delivers queued notes and renders one block. False if the plugin was busy or failed;
    /// queued notes are kept for the next block in that case.
    pub(super) fn run(&mut self, n: usize) -> bool {
        let Ok(mut p) = self.handle.try_lock() else { return false };
        for ev in &self.pending[..self.n_pending] {
            let (channel, note) = (MidiChannel::Ch1, ev.note);
            let m = if ev.on {
                MidiEvent::NoteOn { channel, note, velocity: ev.vel }
            } else {
                MidiEvent::NoteOff { channel, note, velocity: 0 }
            };
            let _ = p.send_midi_event_at(m, (ev.offset as usize).min(n.saturating_sub(1)) as i32);
        }
        self.n_pending = 0;
        p.process_audio(&mut self.buf).is_ok()
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
