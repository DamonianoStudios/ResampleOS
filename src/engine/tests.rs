use super::*;
use crate::music::PPQ;
use std::f32::consts::TAU;
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
    let pattern = Pattern { len_ticks: TICKS_PER_BAR, notes: vec![Note::new(120, 60, 60, 127)] };
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
        Note::new(0, 120, 60, 100),
        Note::new(960, 120, 60, 100),
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
    let crush = SlotCfg { params: [1.0, 1.0, 1.0], ..SlotCfg::new(FxKind::Crush, true) };
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
    let notes = (0..4).map(|b| Note::new(b * PPQ, 60, 60, 100)).collect();
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

#[test]
fn queued_loop_takes_over_at_the_end_of_the_playing_one() {
    let (mut e, mut tx, shared) = rig();
    // Pad 0 hard left, pad 1 hard right, each a short hit on the one of a one-bar pattern.
    let hit = || Some(Arc::new(Pattern { len_ticks: TICKS_PER_BAR, notes: vec![Note::new(0, 60, 60, 127)] }));
    for pad in 0..2 {
        let cfg = PadCfg { pan: if pad == 0 { -1.0 } else { 1.0 }, ..PadCfg::default() };
        let _ = tx.push(Cmd::SetPadCfg { pad, cfg });
        let _ = tx.push(Cmd::SetSample { pad, sample: Some(tone(2400)) });
    }
    let _ = tx.push(Cmd::SetPattern { pad: 0, pattern: hit() });
    let _ = tx.push(Cmd::SceneLen(TICKS_PER_BAR));
    let _ = tx.push(Cmd::SetBpm(120.0));
    let _ = tx.push(Cmd::Play(true));
    let swap = SceneSwap { index: 1, len_ticks: TICKS_PER_BAR, next_bar: false, patterns: vec![(0, None), (1, hit())], automation: None };
    let _ = tx.push(Cmd::QueueScene(Some(Box::new(swap))));

    // One bar at 120 BPM is two seconds.
    let bar = SR as usize * 2;
    let out = run(&mut e, bar * 2);
    let side = |range: std::ops::Range<usize>, ch: usize| range.map(|i| out[i * 2 + ch].abs()).fold(0f32, f32::max);
    assert!(side(0..4800, 0) > 0.2, "the first loop plays pad 0");
    assert!(side(0..bar - 10, 1) < 1e-4, "pad 1 stays out until the switch");
    assert!(side(bar..bar + 4800, 1) > 0.2, "the queued loop comes in on the bar");
    assert!(side(bar..bar * 2, 0) < 1e-4, "and the first loop's pad is gone");
    assert_eq!(shared.scene_index.load(Ordering::Relaxed), 1);
    assert_eq!(shared.scene_starts.load(Ordering::Relaxed), 1, "one loop change in two bars, the second not yet over");
}

#[test]
fn lfo_shapes_stay_in_range_and_start_together() {
    for shape in LfoShape::ALL {
        for i in 0..200 {
            let v = shape.at(i as f64 / 50.0 - 1.0);
            assert!((-1.0..=1.0).contains(&v), "{shape:?} gave {v}");
        }
        // Every shape is at or above zero through the first quarter of its cycle, so
        // changing shape does not flip the direction of a modulation.
        assert!(shape.at(0.1) > 0.0 && shape.at(0.6) < 0.0, "{shape:?}");
    }
    // A synced LFO counts cycles in ticks, a free one in seconds.
    let synced = PadCfg { lfo_sync: crate::music::lfo_sync(4, 0), ..PadCfg::default() };
    assert_eq!(lfo_phase(&synced, PPQ as f64 * 3.0, 99.0), 3.0);
    let free = PadCfg { lfo_rate: 2.0, ..PadCfg::default() };
    assert_eq!(lfo_phase(&free, 12345.0, 1.5), 3.0);
}

#[test]
fn vocal_gate_closes_on_quiet_input_and_ducking_makes_room() {
    let (mut e, mut tx, _) = rig();
    // A quiet hiss into the vocal bus, with the gate set above it.
    let hiss: Vec<f32> = (0..48000).map(|i| ((i * 7919 % 1000) as f32 / 1000.0 - 0.5) * 0.004).collect();
    let _ = tx.push(Cmd::SetPadCfg { pad: 0, cfg: PadCfg { bus: 5, ..PadCfg::default() } });
    let _ = tx.push(Cmd::SetSample { pad: 0, sample: Some(Arc::new(SampleData::new(hiss.clone(), hiss))) });
    let _ = tx.push(Cmd::PadOn { pad: 0, vel: 1.0, semis: 0.0, key: 1 });
    assert!(peak(&run(&mut e, 9600)) > 1e-3, "ungated, the hiss comes through");
    let _ = tx.push(Cmd::SetVocalFx(VocalFx { gate_db: -30.0, ..VocalFx::default() }));
    run(&mut e, 4800);
    assert!(peak(&run(&mut e, 9600)) < 1e-5, "gated, it does not");

    // A mid-range tone on Bus 1 drops by about 6 dB while a vocal plays; a bass tone does not.
    let level_with_vocal = |hz: f32| {
        let (mut e, mut tx, _) = rig();
        let tone: Vec<f32> = (0..96000).map(|i| (i as f32 * hz * TAU / SR as f32).sin() * 0.2).collect();
        let _ = tx.push(Cmd::SetPadCfg { pad: 0, cfg: PadCfg { bus: 1, pan: -1.0, ..PadCfg::default() } });
        let _ = tx.push(Cmd::SetSample { pad: 0, sample: Some(Arc::new(SampleData::new(tone.clone(), tone))) });
        // The "vocal" is hard right and silenced on its own bus, so only Bus 1 is heard.
        let _ = tx.push(Cmd::SetPadCfg { pad: 1, cfg: PadCfg { bus: 5, ..PadCfg::default() } });
        let _ = tx.push(Cmd::SetSample { pad: 1, sample: Some(tone_at(0.3, 96000)) });
        let _ = tx.push(Cmd::SetBus { bus: VOCAL as u8, cfg: BusCfg { mute: true, ..BusCfg::default() } });
        let _ = tx.push(Cmd::SetVocalFx(VocalFx { duck_bus: 1, ..VocalFx::default() }));
        let _ = tx.push(Cmd::PadOn { pad: 0, vel: 1.0, semis: 0.0, key: 1 });
        let before = peak(&run(&mut e, 24000)[24000..]);
        let _ = tx.push(Cmd::PadOn { pad: 1, vel: 1.0, semis: 0.0, key: 2 });
        let during = peak(&run(&mut e, 48000)[48000..]);
        20.0 * (during / before).log10()
    };
    let mid = level_with_vocal(1000.0);
    assert!((-7.0..-4.5).contains(&mid), "midrange moved {mid} dB");
    let low = level_with_vocal(70.0);
    assert!(low.abs() < 0.7, "bass moved {low} dB");
}

fn tone_at(amp: f32, frames: usize) -> Arc<SampleData> {
    let v: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.05).sin() * amp).collect();
    Arc::new(SampleData::new(v.clone(), v))
}

#[test]
fn loop_starts_are_marked_in_the_input_ring() {
    let (mut e, mut tx, shared) = rig();
    let _ = tx.push(Cmd::SceneLen(TICKS_PER_BAR));
    let _ = tx.push(Cmd::SetBpm(120.0));
    run(&mut e, 1000);
    let _ = tx.push(Cmd::Play(true));
    // Two and a half bars: the loop has gone round twice, two seconds apart.
    run(&mut e, SR as usize * 5);
    assert_eq!(shared.play_starts.load(Ordering::Relaxed), 1);
    assert_eq!(shared.scene_starts.load(Ordering::Relaxed), 2);
    let second = shared.loop_frame.load(Ordering::Relaxed);
    // Play landed after the first 1000 frames.
    assert!((second as i64 - (1000 + SR as i64 * 4)).abs() <= 1, "{second}");
}

fn sides(buf: &[f32]) -> (f32, f32) {
    (
        buf.iter().step_by(2).fold(0f32, |m, v| m.max(v.abs())),
        buf.iter().skip(1).step_by(2).fold(0f32, |m, v| m.max(v.abs())),
    )
}

#[test]
fn hitting_a_long_sample_again_restarts_it() {
    let (mut e, mut tx, _) = rig();
    let flat = |frames: usize| Arc::new(SampleData::new(vec![0.25; frames], vec![0.25; frames]));
    let _ = tx.push(Cmd::SetPadCfg { pad: 0, cfg: PadCfg { vol: 1.0, ..PadCfg::default() } });
    let _ = tx.push(Cmd::SetSample { pad: 0, sample: Some(flat(SR as usize * 3)) });
    for key in 0..6 {
        let _ = tx.push(Cmd::PadOn { pad: 0, vel: 1.0, semis: 0.0, key });
        run(&mut e, 2400);
    }
    let out = run(&mut e, 4800);
    assert!((peak(&out[4800..]) - 0.25).abs() < 0.01, "copies piled up: {}", peak(&out[4800..]));
    // A different note of the same pad is a chord, not a retrigger.
    let _ = tx.push(Cmd::PadOn { pad: 0, vel: 1.0, semis: 7.0, key: 9 });
    let out = run(&mut e, 4800);
    assert!((peak(&out[4800..]) - 0.5).abs() < 0.01, "second note missing: {}", peak(&out[4800..]));

    // A short hit is allowed to overlap itself.
    let _ = tx.push(Cmd::SetSample { pad: 1, sample: Some(flat(SR as usize / 2)) });
    let _ = tx.push(Cmd::SetPadCfg { pad: 1, cfg: PadCfg { vol: 1.0, pan: -1.0, ..PadCfg::default() } });
    let _ = tx.push(Cmd::Panic);
    run(&mut e, 2400);
    for key in 0..2 {
        let _ = tx.push(Cmd::PadOn { pad: 1, vel: 1.0, semis: 0.0, key });
        run(&mut e, 2400);
    }
    let out = run(&mut e, 2400);
    assert!(peak(&out[2400..]) > 0.6, "short hits should layer: {}", peak(&out[2400..]));
}

#[test]
fn bus_pan_turns_one_side_down() {
    let (mut e, mut tx, _) = rig();
    let _ = tx.push(Cmd::SetPadCfg { pad: 0, cfg: PadCfg { bus: 1, ..PadCfg::default() } });
    let _ = tx.push(Cmd::SetSample { pad: 0, sample: Some(tone(48000)) });
    let _ = tx.push(Cmd::SetBus { bus: 0, cfg: BusCfg { pan: 0.5, ..BusCfg::default() } });
    let _ = tx.push(Cmd::PadOn { pad: 0, vel: 1.0, semis: 0.0, key: 1 });
    let (left, right) = sides(&run(&mut e, 4800)[4800..]);
    assert!(right > 0.2 && (left / right - 0.5).abs() < 0.02, "left {left} right {right}");
    let _ = tx.push(Cmd::SetBus { bus: 0, cfg: BusCfg { pan: -1.0, ..BusCfg::default() } });
    let (left, right) = sides(&run(&mut e, 4800)[4800..]);
    assert!(left > 0.2 && right < 1e-6, "left {left} right {right}");
}

#[test]
fn a_one_sided_input_is_centred() {
    let (mut e, mut tx, shared) = rig();
    let feed = |itx: &mut Producer<f32>, frames: usize, right: f32| {
        for i in 0..frames {
            let v = (i as f32 * 0.05).sin() * 0.5;
            let _ = itx.push(v);
            let _ = itx.push(v * right);
        }
    };
    let (mut itx, irx) = RingBuffer::new(96000);
    let _ = tx.push(Cmd::SetInput { rx: Some(irx), ratio: 1.0 });
    let _ = tx.push(Cmd::InputCfg { dest: 1, gain: 1.0 });
    feed(&mut itx, 9600, 0.0);
    let (left, right) = sides(&run(&mut e, 9600)[9600..]);
    assert!(left > 0.4 && (left - right).abs() < 0.01, "left {left} right {right}");
    // What is kept for vocal takes is centred too.
    let (l, r) = shared.input_ring.read(6000, 2000);
    assert!(l.iter().zip(&r).all(|(a, b)| (a - b).abs() < 0.01));

    // Told to leave it alone, it does.
    let _ = tx.push(Cmd::InputMode(1));
    feed(&mut itx, 9600, 0.0);
    let (left, right) = sides(&run(&mut e, 9600)[9600..]);
    assert!(left > 0.4 && right < 1e-3, "left {left} right {right}");

    // A real stereo source is not folded.
    let _ = tx.push(Cmd::InputMode(0));
    feed(&mut itx, 9600, -0.5);
    let (left, right) = sides(&run(&mut e, 9600)[9600..]);
    assert!((left - 0.5).abs() < 0.02 && (right - 0.25).abs() < 0.02, "left {left} right {right}");
}

#[test]
fn panic_silences_voices_and_effect_tails() {
    use crate::fx::FxKind;
    let (mut e, mut tx, _) = rig();
    let delay = SlotCfg { params: [0.5, 0.9, 1.0], ..SlotCfg::new(FxKind::Delay, true) };
    let _ = tx.push(Cmd::SetBus { bus: 0, cfg: BusCfg { slots: [delay, SlotCfg::new(FxKind::None, true), SlotCfg::new(FxKind::None, true)], ..BusCfg::default() } });
    let _ = tx.push(Cmd::SetPadCfg { pad: 0, cfg: PadCfg { bus: 1, ..PadCfg::default() } });
    let _ = tx.push(Cmd::SetSample { pad: 0, sample: Some(tone(96000)) });
    let _ = tx.push(Cmd::PadOn { pad: 0, vel: 1.0, semis: 0.0, key: 1 });
    run(&mut e, 48000);
    let _ = tx.push(Cmd::Panic);
    let out = run(&mut e, 48000);
    assert!(peak(&out[2000..]) < 1e-4, "still sounding: {}", peak(&out[2000..]));
}

fn drum_pad(tx: &mut Producer<Cmd>, pad: u16, name: &str, choke: u8, pan: f32) {
    let drum = crate::drum::preset_named(name).expect("factory sound");
    let _ = tx.push(Cmd::SetPadCfg { pad, cfg: PadCfg { kind: PadKind::Drum, drum, choke, pan, vol: 1.0, ..PadCfg::default() } });
}

#[test]
fn a_drum_pad_plays_ends_and_restarts_rather_than_piling_up() {
    let (mut e, mut tx, _) = rig();
    drum_pad(&mut tx, 0, "808 Snare", 0, 0.0);
    let _ = tx.push(Cmd::PadOn { pad: 0, vel: 0.8, semis: 0.0, key: 1 });
    let out = run(&mut e, 4800);
    let one = peak(&out);
    assert!(one > 0.2, "silent: {one}");
    // Hit it fast: each new hit cuts the last, so the level does not build.
    for key in 2..10 {
        let _ = tx.push(Cmd::PadOn { pad: 0, vel: 0.8, semis: 0.0, key });
        let out = run(&mut e, 960);
        // Past the few milliseconds the cut hit takes to fade.
        assert!(peak(&out[600..]) < one * 1.3, "hits piled up: {}", peak(&out[600..]));
    }
    let tail = run(&mut e, SR as usize * 2);
    assert!(peak(&tail[tail.len() - 2000..]) < 1e-4, "never ended");
}

#[test]
fn a_closed_hat_chokes_the_open_one() {
    let (mut e, mut tx, _) = rig();
    drum_pad(&mut tx, 0, "808 Open Hat", 5, -1.0);
    drum_pad(&mut tx, 1, "808 Closed Hat", 5, 1.0);
    let _ = tx.push(Cmd::PadOn { pad: 0, vel: 0.8, semis: 0.0, key: 1 });
    run(&mut e, 4800);
    let _ = tx.push(Cmd::PadOn { pad: 1, vel: 0.8, semis: 0.0, key: 2 });
    let out = run(&mut e, 4800);
    let (left, _) = sides(&out[2400..]);
    assert!(left < 1e-3, "open hat still ringing: {left}");
}

#[test]
fn a_locked_step_plays_its_own_values() {
    // The same kick twice in a bar, the second with its decay locked short.
    let (mut e, mut tx, _) = rig();
    drum_pad(&mut tx, 0, "808 Boom", 0, 0.0);
    let mut short = Note::new(TICKS_PER_BAR / 2, 60, 60, 100);
    short.locks[6] = crate::drum::lock_value(0.05);
    let notes = vec![Note::new(0, 60, 60, 100), short];
    let _ = tx.push(Cmd::SetPattern { pad: 0, pattern: Some(Arc::new(Pattern { len_ticks: TICKS_PER_BAR, notes })) });
    let _ = tx.push(Cmd::SetBpm(120.0));
    let _ = tx.push(Cmd::Play(true));
    // At 120 BPM half a bar is 48000 frames. Look 0.3 s after each hit.
    let out = run(&mut e, 96000);
    let l: Vec<f32> = out.iter().step_by(2).copied().collect();
    let long_tail = peak(&l[14400..19200]);
    let short_tail = peak(&l[48000 + 14400..48000 + 19200]);
    assert!(long_tail > 0.05 && short_tail < long_tail * 0.2, "long {long_tail} short {short_tail}");
}

#[test]
fn a_full_drum_kit_is_cheap_to_run() {
    let (mut e, mut tx, _) = rig();
    let kit = &crate::drum::kits()[0];
    for (i, name) in kit.pads.iter().enumerate() {
        drum_pad(&mut tx, i as u16, name, 0, 0.0);
        let _ = tx.push(Cmd::PadOn { pad: i as u16, vel: 1.0, semis: 0.0, key: 1 });
    }
    let t = std::time::Instant::now();
    run(&mut e, SR as usize);
    let used = t.elapsed().as_secs_f32();
    // All sixteen at once, for one second of audio, in well under a tenth of a second.
    assert!(used < 0.1, "a second of a full kit took {used} s");
}

fn lanes(target: AutoTarget, points: &[(u32, f32)]) -> Option<Automation> {
    Some(Arc::new(vec![AutoLane { target, points: points.iter().map(|&(tick, value)| AutoPoint::new(tick, value)).collect() }]))
}

#[test]
fn automation_moves_a_bus_and_lets_go_when_removed() {
    let (mut e, mut tx, _) = rig();
    let _ = tx.push(Cmd::SetPadCfg { pad: 0, cfg: PadCfg { bus: 1, mode: PlayMode::Loop, ..PadCfg::default() } });
    let flat = Arc::new(SampleData::new(vec![0.5; 4800], vec![0.5; 4800]));
    let _ = tx.push(Cmd::SetSample { pad: 0, sample: Some(flat) });
    let _ = tx.push(Cmd::SetBpm(120.0));
    let _ = tx.push(Cmd::SceneLen(TICKS_PER_BAR));
    // Bus 1's level rises from silence at the downbeat to full at beat 3 and holds.
    let _ = tx.push(Cmd::SetAutomation(lanes(AutoTarget::BusLevel(0), &[(0, 0.0), (TICKS_PER_BAR / 2, 1.0 / LEVEL_MAX)])));
    let _ = tx.push(Cmd::Play(true));
    let _ = tx.push(Cmd::PadOn { pad: 0, vel: 1.0, semis: 0.0, key: 1 });
    // At 120 BPM a bar is 96000 frames; halfway up the ramp is a quarter of the way in.
    let out = run(&mut e, 96000);
    let l: Vec<f32> = out.iter().step_by(2).copied().collect();
    let at = |f: usize| peak(&l[f..f + 480]);
    assert!(at(1000) < 0.05, "should start silent: {}", at(1000));
    assert!((at(24000) / at(60000) - 0.5).abs() < 0.08, "ramp: {} then {}", at(24000), at(60000));
    // Taking the lane away puts the level back where it was set by hand.
    let _ = tx.push(Cmd::SetBus { bus: 0, cfg: BusCfg { level: 0.25, ..BusCfg::default() } });
    let _ = tx.push(Cmd::SetAutomation(None));
    let out = run(&mut e, 4800);
    let level = peak(&out[4800..]);
    assert!((level - 0.25 * 0.4).abs() < 0.03, "level after release {level}");
}

#[test]
fn a_loop_switch_brings_its_own_automation() {
    let (mut e, mut tx, _) = rig();
    let _ = tx.push(Cmd::SetPadCfg { pad: 0, cfg: PadCfg { mode: PlayMode::Loop, ..PadCfg::default() } });
    let flat = Arc::new(SampleData::new(vec![0.5; 4800], vec![0.5; 4800]));
    let _ = tx.push(Cmd::SetSample { pad: 0, sample: Some(flat) });
    let _ = tx.push(Cmd::SetBpm(120.0));
    let _ = tx.push(Cmd::SceneLen(TICKS_PER_BAR));
    let _ = tx.push(Cmd::Play(true));
    let _ = tx.push(Cmd::PadOn { pad: 0, vel: 1.0, semis: 0.0, key: 1 });
    // The next loop pulls the pad's volume right down.
    let swap = SceneSwap { index: 1, len_ticks: TICKS_PER_BAR, next_bar: false, patterns: vec![], automation: lanes(AutoTarget::PadVol(0), &[(0, 0.0)]) };
    let _ = tx.push(Cmd::QueueScene(Some(Box::new(swap))));
    let out = run(&mut e, 96000 * 2);
    let l: Vec<f32> = out.iter().step_by(2).copied().collect();
    assert!(peak(&l[40000..41000]) > 0.2, "first loop should be heard");
    assert!(peak(&l[140000..141000]) < 1e-3, "second loop's automation should silence the pad");
}

/// The latency test signal, sent round a simulated loopback cable that delays it by a
/// known amount, is found at that delay in the input ring.
#[test]
fn latency_ping_comes_back_at_the_loopback_delay() {
    let (mut e, mut tx, shared) = rig();
    let delay = 1000;
    let (mut itx, irx) = RingBuffer::new(SR as usize * 4);
    for _ in 0..delay * 2 {
        let _ = itx.push(0.0f32);
    }
    let _ = tx.push(Cmd::SetInput { rx: Some(irx), ratio: 1.0 });
    let _ = tx.push(Cmd::InputCfg { dest: 0, gain: 1.0 });
    let _ = tx.push(Cmd::Ping);
    let frames = (crate::latency::LISTEN_SECS * SR as f32) as usize + 4800;
    let mut out = vec![0f32; 480 * 2];
    for _ in 0..frames / 480 {
        e.process(&mut out, 2);
        for v in &out {
            let _ = itx.push(*v);
        }
    }
    let from = shared.ping_frame.load(Ordering::Relaxed);
    assert_ne!(from, u64::MAX, "the ping never played");
    let need = (crate::latency::LISTEN_SECS * SR as f32) as u64;
    let (l, _) = shared.input_ring.read(from, from + need);
    let found = crate::latency::measure(&l, SR as f32).expect("ping not found");
    assert!(found.samples.abs_diff(delay) <= 2, "{found:?}, expected {delay}");
    // A perfect match scores 0.75: shifted, one chirp in four still lines up with another.
    assert!(found.confidence > 0.7, "{found:?}");
}

#[test]
fn a_bent_segment_eases_and_its_mirror_eases_the_other_way() {
    let lane = |curve: f32| AutoLane { target: AutoTarget::BusLevel(0), points: vec![AutoPoint { tick: 0, value: 0.0, curve }, AutoPoint::new(100, 1.0)] };
    let straight = lane(0.0).value_at(50.0).unwrap();
    let slow = lane(0.6).value_at(50.0).unwrap();
    let fast = lane(-0.6).value_at(50.0).unwrap();
    assert!((straight - 0.5).abs() < 1e-6);
    assert!(slow < 0.3 && fast > 0.7, "{slow} {fast}");
    assert!((slow + fast - 1.0).abs() < 1e-5, "mirrored bends should be symmetric: {slow} {fast}");
    // The ends stay put however hard it is bent.
    for c in [-1.0, -0.3, 0.3, 1.0] {
        assert!(lane(c).value_at(0.0).unwrap().abs() < 1e-6);
        assert!((lane(c).value_at(100.0).unwrap() - 1.0).abs() < 1e-6);
    }
}

#[test]
fn drum_knob_automation_moves_the_knob_and_lets_go_on_stop() {
    let (mut e, mut tx, _) = rig();
    let mut cfg = PadCfg { kind: PadKind::Drum, ..PadCfg::default() };
    cfg.drum.m[2] = 0.3;
    let _ = tx.push(Cmd::SetPadCfg { pad: 4, cfg });
    let _ = tx.push(Cmd::SetBpm(120.0));
    let _ = tx.push(Cmd::SceneLen(TICKS_PER_BAR));
    let _ = tx.push(Cmd::SetAutomation(lanes(AutoTarget::DrumMacro { pad: 4, m: 2 }, &[(0, 0.9)])));
    let _ = tx.push(Cmd::Play(true));
    run(&mut e, 4800);
    assert!((e.pads[4].cfg.drum.m[2] - 0.9).abs() < 1e-6, "automated: {}", e.pads[4].cfg.drum.m[2]);
    assert!((e.pads[4].cfg.drum.m[0] - cfg.drum.m[0]).abs() < 1e-6, "other knobs untouched");
    let _ = tx.push(Cmd::Play(false));
    run(&mut e, 480);
    assert!((e.pads[4].cfg.drum.m[2] - 0.3).abs() < 1e-6, "back to the hand-set value: {}", e.pads[4].cfg.drum.m[2]);
}

/// A lane on a hosted plugin's parameter sets it while it runs, and puts it back once no
/// lane moves it. Uses the first CLAP instrument installed; skipped where there is none.
fn installed_clap_instrument() -> Option<crate::clap::ClapPlugin> {
    use crate::clap::{ClapPlugin, Library, find_files, standard_dirs};
    let found = find_files(&standard_dirs()).into_iter().find_map(|f| {
        let d = Library::open(&f).ok()?.descriptors().into_iter().find(|d| d.features.iter().any(|x| x == "instrument"))?;
        Some((f, d.id))
    });
    match found {
        Some((path, id)) => Some(ClapPlugin::load(&path, Some(&id), SR as f64, MAX_BLOCK).expect("load")),
        None => {
            eprintln!("no CLAP instrument installed; skipped");
            None
        }
    }
}

#[test]
fn plugin_parameter_automation_sets_and_restores() {
    use crate::hosted::PluginHandle;
    let Some(plugin) = installed_clap_instrument() else { return };
    let param = plugin
        .parameters()
        .into_iter()
        .filter(|p| p.automatable)
        .find(|p| p.name.to_lowercase().contains("volume"))
        .or_else(|| plugin.parameters().into_iter().find(|p| p.automatable))
        .expect("an automatable parameter");
    let handle = Arc::new(std::sync::Mutex::new(plugin));
    let before = handle.lock().unwrap().get_parameter(param.id).unwrap();
    let target = if before > 0.5 { 0.1 } else { 0.9 };
    let mut rt = PluginRt::new(PluginHandle::Clap(handle.clone()), SR as f64);
    let (handle2, p2) = (handle.clone(), param.clone());
    std::thread::spawn(move || {
        let param = p2;
        for _ in 0..4 {
            rt.automate(param.id, target);
            rt.prepare(480);
            assert!(rt.run(480));
        }
        let during = handle2.lock().unwrap().get_parameter(param.id).unwrap();
        assert!((during - target as f64).abs() < 0.02, "{}: automated to {target}, reads {during}", param.name);
        // A block with no lane on it lets go; the next one delivers the old value.
        for _ in 0..2 {
            rt.prepare(480);
            assert!(rt.run(480));
        }
        rt.stop();
    })
    .join()
    .unwrap();
    let after = handle.lock().unwrap().get_parameter(param.id).unwrap();
    assert!((after - before).abs() < 0.02, "{}: was {before}, back to {after}", param.name);
}

/// A plugin that puts out pure silence for a clear signal is flagged after a few seconds,
/// and let back in the moment it makes a sound. An instrument with no notes stands in
/// for an effect that passes nothing.
#[test]
fn a_silent_effect_is_passed_by_until_it_makes_a_sound() {
    use crate::hosted::PluginHandle;
    let Some(plugin) = installed_clap_instrument() else { return };
    let mut rt = PluginRt::new(PluginHandle::Clap(Arc::new(std::sync::Mutex::new(plugin))), SR as f64);
    std::thread::spawn(move || {
        let blocks = |secs: f32| (secs * SR as f32 / 480.0) as usize;
        let mut flagged = false;
        for _ in 0..blocks(2.0) {
            rt.prepare(480);
            assert!(rt.run(480));
            flagged |= rt.gone_silent(480, 0.5, SR as f32);
        }
        assert!(!flagged, "flagged too soon");
        // Quiet input never counts against it.
        for _ in 0..blocks(5.0) {
            rt.prepare(480);
            rt.run(480);
            assert!(!rt.gone_silent(480, 0.001, SR as f32) || flagged);
        }
        for _ in 0..blocks(1.5) {
            rt.prepare(480);
            rt.run(480);
            flagged = rt.gone_silent(480, 0.5, SR as f32);
        }
        assert!(flagged, "three seconds of silence for a clear signal should be flagged");
        rt.push(PlugEv { on: true, note: 60, vel: 120, offset: 0 });
        let mut back = false;
        for _ in 0..blocks(0.2) {
            rt.prepare(480);
            rt.run(480);
            back |= !rt.gone_silent(480, 0.5, SR as f32);
        }
        assert!(back, "making a sound should clear the flag");
        rt.stop();
    })
    .join()
    .unwrap();
}

/// An analyzer slot listens and never changes the sound. A CLAP instrument stands in for
/// the analyzer (any plugin will do); skipped where none is installed.
#[test]
fn an_analyzer_leaves_the_output_untouched() {
    use crate::hosted::PluginHandle;
    let render = |with: bool| {
        let (mut e, mut tx, _) = rig();
        if with {
            let rt = PluginRt::new(PluginHandle::Clap(Arc::new(std::sync::Mutex::new(installed_clap_instrument()?))), SR as f64);
            let _ = tx.push(Cmd::SetAnalyzer { slot: 0, plugin: Some(rt), source: MASTER as u8 });
        }
        let _ = tx.push(Cmd::SetSample { pad: 0, sample: Some(tone(9600)) });
        let _ = tx.push(Cmd::PadOn { pad: 0, vel: 1.0, semis: 0.0, key: 1 });
        Some(run(&mut e, 9600))
    };
    let (Some(plain), Some(watched)) = (render(false), render(true)) else { return };
    assert!(peak(&plain) > 0.2);
    assert_eq!(plain, watched, "the analyzer changed the output");
}

/// A plan with one pad playing a one-note pattern in each of `clips` (start, len) bars.
fn song_plan(pad: u16, clips: &[(u64, u64)], sections: Vec<SongSection>) -> Arc<ArrangementPlan> {
    let bar = TICKS_PER_BAR as u64;
    let pattern = Arc::new(Pattern { len_ticks: TICKS_PER_BAR, notes: vec![Note::new(0, 120, 60, 127)] });
    let clips = clips.iter().map(|&(s, l)| SongClip { start: s * bar, len: l * bar, offset: 0, pattern: pattern.clone() }).collect();
    Arc::new(ArrangementPlan { tracks: vec![SongTrack { pad, clips }], sections })
}

/// Peaks of each 480-frame slice of the left channel.
fn slices(out: &[f32]) -> Vec<f32> {
    out.iter().step_by(2).copied().collect::<Vec<_>>().chunks(480).map(peak).collect()
}

#[test]
fn song_mode_plays_clips_where_they_are_and_nothing_elsewhere() {
    let (mut e, mut tx, shared) = rig();
    let _ = tx.push(Cmd::SetSample { pad: 0, sample: Some(tone(4800)) });
    let _ = tx.push(Cmd::SetBpm(120.0));
    // The pad's own loop pattern would hit on every bar; song mode must ignore it.
    let _ = tx.push(Cmd::SetPattern { pad: 0, pattern: Some(Arc::new(Pattern { len_ticks: TICKS_PER_BAR, notes: vec![Note::new(0, 120, 60, 127)] })) });
    let _ = tx.push(Cmd::SceneLen(TICKS_PER_BAR));
    // A clip on bars 3 and 4 only (zero-based bars 2 and 3).
    let _ = tx.push(Cmd::Song(SongCmd::Plan(Some(song_plan(0, &[(2, 2)], vec![])))));
    let _ = tx.push(Cmd::Song(SongCmd::Mode(true)));
    let _ = tx.push(Cmd::Play(true));
    // At 120 BPM a bar is 96000 frames.
    let s = slices(&run(&mut e, 96000 * 4));
    let bar = |b: usize| &s[b * 200..b * 200 + 20];
    assert!(peak(bar(0)) < 1e-4 && peak(bar(1)) < 1e-4, "nothing before the clip");
    assert!(peak(bar(2)) > 0.2 && peak(bar(3)) > 0.2, "the clip plays its pattern each bar");
    assert!(f64::from_bits(shared.song_pos.load(Ordering::Relaxed)) > 3.9 * TICKS_PER_BAR as f64);
    // Stop, locate into bar 4 and play: the next hit is that bar's downbeat... which has
    // passed, so the first sound is nothing until the clip ends; after it, silence.
    let _ = tx.push(Cmd::Play(false));
    let _ = tx.push(Cmd::Song(SongCmd::Locate(3.5 * TICKS_PER_BAR as f64)));
    let _ = tx.push(Cmd::Play(true));
    let s = slices(&run(&mut e, 96000 * 2));
    assert!(s.iter().all(|v| *v < 1e-4), "nothing starts after the located point");
}

#[test]
fn the_loop_brace_goes_round() {
    let (mut e, mut tx, shared) = rig();
    let _ = tx.push(Cmd::SetSample { pad: 0, sample: Some(tone(4800)) });
    let _ = tx.push(Cmd::SetBpm(120.0));
    let _ = tx.push(Cmd::Song(SongCmd::Plan(Some(song_plan(0, &[(1, 1)], vec![])))));
    let _ = tx.push(Cmd::Song(SongCmd::Mode(true)));
    // Brace over bars 2 and 3; start just inside it.
    let bar = TICKS_PER_BAR as f64;
    let _ = tx.push(Cmd::Song(SongCmd::Brace(Some((bar, 3.0 * bar)))));
    let _ = tx.push(Cmd::Song(SongCmd::Locate(bar * 2.5)));
    let _ = tx.push(Cmd::Play(true));
    // Half a bar to the brace end, then round to bar 2, where the clip hits.
    let s = slices(&run(&mut e, 96000));
    assert!(peak(&s[..90]) < 1e-4, "silent until the brace wraps");
    assert!(peak(&s[100..120]) > 0.2, "the clip at the brace start plays after the wrap");
    let pos = f64::from_bits(shared.song_pos.load(Ordering::Relaxed));
    assert!((bar..3.0 * bar).contains(&pos), "playhead stays inside the brace: {pos}");
}

#[test]
fn a_section_brings_its_loops_automation() {
    let (mut e, mut tx, _) = rig();
    let _ = tx.push(Cmd::SetPadCfg { pad: 0, cfg: PadCfg { mode: PlayMode::Loop, ..PadCfg::default() } });
    let flat = Arc::new(SampleData::new(vec![0.5; 4800], vec![0.5; 4800]));
    let _ = tx.push(Cmd::SetSample { pad: 0, sample: Some(flat) });
    let _ = tx.push(Cmd::SetBpm(120.0));
    let bar = TICKS_PER_BAR as u64;
    // Bar 2 is a section whose loop turns the pad right down.
    let quiet = lanes(AutoTarget::PadVol(0), &[(0, 0.0)]);
    let sections = vec![SongSection { start: bar, len: bar, loop_len: bar, automation: quiet }];
    let _ = tx.push(Cmd::Song(SongCmd::Plan(Some(Arc::new(ArrangementPlan { tracks: vec![], sections })))));
    let _ = tx.push(Cmd::Song(SongCmd::Mode(true)));
    let _ = tx.push(Cmd::Play(true));
    let _ = tx.push(Cmd::PadOn { pad: 0, vel: 1.0, semis: 0.0, key: 1 });
    let s = slices(&run(&mut e, 96000 * 3));
    assert!(s[100] > 0.2, "bar 1 plays at the pad's own level");
    assert!(s[300] < 1e-3, "bar 2 is silenced by its section's automation");
    assert!(s[500] > 0.2, "bar 3, outside the section, lets go again");
}
