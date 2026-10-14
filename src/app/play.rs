//! The transport, playing pads and notes, and recording them into a pattern.

use super::*;

/// A note being held while Rec is on in the piano roll, waiting for its release.
pub(super) struct PendingNote {
    pub(super) key: u32,
    pub(super) pad: usize,
    pub(super) pitch: u8,
    pub(super) vel: u8,
    pub(super) start: f64,
}

impl App {
    pub fn playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    pub fn toggle_play(&mut self) {
        let on = !self.playing();
        if on && self.exporting {
            self.status = "Exporting: playback comes back when it is done".into();
            return;
        }
        if on
            && let Resample::Recording { target, bar_aligned: false, .. } = self.resample {
                // Starting the transport mid-take restarts the take on the downbeat.
                self.resample =
                    Resample::Recording { target, start: self.shared.ring.written(), bar_aligned: true };
            }
        // Starting from the arrangement plays the song from the insert marker; starting
        // from the pads plays the loops.
        let song = on && self.view == View::Arrange;
        if on {
            self.arr.overridden = false;
            self.set_song_mode(song);
            if song {
                self.locate(self.arr.cursor);
            } else {
                self.scenes_on_play();
            }
        } else {
            self.scenes_on_stop();
        }
        self.send(Cmd::Play(on));
        if on && !song {
            self.scenes_after_play();
        }
    }

    /// Stops the transport and silences everything at once: playing pads, held notes and
    /// the tails of the built-in effects.
    pub fn stop_all(&mut self) {
        if self.playing() {
            self.toggle_play();
        }
        self.held_keys.clear();
        self.send(Cmd::Panic);
        self.status = "Stopped all audio".into();
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
            self.vocal.selected = None;
            // A vocal pad shows its takes; any other pad has no use for that tab.
            if self.is_vocal(pad) {
                self.tab = Tab::Vocal;
            } else if self.is_drum(pad) {
                // A drum pad opens on its synth, unless a tab that edits patterns is up
                // (the step sequencer selects pads as you paint).
                if matches!(self.tab, Tab::Wave | Tab::Vocal) {
                    self.tab = Tab::Drum;
                }
            } else if matches!(self.tab, Tab::Vocal | Tab::Drum) {
                self.tab = Tab::Wave;
            }
        }
    }

    pub(super) fn record_on(&mut self, pad: usize, semis: f32, vel: f32, key: u32) {
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

    pub(super) fn record_off(&mut self, key: u32) {
        let Some(at) = self.pending.iter().position(|p| p.key == key) else { return };
        let p = self.pending.swap_remove(at);
        let snap = SNAPS[self.roll.snap].1 as f64;
        let len_ticks = self.pads[p.pad].pattern.len_ticks.max(1) as f64;
        let len = (self.shared.tick() - p.start).max(30.0);
        let start = ((p.start / snap).round() * snap).rem_euclid(len_ticks);
        self.pads[p.pad].pattern.notes.push(Note::new(start as u32, len as u32, p.pitch, p.vel.max(1)));
        self.push_pattern(p.pad);
    }
}

impl App {
    /// Lights the controller's pads to match the screen: the bank on screen, with pads
    /// sounding brightest, the selected one next, and loaded ones faintly. In perform
    /// mode the pads are loops: playing, queued, or there.
    pub(super) fn pad_lights(&mut self) {
        if !self._midi.leds.connected() {
            return;
        }
        let mut levels = [midi::LED_OFF; 16];
        if self.perform {
            let playing = self.shared.scene_index.load(Ordering::Relaxed) as usize;
            for (i, level) in levels.iter_mut().enumerate() {
                *level = if i >= self.scenes.len() {
                    midi::LED_OFF
                } else if self.playing() && i == playing {
                    midi::LED_PLAYING
                } else if self.queued_scene == Some(i) {
                    midi::LED_SELECTED
                } else {
                    midi::LED_LOADED
                };
            }
        } else {
            for (i, level) in levels.iter_mut().enumerate() {
                let pad = self.bank * 16 + i;
                let Some(p) = self.pads.get(pad) else { continue };
                *level = if crate::shared::f32_load(&self.shared.pad_level[pad]) > 0.01 {
                    midi::LED_PLAYING
                } else if pad == self.selected {
                    midi::LED_SELECTED
                } else if !p.is_empty() {
                    midi::LED_LOADED
                } else {
                    midi::LED_OFF
                };
            }
        }
        self._midi.leds.show(levels);
    }
}
