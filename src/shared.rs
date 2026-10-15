//! State shared between the audio thread, the MIDI thread and the UI. Everything here is
//! atomics so the audio thread never blocks and never allocates.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

pub const PADS_PER_BANK: usize = 16;
pub const MAX_BANKS: usize = 64;
pub const MAX_PADS: usize = PADS_PER_BANK * MAX_BANKS;
/// Buses 0..4 are Bus 1-4, index 4 is the master.
pub const NUM_BUSES: usize = 6;
/// The vocal bus sits after Bus 1-4, so any of them can feed it, and before the master.
pub const VOCAL: usize = 4;
pub const MASTER: usize = 5;

/// Display name of a bus by index.
pub fn bus_label(bus: usize) -> String {
    match bus {
        MASTER => "Master".into(),
        VOCAL => "Bus V".into(),
        b => format!("Bus {}", b + 1),
    }
}

/// Index of the bus a pad's `bus` setting (0 = thru, then 1-based) or an input's monitor
/// destination points at.
pub fn bus_index(bus: u8) -> usize {
    if (1..=MASTER as u8).contains(&bus) { bus as usize - 1 } else { MASTER }
}
pub const PEAK_HOP: usize = 512;

const R: Ordering = Ordering::Relaxed;

pub fn f32_load(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(R))
}

pub fn f32_store(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), R)
}

/// Continuous capture of the engine output: 32-bit float stereo, fixed size, written by the
/// audio thread only. Readers may copy any region at any time; samples right at the write
/// head can be a block stale, which is harmless for capture.
pub struct SkipBack {
    data: Box<[AtomicU32]>,
    peaks: Box<[AtomicU32]>,
    pub frames: usize,
    written: AtomicU64,
}

impl SkipBack {
    pub fn new(sample_rate: u32, seconds: usize) -> Self {
        let frames = sample_rate as usize * seconds.max(10);
        let npeaks = frames / PEAK_HOP;
        SkipBack {
            data: (0..frames * 2).map(|_| AtomicU32::new(0)).collect(),
            peaks: (0..npeaks).map(|_| AtomicU32::new(0)).collect(),
            frames,
            written: AtomicU64::new(0),
        }
    }

    /// Total frames ever written; the absolute timeline all capture positions refer to.
    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    /// Audio thread only.
    pub fn write(&self, l: &[f32], r: &[f32], peak_acc: &mut f32) {
        let mut w = self.written.load(R);
        let mut at = (w % self.frames as u64) as usize;
        for i in 0..l.len() {
            self.data[at * 2].store(l[i].to_bits(), R);
            self.data[at * 2 + 1].store(r[i].to_bits(), R);
            *peak_acc = peak_acc.max(l[i].abs()).max(r[i].abs());
            w += 1;
            at += 1;
            if at == self.frames {
                at = 0;
            }
            // PEAK_HOP is a power of two, so this is a mask rather than a division.
            if w.is_multiple_of(PEAK_HOP as u64) {
                let p = ((w / PEAK_HOP as u64 - 1) % self.peaks.len() as u64) as usize;
                f32_store(&self.peaks[p], *peak_acc);
                *peak_acc = 0.0;
            }
        }
        self.written.store(w, Ordering::Release);
    }

    /// Oldest absolute frame still held.
    pub fn oldest(&self) -> u64 {
        self.written().saturating_sub(self.frames as u64)
    }

    /// Copies `[start, end)` (absolute frames), clamped to what the ring still holds.
    pub fn read(&self, start: u64, end: u64) -> (Vec<f32>, Vec<f32>) {
        let end = end.min(self.written());
        let start = start.max(self.oldest()).min(end);
        let n = (end - start) as usize;
        let mut l = Vec::with_capacity(n);
        let mut r = Vec::with_capacity(n);
        for f in start..end {
            let idx = (f % self.frames as u64) as usize * 2;
            l.push(f32::from_bits(self.data[idx].load(R)));
            r.push(f32::from_bits(self.data[idx + 1].load(R)));
        }
        (l, r)
    }

    /// Peak of the `PEAK_HOP`-frame chunk with absolute chunk index `chunk`.
    pub fn peak(&self, chunk: u64) -> f32 {
        f32_load(&self.peaks[(chunk % self.peaks.len() as u64) as usize])
    }
}

/// Seconds of dry input kept for vocal takes: enough for sixteen bars at 40 BPM, or a
/// free pass of three minutes.
/// Plugin slots for analyzers (spectrum, loudness, stereo meters), fed a copy of a bus.
pub const NUM_ANALYZERS: usize = 4;

pub const INPUT_RING_SECS: usize = 180;

pub struct Shared {
    pub sample_rate: u32,
    pub ring: SkipBack,
    /// The external input as it arrives, before monitoring or effects. Vocal takes are cut
    /// from this.
    pub input_ring: SkipBack,
    /// Position in `input_ring` at which the playing loop last started.
    pub loop_frame: AtomicU64,
    /// Counts presses of Play, so a recording can tell a fresh start from a loop going round.
    pub play_starts: AtomicU32,
    /// Per-pad activity (envelope level of its loudest voice), 0 when idle.
    pub pad_level: Box<[AtomicU32]>,
    /// Per-pad playhead in frames, `u32::MAX` when idle.
    pub pad_pos: Box<[AtomicU32]>,
    /// Peak per bus, L then R.
    pub bus_peak: [AtomicU32; NUM_BUSES * 2],
    pub input_peak: AtomicU32,
    pub lufs_momentary: AtomicU32,
    pub lufs_short: AtomicU32,
    pub playing: AtomicBool,
    /// Transport position in ticks from the start of the playing loop (f64 bits); in
    /// song mode, from the start of the song.
    pub tick: AtomicU64,
    /// Where the song playhead is, in ticks (f64 bits), in either mode.
    pub song_pos: AtomicU64,
    /// The clocks LFOs run from, for drawing them: ticks at the project tempo (from Play
    /// while the transport runs) and seconds since the engine started. Both f64 bits.
    pub lfo_ticks: AtomicU64,
    pub lfo_secs: AtomicU64,
    /// Counts every time a loop starts over or another takes its place.
    pub scene_starts: AtomicU32,
    /// The loop the engine is playing.
    pub scene_index: AtomicU32,
    /// Fraction of the audio callback budget used, 0..1.
    pub load: AtomicU32,
    /// Position in `input_ring` at which the last latency test signal started, `u64::MAX`
    /// before one has played. See [`crate::latency`].
    pub ping_frame: AtomicU64,
    /// Bus effect plugins (`bus * 3 + slot`) that have stopped passing any audio and are
    /// being passed by.
    pub fx_silent: [AtomicBool; NUM_BUSES * 3],

    // Written by the UI, read by the MIDI thread so pad hits bypass the UI frame clock.
    pub ui_bank: AtomicU32,
    pub ui_selected: AtomicU32,
    pub ui_chromatic: AtomicBool,
    pub ui_octave: AtomicU32,
    pub ui_scale_mask: AtomicU32,
    /// While set, pad hits pick the resample target instead of sounding.
    pub ui_capture_pads: AtomicBool,
    /// Perform mode: pad notes launch loops, so the MIDI thread leaves them to the UI.
    pub ui_perform: AtomicBool,
    /// Where the input ring stood when the song playhead was where, at the start of the
    /// last block played: (frame, song tick as f64 bits), with a counter that is odd while
    /// they are being written. See [`Shared::song_anchor`].
    pub anchor_seq: AtomicU64,
    pub anchor_frame: AtomicU64,
    pub anchor_tick: AtomicU64,
}

impl Shared {
    pub fn new(sample_rate: u32, skipback_seconds: usize) -> Self {
        Shared {
            sample_rate,
            ring: SkipBack::new(sample_rate, skipback_seconds),
            input_ring: SkipBack::new(sample_rate, INPUT_RING_SECS),
            loop_frame: AtomicU64::new(0),
            play_starts: AtomicU32::new(0),
            pad_level: (0..MAX_PADS).map(|_| AtomicU32::new(0)).collect(),
            pad_pos: (0..MAX_PADS).map(|_| AtomicU32::new(u32::MAX)).collect(),
            bus_peak: std::array::from_fn(|_| AtomicU32::new(0)),
            input_peak: AtomicU32::new(0),
            lufs_momentary: AtomicU32::new((-70f32).to_bits()),
            lufs_short: AtomicU32::new((-70f32).to_bits()),
            playing: AtomicBool::new(false),
            tick: AtomicU64::new(0),
            song_pos: AtomicU64::new(0),
            lfo_ticks: AtomicU64::new(0),
            lfo_secs: AtomicU64::new(0),
            scene_starts: AtomicU32::new(0),
            scene_index: AtomicU32::new(0),
            load: AtomicU32::new(0),
            ping_frame: AtomicU64::new(u64::MAX),
            fx_silent: std::array::from_fn(|_| AtomicBool::new(false)),
            ui_bank: AtomicU32::new(0),
            ui_selected: AtomicU32::new(0),
            ui_chromatic: AtomicBool::new(false),
            ui_octave: AtomicU32::new(0),
            ui_scale_mask: AtomicU32::new(0xfff),
            ui_capture_pads: AtomicBool::new(false),
            ui_perform: AtomicBool::new(false),
            anchor_seq: AtomicU64::new(0),
            anchor_frame: AtomicU64::new(0),
            anchor_tick: AtomicU64::new(0),
        }
    }

    /// From the audio thread: the input ring's frame and the song tick at the start of a
    /// block.
    pub fn set_song_anchor(&self, frame: u64, tick: f64) {
        self.anchor_seq.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.anchor_frame.store(frame, std::sync::atomic::Ordering::Release);
        self.anchor_tick.store(tick.to_bits(), std::sync::atomic::Ordering::Release);
        self.anchor_seq.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }

    /// The last (input frame, song tick) pair, read whole.
    pub fn song_anchor(&self) -> (u64, f64) {
        use std::sync::atomic::Ordering::Acquire;
        loop {
            let a = self.anchor_seq.load(Acquire);
            let (f, t) = (self.anchor_frame.load(Acquire), self.anchor_tick.load(Acquire));
            if a.is_multiple_of(2) && self.anchor_seq.load(Acquire) == a {
                return (f, f64::from_bits(t));
            }
            std::hint::spin_loop();
        }
    }

    pub fn tick(&self) -> f64 {
        f64::from_bits(self.tick.load(R))
    }
}
