use super::*;
use crate::music::PPQ;
use rtrb::RingBuffer;

const SR: u32 = 48000;

fn rig() -> (Engine, Producer<Cmd>, Arc<Shared>) {
    let shared = Arc::new(Shared::new(SR, 300));
    let (tx, rx) = RingBuffer::new(256);
    let (_tx2, rx2) = RingBuffer::new(8);
    let (gtx, _grx) = RingBuffer::new(64);
    (Engine::new(shared.clone(), rx, rx2, gtx), tx, shared)
}

fn run(e: &mut Engine, frames: usize) -> Vec<f32> {
    let mut out = vec![0f32; frames * 2];
    for chunk in out.chunks_mut(480 * 2) {
        e.process(chunk, 2);
    }
    out
}

fn tone(frames: usize) -> Arc<SampleData> {
    let v: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    Arc::new(SampleData::new(v.clone(), v))
}

fn peak(buf: &[f32]) -> f32 {
    buf.iter().fold(0f32, |m, v| m.max(v.abs()))
}

#[test]
fn pad_plays_and_ring_captures_it() {
    let (mut e, mut tx, shared) = rig();
    let _ = tx.push(Cmd::SetSample { pad: 0, sample: Some(tone(4800)) });
    let _ = tx.push(Cmd::PadOn { pad: 0, vel: 1.0, semis: 0.0, key: 1 });
    let out = run(&mut e, 9600);
    assert!(peak(&out[..9600]) > 0.2, "pad should sound");
    assert!(peak(&out[9600 * 2 - 2000..]) < 1e-4, "one-shot should have ended");
    // The skip-back ring holds exactly what went to the output.
    let (l, _) = shared.ring.read(0, 9600);
    assert_eq!(l.len(), 9600);
    for i in (0..9600).step_by(97) {
        assert!((l[i] - out[i * 2]).abs() < 1e-6);
    }
}

#[test]
fn choke_group_cuts_the_other_pad() {
    let (mut e, mut tx, _) = rig();
    for pad in 0..2 {
        let cfg = PadCfg { choke: 3, pan: if pad == 0 { -1.0 } else { 1.0 }, ..PadCfg::default() };
        let _ = tx.push(Cmd::SetPadCfg { pad, cfg });
        let _ = tx.push(Cmd::SetSample { pad, sample: Some(tone(48000)) });
    }
    let _ = tx.push(Cmd::PadOn { pad: 0, vel: 1.0, semis: 0.0, key: 1 });
    run(&mut e, 4800);
    let _ = tx.push(Cmd::PadOn { pad: 1, vel: 1.0, semis: 0.0, key: 2 });
    let out = run(&mut e, 4800);
    // Pad 0 is hard left, pad 1 hard right: after the choke only the right channel remains.
    let tail = &out[4800..];
    let left = tail.iter().step_by(2).fold(0f32, |m, v| m.max(v.abs()));
    let right = tail.iter().skip(1).step_by(2).fold(0f32, |m, v| m.max(v.abs()));
    assert!(left < 1e-3, "choked pad still audible: {left}");
    assert!(right > 0.2);
}

#[test]
fn sequencer_fires_notes_on_time_with_swing() {
    let (mut e, mut tx, _) = rig();
    let cfg = PadCfg { kind: PadKind::Midi, ..PadCfg::default() };
    let _ = tx.push(Cmd::SetPadCfg { pad: 5, cfg });
    // One note on the second 16th of a one-bar pattern.
    let pattern = Pattern { len_ticks: TICKS_PER_BAR, notes: vec![Note { start: 120, len: 60, pitch: 60, vel: 127 }] };
    let _ = tx.push(Cmd::SetPattern { pad: 5, pattern: Some(Arc::new(pattern)) });
    let _ = tx.push(Cmd::SetBpm(120.0));
    let _ = tx.push(Cmd::SetSwing(0.75));
    let _ = tx.push(Cmd::Play(true));
    let out = run(&mut e, 24000);
    let first = out.iter().step_by(2).position(|v| v.abs() > 1e-4).expect("note never fired");
    // Straight it would land at 6000 frames; 75% swing pushes it to 9000.
    assert!(first.abs_diff(9000) < 8, "note started at frame {first}");
}

#[test]
fn bus_routing_reaches_master_through_a_later_bus() {
    let (mut e, mut tx, shared) = rig();
    let _ = tx.push(Cmd::SetPadCfg { pad: 0, cfg: PadCfg { bus: 1, ..PadCfg::default() } });
    let _ = tx.push(Cmd::SetSample { pad: 0, sample: Some(tone(48000)) });
    let _ = tx.push(Cmd::SetBus { bus: 0, cfg: BusCfg { out: 3, ..BusCfg::default() } });
    let _ = tx.push(Cmd::SetBus { bus: 2, cfg: BusCfg { level: 0.5, ..BusCfg::default() } });
    let _ = tx.push(Cmd::PadOn { pad: 0, vel: 1.0, semis: 0.0, key: 1 });
    let out = run(&mut e, 4800);
    let b1 = crate::shared::f32_load(&shared.bus_peak[0]);
    let b3 = crate::shared::f32_load(&shared.bus_peak[4]);
    assert!(b1 > 0.2 && (b3 / b1 - 0.5).abs() < 0.05, "bus1 {b1} bus3 {b3}");
    assert!(peak(&out) > 0.1);
}
#[test]
fn stop_pad_silences_only_that_pad() {
    let (mut e, mut tx, _) = rig();
    for pad in 0..2 {
        let cfg = PadCfg { pan: if pad == 0 { -1.0 } else { 1.0 }, ..PadCfg::default() };
        let _ = tx.push(Cmd::SetPadCfg { pad, cfg });
        let _ = tx.push(Cmd::SetSample { pad, sample: Some(tone(48000)) });
        let _ = tx.push(Cmd::PadOn { pad, vel: 1.0, semis: 0.0, key: 1 });
    }
    run(&mut e, 2400);
    let _ = tx.push(Cmd::StopPad { pad: 0 });
    let out = run(&mut e, 4800);
    let tail = &out[4800..];
    let left = tail.iter().step_by(2).fold(0f32, |m, v| m.max(v.abs()));
    let right = tail.iter().skip(1).step_by(2).fold(0f32, |m, v| m.max(v.abs()));
    assert!(left < 1e-3 && right > 0.2, "left {left} right {right}");
}

#[test]
fn step_pattern_drives_an_audio_pad() {
    let (mut e, mut tx, _) = rig();
    let _ = tx.push(Cmd::SetSample { pad: 2, sample: Some(tone(2400)) });
    // Steps on beats 1 and 3 of a one-bar pattern at 120 BPM: frames 0 and 48000.
    let notes = vec![
        Note { start: 0, len: 120, pitch: 60, vel: 100 },
        Note { start: 960, len: 120, pitch: 60, vel: 100 },
    ];
    let _ = tx.push(Cmd::SetPattern { pad: 2, pattern: Some(Arc::new(Pattern { len_ticks: TICKS_PER_BAR, notes })) });
    let _ = tx.push(Cmd::SetBpm(120.0));
    let _ = tx.push(Cmd::Play(true));
    let out = run(&mut e, 60000);
    let l: Vec<f32> = out.iter().step_by(2).copied().collect();
    assert!(peak(&l[..2400]) > 0.1, "first step silent");
    assert!(peak(&l[10000..47000]) < 1e-4, "sound between steps");
    assert!(peak(&l[48000..50400]) > 0.1, "second step silent");
}

#[test]
fn external_input_is_monitored_and_capturable() {
    let (mut e, mut tx, shared) = rig();
    let (mut itx, irx) = RingBuffer::new(48000);
    for _ in 0..9600 {
        let _ = itx.push(0.25f32);
    }
    let _ = tx.push(Cmd::SetInput { rx: Some(irx), ratio: 1.0 });
    let _ = tx.push(Cmd::InputCfg { dest: 0, gain: 2.0 });
    let _ = tx.push(Cmd::RingSource(5));
    let out = run(&mut e, 2400);
    assert!(peak(&out) < 1e-6, "muted monitor should be silent");
    let (l, _) = shared.ring.read(100, 2000);
    assert!(l.iter().all(|v| (v - 0.5).abs() < 1e-4), "input not captured at gain");
    let _ = tx.push(Cmd::InputCfg { dest: 1, gain: 1.0 });
    let out = run(&mut e, 1200);
    assert!((peak(&out[1200..]) - 0.25).abs() < 1e-3);
}

#[test]
fn idle_bus_wakes_up_with_its_effects_intact() {
    use crate::fx::FxKind;
    let (mut e, mut tx, shared) = rig();
    let crush = SlotCfg { kind: FxKind::Crush, params: [1.0, 1.0, 1.0], on: true };
    let mut bus = BusCfg::default();
    bus.slots[0] = crush;
    let _ = tx.push(Cmd::SetBus { bus: 0, cfg: bus });
    let _ = tx.push(Cmd::SetPadCfg { pad: 0, cfg: PadCfg { bus: 1, ..PadCfg::default() } });
    let _ = tx.push(Cmd::SetSample { pad: 0, sample: Some(tone(4800)) });
    // Long enough in silence for the bus to stop running its chain.
    run(&mut e, SR as usize * 25);
    let _ = tx.push(Cmd::PadOn { pad: 0, vel: 1.0, semis: 0.0, key: 1 });
    let out = run(&mut e, 4800);
    let (dry, _) = (tone(4800), ());
    // Heavy bit-crushing: the output is audible and clearly not the dry signal.
    assert!(peak(&out) > 0.1, "bus stayed asleep");
    let differs = (0..2400).filter(|&i| (out[i * 2] - dry.l[i] * 0.8).abs() > 0.02).count();
    assert!(differs > 1000, "effect was bypassed after waking ({differs} samples differ)");
    assert!(crate::shared::f32_load(&shared.bus_peak[0]) > 0.1);
}

#[test]
fn humanize_moves_notes_off_the_grid_without_losing_any() {
    let (mut e, mut tx, _) = rig();
    let cfg = PadCfg { human_time: 1.0, human_vel: 1.0, ..PadCfg::default() };
    let _ = tx.push(Cmd::SetPadCfg { pad: 0, cfg });
    // A short click so each hit is easy to find: 20 frames of full-scale signal.
    let click = Arc::new(SampleData::new(vec![1.0; 20], vec![1.0; 20]));
    let _ = tx.push(Cmd::SetSample { pad: 0, sample: Some(click) });
    // One note per beat in a one-bar pattern at 120 BPM: 24000 frames apart.
    let notes = (0..4).map(|b| Note { start: b * PPQ, len: 60, pitch: 60, vel: 100 }).collect();
    let _ = tx.push(Cmd::SetPattern { pad: 0, pattern: Some(Arc::new(Pattern { len_ticks: TICKS_PER_BAR, notes })) });
    let _ = tx.push(Cmd::SetBpm(120.0));
    let _ = tx.push(Cmd::Play(true));
    let out = run(&mut e, 24000 * 12);
    let l: Vec<f32> = out.iter().step_by(2).copied().collect();
    // Find each hit's first frame and its level.
    let mut hits = Vec::new();
    let mut i = 0;
    while i < l.len() {
        if l[i].abs() > 0.01 {
            hits.push((i, l[i + 5].abs()));
            i += 2000;
        } else {
            i += 1;
        }
    }
    assert_eq!(hits.len(), 12, "every note should fire exactly once per pass");
    let drift: Vec<i64> = hits.iter().enumerate().map(|(n, h)| h.0 as i64 - n as i64 * 24000).collect();
    // 30 ticks at 120 BPM is 1500 frames; the first note cannot go earlier than time zero.
    assert!(drift.iter().all(|d| d.abs() <= 1500), "drift out of range: {drift:?}");
    assert!(drift.iter().filter(|d| d.abs() > 20).count() >= 8, "timing barely moved: {drift:?}");
    let (lo, hi) = hits.iter().fold((f32::MAX, 0f32), |(lo, hi), h| (lo.min(h.1), hi.max(h.1)));
    assert!(hi / lo > 1.15, "velocity barely varied: {lo}..{hi}");
}

/// Two pads: a long quiet "bed" on Bus 2 (hard left) and a short loud "kick" on Bus 1
/// (hard right), with Bus 2 keyed from Bus 1.
fn sidechain_rig(amount: f32) -> (Engine, Producer<Cmd>) {
    let (e, mut tx, _) = rig();
    let bed = Arc::new(SampleData::new(vec![0.25; 48000], vec![0.25; 48000]));
    let kick = Arc::new(SampleData::new(vec![0.9; 2400], vec![0.9; 2400]));
    let _ = tx.push(Cmd::SetPadCfg { pad: 0, cfg: PadCfg { bus: 2, pan: -1.0, vol: 1.0, ..PadCfg::default() } });
    let _ = tx.push(Cmd::SetSample { pad: 0, sample: Some(bed) });
    let _ = tx.push(Cmd::SetPadCfg { pad: 1, cfg: PadCfg { bus: 1, pan: 1.0, vol: 1.0, ..PadCfg::default() } });
    let _ = tx.push(Cmd::SetSample { pad: 1, sample: Some(kick) });
    let _ = tx.push(Cmd::SetBus { bus: 1, cfg: BusCfg { sc_src: 1, sc_amount: amount, sc_release: 0.05, ..BusCfg::default() } });
    (e, tx)
}

fn left_peak(out: &[f32], from: usize, to: usize) -> f32 {
    out[from * 2..to * 2].iter().step_by(2).fold(0f32, |m, v| m.max(v.abs()))
}

#[test]
fn sidechain_ducks_a_bus_while_its_key_sounds() {
    let (mut e, mut tx) = sidechain_rig(-1.0);
    let _ = tx.push(Cmd::PadOn { pad: 0, vel: 1.0, semis: 0.0, key: 1 });
    let before = run(&mut e, 4800);
    let _ = tx.push(Cmd::PadOn { pad: 1, vel: 1.0, semis: 0.0, key: 2 });
    let during = run(&mut e, 2400);
    let after = run(&mut e, 24000);
    let open = left_peak(&before, 2400, 4800);
    assert!(open > 0.2, "bed should be audible before the kick: {open}");
    assert!(left_peak(&during, 1200, 2400) < open * 0.1, "bed was not ducked");
    assert!(left_peak(&after, 20000, 24000) > open * 0.9, "bed did not come back");
}

#[test]
fn sidechain_swell_opens_a_bus_only_with_its_key() {
    let (mut e, mut tx) = sidechain_rig(1.0);
    let _ = tx.push(Cmd::PadOn { pad: 0, vel: 1.0, semis: 0.0, key: 1 });
    let before = run(&mut e, 4800);
    let _ = tx.push(Cmd::PadOn { pad: 1, vel: 1.0, semis: 0.0, key: 2 });
    let during = run(&mut e, 2400);
    assert!(left_peak(&before, 2400, 4800) < 0.01, "bed should be held down without its key");
    assert!(left_peak(&during, 1200, 2400) > 0.2, "bed did not swell with the key");
}
