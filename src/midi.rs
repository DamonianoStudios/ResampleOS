//! MIDI controller input. Pad hits go straight from the MIDI thread to the engine so
//! they are not delayed by the UI frame rate; everything is also forwarded to the UI.

use crate::engine::Cmd;
use crate::shared::Shared;
use midir::{Ignore, MidiInput, MidiInputConnection};
use rtrb::Producer;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// First of the 16 pad notes (C1), the layout most pad controllers ship with.
pub const PAD_NOTE_BASE: u8 = 36;
pub const CC_KNOBS: [u8; 3] = [70, 71, 72];
pub const CC_BANK_DOWN: u8 = 20;
pub const CC_BANK_UP: u8 = 21;
pub const CC_RESAMPLE: u8 = 22;
pub const CC_SKIPBACK: u8 = 23;
pub const CC_ROLL: u8 = 24;
pub const CC_CHROMATIC: u8 = 25;

#[derive(Clone, Copy, Debug)]
pub enum MidiEvent {
    /// `pad` is the pad that sounded (or would have, while picking a resample target).
    NoteOn { pad: u16, semis: f32, vel: f32, key: u32, is_pad: bool },
    NoteOff { key: u32 },
    Cc { cc: u8, value: u8 },
}

pub struct Midi {
    pub connections: Vec<MidiInputConnection<()>>,
    pub port_names: Vec<String>,
}

/// Semitone offset of scale degree `idx`, for a scale given as a 12-bit pitch-class mask.
fn degree_semis(mask: u32, idx: i32) -> i32 {
    let n = (mask & 0xfff).count_ones() as i32;
    if n == 0 {
        return idx;
    }
    let within = (0..12).filter(|b| mask & (1 << b) != 0).nth(idx.rem_euclid(n) as usize).unwrap_or(0);
    idx.div_euclid(n) * 12 + within
}

/// Works out what a note means given the UI's current mode.
/// Returns (pad, semitone offset, came from a pad-range note).
pub fn map_note(shared: &Shared, note: u8) -> (u16, f32, bool) {
    let octave = shared.ui_octave.load(Ordering::Relaxed) as i32 * 12;
    let selected = shared.ui_selected.load(Ordering::Relaxed) as u16;
    let in_pads = (PAD_NOTE_BASE..PAD_NOTE_BASE + 16).contains(&note);
    if in_pads {
        let i = (note - PAD_NOTE_BASE) as i32;
        if shared.ui_chromatic.load(Ordering::Relaxed) {
            let mask = shared.ui_scale_mask.load(Ordering::Relaxed);
            (selected, (degree_semis(mask, i) + octave) as f32, true)
        } else {
            (shared.ui_bank.load(Ordering::Relaxed) as u16 * 16 + i as u16, 0.0, true)
        }
    } else {
        // Anything outside the pad block is a keyboard: play the selected pad melodically.
        (selected, note as f32 - 60.0, false)
    }
}

pub fn start(shared: Arc<Shared>, tx: Producer<Cmd>, ui: Sender<MidiEvent>) -> Midi {
    let mut midi = Midi { connections: Vec::new(), port_names: Vec::new() };
    let Ok(probe) = MidiInput::new("ResampleOS") else { return midi };
    // One engine queue shared by every port's callback thread.
    let tx = Arc::new(Mutex::new(tx));
    for port in probe.ports() {
        let Ok(mut input) = MidiInput::new("ResampleOS") else { continue };
        input.ignore(Ignore::All);
        let name = input.port_name(&port).unwrap_or_else(|_| "MIDI in".into());
        let (shared, tx, ui) = (shared.clone(), tx.clone(), ui.clone());
        // Remembers where each held note went so its release finds the same voice even
        // if the bank or mode changed in between.
        let mut held: [Option<(u16, u32)>; 128] = [None; 128];
        let conn = input.connect(
            &port,
            "resampleos-in",
            move |_, msg, _| {
                if msg.len() < 3 {
                    return;
                }
                let (status, d1, d2) = (msg[0] & 0xf0, msg[1] & 0x7f, msg[2] & 0x7f);
                match status {
                    0x90 if d2 > 0 => {
                        let (pad, semis, is_pad) = map_note(&shared, d1);
                        let key = 0x4000_0000 | d1 as u32;
                        let vel = (d2 as f32 / 127.0).powf(0.7);
                        if !shared.ui_capture_pads.load(Ordering::Relaxed) {
                            if let Ok(mut tx) = tx.lock() {
                                let _ = tx.push(Cmd::PadOn { pad, vel, semis, key });
                            }
                            held[d1 as usize] = Some((pad, key));
                        }
                        let _ = ui.send(MidiEvent::NoteOn { pad, semis, vel, key, is_pad });
                    }
                    0x80 | 0x90 => {
                        if let Some((pad, key)) = held[d1 as usize].take() {
                            if let Ok(mut tx) = tx.lock() {
                                let _ = tx.push(Cmd::PadOff { pad, key });
                            }
                            let _ = ui.send(MidiEvent::NoteOff { key });
                        }
                    }
                    0xb0 => {
                        let _ = ui.send(MidiEvent::Cc { cc: d1, value: d2 });
                    }
                    _ => {}
                }
            },
            (),
        );
        if let Ok(conn) = conn {
            midi.connections.push(conn);
            midi.port_names.push(name);
        }
    }
    midi
}
