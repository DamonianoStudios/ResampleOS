# Contributing to ResampleOS

Thanks for taking an interest. This page covers building, testing, and the handful of rules the code depends on.

## Building

You need [Rust](https://rustup.rs) 1.88 or newer. On Windows, install the MSVC build tools; the Windows SDK's resource compiler is used to embed the application icon, and the build carries on without the icon if it is missing.

```bash
cargo run            # debug build; dependencies are still optimised, so audio is usable
cargo run --release  # what you would ship
cargo test --release
cargo clippy --release
```

Only Windows has been tested. Reports and fixes for macOS and Linux are welcome; expect the plugin editor windows, the icon embedding in `build.rs`, and the font loading in `src/ui/theme.rs` to need attention.

## Before you change the engine

Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) first. Everything in `src/engine/` runs on the audio thread, and three rules hold there:

1. **Do not allocate.** No `Vec::push` past capacity, no `Box::new`, no `String`, no `format!`. Buffers are sized when the engine is built.
2. **Do not free.** Dropping the last `Arc` to a sample frees it. Send it back on the garbage queue with `trash` instead.
3. **Do not block.** No `Mutex::lock`, no channel `recv`, no file or console I/O. Hosted plugins are reached with `try_lock` only.

A change that needs new state from the UI should add a variant to `Cmd` rather than share memory.

Engine behaviour is tested without a device in `src/engine/tests.rs`. A new engine feature should come with a test there; the existing ones show the pattern: build a rig, push commands, render, and assert on the samples.

## Before you change the UI

- The UI edits the model in `src/app/` and sends commands. It does not reach into the engine.
- Add actions as methods on `App` in the relevant `src/app/` file, and call them from the UI. Destructive pad edits go through `PadAction`.
- You do not need to do anything for undo. Any change to the project state is picked up automatically (see `src/app/history.rs`). If you add a new field that undo should cover, add it to `Snapshot` and to `PadModel::same_as` if it lives on a pad.
- Use the colours and helpers in `src/ui/theme.rs` and `src/ui/widgets.rs` rather than hard-coding new ones.
- Check any text you add: the UI font is the system font, with egui's built-in font as fallback, and symbols outside common ranges may not render.

## Style

- Run `cargo clippy --release` and keep it free of new warnings. Indexed loops in DSP code are left as they are where they read better than iterator chains.
- Comments say why, not what. Leave out comments that restate the code.
- Match the file you are in.

## The logo and icon

The logo files in `assets/` are generated. To change the mark, edit `assets/make_logo.py` and run it (it needs Pillow):

```bash
python assets/make_logo.py
```

## Reporting bugs

Please include what you did, what you expected, what happened, your audio device and sample rate (shown in Preferences), and, for a plugin problem, the plugin's name and vendor.
