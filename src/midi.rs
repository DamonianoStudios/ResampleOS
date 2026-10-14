//! MIDI controller input. Pad hits go straight from the MIDI thread to the engine so
//! they are not delayed by the UI frame rate; everything is also forwarded to the UI.

use crate::engine::Cmd;
use crate::shared::Shared;
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use rtrb::Producer;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
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
    pub leds: Leds,
}

impl Midi {
    pub fn none() -> Midi {
        Midi { connections: Vec::new(), port_names: Vec::new(), leds: Leds::default() }
    }
}

/// Pad lights on the controllers: most pad controllers light a pad when sent its own note,
/// brighter (or in another colour) for a higher velocity. Only output ports that belong
/// to a connected input are used, so nothing is ever played on a synth by mistake. A port
/// that sends what it receives straight back (a virtual loopback port, a device with MIDI
/// thru) would turn every light into a pad hit, so notes coming in just after the same
/// note went out are ignored.
#[derive(Default)]
pub struct Leds {
    outs: Vec<MidiOutputConnection>,
    /// The channel the controller sends its pads on, which is where it listens for lights.
    channel: Arc<AtomicU8>,
    /// When each note was last sent as a light, in milliseconds since `EPOCH`.
    echo: Arc<Vec<AtomicU64>>,
    sent: [u8; 16],
}

/// How long after a light is sent the same note coming in counts as its echo.
const ECHO_MS: u64 = 250;

fn now_ms() -> u64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64 + 1
}

/// Pad light levels, as note velocities.
pub const LED_OFF: u8 = 0;
pub const LED_LOADED: u8 = 8;
pub const LED_SELECTED: u8 = 40;
pub const LED_PLAYING: u8 = 127;

impl Leds {
    pub fn connected(&self) -> bool {
        !self.outs.is_empty()
    }

    /// Turns every light off and stops sending them.
    pub fn close(&mut self) {
        self.show([LED_OFF; 16]);
        self.outs.clear();
    }

    /// Lights the sixteen pads; only changes are sent.
    pub fn show(&mut self, levels: [u8; 16]) {
        let ch = self.channel.load(Ordering::Relaxed) & 0x0f;
        for (i, &level) in levels.iter().enumerate() {
            if self.sent[i] == level {
                continue;
            }
            self.sent[i] = level;
            let note = PAD_NOTE_BASE + i as u8;
            self.echo[note as usize].store(now_ms(), Ordering::Relaxed);
            let msg = [0x90 | ch, note, level.min(127)];
            for out in self.outs.iter_mut() {
                let _ = out.send(&msg);
            }
        }
    }
}

impl Drop for Leds {
    fn drop(&mut self) {
        self.close();
    }
}

/// A port name without the instance number Windows adds ("MPD218 1" and "MPD218 0").
fn base_name(name: &str) -> &str {
    name.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end()
}

fn open_leds(inputs: &[String], channel: Arc<AtomicU8>, echo: Arc<Vec<AtomicU64>>) -> Leds {
    let mut leds = Leds { outs: Vec::new(), channel, echo, sent: [u8::MAX; 16] };
    let Ok(probe) = MidiOutput::new("ResampleOS") else { return leds };
    for port in probe.ports() {
        let Ok(out) = MidiOutput::new("ResampleOS") else { continue };
        let Ok(name) = out.port_name(&port) else { continue };
        if !inputs.iter().any(|i| base_name(i) == base_name(&name)) {
            continue;
        }
        if let Ok(conn) = out.connect(&port, "resampleos-leds") {
            leds.outs.push(conn);
        }
    }
    leds
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

/// Opens every MIDI input. With `lights`, pad lights are sent back to the controllers.
pub fn start(shared: Arc<Shared>, tx: Producer<Cmd>, ui: Sender<MidiEvent>, lights: bool) -> Midi {
    let mut midi = Midi::none();
    let channel = Arc::new(AtomicU8::new(0));
    let echo: Arc<Vec<AtomicU64>> = Arc::new((0..128).map(|_| AtomicU64::new(0)).collect());
    let Ok(probe) = MidiInput::new("ResampleOS") else { return midi };
    // One engine queue shared by every port's callback thread.
    let tx = Arc::new(Mutex::new(tx));
    for port in probe.ports() {
        let Ok(mut input) = MidiInput::new("ResampleOS") else { continue };
        input.ignore(Ignore::All);
        let name = input.port_name(&port).unwrap_or_else(|_| "MIDI in".into());
        let (shared, tx, ui, channel, echo) = (shared.clone(), tx.clone(), ui.clone(), channel.clone(), echo.clone());
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
                // A light we sent, coming back round.
                if matches!(status, 0x80 | 0x90) && now_ms().saturating_sub(echo[d1 as usize].load(Ordering::Relaxed)) < ECHO_MS {
                    return;
                }
                match status {
                    0x90 if d2 > 0 => {
                        let (pad, semis, is_pad) = map_note(&shared, d1);
                        if is_pad {
                            channel.store(msg[0] & 0x0f, Ordering::Relaxed);
                        }
                        let key = 0x4000_0000 | d1 as u32;
                        let vel = (d2 as f32 / 127.0).powf(0.7);
                        let launching = is_pad && shared.ui_perform.load(Ordering::Relaxed);
                        if !launching && !shared.ui_capture_pads.load(Ordering::Relaxed) {
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
    if lights {
        midi.leds = open_leds(&midi.port_names, channel, echo);
    }
    midi
}
