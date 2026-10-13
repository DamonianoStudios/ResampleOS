# Shortcuts and MIDI

Everything in ResampleOS can be reached from the keyboard. Shortcuts are ignored while you are typing in a text field or while the plugin browser is open.

## Playing pads

The sixteen pads of the current bank are on the right-hand side of the keyboard, laid out like the matrix on screen, with pad 1 at the bottom left:

| Pads | Keys |
| --- | --- |
| 13 to 16 | `6` `7` `8` `9` |
| 9 to 12 | `Y` `U` `I` `O` |
| 5 to 8 | `H` `J` `K` `L` |
| 1 to 4 | `N` `,` `.` `/` |

| Shortcut | Action |
| --- | --- |
| `Shift` + pad key | Select the pad without playing it |
| `M` + pad | Mute or unmute the pad |
| `Q` + pad | Stop that pad's playback |
| `Backspace` | Stop the selected pad's playback |
| `X` | Chromatic mode: the sixteen pads play the selected pad at sixteen pitches |
| `[` and `]` | Octave down and up, for chromatic mode and MIDI pads |
| `T` | Roll (note repeat) while a pad is held |
| `←` `→` or `Page Down` `Page Up` | Previous and next bank |

## Transport and capture

| Shortcut | Action |
| --- | --- |
| `Space` | Start or stop the transport |
| `R` | Resample: arm, then hit the destination pad, then `R` again to bake |
| `S` | Skip-back: the last 16 bars to the selected pad, or the next empty pad if it is in use |
| `Esc` | Cancel a selection or an armed resample, and silence every voice |

## Editing

| Shortcut | Action |
| --- | --- |
| `Ctrl+Z` / `Ctrl+Y` | Undo and redo any project edit |
| `Ctrl+Shift+Z` / `Ctrl+Shift+Y` | Step the selected pad back and forward through its takes |
| `Ctrl+C` / `Ctrl+V` | Copy the selected pad, and paste onto the selected pad |
| `Delete` | Clear the selected pad. In the piano roll, clear its pattern instead |
| `C` | Chop the selected pad's transient slices onto the lowest empty bank |
| `Ctrl+K` | Key and BPM harmonizer. `Enter` conforms |

## Views

| Shortcut | Action |
| --- | --- |
| `Tab` | Switch between the waveform and the piano roll |
| `G` | Step sequencer for the current bank |
| `1` to `4`, `5` | Focus Bus 1 to 4, or the master |
| `F1` | Shortcut help |
| `F2` | VST3 browser, to load an instrument on the selected pad |
| `Ctrl+,` | Preferences |
| `Ctrl+S` / `Ctrl+O` | Save and open a project |

## Mouse

| Where | Gesture | Action |
| --- | --- | --- |
| Pad | Click | Play and select. Right-click for the pad menu |
| Bank tab | Right-click | Remove the bank, or all empty banks |
| Pad matrix | `Ctrl` + scroll | Zoom between 4×4, 8×8 and 16×16 |
| Waveform | Drag | Set the play region. Double-click resets it |
| Waveform | Right-click | Add or remove a transient marker |
| Waveform | Scroll, `Shift` + scroll | Zoom, pan |
| Piano roll | Click, drag | Add a note, move it. Drag its right edge to resize. Hold `Alt` to ignore snap |
| Piano roll | Right-click a note | Delete it |
| Step grid | Click or drag | Paint steps. Right-click a step to cycle normal, accent, soft |
| Knob | Drag | Change. `Shift` for fine. Double-click to reset |
| Skip-back bar | Drag | Select a region. Drag the selection onto a pad to capture it |

## MIDI controllers

Every MIDI input present when the app starts is opened. Pad hits go straight from the MIDI thread to the audio engine, so they are not delayed by the screen's frame rate.

### Notes

| Notes | Action |
| --- | --- |
| 36 to 51 (C1 to D#2) | Pads 1 to 16 of the current bank. In chromatic mode, the selected pad at sixteen pitches |
| Any other note | The selected pad played melodically, with note 60 as its original pitch |

### Control changes

| CC | Action |
| --- | --- |
| 70, 71, 72 | The three knobs of the focused effect slot |
| 20, 21 | Bank down, bank up |
| 22 | Resample (arm, then bake) |
| 23 | Skip-back: last 16 bars |
| 24 | Roll on or off |
| 25 | Chromatic mode on or off |

Button CCs act on any value above zero. The mapping is fixed; there is no MIDI learn yet, and the app does not send LED feedback to controllers.
