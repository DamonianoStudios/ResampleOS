//! Vocal pads: recording takes against the playing loop, editing them, and baking.
//!
//! Takes are cut from the engine's ring of dry input. The engine marks where in that ring
//! each loop began, so a take is the stretch between two marks (plus the input latency).
//! Every edit marks the pad for re-rendering; the render becomes the pad's sample.

use super::*;
use crate::engine::VocalFx;
use crate::vocal::{LaneKind, TakeAudio, VocalStack, phrases};
use std::collections::{HashMap, HashSet};

/// What a drag in the take lanes does.
#[derive(Clone, Copy, PartialEq)]
pub enum VocalTool {
    /// Swipe across a take to put that stretch of it in the comp.
    Comp,
    /// Drag clips in time or between lanes; Alt slips the audio inside a clip.
    Move,
    /// Click to cut a clip in two.
    Cut,
}

/// Grid the vocal editor snaps to: name and divisions per bar (0 = off).
pub const VOCAL_SNAPS: [(&str, u32); 5] = [("Off", 0), ("Bar", 1), ("1/4", 4), ("1/8", 8), ("1/16", 16)];

/// A clip being dragged.
#[derive(Clone, Copy)]
pub struct VocalDrag {
    pub lane: usize,
    pub clip: usize,
    /// Pointer position on the timeline when the drag began, and the clip's start and
    /// offset then.
    pub grab: i64,
    pub start: i64,
    pub offset: i64,
    pub slip: bool,
}

/// View state of the vocal tab.
pub struct VocalUi {
    pub tool: VocalTool,
    pub snap: usize,
    /// Record one continuous pass instead of a take per loop.
    pub free: bool,
    /// Record only into `range`.
    pub punch: bool,
    /// Time selection on the ruler, in frames.
    pub range: Option<(i64, i64)>,
    pub selected: Option<(usize, usize)>,
    pub drag: Option<VocalDrag>,
    /// A comp swipe in progress: lane and the frame it started at.
    pub swipe: Option<(usize, i64)>,
    /// Where on the timeline the context menu was opened.
    pub menu_frame: i64,
    /// How far the lanes are scrolled, in points.
    pub scroll: f32,
    /// Hear the input through the vocal bus while armed.
    pub monitor: bool,
    /// Print the vocal bus effects when baking.
    pub bake_fx: bool,
}

impl Default for VocalUi {
    fn default() -> Self {
        VocalUi { tool: VocalTool::Comp, snap: 3, free: false, punch: false, range: None, selected: None, drag: None, swipe: None, menu_frame: 0, scroll: 0.0, monitor: true, bake_fx: false }
    }
}

/// A recording in progress.
pub struct VocalRec {
    pub pad: usize,
    /// Input-ring frame at which the pass being recorded began; `None` while waiting for
    /// the loop to come round.
    pub pass_from: Option<u64>,
    /// Loop changes during a free pass: ring frame and loop number.
    marks: Vec<(u64, u32)>,
    /// Where the input was being monitored before arming.
    restore_dest: u8,
}

#[derive(Clone, Copy, PartialEq)]
pub enum RecState {
    Idle,
    /// Armed, waiting for the transport or the next time round the loop.
    Waiting,
    Recording,
}

struct PendingTake {
    pad: usize,
    from: u64,
    to: u64,
    range: Option<(i64, i64)>,
    marks: Vec<(i64, u32)>,
    /// The timeline length this take implies, if the stack has none yet.
    len: i64,
}

/// Recording and render bookkeeping.
#[derive(Default)]
pub struct VocalWork {
    pub rec: Option<VocalRec>,
    pending: Vec<PendingTake>,
    /// Pads whose stack changed since their last render.
    dirty: HashSet<usize>,
    /// Pitch-shifted copies of takes for play-through lanes, by buffer address and cents.
    shifted: HashMap<(usize, i32), Arc<SampleData>>,
    shifting: HashSet<(usize, i32)>,
    seen_plays: u32,
    seen_loops: u32,
    was_playing: bool,
}

impl App {
    fn stack_mut(&mut self, pad: usize) -> Option<&mut VocalStack> {
        self.vocal_work.dirty.insert(pad);
        self.pads.get_mut(pad)?.vocal.as_mut()
    }

    /// Asks for a pad's comp to be rendered afresh from its takes.
    pub fn vocal_refresh(&mut self, pad: usize) {
        self.vocal_work.dirty.insert(pad);
    }

    pub fn is_vocal(&self, pad: usize) -> bool {
        self.pads.get(pad).is_some_and(|p| p.cfg.kind == PadKind::Vocal)
    }

    /// True when the vocal tab is showing a vocal pad, which is when its keys apply.
    pub fn vocal_context(&self) -> bool {
        self.tab == Tab::Vocal && self.is_vocal(self.selected)
    }

    /// Turns a pad into a vocal pad on the vocal bus.
    pub fn make_vocal(&mut self, pad: usize) {
        let p = &mut self.pads[pad];
        if p.cfg.kind != PadKind::Vocal {
            p.cfg.kind = PadKind::Vocal;
            p.cfg.bus = VOCAL as u8 + 1;
            p.cfg.mode = PlayMode::OneShot;
            if p.name.is_empty() {
                p.name = "Vox".into();
            }
            p.vocal.get_or_insert_with(VocalStack::default);
            self.push_pad(pad);
        }
        self.tab = Tab::Vocal;
    }

    /// Keys that mean something else while the vocal tab is showing a vocal pad. Returns
    /// whether the key was one of them.
    pub(super) fn vocal_key(&mut self, key: egui::Key, shift: bool) -> bool {
        use egui::Key;
        let pad = self.selected;
        let has_clip = self.vocal.selected.is_some();
        match key {
            Key::C => self.vocal.tool = VocalTool::Comp,
            Key::D => self.vocal.tool = VocalTool::Move,
            Key::X => self.vocal.tool = VocalTool::Cut,
            Key::G => self.vocal.snap = (self.vocal.snap + 1) % VOCAL_SNAPS.len(),
            Key::M if has_clip => self.vocal_mute_selected(pad),
            Key::Backspace | Key::Delete if has_clip => self.vocal_delete_selected(pad),
            // Shift + a pad key steps the comp through the takes for that sixteenth of
            // the timeline.
            _ => match PAD_KEYS.iter().position(|&k| k == key).filter(|_| shift) {
                Some(slot) => self.vocal_cycle_slot(pad, slot),
                None => return false,
            },
        }
        true
    }

    /// Where in the input ring the pass now being recorded began.
    pub fn vocal_recording_from(&self) -> Option<u64> {
        let rec = self.vocal_work.rec.as_ref()?;
        rec.pass_from.filter(|_| rec.pad == self.selected && self.playing())
    }

    pub fn vocal_armed(&self) -> bool {
        self.vocal_work.rec.is_some()
    }

    pub fn rec_state(&self, pad: usize) -> RecState {
        match &self.vocal_work.rec {
            Some(r) if r.pad == pad && r.pass_from.is_some() && self.playing() => RecState::Recording,
            Some(r) if r.pad == pad => RecState::Waiting,
            _ => RecState::Idle,
        }
    }

    /// Frames the input arrives late by, from the preference.
    fn latency_frames(&self) -> u64 {
        (self.prefs.input_latency_ms.max(0.0) / 1000.0 * self.sr()) as u64
    }

    fn snap_frames(&self) -> f64 {
        match VOCAL_SNAPS[self.vocal.snap].1 {
            0 => 1.0,
            per_bar => self.frames_per_bar() / per_bar as f64,
        }
    }

    /// Nearest grid line to a timeline frame.
    pub fn vocal_snap(&self, frame: i64) -> i64 {
        let grid = self.snap_frames();
        ((frame as f64 / grid).round() * grid).round() as i64
    }

    // ---- Recording.

    /// Shift+R: arms or disarms a vocal pad. Recording itself follows the transport.
    pub fn vocal_arm(&mut self, pad: usize) {
        if self.vocal_work.rec.as_ref().is_some_and(|r| r.pad == pad) {
            self.vocal_disarm();
            return;
        }
        self.vocal_disarm();
        if !self.is_vocal(pad) {
            self.make_vocal(pad);
        }
        if self.input_sel.is_none() {
            if self.input_names.is_empty() {
                self.status = "No audio input found to record from".into();
                return;
            }
            self.set_input(Some(0));
            if self.input_sel.is_none() {
                return;
            }
        }
        let restore_dest = self.input_dest;
        // Monitor destinations count from 2 for Bus 1; 0 keeps the input out of the mix.
        self.input_dest = if self.vocal.monitor { VOCAL as u8 + 2 } else { 0 };
        self.send(Cmd::InputCfg { dest: self.input_dest, gain: self.input_gain });
        // The pad's own comp stays out of the way while it is being added to.
        let muted = PadCfg { mute: true, ..self.pads[pad].cfg };
        self.send(Cmd::SetPadCfg { pad: pad as u16, cfg: muted });
        self.send(Cmd::StopPad { pad: pad as u16 });
        self.vocal_work.rec = Some(VocalRec { pad, pass_from: None, marks: Vec::new(), restore_dest });
        self.status = if self.playing() {
            format!("{} armed: recording from the top of the loop", pad_label(pad))
        } else {
            format!("{} armed: press Space to record", pad_label(pad))
        };
    }

    pub fn vocal_disarm(&mut self) {
        let Some(rec) = self.vocal_work.rec.take() else { return };
        self.input_dest = rec.restore_dest;
        self.send(Cmd::InputCfg { dest: self.input_dest, gain: self.input_gain });
        if rec.pad < self.pads.len() {
            self.push_pad(rec.pad);
        }
    }

    /// Called every frame: turns the engine's loop marks into takes.
    pub(super) fn vocal_poll(&mut self) {
        let (plays, loops) = (self.shared.play_starts.load(Ordering::Relaxed), self.shared.scene_starts.load(Ordering::Relaxed));
        let mark = self.shared.loop_frame.load(Ordering::Relaxed);
        let playing = self.playing();
        let fresh_start = plays != self.vocal_work.seen_plays;
        let came_round = loops != self.vocal_work.seen_loops;
        self.vocal_work.seen_plays = plays;
        self.vocal_work.seen_loops = loops;
        let stopped = self.vocal_work.was_playing && !playing;
        self.vocal_work.was_playing = playing;

        let (free, range) = (self.vocal.free, self.vocal.range.filter(|_| self.vocal.punch));
        let loop_len = (self.scenes[self.scene].bars as f64 * self.frames_per_bar()).round() as i64;
        let scene = self.scene as u32 + 1;
        let now = self.shared.input_ring.written();
        if let Some(rec) = self.vocal_work.rec.as_mut() {
            let pad = rec.pad;
            let mut done = None;
            if fresh_start {
                rec.pass_from = Some(mark);
                rec.marks.clear();
            } else if came_round && playing {
                match rec.pass_from {
                    // A free pass runs on; note where the loop changed.
                    Some(from) if free => rec.marks.push((mark.saturating_sub(from), scene)),
                    Some(from) => {
                        done = Some((from, mark, loop_len));
                        rec.pass_from = Some(mark);
                    }
                    None => rec.pass_from = Some(mark),
                }
            }
            if stopped && let Some(from) = rec.pass_from.take() {
                // Keep what was sung before stopping, unless it is barely anything.
                let sung = now.saturating_sub(from) as i64;
                if free || sung > loop_len / 4 {
                    done = Some((from, now, if free { sung } else { loop_len }));
                }
            }
            if let Some((from, to, len)) = done {
                let marks = rec.marks.iter().map(|(f, s)| (*f as i64, *s)).collect();
                self.vocal_work.pending.push(PendingTake { pad, from, to, range, marks, len });
            }
            if stopped {
                self.vocal_disarm();
            }
        }

        // A take can be cut once the ring holds its end, latency included.
        let late = self.latency_frames();
        let ready: Vec<PendingTake> = {
            let pending = &mut self.vocal_work.pending;
            let (ready, wait) = std::mem::take(pending).into_iter().partition(|t| t.to + late <= now);
            *pending = wait;
            ready
        };
        for take in ready {
            let (l, r) = self.shared.input_ring.read(take.from + late, take.to + late);
            self.vocal_add_take(take.pad, SampleData::new(l, r), take.range, take.len, take.marks);
        }
    }

    /// Adds a pass to a pad's stack as a new take lane.
    fn vocal_add_take(&mut self, pad: usize, audio: SampleData, range: Option<(i64, i64)>, len: i64, marks: Vec<(i64, u32)>) {
        if pad >= self.pads.len() || !self.is_vocal(pad) {
            return;
        }
        if audio.peak() < 1e-4 {
            self.status = format!("{}: that pass was silent, so no take was kept. Check the input.", pad_label(pad));
            return;
        }
        let scene_ticks = self.scenes[self.scene].bars * TICKS_PER_BAR;
        let Some(stack) = self.stack_mut(pad) else { return };
        // The timeline is as long as the longest pass so far.
        stack.len = stack.len.max(len).max(if stack.lanes.is_empty() { audio.len() as i64 } else { 0 });
        if !marks.is_empty() {
            stack.markers = marks;
        }
        let lane = stack.add_take(TakeAudio::new(audio), range);
        let name = stack.lanes[lane].name.clone();
        let count = stack.lanes.iter().filter(|l| l.kind == LaneKind::Take).count();
        // The pad plays its comp from the top of the loop it was recorded over.
        let p = &mut self.pads[pad];
        if p.pattern.notes.is_empty() {
            p.pattern.len_ticks = scene_ticks;
            p.pattern.notes.push(Note { start: 0, len: scene_ticks.min(TICKS_PER_BAR), pitch: 60, vel: 100 });
            self.push_pattern(pad);
        }
        self.status = format!("{}: {name} recorded ({count} take{})", pad_label(pad), if count == 1 { "" } else { "s" });
    }

    /// Brings an audio file in as a take.
    pub fn vocal_import(&mut self, pad: usize) {
        let picked = rfd::FileDialog::new().add_filter("Audio", &["wav", "mp3", "flac", "ogg", "aif", "aiff"]).pick_file();
        if let Some(path) = picked {
            self.vocal_import_file(pad, &path);
        }
    }

    pub fn vocal_import_file(&mut self, pad: usize, path: &std::path::Path) {
        match sample::load_file(path, self.shared.sample_rate) {
            Ok(audio) => {
                let loop_len = (self.scenes[self.scene].bars as f64 * self.frames_per_bar()).round() as i64;
                self.vocal_add_take(pad, audio, None, loop_len, Vec::new());
            }
            Err(e) => self.status = format!("Import failed: {e}"),
        }
    }

    // ---- Rendering.

    /// Called once per frame after the UI: re-renders stacks that changed, once the mouse
    /// is up, and hands the result to the engine.
    pub(super) fn vocal_commit(&mut self, ctx: &egui::Context) {
        if self.vocal_work.dirty.is_empty() || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        let pads: Vec<usize> = self.vocal_work.dirty.drain().collect();
        for pad in pads {
            self.vocal_render(pad);
        }
    }

    fn vocal_render(&mut self, pad: usize) {
        let sr = self.sr();
        let Some(stack) = self.pads.get(pad).and_then(|p| p.vocal.as_ref()) else { return };
        // The render only reads; shifts it could not find are collected to be made after.
        let wanted = std::cell::RefCell::new(Vec::new());
        let cache = &self.vocal_work.shifted;
        let out = stack.render(sr, &|audio, semis| {
            let key = (Arc::as_ptr(&audio.data) as usize, (semis * 100.0).round() as i32);
            let found = cache.get(&key).cloned();
            if found.is_none() {
                wanted.borrow_mut().push((key, audio.data.clone(), semis));
            }
            found
        });
        let wanted = wanted.into_inner();
        let empty = stack.is_empty();
        let p = &mut self.pads[pad];
        p.takes = if empty { Vec::new() } else { vec![Arc::new(out)] };
        p.take_idx = 0;
        p.cfg.start = 0;
        p.cfg.end = 0;
        p.transients.clear();
        p.refresh_mini();
        self.push_sample(pad);
        if self.rec_state(pad) == RecState::Idle {
            self.push_pad(pad);
        }
        // Pitch shifts are worked out in the background; the pad is rendered again when
        // each arrives.
        for (key, audio, semis) in wanted {
            if self.vocal_work.shifting.insert(key) {
                let tx = self.jobs_tx.clone();
                self.busy += 1;
                std::thread::spawn(move || {
                    let sample = stretch::conform(&audio, &[], 1.0, semis, sr);
                    let _ = tx.send(JobResult::Shifted { key, pad, sample });
                });
            }
        }
    }

    pub(super) fn vocal_shift_done(&mut self, key: (usize, i32), pad: usize, sample: SampleData) {
        self.vocal_work.shifting.remove(&key);
        self.vocal_work.shifted.insert(key, Arc::new(sample));
        self.vocal_work.dirty.insert(pad);
    }

    // ---- Editing. Each of these is one undo step, since the stack is part of the pad.

    /// Puts a stretch of a take into the comp.
    pub fn vocal_comp(&mut self, pad: usize, from: i64, to: i64, lane: usize) {
        let (from, to) = (self.vocal_snap(from.min(to)), self.vocal_snap(from.max(to)));
        if let Some(stack) = self.stack_mut(pad)
            && stack.lanes.get(lane).is_some_and(|l| l.kind == LaneKind::Take)
        {
            stack.set_comp(from, to, Some(lane));
            stack.hand_comped = true;
        }
    }

    /// For slot `i` of sixteen across the timeline, moves the comp on to the next take.
    pub fn vocal_cycle_slot(&mut self, pad: usize, i: usize) {
        let Some(stack) = self.stack_mut(pad) else { return };
        let takes: Vec<usize> = (0..stack.lanes.len()).filter(|l| stack.lanes[*l].kind == LaneKind::Take).collect();
        if takes.is_empty() {
            return;
        }
        let (from, to) = (stack.len * i as i64 / 16, stack.len * (i as i64 + 1) / 16);
        let now = stack.comp_lane_at((from + to) / 2);
        let next = match now.and_then(|l| takes.iter().position(|t| *t == l)) {
            Some(at) => takes[(at + 1) % takes.len()],
            None => takes[0],
        };
        stack.set_comp(from, to, Some(next));
        stack.hand_comped = true;
    }

    pub fn vocal_cut(&mut self, pad: usize, lane: usize, clip: usize, at: i64) {
        let at = self.vocal_snap(at);
        if let Some(stack) = self.stack_mut(pad)
            && let Some(second) = stack.split(lane, clip, at)
        {
            self.vocal.selected = Some((lane, second));
        }
    }

    pub fn vocal_delete_selected(&mut self, pad: usize) {
        let Some((lane, clip)) = self.vocal.selected.take() else { return };
        if let Some(clips) = self.stack_mut(pad).and_then(|s| s.lanes.get_mut(lane)).map(|l| &mut l.clips)
            && clip < clips.len()
        {
            clips.remove(clip);
        }
    }

    pub fn vocal_mute_selected(&mut self, pad: usize) {
        let Some((lane, clip)) = self.vocal.selected else { return };
        if let Some(c) = self.stack_mut(pad).and_then(|s| s.lanes.get_mut(lane)).and_then(|l| l.clips.get_mut(clip)) {
            c.muted ^= true;
        }
    }

    /// Ctrl+D: repeats the selected clip at the next grid line after it.
    pub fn vocal_duplicate_selected(&mut self, pad: usize) {
        let Some((lane, clip)) = self.vocal.selected else { return };
        let grid = self.snap_frames();
        let Some(stack) = self.stack_mut(pad) else { return };
        let Some(end) = stack.lanes.get(lane).and_then(|l| l.clips.get(clip)).map(|c| c.end()) else { return };
        let at = ((end as f64 / grid).ceil() * grid).round() as i64;
        if let Some(copy) = stack.duplicate_clip(lane, clip, at) {
            self.vocal.selected = Some((lane, copy));
        } else {
            self.status = "No room after that clip to repeat it".into();
        }
    }

    pub fn vocal_double(&mut self, pad: usize, lane: usize) {
        if let Some(stack) = self.stack_mut(pad) {
            stack.duplicate_as_thru(lane);
        }
    }

    pub fn vocal_remove_lane(&mut self, pad: usize, lane: usize) {
        self.vocal.selected = None;
        if let Some(stack) = self.stack_mut(pad) {
            stack.remove_lane(lane);
        }
    }

    /// Applies an edit made directly on the stack by the UI (a drag, a lane setting).
    pub fn vocal_edit(&mut self, pad: usize, edit: impl FnOnce(&mut VocalStack)) {
        if let Some(stack) = self.stack_mut(pad) {
            edit(stack);
        }
    }

    // ---- Baking.

    /// The comp as it sounds, optionally through the vocal bus effects.
    fn vocal_print(&mut self, pad: usize) -> Option<SampleData> {
        if self.vocal_work.dirty.remove(&pad) {
            self.vocal_render(pad);
        }
        let render = self.pads[pad].sample()?.clone();
        Some(if self.vocal.bake_fx { self.through_bus(VOCAL, &render) } else { SampleData::new(render.l.clone(), render.r.clone()) })
    }

    /// Flattens the comp and its doubles onto the next empty pad as an ordinary sample.
    pub fn vocal_bake_to_pad(&mut self, pad: usize) {
        let Some(print) = self.vocal_print(pad) else {
            self.status = "Nothing recorded on this pad yet".into();
            return;
        };
        let Some(dest) = self.next_empty_pad() else { return };
        let cfg = PadCfg { kind: PadKind::Audio, bus: if self.vocal.bake_fx { 0 } else { VOCAL as u8 + 1 }, mute: false, ..self.pads[pad].cfg };
        self.pads[dest].cfg = cfg;
        let name = format!("{} comp", self.pads[pad].name);
        self.install_take(dest, print, Some(name), None);
        self.status = format!("Baked the comp to {}", pad_label(dest));
    }

    /// Slices the comp into its phrases and spreads them across the lowest empty bank.
    pub fn vocal_bake_to_bank(&mut self, pad: usize) {
        let Some(print) = self.vocal_print(pad) else {
            self.status = "Nothing recorded on this pad yet".into();
            return;
        };
        let mut found = phrases(&print, self.sr());
        if found.is_empty() {
            self.status = "No phrases found in the comp".into();
            return;
        }
        found.truncate(16);
        let Some(bank) = self.lowest_empty_bank() else {
            self.status = "No empty bank left".into();
            return;
        };
        let cfg = PadCfg { kind: PadKind::Audio, bus: if self.vocal.bake_fx { 0 } else { VOCAL as u8 + 1 }, mute: false, ..self.pads[pad].cfg };
        let name = self.pads[pad].name.clone();
        for (i, (a, b)) in found.iter().enumerate() {
            let dest = bank * 16 + i;
            self.pads[dest].cfg = cfg;
            self.install_take(dest, print.slice(*a, *b), Some(format!("{name} {}", i + 1)), Some(Vec::new()));
        }
        self.bank = bank;
        let plural = if found.len() == 1 { "" } else { "s" };
        self.status = format!("Baked {} phrase{plural} to bank {}", found.len(), crate::music::bank_name(bank));
    }

    pub fn set_vocal_fx(&mut self, fx: VocalFx) {
        if fx != self.vocal_fx {
            self.vocal_fx = fx;
            self.send(Cmd::SetVocalFx(fx));
        }
    }
}
