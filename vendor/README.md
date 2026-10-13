# Vendored crates

## vst3-host

`vst3-host/` is [vst3-host](https://github.com/HelgeSverre/rust-vst3-host) 0.10.0 (MIT, see `vst3-host/LICENSE`) as published on crates.io, with five changes. `[patch.crates-io]` in the top-level `Cargo.toml` makes the build use it. Each change is marked `ResampleOS patch` in the source.

1. **Component and controller messages are forwarded from any thread** (`src/internal/com_implementations.rs`, `ConnectionProxy::notify`). Upstream drops a message that does not arrive on the thread that loaded the plugin. FabFilter's editors, among others, poll their processor for meter and analyser data from a thread of their own, so with upstream's behaviour Pro-Q showed no spectrum and Pro-L no level or gain-reduction history.
2. **Processor output parameters reach the controller** (`src/internal/plugin_impl.rs`, `get_parameter_changes`). Points a processor writes to its output parameter queue are passed to `IEditController::setParamNormalized` when the host drains them on the control thread. Upstream only returns them to the host.
3. **The editor window is sized after the content scale is applied** (`src/window.rs`, `PluginWindow::open`, Windows). Upstream sizes the window from the editor's size at 100% scale. An editor that grows to match the display scale without asking for a resize was left cropped.
4. **`discovery::list_audio_classes`** (`src/discovery.rs`) lists every audio class a module exports without instantiating one. Upstream's `get_plugin_info` describes a single class per file, which hid all but one plugin of a shell such as Waves' WaveShell.
5. **Editor servicing delivers due timers** (`src/window.rs`, `PluginWindow::service_platform_events`, Windows). Windows only produces `WM_TIMER` when a thread's message queue is otherwise empty, and ResampleOS repaints continuously while audio plays or an editor is open, so editors that animate from a timer never got one. Waves' meters (the needle on CLA-2A, for instance) stayed frozen. Timer messages are now asked for explicitly, a bounded number per frame.

The copy leaves out upstream's changelog, lock file and helper script. To move to a newer release, replace `vst3-host/` with the new sources and reapply the five changes, or drop any that upstream has since made.
