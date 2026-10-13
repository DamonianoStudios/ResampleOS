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
5. **Buses.** Process Bus 1 to 4 and then the master. A bus may feed any later bus, which makes cycles impossible by construction. Sidechain keys are measured before any bus is processed, so a key always means "what is being sent into that bus". A bus that has had no input for 20 seconds skips its effect chain until signal returns; buses hosting a plugin are exempt.
6. **Capture.** Write the chosen source to the skip-back ring and meter the master.
7. **Metronome.** Added last, so it is never captured.

Blocks are at most 512 frames. A larger device buffer is rendered as several blocks.

### Skip-back

The skip-back buffer (`SkipBack` in `src/shared.rs`) is a fixed ring of 32-bit float stereo frames stored as atomics, with a parallel ring of peak values for drawing. The engine writes it; the UI can read any region at any time without a lock. Positions are absolute frame counts, so "the last 16 bars" and "from Mark In to Mark Out" are simple arithmetic.

Resample is built on the same ring. Arming records a start position; baking copies the region between then and now. Nothing is allocated on the audio thread.

## The app layer

`src/app/` holds `App` and every action the user can take, split by area:

| File | Area |
| --- | --- |
| `mod.rs` | State, startup, pushing model state to the engine, playing notes, per-frame housekeeping |
| `pads.rs` | Samples, the take stack, banks, chopping, and `PadAction` |
| `capture.rs` | External input, skip-back, resample |
| `conform.rs` | Harmonizer, warping, baking bus effects |
| `plugins.rs` | Plugin instances and editor windows |
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

Plugin loading, processing and editor windows use the [`vst3-host`](https://github.com/HelgeSverre/rust-vst3-host) crate.

**Scanning** (`src/plugins.rs`) never loads a plugin inside the app. The executable has a second mode, `--scan-vst3 <path>`, that examines one plugin and prints one line. The app runs several of these at a time, each with a 25 second limit, and caches results against the file's timestamp and size.

**Running.** A loaded plugin sits behind `Arc<Mutex<Plugin>>`, shared between the UI (editor, state, tempo) and the engine. The engine only ever uses `try_lock`: if the UI holds the lock, that plugin is silent for one block rather than stalling the audio thread. Notes for a busy plugin are kept for the next block. Output is checked for NaN and infinity before it is mixed.

Hosted plugins run inside the app's process. A plugin that crashes while playing takes the app with it.

## The UI

`src/ui/` draws with [egui](https://github.com/emilk/egui), an immediate-mode toolkit: the whole interface is redrawn from `App` on every frame, and there is no separate widget state to keep in step.

- `theme.rs` has colours, fonts and the egui style. `widgets.rs` has the custom controls.
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
