# Architecture

This document explains how ResampleOS is put together, for anyone reading or changing the code.

## The shape of it

ResampleOS is one Rust binary with three kinds of thread.

| Thread | Owns | Rules |
| --- | --- | --- |
| Audio callback | The `Engine` and everything it plays | Never blocks, never allocates, never frees |
| UI (main) | The `App`: the editable project and all windows | May block briefly; does the allocating and freeing |
| MIDI input, one per port | A queue into the engine | Translates notes and forwards them |

Background work (decoding files, analysis, stretching, plugin scanning) runs on short-lived threads or processes that report back to the UI thread through channels.

```
            commands (lock-free queue)
   UI  ───────────────────────────────────────▶  Engine  ──▶  audio device
   ▲   ◀───────────────────────────────────────    │
   │      garbage (lock-free queue)                 │
   │                                                ▼
   └──────────────  Shared (atomics)  ◀── meters, playheads, skip-back ring
```

### The UI never touches audio state

The project the user edits lives in `App` (`src/app/`, with the data types in `src/model.rs`). The engine has its own copy of what it needs to play. To change anything, the UI updates its model and sends a `Cmd` (`src/engine/types.rs`) down a lock-free queue. The engine applies commands at the start of each audio block.

Pad and bus settings are small `Copy` structs sent by value. Sample buffers and patterns are sent as `Arc`s. When the engine replaces one, it must not drop the old `Arc` itself, since that could free memory on the audio thread. It pushes it onto a second queue, the garbage queue, and the UI thread drops it.

### Reading back

Things the UI needs to see continuously (meter levels, pad activity, playheads, transport position, engine load) are atomics in `Shared` (`src/shared.rs`), written by the engine and read by the UI whenever it draws.

## The engine

`src/engine/` is the real-time core.

| File | Contents |
| --- | --- |
| `mod.rs` | The `Engine` struct, command handling, voice allocation |
| `render.rs` | One block of audio, as a sequence of stages |
| `types.rs` | `PadCfg`, `BusCfg`, `Pattern`, `Cmd`, `Garbage` |
| `plugin_rt.rs` | The audio thread's handle on a hosted plugin |
| `lufs.rs` | Loudness metering |
| `tests.rs` | Tests that drive the engine without an audio device |

`render_block` runs these stages in order:

1. **Sequence.** Find every pattern note that starts inside this block and trigger it at its exact sample offset. Swing is a piecewise-linear warp of tick time. Humanize offsets come from a hash of (pad, note, pass), so a note's drift is the same whichever block asks about it, and a note that drifts across a loop boundary still fires once.
2. **Voices.** Render sample voices (Hermite interpolation, per-voice filter and envelope) and internal synth voices into their pads' bus buffers.
3. **Plugins.** Render hosted instruments.
4. **Input.** Pull external input from the capture thread's ring, rate-matched to the engine.
5. **Buses.** Process Bus 1 to 4, the vocal bus, and then the master. A bus may feed any later bus, which makes cycles impossible by construction. Sidechain keys are measured before any bus is processed, so a key always means "what is being sent into that bus". A bus that has had no input for 20 seconds skips its effect chain until signal returns; buses hosting a plugin are exempt.
6. **Capture.** Write the chosen source to the skip-back ring and meter the master.
7. **Metronome.** Added last, so it is never captured.

Blocks are at most 512 frames. A larger device buffer is rendered as several blocks.

### Loops

A project has up to sixteen loops: sets of patterns over the same pads. The engine holds only the patterns of the loop that is playing, and its transport position counts from the start of that loop and wraps at its length.

A change of loop has to land exactly on the beat, so the UI does not make it. It sends the incoming loop's patterns ahead of time in one `SceneSwap` (`Cmd::QueueScene`). When the playing loop ends, or at the next bar line if the swap asks for that, the engine exchanges the patterns in place, in the middle of a block if need be, and sends the outgoing ones back on the garbage queue. It counts every loop start in `Shared`, and the UI follows: it moves its own model to the loop the engine reports and, if a chain is running, queues the entry after that. The UI has a whole loop in which to do so.

On the UI side the pads always carry the patterns of the loop on screen; the others are kept in `App::scenes` (`src/app/scenes.rs`). Undo snapshots store all patterns with their loops and none on the pads, so moving between loops is not seen as an edit.

### Vocal takes

The engine knows almost nothing about vocals. To it, a vocal pad is an audio pad, and what it adds is small:

- A second ring, `Shared::input_ring`, that always holds the last three minutes of dry input.
- A mark, `Shared::loop_frame`, of where in that ring the playing loop began, set sample-accurately when Play is pressed and each time a loop starts.
- The vocal bus's gate, de-esser and midrange ducking (`vocal_front_end` and `make_room` in `render.rs`).

Everything else is on the UI side. `src/vocal.rs` holds the model: a `VocalStack` of lanes, each a list of clips into shared recordings, and a comp that says which take is heard where. `App::vocal_poll` (`src/app/vocal.rs`) watches the loop marks while a pad is armed and cuts each pass out of the ring as a take, shifted by the input latency. Whenever a stack changes it is rendered to one buffer, with a 2 ms equal-power fade at every clip edge and a crossfade where the comp changes take, and that buffer is sent to the engine as the pad's sample. A one-note pattern starts it at the top of the loop. Pitch-shifted copies of takes for play-through lanes are made on a background thread and cached.

Because the stack is part of the pad, undo covers vocal edits like any other.

`src/tune.rs` is the pitch correction used by the Pitch Lock effect: a YIN-style detector on a decimated copy of the input, and pitch-synchronous overlap-add to rebuild the voice at the corrected pitch. Grains are moved to change pitch and read faster or slower to move formants, which is what keeps the two independent.

### Skip-back

The skip-back buffer (`SkipBack` in `src/shared.rs`) is a fixed ring of 32-bit float stereo frames stored as atomics, with a parallel ring of peak values for drawing. The engine writes it; the UI can read any region at any time without a lock. Positions are absolute frame counts, so "the last 16 bars" and "from Mark In to Mark Out" are simple arithmetic.

Resample is built on the same ring. Arming records a start position; baking copies the region between then and now. Nothing is allocated on the audio thread.

## The app layer

`src/app/` holds `App` and every action the user can take, split by area:

| File | Area |
| --- | --- |
| `mod.rs` | The `App` struct, startup, pushing model state to the engine, per-frame housekeeping |
| `state.rs` | The small types describing what the app is showing: tabs, modes, editor state |
| `play.rs` | The transport, playing pads and notes, recording them into a pattern |
| `pads.rs` | Samples, the take stack, banks, chopping, and `PadAction` |
| `capture.rs` | External input, skip-back, resample |
| `conform.rs` | Harmonizer, warping, baking bus effects |
| `plugins.rs` | Plugin instances and editor windows |
| `scenes.rs` | Loops, the chain, launching |
| `vocal.rs` | Vocal pads: recording takes, editing them, rendering and baking |
| `roll.rs` | Piano roll selection edits |
| `history.rs` | Undo and redo |
| `input.rs` | Keyboard shortcuts and MIDI controller messages |

### Undo by comparison

Undo does not ask each action to record how to reverse itself. Once per frame, `track_history` compares the project with the last committed snapshot. If it differs, and no mouse button is down and no text is being typed, the previous snapshot is pushed onto the undo stack. A gesture therefore becomes one step, and a new feature gets undo without doing anything.

This is affordable because snapshots share their heavy parts. Sample buffers and plugin instances are `Arc`s, compared by identity and cloned by reference.

Restoring a snapshot replaces the model and re-sends only the pads and plugin slots that differ.

### Takes

A pad's take stack is a `Vec<Arc<SampleData>>` and an index. Destructive edits go through one function, `install_take`, which pushes a buffer, drops any redo branch, and caps the stack at 16.

## Analysis and stretching

`src/analysis.rs`:

- **Onsets:** multi-band spectral flux, each band normalised by its own average, peak-picked against a local mean, then refined to the point where the level jumps and backed up to a zero crossing.
- **Tempo:** autocorrelation of the onset envelope, scored with a comb at the beat, its multiples and its half, with a penalty at one third of the lag to reject the common two-thirds error. A loop that is a whole number of bars long snaps to the exact tempo its length implies.
- **Key:** a 12-bin pitch class profile, correlated with the Krumhansl-Schmuckler major and minor profiles.
- **Pitch:** YIN, taking the median over the sounding frames, and reporting nothing unless most frames are clearly periodic.

The onset envelope is the expensive part and does not depend on sensitivity, so it is computed once per take and cached.

`src/stretch.rs` stretches between transient markers with WSOLA (overlap-add with a waveform-similarity search). The first 15 ms of each transient are copied through unchanged, and each segment's first and last grains are pinned to the input's own start and end, so segments splice without a crossfade. Pitch shifting is a stretch followed by resampling.

## Plugin hosting

Plugin loading, processing and editor windows use the [`vst3-host`](https://github.com/HelgeSverre/rust-vst3-host) crate. The build uses the copy in `vendor/vst3-host`, which carries a few small changes that plugin editors and the scanner depend on; [`vendor/README.md`](../vendor/README.md) lists them.

**Scanning** (`src/plugins.rs`) never loads a plugin inside the app. The executable has a second mode, `--scan-vst3 <path>`, that lists every plugin in one file and prints one line. It reads the file's class list without creating any plugin, so a shell holding hundreds of plugins costs about as much as a file holding one. Catalog entries, pad and bus slots and saved projects identify a plugin by file and class id. The app runs several of these at a time, each with a 25 second limit, and caches results against the file's timestamp and size.

**Running.** A loaded plugin sits behind `Arc<Mutex<Plugin>>`, shared between the UI (editor, state, tempo) and the engine. The engine never parks on the lock. It retries for a few tens of microseconds, which outlasts the UI's per-frame servicing, and only then gives the block up: an instrument is silent for that block and an effect passes its input through. Notes for a busy plugin are kept for the next block. Output is checked for NaN and infinity before it is mixed.

**Feeding editors.** A plugin's processor and its editor are separate objects, and the host carries data between them. While an editor is open, the UI thread drains what the processor reported each frame (`EditorWin::feed` in `src/app/plugins.rs`), which hands output parameter points such as gain-reduction readouts to the editor. Plugins that instead have the editor poll the processor through connection-point messages, often from a thread of their own, rely on the vendored crate forwarding those messages.

Hosted plugins run inside the app's process. A plugin that crashes while playing takes the app with it.

## The UI

`src/ui/` draws with [egui](https://github.com/emilk/egui), an immediate-mode toolkit: the whole interface is redrawn from `App` on every frame, and there is no separate widget state to keep in step.

- `theme.rs` has colours, fonts and the egui style. `widgets.rs` has the custom controls.
- The brushed steel and the bevelled keys are added after layout, not by the widgets. Panels are filled with `theme::PANEL` and buttons with a few flat colours at one corner radius; `theme::finish`, called once per frame, goes through the painted shapes and replaces those rectangles with textured plates and shaded keys. egui's own buttons, menus and drop-downs get the look without their call sites changing. A rectangle that should stay flat must not use the key radius (6).
- Each zone is a card with an exact rectangle (`theme::card_in`), so zones cannot overlap.
- Settings are edited on a copy and committed once per frame. Anything that rewrites a pad is returned as a `PadAction` and run after the commit.
- The window repaints at full rate only while something is moving, and about fifteen times a second otherwise.

## Testing

`cargo test --release` runs:

- **Engine tests** (`src/engine/tests.rs`) that build an `Engine` with no audio device, send it commands, render blocks and check the samples: playback, choke groups, sequencer timing with swing, bus routing, stopping a pad, step patterns, external input, bus sleeping, humanize and sidechaining.
- **Analysis tests** on synthesised material: tempo, onsets, key and pitch.
- **Stretch tests** for output length and transient position.
- **Music tests** for scales, swing and bank naming.

The UI and device I/O have no automated tests.
