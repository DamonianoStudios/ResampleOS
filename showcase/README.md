# Showcase projects

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
