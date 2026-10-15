//! Standard MIDI files: patterns in and out.
//!
//! Reading turns each track that has notes (or, in a single-track file, each channel) into
//! a list of notes in ResampleOS ticks; writing does the reverse, a track per pattern, in
//! a format 1 file at 480 ticks a beat with the project tempo.

use crate::engine::Note;
use crate::music::{PPQ, TICKS_PER_BAR};
use midly::num::{u4, u7, u15, u24, u28};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use std::path::Path;

/// One part of a MIDI file.
#[derive(Clone, Debug, PartialEq)]
pub struct MidiPart {
    pub name: String,
    /// In ResampleOS ticks, from the start of the file.
    pub notes: Vec<Note>,
    /// Whole bars covering every note.
    pub len_ticks: u32,
    /// Played on channel 10, the General MIDI drum channel.
    pub drums: bool,
}

/// What a MIDI file holds.
#[derive(Clone, Debug, PartialEq)]
pub struct MidiSong {
    pub parts: Vec<MidiPart>,
    /// The first tempo in the file, if it has one.
    pub bpm: Option<f32>,
}

pub fn read(path: &Path) -> Result<MidiSong, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    parse(&bytes)
}

pub fn parse(bytes: &[u8]) -> Result<MidiSong, String> {
    let smf = Smf::parse(bytes).map_err(|e| format!("not a MIDI file ({e})"))?;
    let per_beat = match smf.header.timing {
        Timing::Metrical(t) => t.as_int().max(1) as u64,
        Timing::Timecode(..) => return Err("MIDI files timed in SMPTE frames are not supported".into()),
    };
    let to_ticks = |t: u64| (t * PPQ as u64 / per_beat) as u32;
    let mut bpm = None;
    let mut parts = Vec::new();
    let single = smf.tracks.len() == 1;
    for (k, track) in smf.tracks.iter().enumerate() {
        let mut name = String::new();
        let mut now = 0u64;
        // Per channel: notes held (pitch -> start, velocity) and notes done.
        let mut held: Vec<(u8, u8, u64, u8)> = Vec::new();
        let mut done: Vec<(u8, Note)> = Vec::new();
        for ev in track {
            now += ev.delta.as_int() as u64;
            match ev.kind {
                TrackEventKind::Meta(MetaMessage::TrackName(n)) if name.is_empty() => name = String::from_utf8_lossy(n).trim().to_string(),
                TrackEventKind::Meta(MetaMessage::Tempo(us)) if bpm.is_none() => bpm = Some(60_000_000.0 / us.as_int().max(1) as f32),
                TrackEventKind::Midi { channel, message } => {
                    let ch = channel.as_int();
                    let (key, on) = match message {
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => (key.as_int(), Some(vel.as_int())),
                        MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => (key.as_int(), None),
                        _ => continue,
                    };
                    // A note ends at its off, or at a new on of the same key.
                    if let Some(i) = held.iter().position(|h| h.0 == ch && h.1 == key) {
                        let (_, pitch, start, vel) = held.swap_remove(i);
                        let (s, e) = (to_ticks(start), to_ticks(now));
                        done.push((ch, Note::new(s, (e - s).max(1), pitch, vel.max(1))));
                    }
                    if let Some(vel) = on {
                        held.push((ch, key, now, vel));
                    }
                }
                _ => {}
            }
        }
        // Notes never let go end with the track.
        for (ch, pitch, start, vel) in held {
            let s = to_ticks(start);
            done.push((ch, Note::new(s, (to_ticks(now) - s).max(PPQ / 4), pitch, vel.max(1))));
        }
        if done.is_empty() {
            continue;
        }
        // A single-track file holds every part, one per channel.
        let mut channels: Vec<u8> = done.iter().map(|(c, _)| *c).collect();
        channels.sort_unstable();
        channels.dedup();
        let groups: Vec<Option<u8>> = if single && channels.len() > 1 { channels.iter().map(|c| Some(*c)).collect() } else { vec![None] };
        for group in groups {
            let mut notes: Vec<Note> = done.iter().filter(|(c, _)| group.is_none_or(|g| *c == g)).map(|(_, n)| *n).collect();
            notes.sort_by_key(|n| (n.start, n.pitch));
            let end = notes.iter().map(|n| n.start + n.len).max().unwrap_or(0);
            let drums = match group {
                Some(c) => c == 9,
                None => done.iter().all(|(c, _)| *c == 9),
            };
            let label = match (name.is_empty(), group) {
                (false, None) => name.clone(),
                (false, Some(c)) => format!("{name} ch {}", c + 1),
                (true, Some(c)) => format!("Channel {}", c + 1),
                (true, None) => format!("Track {}", k + 1),
            };
            parts.push(MidiPart { name: label, notes, len_ticks: end.div_ceil(TICKS_PER_BAR).max(1) * TICKS_PER_BAR, drums });
        }
    }
    if parts.is_empty() {
        return Err("the MIDI file has no notes".into());
    }
    Ok(MidiSong { parts, bpm })
}

/// Writes `parts` (name, notes, drums) as a format 1 file at `bpm`.
pub fn write(path: &Path, parts: &[(String, Vec<Note>, bool)], bpm: f32) -> Result<(), String> {
    let bytes = encode(parts, bpm)?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

pub fn encode(parts: &[(String, Vec<Note>, bool)], bpm: f32) -> Result<Vec<u8>, String> {
    let header = Header::new(Format::Parallel, Timing::Metrical(u15::new(PPQ as u16)));
    let mut smf = Smf::new(header);
    let us = (60_000_000.0 / bpm.max(1.0)) as u32;
    smf.tracks.push(vec![
        TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(us.min(0xFF_FFFF)))) },
        TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::TimeSignature(4, 2, 24, 8)) },
        TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) },
    ]);
    for (name, notes, drums) in parts {
        let channel = u4::new(if *drums { 9 } else { 0 });
        // (tick, is on, pitch, velocity), offs before ons at the same tick.
        let mut evs: Vec<(u32, bool, u8, u8)> = Vec::with_capacity(notes.len() * 2);
        for n in notes {
            evs.push((n.start, true, n.pitch.min(127), n.vel.clamp(1, 127)));
            evs.push((n.start + n.len.max(1), false, n.pitch.min(127), 0));
        }
        evs.sort_by_key(|e| (e.0, e.1));
        let mut track = vec![TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::TrackName(name.as_bytes())) }];
        let mut last = 0u32;
        for (t, on, pitch, vel) in evs {
            let message = if on { MidiMessage::NoteOn { key: u7::new(pitch), vel: u7::new(vel) } } else { MidiMessage::NoteOff { key: u7::new(pitch), vel: u7::new(0) } };
            track.push(TrackEvent { delta: u28::new(t - last), kind: TrackEventKind::Midi { channel, message } });
            last = t;
        }
        track.push(TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) });
        smf.tracks.push(track);
    }
    let mut out = Vec::new();
    smf.write(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_go_out_and_come_back_as_they_were() {
        let bass = vec![Note::new(0, 240, 36, 100), Note::new(480, 480, 43, 90), Note::new(1920 * 3, 120, 41, 127)];
        let drums = vec![Note::new(0, 60, 36, 120), Note::new(480, 60, 38, 100)];
        let bytes = encode(&[("Bass".into(), bass.clone(), false), ("Drums".into(), drums.clone(), true)], 92.0).unwrap();
        let song = parse(&bytes).unwrap();
        assert!((song.bpm.unwrap() - 92.0).abs() < 0.01);
        assert_eq!(song.parts.len(), 2);
        assert_eq!(song.parts[0].name, "Bass");
        assert_eq!(song.parts[0].notes.iter().map(|n| (n.start, n.len, n.pitch, n.vel)).collect::<Vec<_>>(), bass.iter().map(|n| (n.start, n.len, n.pitch, n.vel)).collect::<Vec<_>>());
        assert_eq!(song.parts[0].len_ticks, 4 * TICKS_PER_BAR, "whole bars over the last note");
        assert!(song.parts[1].drums && !song.parts[0].drums);
    }

    #[test]
    fn another_resolution_and_one_track_of_channels_are_read() {
        // A format 0 file at 96 ticks a beat: a note on channel 1 and one on channel 10.
        let header = Header::new(Format::SingleTrack, Timing::Metrical(u15::new(96)));
        let mut smf = Smf::new(header);
        let ev = |d: u32, ch: u8, m: MidiMessage| TrackEvent { delta: u28::new(d), kind: TrackEventKind::Midi { channel: u4::new(ch), message: m } };
        smf.tracks.push(vec![
            ev(0, 0, MidiMessage::NoteOn { key: u7::new(60), vel: u7::new(80) }),
            ev(48, 9, MidiMessage::NoteOn { key: u7::new(36), vel: u7::new(110) }),
            ev(48, 0, MidiMessage::NoteOn { key: u7::new(60), vel: u7::new(0) }),
            ev(0, 9, MidiMessage::NoteOff { key: u7::new(36), vel: u7::new(0) }),
            TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) },
        ]);
        let mut bytes = Vec::new();
        smf.write(&mut bytes).unwrap();
        let song = parse(&bytes).unwrap();
        assert_eq!(song.parts.len(), 2);
        assert_eq!((song.parts[0].notes[0].start, song.parts[0].notes[0].len), (0, PPQ), "96 a beat becomes 480 a beat");
        assert_eq!(song.parts[1].notes[0].start, PPQ / 2);
        assert!(song.parts[1].drums);
        assert!(parse(b"not midi").is_err());
    }
}
