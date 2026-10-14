//! Keyboard shortcuts and MIDI controller messages.

use super::*;

impl App {
    pub(super) fn midi_cc(&mut self, cc: u8, value: u8) {
        if let Some(k) = midi::CC_KNOBS.iter().position(|&c| c == cc) {
            self.buses[self.focus_bus].slots[self.focus_slot].params[k] = value as f32 / 127.0;
            self.push_bus(self.focus_bus);
            return;
        }
        if value == 0 {
            return;
        }
        match cc {
            midi::CC_BANK_DOWN => self.bank = self.bank.saturating_sub(1),
            midi::CC_BANK_UP => self.bank = (self.bank + 1).min(self.banks - 1),
            midi::CC_RESAMPLE => self.resample_key(),
            midi::CC_SKIPBACK => self.capture_last_bars(16.0),
            midi::CC_ROLL => self.toggle_roll(),
            midi::CC_CHROMATIC => self.chromatic ^= true,
            _ => {}
        }
    }

    pub fn toggle_roll(&mut self) {
        self.roll_on ^= true;
        self.send(Cmd::Roll { on: self.roll_on, ticks: ROLL_DIVS[self.roll_div].1 });
    }

    pub(super) fn keys(&mut self, ctx: &egui::Context) {
        use egui::Key;
        // Tab is ours; stop egui from using it to walk widget focus.
        let tab = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Tab));
        // The plugin browser is a search box first: while it is up, letters are text, never
        // shortcuts, even on the frames before its field has taken focus.
        let typing = ctx.text_edit_focused() || self.browser.open;
        if !typing {
            // Sliders and buttons would otherwise keep focus and swallow Space and the pad keys.
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
        }
        self.m_down = !typing && ctx.input(|i| i.key_down(Key::M));
        self.q_down = !typing && ctx.input(|i| i.key_down(Key::Q));
        if tab && !typing {
            self.tab = if self.tab == Tab::Roll { Tab::Wave } else { Tab::Roll };
            if self.tab == Tab::Roll {
                self.set_kind_if_empty_midi();
            }
        }
        let events = ctx.input(|i| i.events.clone());
        for ev in events {
            let egui::Event::Key { key, pressed, repeat, modifiers, .. } = ev else { continue };
            if !pressed {
                if let Some(at) = self.held_keys.iter().position(|h| h.0 == key) {
                    let (_, pad, id) = self.held_keys.swap_remove(at);
                    self.note_off(pad, id);
                }
                continue;
            }
            if repeat || typing {
                continue;
            }
            let (ctrl, shift) = (modifiers.command, modifiers.shift);
            if self.view == crate::app::View::Arrange && self.arrange_key(key, ctrl) {
                continue;
            }
            if ctrl {
                match key {
                    Key::K => self.open_harmonizer(),
                    Key::Comma => self.open_prefs(),
                    Key::Z if shift => self.step_take(self.selected, -1),
                    Key::Y if shift => self.step_take(self.selected, 1),
                    Key::Z => self.undo(),
                    Key::Y => self.redo(),
                    Key::S => crate::project::save_dialog(self),
                    Key::O => crate::project::open_dialog(self),
                    Key::N => self.request_new(),
                    Key::D if self.vocal_context() => self.vocal_duplicate_selected(self.selected),
                    Key::B if self.vocal_context() && shift => self.vocal_bake_to_bank(self.selected),
                    Key::B if self.vocal_context() => self.vocal_bake_to_pad(self.selected),
                    Key::B if self.is_drum(self.selected) => self.drum_bake(self.selected, false),
                    Key::M if self.is_drum(self.selected) => self.drum_bake(self.selected, true),
                    Key::A if self.tab == Tab::Roll => self.roll_select_all(),
                    Key::D if self.roll_has_selection() => self.roll_duplicate(),
                    Key::C => self.clipboard = Some(self.selected),
                    Key::V => {
                        if let Some(from) = self.clipboard {
                            self.copy_pad(from, self.selected);
                        }
                    }
                    _ => {}
                }
                continue;
            }
            if self.roll_has_selection() {
                let by = if shift { 12 } else { 1 };
                match key {
                    Key::ArrowUp => self.roll_transpose(by),
                    Key::ArrowDown => self.roll_transpose(-by),
                    Key::ArrowLeft => self.roll_nudge(-1),
                    Key::ArrowRight => self.roll_nudge(1),
                    Key::Delete => self.roll_delete_selected(),
                    _ => {}
                }
                if matches!(key, Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::Delete) {
                    continue;
                }
            }
            // Shift+R arms the selected vocal pad from anywhere.
            if key == Key::R && shift {
                self.vocal_arm(self.selected);
                continue;
            }
            if key == Key::Space && shift {
                self.stop_all();
                continue;
            }
            if self.vocal_context() && self.vocal_key(key, shift) {
                continue;
            }
            if let Some(i) = PAD_KEYS.iter().position(|&k| k == key) {
                if self.perform {
                    self.launch_scene(i);
                } else if shift && !self.chromatic {
                    self.select(self.bank * 16 + i);
                } else {
                    let (pad, id) = self.slot_hit(i, 0x1000 + i as u32);
                    self.held_keys.push((key, pad, id));
                }
                continue;
            }
            match key {
                Key::Enter if self.harm.open => {
                    self.conform(self.selected);
                    self.harm.open = false;
                }
                Key::Space => self.toggle_play(),
                Key::R => self.resample_key(),
                Key::S => self.capture_last_bars(16.0),
                Key::C => self.chop(self.selected),
                Key::X => self.chromatic ^= true,
                Key::T => self.toggle_roll(),
                Key::P => self.perform ^= true,
                Key::Num1 => self.focus_bus = 0,
                Key::Num2 => self.focus_bus = 1,
                Key::Num3 => self.focus_bus = 2,
                Key::Num4 => self.focus_bus = 3,
                Key::Num5 => self.focus_bus = MASTER,
                Key::OpenBracket => self.octave = (self.octave - 1).max(-4),
                Key::CloseBracket => self.octave = (self.octave + 1).min(4),
                Key::ArrowLeft | Key::PageDown => self.bank = self.bank.saturating_sub(1),
                Key::ArrowRight | Key::PageUp => self.bank = (self.bank + 1).min(self.banks - 1),
                Key::Delete => self.delete_key(),
                Key::Backspace => self.stop_pad(self.selected),
                Key::G => self.tab = if self.tab == Tab::Steps { Tab::Wave } else { Tab::Steps },
                Key::Escape => {
                    self.roll.sel.clear();
                    self.skip.resume();
                    self.harm.open = false;
                    self.show_prefs = false;
                    if self.resample == Resample::Armed {
                        self.resample = Resample::Idle;
                    }
                    self.send(Cmd::Panic);
                }
                Key::F1 => self.show_help ^= true,
                Key::F3 => self.toggle_view(),
                Key::F2 => self.open_browser(PluginTarget::Pad(self.selected)),
                _ => {}
            }
        }
    }

    pub(super) fn delete_key(&mut self) {
        if self.tab == Tab::Roll && self.pads[self.selected].cfg.kind == PadKind::Midi {
            self.pads[self.selected].pattern.notes.clear();
            self.push_pattern(self.selected);
        } else {
            self.clear_pad(self.selected);
        }
    }

    /// Opening the piano roll on an empty pad turns it into a MIDI pad, as in the spec's flow.
    pub fn set_kind_if_empty_midi(&mut self) {
        if self.pads[self.selected].is_empty() {
            self.set_kind(self.selected, PadKind::Midi);
        }
    }

    pub fn midi_target_label(&self, t: MidiTarget) -> String {
        match t {
            MidiTarget::Plugin => "plugin instrument".into(),
            MidiTarget::Synth => "Internal synth".into(),
            MidiTarget::Pad(p) => format!("Pad {} (chromatic)", pad_label(p as usize)),
        }
    }
}
