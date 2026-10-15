//! MIDI files in and out: a pad's pattern, or the whole song, to a `.mid` file; a `.mid`
//! file onto pads (as their patterns) or onto the timeline (as clips with notes of their
//! own).

use super::*;
use crate::engine::Pattern;
use crate::midifile::{self, MidiPart};
use std::path::{Path, PathBuf};

/// Longest pattern a loop holds, in ticks.
const LOOP_MAX: u32 = 16 * TICKS_PER_BAR;

/// Whether `path` is a MIDI file, by its name.
pub fn is_midi(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi"))
}

/// The notes a clip plays of pattern `p`, in song ticks: each time round the pattern that
/// falls in the clip, from its offset, each note cut at the clip's end.
fn clip_notes(c: &Clip, p: &Pattern) -> Vec<Note> {
    let len = p.len_ticks as u64;
    let mut out = Vec::new();
    let mut k = c.offset / len;
    loop {
        let base = c.start as i64 + (k * len) as i64 - c.offset as i64;
        if base >= c.end() as i64 {
            break;
        }
        for n in &p.notes {
            let t = base + n.start as i64;
            if t < c.start as i64 || t >= c.end() as i64 {
                continue;
            }
            let l = (n.len as u64).min(c.end() - t as u64) as u32;
            out.push(Note { start: t as u32, len: l.max(1), ..*n });
        }
        k += 1;
    }
    out
}

impl App {
    /// A MIDI file onto the pads, from `pad` on: each part as a pad's pattern (up to
    /// sixteen bars, the longest a loop runs). Empty pads become MIDI pads.
    pub fn import_midi_to_pads(&mut self, path: &Path, pad: usize) {
        let song = match midifile::read(path) {
            Ok(s) => s,
            Err(e) => {
                self.status = format!("{}: {e}", path.display());
                return;
            }
        };
        let mut cut = false;
        let mut n = 0;
        for (k, part) in song.parts.iter().enumerate() {
            let idx = pad + k;
            if idx >= self.pads.len() {
                break;
            }
            if self.pads[idx].is_empty() && self.pads[idx].cfg.kind != PadKind::Midi && self.pads[idx].cfg.kind != PadKind::Drum {
                self.set_kind(idx, PadKind::Midi);
            }
            if self.pads[idx].name.is_empty() || self.pads[idx].cfg.kind == PadKind::Midi {
                self.pads[idx].name = part.name.clone();
            }
            let len = part.len_ticks.min(LOOP_MAX);
            cut |= part.len_ticks > LOOP_MAX;
            self.pads[idx].pattern = Pattern { len_ticks: len, notes: part.notes.iter().filter(|x| x.start < len).copied().collect() };
            self.push_pattern(idx);
            n += 1;
        }
        let file = path.file_name().map_or(String::new(), |f| f.to_string_lossy().into_owned());
        self.status = format!(
            "{file}: {n} part{} onto {}{}{}",
            if n == 1 { "" } else { "s" },
            pad_label(pad),
            if n > 1 { format!(" to {}", pad_label(pad + n - 1)) } else { String::new() },
            if cut { ". Loops hold sixteen bars, so the rest was left out; drop it on the song timeline to keep it all." } else { "" }
        );
    }

    /// A MIDI file onto the timeline at `at`: each part a clip with notes of its own, the
    /// first on `pad`'s track (if given), the others on new tracks.
    pub fn import_midi_to_song(&mut self, path: &Path, pad: Option<usize>, at: u64) {
        let song = match midifile::read(path) {
            Ok(s) => s,
            Err(e) => {
                self.status = format!("{}: {e}", path.display());
                return;
            }
        };
        let at = at / TICKS_PER_BAR as u64 * TICKS_PER_BAR as u64;
        let mut tracks = Vec::new();
        for (k, part) in song.parts.iter().enumerate() {
            let track = match pad.filter(|_| k == 0) {
                Some(p) => p,
                None => {
                    let Some(p) = self.free_track_pad() else { break };
                    self.set_kind(p, PadKind::Midi);
                    self.pads[p].name = part.name.clone();
                    self.push_pad(p);
                    p
                }
            };
            self.arrangement.patterns.push(Pattern { len_ticks: part.len_ticks, notes: part.notes.clone() });
            let id = self.arrangement.patterns.len() - 1;
            self.arrangement.place(track, Clip::own_notes(at, part.len_ticks as u64, id));
            tracks.push(track);
        }
        self.view = View::Arrange;
        let file = path.file_name().map_or(String::new(), |f| f.to_string_lossy().into_owned());
        self.status = format!("{file}: {} part{} on the timeline from bar {}", tracks.len(), if tracks.len() == 1 { "" } else { "s" }, at / TICKS_PER_BAR as u64 + 1);
    }

    /// Asks for a MIDI file and brings it in: onto the timeline in the song view, onto the
    /// selected pad (and those after it) otherwise.
    pub fn import_midi_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new().set_title("Import MIDI").add_filter("MIDI", &["mid", "midi"]).pick_file() else { return };
        if self.view == View::Arrange {
            self.import_midi_to_song(&path, None, self.arr.cursor);
        } else {
            self.import_midi_to_pads(&path, self.selected);
        }
    }

    fn midi_save_path(&self, name: &str) -> Option<PathBuf> {
        let safe: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' }).collect();
        rfd::FileDialog::new().set_title("Export MIDI").set_file_name(format!("{}.mid", safe.trim())).add_filter("MIDI", &["mid"]).save_file()
    }

    /// The selected pad's pattern to a MIDI file.
    pub fn export_pad_midi(&mut self, pad: usize) {
        let p = &self.pads[pad];
        if p.pattern.notes.is_empty() {
            self.status = format!("{} has no notes to export", pad_label(pad));
            return;
        }
        let name = if p.name.is_empty() { pad_label(pad) } else { p.name.clone() };
        let part = (name.clone(), p.pattern.notes.clone(), p.cfg.kind == PadKind::Drum);
        let Some(path) = self.midi_save_path(&name) else { return };
        self.status = match midifile::write(&path, &[part], self.bpm) {
            Ok(()) => format!("Exported {} as MIDI to {}", pad_label(pad), path.display()),
            Err(e) => format!("MIDI export failed: {e}"),
        };
    }

    /// Every track's notes over the whole song, as they play: loops' patterns written out
    /// where their clips run, and clips' own notes. Audio clips have none.
    pub fn song_midi_parts(&self) -> Vec<MidiPart> {
        let scenes = self.scenes_complete();
        let mut parts = Vec::new();
        for (pad, clips) in &self.arrangement.tracks {
            let mut notes = Vec::new();
            for c in clips {
                let pattern = match c.notes {
                    Some(n) => self.arrangement.patterns.get(n).cloned(),
                    None if c.plays_loop() => scenes.get(c.scene).map(|s| s.pattern(*pad)),
                    None => None,
                };
                let Some(p) = pattern.filter(|p| p.len_ticks > 0 && !p.notes.is_empty()) else { continue };
                notes.extend(clip_notes(c, &p));
            }
            if notes.is_empty() {
                continue;
            }
            notes.sort_by_key(|n| (n.start, n.pitch));
            let p = &self.pads[*pad];
            let name = if p.name.is_empty() { pad_label(*pad) } else { format!("{} {}", pad_label(*pad), p.name) };
            let end = notes.iter().map(|n| n.start + n.len).max().unwrap_or(0);
            parts.push(MidiPart { name, notes, len_ticks: end.div_ceil(TICKS_PER_BAR) * TICKS_PER_BAR, drums: p.cfg.kind == PadKind::Drum });
        }
        parts
    }

    /// The song's notes to a MIDI file, a track per pad.
    pub fn export_song_midi(&mut self) {
        let parts = self.song_midi_parts();
        if parts.is_empty() {
            self.status = "The song has no notes to export (audio clips have none)".into();
            return;
        }
        let name = self.project_dir.as_ref().and_then(|d| d.file_name()).map_or("song".to_string(), |n| n.to_string_lossy().into_owned());
        let Some(path) = self.midi_save_path(&name) else { return };
        let out: Vec<(String, Vec<Note>, bool)> = parts.into_iter().map(|p| (p.name, p.notes, p.drums)).collect();
        self.status = match midifile::write(&path, &out, self.bpm) {
            Ok(()) => format!("Exported {} track{} of the song as MIDI to {}", out.len(), if out.len() == 1 { "" } else { "s" }, path.display()),
            Err(e) => format!("MIDI export failed: {e}"),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_looped_clip_writes_out_every_pass() {
        let b = TICKS_PER_BAR;
        let p = Pattern { len_ticks: b, notes: vec![Note::new(0, 240, 60, 100), Note::new(b * 3 / 4, 480, 62, 100)] };
        // Two and a half bars from bar 2, starting half a bar into the pattern: the late
        // note comes first, a quarter bar in, then each pass in full, cut at the end.
        let c = Clip::pattern(b as u64, (b * 5 / 2) as u64, (b / 2) as u64, 0);
        let got: Vec<(u32, u32, u8)> = clip_notes(&c, &p).iter().map(|n| (n.start, n.len, n.pitch)).collect();
        assert_eq!(got, vec![(b * 5 / 4, 480, 62), (b * 3 / 2, 240, 60), (b * 9 / 4, 480, 62), (b * 5 / 2, 240, 60), (b * 13 / 4, b / 4, 62)]);
    }
}
