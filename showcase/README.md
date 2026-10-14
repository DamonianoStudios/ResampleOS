# Showcase projects

## Built-In Boom Bap

**Built-In Boom Bap** is made only from ResampleOS's own sounds: the drum synth, the internal synths and the built-in effects. It needs no samples or plugins, so it ships with the app. Open it from **File > Open project…** (choose the `showcase/built-in-boom-bap` folder) or from the command line:

```bash
cargo run --release -- showcase/built-in-boom-bap
```

Press `Space`. The chain is on, so it plays the whole two-minute arrangement: Intro, Verse A, Verse B, Break, Verse A, Verse B, Outro. It is 88 BPM in A minor, with 57% swing.

What it shows:

- **Drum synth pads** A01 to A08 and A13 to A16: kick, snare, clap, rim, closed and open hat (choked together), shaker, conga, an 808 boom, a laser zap, a ride and a tom. Some knobs move with velocity or wander a little on every hit, and the hats and percussion are humanized.
- **Internal synths** on A09 to A12: an 808 sub bass, FM keys playing Am7, Fmaj7, Dm7, Em7, a square-wave lead in Verse B, and a soft saw pad.
- **Buses:** drums glued by the compressor and tape; keys through vinyl, a DJ filter and reverb; the lead through a tape echo and reverb; the bass ducked under the drums by the sidechain.
- **Automation**, one idea per loop (open the **Automation** tab on each):
  - Intro: the keys' DJ filter opens over four bars.
  - Verse B: echo throws on the lead at the end of bars 2 and 8.
  - Break: the filter sweeps the keys thin while the pad swells.
  - Outro: the filter closes and the master fades out.

`make_builtin_showcase.py` writes the project, with every knob setting spelled out, so it doubles as an example of building projects from a script:

```bash
python showcase/make_builtin_showcase.py
```

## Taking It Back

`make_showcase.py` builds **Taking It Back**, a boom bap track at 87 BPM in A minor, from samples in a local Splice library. It is four loops (Intro, Verse, Lift, Break) chained into a 40-bar arrangement:

```bash
python showcase/make_showcase.py
cargo run --release -- showcase/taking-it-back
```

Pass the Splice folder as an argument if it is not `Documents/Splice`. Press `Space` to play the chain, `G` to watch each loop in the step sequencer, and `P` for perform mode, where the pad keys `N` `,` `.` `/` launch the four loops by hand.

What it shows:

- Drums on pads A01 to A08, programmed as patterns with swing and per-pad humanize, the hats in a choke group.
- Four loops and a chain: Intro, Verse, Lift, Verse, Lift, Break, then round again.
- Eight-bar keys, bass and vibraphone loops and a four-bar gu zheng loop on A09 to A12, started on the bar by one-note patterns. The vibraphone comes in for the Lift and the gu zheng carries the Break.
- A tempo-synced LFO panning the vibraphone, a bar each way.
- An internal-synth answer phrase on A14 in the Lift, written in the piano roll.
- Bus 1 (drums) through the compressor and cassette; Bus 2 (music) through vinyl and cassette.
- Bus 3 (bass) ducked by the drums, and Bus 4 (vinyl crackle) swelling with them, both through the bus sidechain.

The generated project folder is not committed. The samples are licensed to the Splice account that downloaded them and may not be redistributed as files, so share a recording of the project rather than the project itself.
