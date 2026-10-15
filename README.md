<p align="center">
  <img src="assets/banner.svg" alt="ResampleOS" width="720">
</p>

ResampleOS is a pad-based DAW built around committing to audio. Linear DAWs make destructive work slow; hardware samplers make it fast but run out of pads, polyphony and screen. ResampleOS takes the sampler workflow (chop, resample, bake, repeat) and gives it unlimited banks, a piano roll, a step sequencer, plugin hosting and a capture buffer that never stops recording.

![ResampleOS: pad matrix, waveform inspector and bus effects](docs/images/screenshot.png)

| | |
| --- | --- |
| ![Playing a project: pads light as they sound](docs/images/demo-play.gif) | ![Comping vocal takes by swiping](docs/images/demo-vocal.gif) |
| Pads light as they sound, and the skip-back buffer is always recording | Swipe across a take to put that stretch of it in the comp |
| ![Vocal takes](docs/images/vocal.png) | ![Step sequencer](docs/images/steps.png) |
| Vocal takes, comp and doubles | Step sequencer for the whole bank |
| ![Mod / Warp](docs/images/mod-warp.png) | ![Perform mode](docs/images/perform.png) |
| Filter, LFO and warp per pad | Perform mode: the pads launch loops | 

![Piano Roll](docs/images/ResampleOS_PianoRoll.png) | A Piano roll | Piano roll: Place midi notes |

A short tour of the tabs: [demo-tour.gif](docs/images/demo-tour.gif).

![The arrangement: the demo song, with loops as sections, clips per pad, and the recording and capture tools](docs/images/arrangement.png)

> **Status: pre-release.** ResampleOS is developed and tested on Windows 11. It is written with cross-platform libraries, but it has not been built or run on macOS or Linux, and the plugin editor windows, icon embedding and UI font loading are Windows-specific today.

## What it does

- **Pads and banks.** A 4×4 pad matrix per bank, with banks A to Z, then AA onwards (64 in all). Zoom out to 8×8 or 16×16 to see several banks at once.
- **Resample to a pad.** Arm, hit the destination pad, perform, and the result (pads, bus effects and all) is baked onto that pad. If the transport is running the take starts on the next downbeat and is trimmed to whole bars.
- **Skip-back.** The last five minutes of output are always in memory. Grab the last 16 bars with one key, or drag a selection from the top-bar waveform onto a pad.
- **Take stack.** Every destructive edit (crop, reverse, normalize, pitch, conform, baked effects) becomes a new take. Each pad keeps 16 and can step back through them on its own.
- **Key and tempo conform.** Samples are analysed on load for tempo, key, pitch and transients. One action stretches and re-pitches a sample to the project, keeping transients intact, by phase vocoder (with optional formant keeping), beat slicing, or true varispeed.
- **Transient chop.** One key spreads a sample's slices across the next empty bank.
- **Piano roll and step sequencer.** Scale-locked piano roll with a velocity lane and free microtiming; a step grid per bank for drum patterns; swing and humanize on top.
- **Loops, chain and perform mode.** Up to sixteen loops of 1 to 16 bars over the same pads, switched on the beat, chained into an arrangement, or launched live from the pads.
- **Arrangement.** A song timeline beside the pads, as long as the song needs: a track per pad; clips that play the pad's loops, notes of their own, or audio (locked to the timeline, with gain, fades, crossfades, takes, and stretching to follow the tempo); sections that bring their loop's automation, with names and colours of their own; song-length automation lanes; markers; inserting and deleting time; box select, and snapping that `Alt` turns off. The chain becomes a song in one click.
- **Recording onto the song.** Armed tracks record the input or the notes played; loops launched while recording become sections; each pass round the loop brace is kept as a take. And everything played is remembered, so **Capture** puts the last bars of a jam onto the timeline after the fact.
- **Consolidate, bake and export.** Render clips to one audio clip in place (`Ctrl+J`) or through their effects onto a new track; export the song (or the loop brace) to WAV.
- **Automation.** Bus effects, levels and pans, pad volume, pan and cutoff, drum synth knobs and any plugin parameter can move through each loop: drawn in a lane with straight or curved segments, or recorded by turning the control (in ResampleOS or in a plugin's window) while the loop plays.
- **Drum synth.** Five engines (kick, snare and clap, metallic, FM, modal) with eight macros each, velocity and random amounts per knob, knob values locked to individual steps, seven 16-pad kits, and one key to bake a hit into a sample that can always be turned back into the synth.
- **Vocal takes.** A vocal pad records a take each time round the loop, with swipe comping, cut, move and slip editing, doubles, and baking to a pad or to a bank of phrases. A dedicated vocal bus has scale-locked pitch correction, levelling and a dub throw.
- **Performance buses.** Four buses, a vocal bus and a master, three effect slots each: vinyl, cassette, 3-band EQ, DJ isolator and filter, bit-crush, compressor, tape delay, reverb. Buses can feed later buses and can be sidechained from one another.
- **VST3 and CLAP hosting.** Instruments on pads, effects on buses and four analyzer slots that listen without touching the mix, with a scanner that copes with libraries of a thousand plugins or more. Plugin editors open in their own windows, scaled for high-DPI displays.
- **Sample browser.** Your sample folders inside the app, beside the pads or the timeline: browse, search, favourites, audition with the arrow keys, and drag onto a pad or the timeline.
- **MIDI files** in and out: onto pads or the timeline, and a pad or the whole song out. **Stems** by track or by bus, and **freeze** to spare the processor.
- **External input**, **MIDI controllers**, **project-wide undo and redo**.

## Getting started

You need [Rust](https://rustup.rs) 1.88 or newer and, on Windows, the MSVC build tools.

```bash
git clone https://github.com/DamonianoStudios/ResampleOS
cd ResampleOS
cargo run --release
```

The first build takes a few minutes. Audio files given on the command line load onto pads starting at A01:

```bash
cargo run --release -- loop.wav kick.wav snare.wav
```

A project folder opens that project instead:

```bash
cargo run --release -- path/to/project
```

To run the tests:

```bash
cargo test --release
```

## A five-minute tour

1. Drop a loop onto pad **A01**. Its tempo and key appear above the waveform.
2. Press **Ctrl+K**, then **Enter**: the loop is conformed to the project tempo and key as take 2.
3. Press **C**: its slices land on bank B.
4. Play the slices with **N , . /**, **H J K L**, **Y U I O** and **6 7 8 9** (the 4×4, bottom row first).
5. Press **R**, hit an empty pad on bank C, play something, press **R** again. That pad now holds what you played.
6. Select another empty pad, press **Tab**, and draw a bassline in the piano roll.
7. Press **G** to draw a drum pattern in the step sequencer, and **Space** to hear it all together.
8. Liked something you just heard? Press **S** to pull the last 16 bars onto a pad.

Press **F1** in the app for the shortcut list.

## Documentation

| | |
| --- | --- |
| [User guide](docs/USER_GUIDE.md) | Every part of the app, in the order you meet it |
| [Shortcuts and MIDI](docs/SHORTCUTS.md) | Keyboard reference and the controller mapping |
| [Architecture](docs/ARCHITECTURE.md) | How the engine, UI and plugin hosting fit together |
| [Roadmap](docs/ROADMAP.md) | Where ResampleOS is going: the arrangement view and the road to 1.0 |
| [Contributing](CONTRIBUTING.md) | Building, testing and the conventions the code follows |

## Known limitations

- VST3 and CLAP only: no VST2 or AU. CLAP editors open on Windows only.
- Some VST3 effects load and show their editor but pass no audio (three of fourteen vendors in testing). The bus now plays without such an effect rather than going silent, and the slot is marked.
- Hosted plugins are not crash-isolated once loaded; only scanning is.
- A new sample rate needs the app to restart (the project is carried across). Device, buffer size and skip-back length change in place.
- Input latency is measured with a loopback (a cable from output to input, or a microphone near a speaker). Monitoring through the app is as fast as the audio device allows, which on shared-mode Windows audio is not instant.
- The pitch correction and the vocal comp tools were verified with test signals and imported audio, not with a live microphone.
- Pad lights follow the common convention (a pad's note, velocity for brightness); controllers that need their own lighting messages stay dark. There is no controller mapping for vocal comping (the keyboard has one).

## License

ResampleOS is released under the [MIT License](LICENSE).

## Acknowledgements

ResampleOS is built on [egui](https://github.com/emilk/egui), [cpal](https://github.com/RustAudio/cpal), [symphonia](https://github.com/pdeljanov/Symphonia), [rustfft](https://github.com/ejmahler/RustFFT), [midir](https://github.com/Boddlnagg/midir), [rtrb](https://github.com/mgeier/rtrb) and [vst3-host](https://github.com/HelgeSverre/rust-vst3-host) and [clap-sys](https://github.com/micahrj/clap-sys). VST is a trademark of Steinberg Media Technologies GmbH.
