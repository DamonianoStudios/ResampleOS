# ResampleOS roadmap

This is the plan for taking ResampleOS from a pad-based sketching and performance tool to a DAW people can finish and release whole songs in, without giving up what makes it different. It covers the arrangement view in detail, then everything else a producer moving from an established DAW will expect to find.

Nothing here is a promise of dates. Sizes are relative: **S** is a few days of work, **M** one to two weeks, **L** several weeks, **XL** a month or more.

## Where ResampleOS stands

**What it already does well, and should keep doing well:**

- Committing to audio: resampling, skip-back capture of the last minutes of output, a 16-deep take stack on every pad, baking effects and synths into samples.
- Pads and loops: unlimited banks, a piano roll and step sequencer, 16 loops per project with a chain and perform mode.
- Built-in sound sources: a drum synth with five engines and kits, four internal synths, a vocal recorder with swipe comping, doubles and pitch correction.
- Mixing: four buses, a vocal bus and a master, each with three effect slots, sidechaining, built-in effects, VST3 and CLAP hosting, analyzer slots.
- Automation per loop, with curves, on built-in controls, drum knobs and plugin parameters.
- An arrangement (since 0.2, completed in 0.3): pattern, note and audio clips on a song timeline, recording onto it, capture of a performance after the fact, song-length automation, markers, consolidate and bake, and song export.
- Tempo and key conform with three warp methods, and pitch detection.

**What stops it being a full DAW today:**

| Gap | Effect on the user |
| --- | --- |
| No sample browser | Samples come in by dragging from Explorer or one file dialog at a time |
| No MIDI file import or export | Work can't move to or from another DAW |
| Fixed 4/4, one tempo | No time signature or tempo changes |
| Windows shared-mode audio only (no ASIO, no exclusive mode) | Latency is too high for tracking through the app |
| No plugin delay compensation | Lookahead limiters and linear-phase EQs pull their bus out of time |
| Fixed mixer: four buses, three slots each, no sends | Bigger mixes run out of room; no shared reverb or delay sends |
| No autosave or crash recovery | One plugin crash loses unsaved work |
| Windows only | Most of the market is on macOS |

## Principles

1. **Commit is a feature.** New features should make it easier to turn ideas into audio and move on, not harder. Every new kind of material (a clip, a track, a section) should be resampleable and bakeable.
2. **Capture first.** ResampleOS records what you play whether you meant to record or not (skip-back). The arrangement should work the same way: a performance can be put on the timeline after the fact.
3. **Pads stay the instrument.** The arrangement is a place to put what the pads play, not a separate world with its own tracks and devices.
4. **One source of truth.** A pattern or a take used in the arrangement and in a loop is the same object, so editing it in one place changes it everywhere unless the user makes it unique.
5. **The audio thread never waits.** Every new feature keeps to the existing rule: the engine works from immutable snapshots sent over a queue, and nothing it touches is freed on its thread.

## Milestones

| Version | Theme | Headline |
| --- | --- | --- |
| 0.2 | Arrangement, part 1 | A song timeline per project; loops and patterns placed as clips; play from anywhere; the chain becomes a song in one click; song export |
| 0.3 | Arrangement, part 2 | Audio clips, recording onto the timeline, performance capture, song-length automation, markers (released in 0.3.0) |
| 0.4 | Browser, mixer and export | A built-in sample browser (see [below](#sample-browser)), return tracks and sends, more buses and slots, stems, freeze, MIDI file import and export (in progress; the browser, MIDI files, stems and freeze released early in [0.3.5](releases/v0.3.5.md): see [0.4 progress](#04-progress)) |
| 0.5 | Pro audio | ASIO and exclusive-mode audio, plugin delay compensation, multi-core engine, plugin crash isolation, autosave |
| 0.6 | Time and MIDI depth | Tempo and time-signature changes, CC, pitch-bend and MPE lanes, groove, chord and scale tools, MIDI out and sync |
| 0.7 | Sound | Multisample sampler, a wavetable synth, better stock effects, tags and a stock library in the sample browser |
| 0.8 | Control | MIDI learn everywhere, controller scripts (Launchpad, APC, Push, MPK), Ableton Link |
| 1.0 | Release | macOS (and Linux) builds, signed installers, auto-update, onboarding, stability work |

Each milestone ends with a release, release notes in `docs/releases/` and an updated user guide.

---

## The arrangement view

### What it is

A horizontal timeline as long as the song needs, in bars and beats, below a ruler with locators and a loop brace. Each pad that has something to play gets a **track**. On a track sit **clips**: a stretch of a pattern for MIDI and drum pads, a stretch of audio for audio and vocal pads. Above the tracks, a **loop lane** shows which loop each section came from. Song-length automation lanes open under any track or bus.

It lives beside the pads rather than replacing them. As in Ableton Live, the loops are a session for jamming and building parts, and the arrangement is where they become a song. Launching a loop or hitting a pad while the arrangement plays overrides that pad until **Back to arrangement** is pressed.

### How it fits the existing model

| Today | In the arrangement |
| --- | --- |
| A pad holds one pattern per loop | A pad's track holds any number of clips. Each clip refers to a pattern, either a loop's pattern (linked) or its own copy (unique) |
| A loop is up to 16 bars and repeats | A loop dropped on the timeline becomes a **section**: one clip per pad that has a pattern in it, as long as the user drags it |
| The chain plays loops in order | **Chain to arrangement** writes the chain out as sections, one after another; this is the quickest way from jam to song |
| Automation belongs to a loop | Sections bring their loop's automation with them, as song-time lanes the user can then edit freely |
| A pad's sample plays from its start when hit | An audio clip plays a region of a take, locked to the timeline, from any point |
| Vocal takes are cut per loop pass | Vocal pads can record straight onto the timeline as long free takes, comped with the same tools |

The pad stays the unit of sound: its kind, synth or plugin, bus, effects, volume and pan apply to everything on its track. A track has no devices of its own. That keeps the arrangement light and keeps one way of doing things.

### Data model

```text
Arrangement
  tracks: Vec<Track>            one per pad in use, in pad order unless reordered
  sections: Vec<Section>        the loop lane: (start, length, loop index, name, colour)
  markers: Vec<Marker>          (tick, name)
  automation: Vec<SongLane>     like AutoLane, but in song ticks
  loop_brace: (start, end, on)
  length: u64                   end marker; export renders to here

Track
  pad: usize
  clips: Vec<Clip>              sorted by start, never overlapping
  height, folded, colour

Clip
  start: u64, len: u64          song ticks
  offset: u64                   where in the source the clip starts
  looped: bool                  repeat the source to fill the length
  source:
    Pattern(Arc<Pattern>, link: Option<loop index>)
    Audio(Arc<SampleData>, warp: Option<WarpSettings>)
  gain, fade_in, fade_out       audio clips
  muted
```

Song time is a `u64` tick count at the existing 480 PPQ, so a song can be as long as anyone could want. Patterns and audio are shared with `Arc`, which keeps undo snapshots cheap: the existing compare-and-snapshot undo keeps working.

### Engine changes

The engine plays loops today by wrapping its tick at the loop length and swapping pattern sets at loop boundaries. Song mode adds a second transport alongside it.

1. **Song clock.** A song tick that does not wrap, plus a loop brace that does. `Cmd::Locate(tick)` jumps there. The loop transport stays as it is for session play.
2. **Arrangement snapshot.** The UI sends an `Arc<ArrangementPlan>`: per pad, its clips flattened to what the engine needs (start, end, offset, source). The engine swaps it in between blocks, as it does `SceneSwap` today, and sends the old one back to be dropped.
3. **Note scheduling.** Each block, for each pad, find the clip under the playhead (binary search) and fire its notes that start in the block, as `collect_notes` does now for a looping pattern. Clip edges cut notes off cleanly.
4. **Audio clips.** A new voice type that plays a take sample-locked to song time and can start part-way through (on locate or loop-brace wrap). It declicks at clip edges and applies clip fades and gain. Warped clips are rendered when placed or when the tempo changes, as conform does today; real-time warping comes later.
5. **Chase on locate.** Starting mid-song starts any audio clip at the right offset, and optionally re-triggers MIDI notes that started earlier and are still held (a per-track "chase notes" switch, on for synths and off for drums).
6. **Session override.** A per-pad flag: while a pad is being played live or launched from a loop, its arrangement clips are ignored until **Back to arrangement**.
7. **Song automation.** Lanes evaluated at the song tick, layered as per-loop automation is now, with the same let-go-on-stop behaviour.

None of this needs the audio thread to allocate: plans are built on the UI thread and only read in the callback.

### Screen and interaction

- **A view switch** in the top bar (and `Tab` or `F3`): **Pads** shows today's screen; **Arrange** puts the timeline where the pad matrix and inspector are. The skip-back strip and the buses stay. Double-clicking a clip opens it in the inspector (piano roll, steps, waveform or vocal comp) in a split under the timeline.
- **Ruler:** bars and beats, click to move the playhead, drag to set the loop brace, markers above.
- **Track header:** pad label, name and colour, mute, solo, arm, a level meter, fold, and the pad's kind icon. Clicking it selects the pad, so the inspector and buses follow, just as clicking a pad does now.
- **Clips:** drag to move (with snap, `Alt` to bypass), drag edges to trim, drag the top corner to loop, `Ctrl+D` duplicate, `Ctrl+E` split at the playhead, `Ctrl+J` consolidate (render a selection to one audio clip, the arrangement's resample), right-click for make unique, rename, colour, reverse, and bake to audio. Pattern clips show their notes; audio clips show their waveform.
- **Drag in from anywhere:** a loop from the loop row (a section), a pad from the matrix (its pattern or its sample), a file from the explorer, a selection from skip-back.
- **Zoom and scroll** reuse the piano roll's (`Ctrl`+scroll zooms around the pointer, `Shift`+scroll moves along, follow the playhead).
- **Draw mode** (`B`) for drawing pattern clips and automation.
- **Automation lanes** open under a track or a bus, with the same points, curves and right-click Automate as the loop lanes.

### Capturing into the arrangement

Capture first is the part that makes this ResampleOS's own arrangement view rather than a copy.

- **Chain to arrangement:** writes the chain as sections in one step.
- **Record arrangement:** with the arrangement armed, launching loops and playing pads writes sections and clips at the playhead, as Live's session-to-arrangement recording does.
- **Performance skip-back:** the engine already keeps the last minutes of audio. Keep the last minutes of *events* too (pad hits, loop launches, knob moves) with their song times. **Capture performance** puts the last N bars of what was played on the timeline as clips and lanes, after the fact.
- **Audio recording:** an armed audio or vocal track records the input onto the timeline, with loop-brace takes stacked for comping in the vocal tools.

### Delivery in stages

**0.2: Arrangement, part 1** (released in 0.2.0; see [the release notes](releases/v0.2.0.md))

| Work | Size |
| --- | --- |
| Song clock, locate, loop brace in the engine | M |
| Arrangement model, project file version 3 (old projects open with an empty arrangement), undo | M |
| Pattern clips: scheduling, linked and unique patterns | L |
| Timeline UI: ruler, tracks, clips, move, trim, loop, split, duplicate, zoom, scroll | L |
| Sections from loops; Chain to arrangement | M |
| Pads / Arrange view switch, clip editing in the inspector | M |
| Session override and Back to arrangement | S |
| Song export: render the arrangement (or the loop brace) to WAV, 16 or 24 bit or 32-bit float, with optional normalise and tail | M |

Done when: a chain of loops becomes a song with one click, plays from any bar, can be lengthened and varied clip by clip, saves and reopens, undoes, and exports to a WAV identical (sample for sample) to what was heard.

**0.3: Arrangement, part 2** (released in 0.3.0; see [the release notes](releases/v0.3.0.md))

| Work | Size | Status |
| --- | --- | --- |
| Box select and select all on the timeline | S | Done |
| Audio clips: sample-locked playback, offsets, fades, gain, pre-rendered warp | L | Done |
| Recording onto the timeline (MIDI and audio), takes from loop-brace passes | L | Done: armed tracks record audio or notes; audio keeps a take per brace pass |
| Record arrangement from loop launches and pad play | M | Done: launches become sections, armed pads' hits become clips |
| Performance skip-back (event history) and Capture performance | M | Done: ten minutes of hits, loop passes and control moves |
| Song-length automation lanes; per-loop lanes carried in with sections | M | Done |
| Markers, section names and colours, insert and delete time | M | Done |
| Consolidate (`Ctrl+J`) and bake selection, using the resample engine | M | Done |
| Crossfades between adjacent audio clips | S | Done |

Also in 0.3: clips with notes of their own (recorded or captured, not using up a loop), snapping on or off with `Alt` to place freely, and split at the pointer.

Done when: a whole song with vocals, one-off fills and mix automation can be built and exported without touching the loop page after the first draft.

### Risks and open questions

- **Engine complexity.** Two transports doubles the paths through the sequencer. Mitigation: build the song clock so that loop play is a special case of it (a song of one repeating section) and retire the separate path once the arrangement is solid.
- **Pads with several patterns at once.** A pad plays one pattern at a time in the engine. Clips on one track never overlap, which keeps that true; a second simultaneous part means a second pad.
- **Undo on long songs.** Comparing snapshots is cheap only while everything heavy is shared. Clip lists must be `Arc`-shared per track so an edit copies one track, not the song.
- **Screen space.** The arrangement needs the width the pad matrix uses now. The split between timeline and inspector needs testing at 1080p.
- **Linked versus unique patterns.** Live makes clips independent; ResampleOS's loops suggest linking. The plan links by default with a visible link badge and one-click make unique. This needs user testing.

---

## Sample browser

### What it is

A browser for the user's own sample folders, inside ResampleOS, so finding and auditioning sounds never means leaving the app or dragging from Explorer. It opens as a panel on the left of either view, **Pads** or **Arrange**, over the pad matrix or the track headers, and closes again with the same key (`F4`) or button.

### Folders

- **Sample folders** are set in Preferences: one or several (a library drive, a splice folder, a project's bounces), each with a name. They are remembered with the settings, not the project.
- **Add folder…** in the browser itself, and right-click a folder to remove it, rename it or show it in Explorer.
- Each folder is a tree; a **Recent** list shows the files used lately, and **Project** shows the files the open project uses.

### Finding a sound

- **Search** across every folder by name as you type, with filters for kind (one-shot or loop), tempo range, key and length.
- **Analysis** runs in the background the first time a folder is seen and is cached (tempo, key, length, one-shot or loop, a waveform overview), using the analysis pads already run on load. The cache is a small file per folder in the settings folder, refreshed when files change.
- **Favourites:** a star on any file; **Favourites** is a filter.
- Results show name, length, tempo and key, and a waveform strip.

### Auditioning

- Click a file, or move through the list with the arrow keys, and it plays at once on a preview voice, outside the buses, at a preview level of its own.
- **Sync** plays loops stretched to the project tempo (and optionally the key), starting on the next beat while the transport runs, using the same warp as conform. **Raw** plays them as they are.
- Previews never touch the pads, the take stacks or undo.

### Getting it into the project

- **Pads view:** drag a file onto a pad (several files spill onto the next pads, as from Explorer); `Enter` loads the selected file onto the selected pad; `Shift+Enter` onto the next empty pad.
- **Arrange view:** drag a file onto a track for an audio clip there, or onto empty space for a new track (as dropping from Explorer does now); `Enter` puts it at the insert marker on the selected track.
- Drags show where the file will land before it is dropped. Conform on import applies as it does for any load.

### Data model and engine

```text
Prefs
  sample_folders: Vec<(name, path)>
  favourites: Vec<path>
BrowserCache (per folder, in the settings folder)
  entries: Vec<(relative path, size, modified, length, tempo, key, kind, overview)>
```

The preview is a voice of its own in the engine, fed a decoded file over the command queue like a pad's sample and dropped through the garbage queue; it plays to the master after the master's effects, so it is heard without being processed. Decoding and stretching happen on a background thread so scrolling through a list never waits for a file.

### Delivery in stages

| Work | Size |
| --- | --- |
| Folders in Preferences, the panel in both views, tree and file list, search by name | M |
| Preview voice, click and arrow-key audition, Raw and Sync preview | M |
| Drag and `Enter` to pads, tracks and the timeline | S |
| Background analysis with a per-folder cache; filters for kind, tempo, key and length | M |
| Favourites, Recent and Project lists | S |

Done when: with a library of 50,000 files in several folders, a user can find a kick by typing, hear ten of them in a few seconds with the arrow keys, and drag one onto a pad or the timeline without leaving the app or waiting.

### Risks and open questions

- **Large libraries.** The first scan of a big drive must not block anything; analysis runs at low priority and results appear as they come.
- **Screen space.** The panel takes width from the pad matrix or the timeline; it needs to be resizable and to remember its width per view.
- **Network and cloud folders** (OneDrive placeholders) may download a file just to read it; analysis should skip files that are not on the disk.

### 0.4 progress

| Work | Status |
| --- | --- |
| Sample browser: folders, browse, search, favourites, recent, audition, drag and Enter to pads and the timeline, in both views | Done |
| Sample browser: background analysis (tempo, key, one-shot or loop) with filters, and Sync audition at project tempo | Not started |
| MIDI file import (pads and timeline) and export (pad and song) | Done |
| Stem export, by track or by bus | Done |
| Freeze and unfreeze a track | Done |
| Return tracks and sends | Not started: needs a decision on how returns relate to today's buses |
| More buses, and more slots per bus | Not started |

---

## Competitive features beyond the arrangement

Each area lists what is missing, in priority order. **Priority** is relative to the others: **P1** blocks people from using ResampleOS as their main DAW, **P2** is expected by most, **P3** is nice to have.

### Audio engine and performance

| Feature | Why | Priority | Size |
| --- | --- | --- | --- |
| ASIO and WASAPI exclusive mode | Shared-mode latency is too high to record through the app; nearly every interface ships an ASIO driver | P1 | M |
| Plugin delay compensation | Lookahead and linear-phase plugins must not pull a bus out of time; the hosts already report latency | P1 | L |
| Multi-core processing | The engine runs on one thread; buses and plugin chains can run in parallel once their dependencies are known | P2 | XL |
| Plugin crash isolation | Run plugins in a separate process, at least as an option per plugin. A plugin crash should cost one plugin, not the session | P2 | XL |
| Real-time warping | Change tempo without re-rendering audio clips; follow tempo changes | P2 | L |
| 64-bit mix bus, oversampling for drive effects | Headroom and cleaner saturation | P3 | M |

### Mixing

| Feature | Why | Priority | Size |
| --- | --- | --- | --- |
| Return tracks and sends | Shared reverbs and delays, the usual way to mix | P1 | L |
| Variable number of buses, and of slots per bus | Four buses and three slots run out in a full song | P1 | L |
| Pre- and post-fader inserts, slot reordering by drag | Standard mixer flexibility | P2 | M |
| Group tracks and bus folders | Organising large sessions | P2 | M |
| Sidechain into hosted plugins | Plugin compressors and gates keyed from another bus | P2 | M |
| Multi-output instruments | Drum plugins with separate outputs per drum | P3 | L |
| A mixer view with every pad as a channel strip | Mixing at the pad level without the inspector | P3 | M |

### Editing and MIDI

| Feature | Why | Priority | Size |
| --- | --- | --- | --- |
| MIDI file import and export | Moving work between DAWs and collaborators | P1 | S |
| CC, pitch bend, aftertouch and MPE lanes in the piano roll | Expressive parts and hardware synths | P2 | L |
| Time signatures and tempo changes (a tempo track) | Anything not in steady 4/4 | P2 | L |
| Quantize options: strength, swing, groove templates extracted from audio | Feel without hand-editing | P2 | M |
| Chord and scale tools: chord stamps, strum, arpeggiator | Fast writing | P2 | M |
| Manual warp markers on audio | Fixing timing that detection gets wrong | P2 | M |
| Slice audio to a drum pad pattern (beat slicing to MIDI) | A classic sampler move; the transient detector is already there | P2 | S |
| Fold to scale, note lengths legato, humanize per selection | Everyday piano roll tools | P3 | S |

### Instruments and sound

| Feature | Why | Priority | Size |
| --- | --- | --- | --- |
| Multisample sampler (zones, velocity layers, round robin, loops) | Playing real instruments from samples | P2 | L |
| Wavetable synth | The most expected modern synth type | P2 | L |
| Stock effects: parametric EQ with analyser, glue and multiband compressors, limiter with true peak, algorithmic reverb, saturation, chorus and phaser | Mixing without third-party plugins | P2 | L |
| Sample browser with preview at project tempo and key, favourites and search (planned for 0.4, see [Sample browser](#sample-browser)); tags later | Finding sounds fast | P1 | M |
| Stock library: kits, loops and presets | A new user can make music on day one | P3 | L |

### Recording

| Feature | Why | Priority | Size |
| --- | --- | --- | --- |
| Multi-input recording (several inputs to several tracks at once) | Recording a band or several hardware synths | P2 | M |
| Punch, pre-roll and count-in on the timeline | Standard tracking workflow | P2 | S |
| Comping for any audio track (the vocal tools, generalised) | Already built for vocals | P2 | M |
| Input monitoring with low-latency direct path | Tracking through effects | P2 | M |

### Control and sync

| Feature | Why | Priority | Size |
| --- | --- | --- | --- |
| MIDI learn for every control | Any controller, any knob | P1 | M |
| MIDI clock and MTC out and in, MIDI out to hardware synths | Working with hardware | P2 | M |
| Ableton Link | Jamming with other apps and devices in time | P2 | M |
| Controller scripts: Launchpad, APC40/Mini, Push, MPK, Maschine | Pads and loops on hardware grids, with lights | P2 | L |
| Mackie Control and HUI | Fader controllers | P3 | M |

### Files and projects

| Feature | Why | Priority | Size |
| --- | --- | --- | --- |
| Song and stem export (WAV, FLAC, MP3), with loudness normalising | Getting music out | P1 | M |
| Autosave and crash recovery | Losing work loses users | P1 | S |
| Collect all and save (copy referenced files into the project), missing-file finder | Projects that move between machines | P2 | S |
| Project templates | Starting points | P3 | S |
| Archive project to one file | Sharing and backup | P3 | S |

### Platform and product

| Feature | Why | Priority | Size |
| --- | --- | --- | --- |
| macOS build (Core Audio, AU hosting, Apple silicon) | Most of the market | P1 | XL |
| Signed installers and auto-update | SmartScreen and Gatekeeper warnings put people off | P1 | M |
| Linux build (JACK and PipeWire, LV2 later) | A smaller but loyal market | P3 | L |
| Onboarding: a first-run tour and example projects | Converting downloads into users | P2 | M |
| Keyboard shortcut editor, colour themes | Comfort for long sessions | P3 | M |
| Accessibility: screen-reader labels, high-contrast theme | Reaching more people | P3 | M |

### How it compares

Where ResampleOS stands against what users of the established DAWs take for granted, and the version that closes each gap.

| Capability | Now | Closed in |
| --- | --- | --- |
| Session-style loop launching | Yes | Now |
| Linear arrangement | No | 0.2 to 0.3 |
| Song and stem export | Pads only | 0.2 and 0.4 |
| Resampling and capture | Yes, beyond most | Now |
| Plugin formats | VST3 and CLAP | AU with macOS (1.0) |
| Low-latency drivers | No | 0.5 |
| Delay compensation | No | 0.5 |
| Sends and returns | No | 0.4 |
| Automation | Per loop, with curves | Song-length in 0.3 |
| MIDI files, CC and MPE | No | 0.4 and 0.6 |
| Tempo and meter changes | No | 0.6 |
| Comping | Vocals | All audio in 0.6 |
| Pitch correction | Built in | Now |
| Sampler with zones | No | 0.7 |
| Controller integration | Basic MIDI | 0.8 |
| macOS | No | 1.0 |

## Technical foundations

Work that has to land early because the features above build on it.

1. **Song clock in the engine** (0.2). Arrangement, export, recording and sync all need it.
2. **An offline renderer that is the real engine.** The `render_project` test already plays a project through the engine without a device; turning it into the export path guarantees export matches playback (0.2).
3. **A processing graph instead of fixed buses** (0.4). Sends, variable buses, groups, delay compensation and multi-core all need the mix to be a graph the engine walks in dependency order. Designing it once for all of them avoids three rewrites.
4. **Latency reporting through the graph** (0.5), for delay compensation.
5. **A host abstraction ready for AU** (1.0). `PluginHandle` already hides the format; AU joins it on macOS.
6. **Project format versioning and migration tests** for every format change, so old projects always open.

## Not planned

- Video tracks and scoring to picture.
- Notation and score editing.
- Surround and immersive mixing.
- A web or mobile version.

These are large, serve a small part of ResampleOS's audience, and pull away from what makes it different. They can be revisited after 1.0.
