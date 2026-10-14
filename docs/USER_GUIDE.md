# ResampleOS user guide

This guide walks through the app in the order you are likely to meet it. For a keyboard reference, see [Shortcuts and MIDI](SHORTCUTS.md).

- [The screen](#the-screen)
- [Pads and banks](#pads-and-banks)
- [Working with samples](#working-with-samples)
- [Key and tempo conform](#key-and-tempo-conform)
- [Capturing audio](#capturing-audio)
- [Sequencing](#sequencing)
- [Buses and effects](#buses-and-effects)
- [VST3 plugins](#vst3-plugins)
- [Undo, takes and projects](#undo-takes-and-projects)
- [Preferences](#preferences)
- [Troubleshooting](#troubleshooting)

## The screen

| Area | What lives there |
| --- | --- |
| Top left | Transport: play, tempo, key and scale, swing, Resample, metronome, Undo and Redo |
| Top right | Skip-back: a scrolling picture of the last few seconds or minutes of output |
| Left | The pad matrix and bank tabs |
| Centre | The inspector for the selected pad: Waveform, Piano Roll, Steps and Mod / Warp tabs |
| Bottom right | Bus 1 to 4 and the master, each with three effect slots |
| Footer | Capture source, external input, roll, chromatic mode, choke matrix, conform mode, meters and status |

The pad LED colours mean:

| Colour | Meaning |
| --- | --- |
| Amber | The pad holds a sample |
| Cyan | The pad is a MIDI pad |
| Green | The pad is sounding |
| Purple | The pad is muted or in a choke group |
| Flashing red | The pad is the destination of a resample in progress |

## Pads and banks

A pad is one of two kinds:

- An **audio pad** holds a sample and plays it as a one-shot, gated, or as a latched loop.
- A **MIDI pad** holds a pattern and plays the internal synth, a VST3 instrument, or another pad's sample at different pitches.

Click a pad or press its key to play it. The pad's settings are in the row under the inspector's tabs: volume, pan, mute, solo, Stop, bus, choke group, play mode and reverse. Drag the pan box sideways (`Shift` for fine) and double-click it to centre.

Hitting a pad again while a long sample (over a second) is still playing restarts it: the copy already playing is faded out, so repeated hits do not pile up. Short hits are left to overlap, and a different note of the same pad (Chromatic mode, a MIDI pad playing it) still layers, so chords work.

**Stop All** in the top bar (`Shift+Space`) stops the transport and silences everything at once, including the tails of the built-in effects. A hosted plugin's own tail (a VST reverb, say) is left to the plugin.

**Play modes** apply to audio pads:

| Mode | Behaviour |
| --- | --- |
| 1-Shot | Plays to the end regardless of how long you hold |
| Gate | Plays while held |
| Loop | Loops until you hit the pad again |

**Choke groups** make pads cut each other off, the way an open hi-hat is cut by a closed one. Set a pad's group in the inspector, or open **Choke Matrix** in the footer to assign the whole bank at once. In the matrix, click a cell or drag across several: dragging down a column puts every pad you cross in that group, and a drag that starts on a lit cell clears instead.

**Chromatic mode** (`X`) turns the sixteen pads into a keyboard for the selected pad. In Strict Chromatic conform mode the pads step by semitones; in Smart Conform they step through the project scale.

**Banks.** The `+` after the bank tabs adds a bank. Right-click a bank tab to remove that bank, or to remove every empty bank. Removing a bank moves the later ones down a letter.

**Stopping a pad.** `Backspace` stops the selected pad, holding `Q` and hitting a pad stops that pad, and `Esc` silences everything.

## Working with samples

Load a sample by dropping a file onto a pad (several files spill onto the following pads), by right-clicking a pad, or from the File menu. WAV, MP3, FLAC, OGG and AIFF are read. Files are converted to the engine's sample rate on load.

The **Waveform** tab shows the active take:

- Drag to set the play region. Double-click to reset it.
- Cyan lines are transient markers. Right-click to add or remove one. The **Sens** slider changes how many are detected.
- Scroll to zoom, `Shift` + scroll to pan.

The **BAKE** buttons are destructive edits. Each one creates a new take:

| Button | What it prints |
| --- | --- |
| Crop | Keeps only the play region |
| Normalize | Raises the level so the loudest peak sits just under full scale |
| Reverse | Reverses the audio |
| Fade In / Fade Out | Fades across the play region |
| Pitch | Prints the pad's pitch setting into the audio as a speed change, then zeroes the setting |
| Bus FX | Runs the pad's bus effects into the audio, then routes the pad straight to the master so the effects are not applied twice |

**Chop** (`C`) cuts the sample at its transient markers and places the slices on the lowest empty bank. With more than fifteen markers, the fifteen hardest hits are kept, so the slices always fit one bank.

The **Mod / Warp** tab has three panels.

- **Pitch · Filter:** coarse and fine pitch, and a low-pass filter with a display of its response.
- **LFO:** modulates pitch, volume, pan or the filter cutoff. Choose a shape (sine, triangle, saw, square) and a depth. The rate is either free, in Hz, or with **SYNC** on a note value locked to the project tempo: 4 bars down to 1/32, each straight, dotted or triplet. The scope draws two cycles of the wave at its depth, shows how far the modulation reaches (for example "pitch ±1.5 st"), and has a dot that follows the LFO as the engine runs it. A synced LFO starts its cycle when the transport starts.
- **Warp:** changes pitch and length independently and bakes the result as a new take.

## Key and tempo conform

When a sample loads, ResampleOS estimates its tempo, its key, and, for a single-note sound, its pitch. `Ctrl+K` opens the harmonizer for the selected pad.

- The detected values are shown with a confidence figure. If the tempo is off by a factor of two, use the `÷2` and `×2` buttons. You can also correct the key by hand.
- **Conform to project** stretches the sample to the project tempo and shifts it to the project key, as a new take. `Enter` does the same.
- **Set project to this sample** does the opposite: the project takes the sample's tempo and key.

Two conform modes are available, in the harmonizer and the footer:

| Mode | Pitch shift |
| --- | --- |
| Strict Chromatic | Moves the detected root onto the project root by the shorter route. An E major sample in a C minor project moves down 4 semitones. |
| Smart Conform | Chooses the shift that puts the most of the sample's notes onto degrees of the project scale. |

With **Lock transients** on, the first few milliseconds of every hit pass through at original speed and only the decay between hits is stretched, so drums keep their attack.

Detection is an estimate. Sparse, rubato or very short material may come back with no tempo at all, and the app says so rather than guessing.

## Capturing audio

### Resample

Resample records the output onto a pad.

1. Press `R`. The button asks for a target.
2. Hit the pad to record onto. It flashes red.
3. Perform.
4. Press `R` again to bake.

If the transport is running when you choose the target, recording starts on the next downbeat and the result is trimmed (or padded) to a whole number of bars and set to Loop. If the transport is stopped, silence before your first sound and after your last is trimmed, and the pad is a one-shot.

The baked pad is routed straight to the master, since the bus effects are already in the audio.

### Skip-back

The output is recorded continuously into a buffer five minutes long by default. Nothing needs arming.

- `S` puts the last 16 bars on the selected pad. If that pad already holds something, the next empty pad is used instead.
- **Mark In** and **Mark Out** bracket a passage as it plays.
- Drag on the waveform to select a passage after the fact, then drag the selection onto a pad, or use **To Next Pad** or **Slice to Bank**.

While you are selecting, the display is held still and says so. A click on the bar, `Esc`, **Go Live**, or capturing the selection lets it follow the audio again. Audio played during the hold is still recorded.

### What gets captured

**CAPTURE** in the footer chooses what Resample and skip-back record: the master, one bus after its effects, or the external input on its own.

### External input

Choose a device under **INPUT** in the footer. Next to it are where the input is heard (Muted, Thru, or a bus, so you can put effects on it) a gain, and how its two channels are treated. **Auto** centres a source that arrives on one channel only, such as a microphone on input 1 of a stereo interface, and leaves a real stereo source alone; **Stereo** never touches it; **Mono** always sums the two. This applies to what you hear, to Capture and to vocal takes. To record the input dry, set CAPTURE to Input; it is recorded even when monitoring is muted.

## Sequencing

Sequenced pads play when the transport runs (`Space`). **Swing** in the top bar shifts every second sixteenth note later, from 50% (straight) to 75%.

### Piano roll

Select an empty pad and press `Tab`, or make a pad a MIDI pad from its right-click menu.

- **Engine** chooses what the pad plays: the internal synth (808 Sub, Saw Lead, Square Pluck, FM Keys), a VST3 instrument, or another pad's sample.
- Click to add a note, drag to move it, drag its right edge to resize, right-click to delete.
- Click a note to select it. `Shift`+click or `Ctrl`+click adds a note to the selection or takes it out, `Shift`+drag from an empty spot draws a box around several, and `Ctrl+A` selects everything. `Esc` clears the selection.
- Dragging or resizing any selected note moves or resizes the whole selection.
- Hold `Ctrl` while dragging to duplicate: copies stay behind and the notes you grabbed move. `Ctrl+D` repeats the selection directly after itself.
- With notes selected, `Shift+Up` and `Shift+Down` move them an octave, `Up` and `Down` move them one step (one scale degree while scale lock is on), `Left` and `Right` nudge them by one grid step, and `Delete` removes them.
- The lane at the bottom sets velocity.
- **Snap** sets the grid. Off gives free placement; holding `Alt` ignores the grid for one drag.
- **Scale lock** greys out notes outside the project scale and keeps new notes on it.
- **Ghosts** shows other MIDI pads' notes in the bank, and the transients of looping audio pads, as a guide.
- **Rec** writes notes you play into the pattern while the transport runs.

### Loops, the chain and perform mode

A project holds up to sixteen **loops**. Each is its own set of patterns over the same pads, 1 to 16 bars long, so a loop can be an intro, a verse, a variation or a breakdown. The **LOOPS** row at the top of the pad zone shows them.

- Click a number to work on that loop. The piano roll and step sequencer always show the loop that is selected, which is also the loop that plays.
- **+** adds a loop as a copy of the current one, ready to be varied. `Shift`+click **+** for an empty one. Right-click a loop to name or delete it.
- **Bars** sets the loop's length. Patterns shorter than the loop repeat inside it, and the loop grows by itself if you make a pattern longer.
- While the transport runs, clicking another loop does not cut over at once: the loop is lined up (its number flashes) and takes over when the playing loop ends, exactly on the beat. **Switch at** changes that to the next bar line.
- When a loop ends or hands over, long samples and held synth notes it started are cut, so the next loop starts clean. Short hits ring out.

The **chain** plays loops one after another. `Shift`+click loops in the order you want them (a loop can appear several times), turn **CHAIN** on, and press play: playback starts at the first entry and goes round. Click an entry to remove it. You can still launch a loop by hand while the chain runs; the chain carries on afterwards.

**Perform mode** (`P`, or the **PERFORM** button) turns the pad matrix into sixteen loop launchers. The pad keys and a controller's pads launch loops instead of playing sounds, each launcher shows its loop's length, how many pads play in it and how far through it is, and the one lined up next is marked. The bus strips stay live for effects and mutes. Press `P` again to get the pads back.

## Step sequencer

Press `G`. Each row is a pad of the current bank; each column is a step. Click or drag to paint, right-click a step to cycle normal, accent and soft. **Bars** and **Grid** set the length and resolution. The steps are the same patterns the piano roll edits, so a pad can be worked on in either.

### Humanize

**Time** and **Vel** in the piano roll (for one pad) and the step sequencer (for the whole bank) loosen a pattern at playback. At 100%, a note can land up to about a sixteenth of a beat early or late, and velocities vary by up to half. Every pass through the pattern is different. The pattern itself is not changed, so turning humanize back to zero restores exact timing.

### Roll

With **Roll** on (`T`), a held pad retriggers at the rate chosen beside it.

## Buses and effects

Each pad is sent either straight to the master (**Thru**), to one of four buses, or to the vocal bus, **Bus V**. A bus has three effect slots, a level, a pan (the box beside **SC**: drag sideways, double-click to centre) and an output that can be the master or a later bus. Bus V comes after Bus 1 to 4, so any of them can feed it.

| Effect | Controls |
| --- | --- |
| Vinyl Sim | Warble, Dust, Age |
| Cassette | Drive, Flutter, Tone |
| 3-Band EQ | Low, Mid, High: each cuts or boosts by up to 15 dB, flat at centre. **Lo** and **Hi** sliders set where the shelves turn |
| DJ Isolator | Low, Mid, High (zero kills the band) |
| DJ Filter | Cutoff (left of centre is low-pass, right is high-pass), Reso, Mix |
| 12-Bit Crush | Rate, Bits, Mix |
| FET Comp | Thresh, Ratio, Mix |
| Tape Delay | Time (tempo-synced), Feedbk, Mix |
| Reverb | Size, Damp, Mix |
| Pitch Lock | Retune (slow and natural to instant), Formant (up to 7 semitones either way), Shift (an octave either way). Pulls a single voice onto the project key and scale. It is for one dry voice at a time: harmonies, doubles or a vocal with reverb already on it give it no single pitch to follow |
| Vocal Comp | Level (a slow leveller), Peak (a fast limiter after it), Grit (drive) |
| Dub Throw | Throw (how much is sent into the echo), Time (tempo-synced), Space (reverb and repeats) |

In the **3-Band EQ**, the low band is a shelf below the **Lo** frequency (40 Hz to 600 Hz) and the high band a shelf above the **Hi** frequency (1.5 kHz to 16 kHz). The mid band is a bell centred halfway between the two, and it narrows as you bring them together. Hover over a slider to read its frequency and where the bell sits. Unlike the DJ Isolator, which can remove a band completely, the EQ is for shaping.

Drag a knob to change it, hold `Shift` for fine control, double-click to reset. Keys `1` to `4` and `5` focus a bus; a hardware controller's three knobs follow the slot you last touched.

### Sidechain

**SC** in a bus header lets another bus control this one's level.

- **Keyed by** chooses the bus to listen to. The key is the sound being sent into that bus, before its effects.
- **Duck** turns this bus down while the key sounds. Put the kick on Bus 1 and the bass on Bus 2, key Bus 2 from Bus 1, and the bass makes room for each kick.
- **Swell** does the reverse: this bus is held down and rises with the key. Put vinyl crackle on a bus keyed from the snare's bus and it blooms on every snare hit.
- **Depth** sets how far the level moves, and **Release** how quickly it recovers.

The SC button is lit purple when ducking and green when swelling.

## Vocals

A **vocal pad** records takes over the playing loop, lets you build one good performance out of them, and plays the result in time with everything else. Select an empty pad and click **Vocal** at the top right of the inspector, or choose **Make vocal pad** from the pad's right-click menu. Vocal pads are magenta, go to Bus V, and open the **Vocal** tab when selected.

### Recording

1. Choose your microphone under **MIC** and set its gain so the meter moves without lighting the clip light.
2. Press `Shift+R` (or **ARM**). The pad pulses while it waits.
3. Press `Space`. From the top of the loop, each time round becomes a new take, drawn in red as it comes in. Sing as many passes as you like.
4. Press `Space` again to stop. A pass you were part-way through is kept if you got a quarter of the way.

- **Loop / Free:** Loop makes a take per pass of the loop. Free records one continuous take for as long as the transport runs, while you launch loops or play the chain, and marks where each loop took over. A free take plays on across loop changes.
- **Punch:** drag a range on the ruler, turn Punch on, and only that range is recorded and put into the comp. Click the ruler to clear the range.
- **Mon:** hear yourself through Bus V while armed. Turn it off if you monitor directly from your audio interface.
- **Latency (ms):** recorded audio always arrives a little late. Takes are moved earlier by this amount. If your takes sit behind the beat, raise it until they sit on it; the setting is remembered for the machine.
- While a pad is armed its own comp is silent, so you sing against the beat and not against yourself.
- **Import…** brings an audio file in as a take.
- Up to three minutes of input are kept, which is the limit for one free take.

### Comping and editing

The top lane is the **comp**: what the pad plays. Under it are the takes, one colour each, and the comp is drawn in the colours of the takes it comes from. A new take becomes the whole comp until you start choosing by hand.

| Tool | Key | What a drag or click does |
| --- | --- | --- |
| Comp | `C` | Swipe across a take: that stretch of it goes into the comp |
| Move | `D` | Drag a clip in time, or onto another lane. `Alt`+drag slips the audio inside the clip without moving its edges, to tighten timing |
| Cut | `X` | Click a clip to cut it in two |

- Edges snap to the grid chosen beside the tools (`G` steps through Off, Bar, 1/4, 1/8, 1/16).
- Click a clip to select it: `M` mutes it, `Backspace` deletes it, `Ctrl+D` repeats it at the next grid line. Right-click a clip for the same actions and **Use this clip in the comp**.
- `Shift` + a pad key steps the comp through the takes for that sixteenth of the timeline, for comping by ear without the mouse.
- Every clip edge is faded over 2 ms, and where the comp changes take the two are crossfaded, so cuts do not click.
- Each lane has **M** (mute), **2x** (double) and **×** (remove), and a level.

**Doubles.** **2x** copies a lane to a **THRU** lane. A take lane is heard only where the comp selects it; a THRU lane always plays alongside the comp. Each has its own level, pan and pitch in cents, and new doubles come panned to alternate sides and detuned a few cents, which is the usual thickening trick. Set the pitch to 1200 or -1200 for an octave harmony. Click **TAKE** or **THRU** on a lane to change what it is. Pitch on a lane moves the formants with it; the Pitch Lock effect on the bus is the one with separate formant control.

### Bus V

The vocal bus starts with **Pitch Lock** (switched off until you want it), **Vocal Comp** and **Dub Throw**. While the Vocal tab is open the footer shows three more controls for it:

- **Gate:** silences the bus below a level, for room noise between phrases.
- **De-ess:** tames harsh S and T sounds.
- **Make room:** choose a bus, and while the vocal sounds that bus's midrange (300 Hz to 3.5 kHz) comes down by up to 6 dB. Its lows and highs are untouched, so a sample-heavy bus gets out of the way without the drums ducking.

Bus V can also key the ordinary sidechain of any other bus, and can be chosen as the Capture source.

### Baking

- **To pad** (`Ctrl+B`) flattens the comp and its doubles onto the next empty pad as an ordinary sample.
- **To bank** (`Ctrl+Shift+B`) cuts the comp at its silences and puts each phrase on its own pad of an empty bank, ready to play as chops.
- Tick **FX** to print Bus V's built-in effects into the result (hosted plugins are not printed).

The Waveform tab's destructive edits do not apply to a vocal pad, since its audio is rebuilt from the takes. Bake it to a pad first.

## VST3 plugins

- **Instruments:** press `F2`, or choose **VST3 instrument…** from a pad's right-click menu or the piano roll's Engine menu.
- **Effects:** choose **VST3 effect…** in a bus slot's menu.

The first time the browser opens, it scans the standard VST3 folders. Each plugin is examined by a separate short-lived process, several at a time, with a time limit, so a plugin that hangs or crashes during the scan cannot take the app down. Some plugins show a dialog while being examined; these close by themselves when the time limit is reached. Results are cached, so later scans only look at plugins that are new or changed.

A single plugin file can hold many plugins. Waves is the usual example: each WaveShell file holds the whole installed Waves collection, with separate mono and stereo versions of most plugins. Every plugin in a file is listed on its own, so search for the name and pick the version you want; on a bus, that is normally the Stereo one.

In the browser, type to search by name or vendor, press `Enter` to load the top match, or double-click any row. Plugins that could not be read are listed under **Failed** with the reason.

A loaded plugin opens its own editor window, follows the project tempo, and is saved with the project. Meters, gain-reduction displays and spectrum analysers in the editor follow the audio on the plugin's pad or bus.

## Undo, takes and projects

There are two kinds of going back.

- **Undo and redo** (`Ctrl+Z`, `Ctrl+Y`, or the buttons in the top bar) cover the project as a whole: pad settings, samples, patterns, banks, bus settings, tempo and key. A knob drag or a typed name counts as one step. Sixty-four steps are kept.
- **Takes** (`Ctrl+Shift+Z`, `Ctrl+Shift+Y`, or the arrows in the Waveform tab) belong to one pad. They step through that pad's baked versions without touching anything else.

Undo covers every loop and the chain. Moving between loops is not an edit and is not undone. Undo does not cover preferences, what is selected or shown, or the settings inside a plugin's own editor.

**Projects** are folders. `Ctrl+S` writes `project.json`, a WAV per pad into `samples/`, and plugin states into `plugins/`. `Ctrl+O` opens a project folder. A project stores each pad's active take, not its earlier ones.

## Preferences

`Ctrl+,` or **Prefs** in the top bar.

| Setting | Notes |
| --- | --- |
| Output device | System default, or a named device |
| Sample rate | Limited to what the chosen device supports |
| Buffer size | Smaller is more responsive and more demanding. Driver default is the safe choice |
| Skip-back length | 1 to 15 minutes, with the memory it will use |
| Scale | Size of the interface |
| VST3 folders | Folders to scan besides the standard ones |

Scale and plugin folders apply immediately. The audio settings and skip-back length apply the next time the app starts; **Save and restart** does that for you. Save your project first, because a restart discards unsaved work.

Preferences are kept in `%APPDATA%\ResampleOS\settings.json`, alongside the plugin cache.

## Troubleshooting

**No sound.** Check the status line in the footer at startup: it names the output device in use, or says why audio could not start. Check that the pad is not muted, that no other pad is soloed, and that its bus and the master are not muted.

**A bus went silent after loading a VST3 effect.** Some effects load but pass no audio in this version. Switch the slot off with its checkbox, or pick a different effect.

**The skip-back bar is not moving.** It is held for a selection. Click it, or press `Esc`.

**A plugin is missing from the browser.** Look under **Failed** for the reason. 32-bit plugins cannot be loaded. If it lives outside the standard folders, add its folder in Preferences and press **Rescan**.

**A dialog appeared during a plugin scan.** A plugin showed it while being examined. It closes on its own within 25 seconds, and that plugin is listed under Failed.

**Crackles or dropouts.** Raise the buffer size in Preferences. The **DSP** figure in the footer shows how much of the available time the engine is using.

**Tempo or key detection is wrong.** Correct it in the harmonizer before conforming. Detection is least reliable on short, sparse or freely timed material.
