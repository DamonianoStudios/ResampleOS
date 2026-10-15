//! Plugin instances (VST3 and CLAP): loading, editor windows, and keeping them in step
//! with the engine.

use super::*;
use crate::hosted::{Edit, PluginHandle};

enum Window {
    Vst3(vst3_host::PluginWindow),
    #[cfg(windows)]
    Clap(crate::clap::ClapEditor),
}

/// An open plugin editor window.
pub(super) struct EditorWin {
    id: usize,
    window: Window,
    handle: PluginHandle,
}

impl EditorWin {
    /// Lets the plugin hand what its processor reported since the last frame to its editor
    /// (output parameter points such as meters, and data-exchange blocks, are written on
    /// the audio thread and have to be delivered on this one), and collects the moves the
    /// user made in the editor.
    fn feed(&mut self, edits: &mut Vec<(usize, Edit)>) {
        match &self.handle {
            PluginHandle::Vst3(h) => {
                // Never wait here: the audio thread holds this lock for most of every block.
                let Ok(mut p) = h.try_lock() else { return };
                let _ = p.get_parameter_changes();
                let _ = p.take_data_exchange_blocks();
                for e in p.take_parameter_edits() {
                    let edit = match (e.kind, e.value) {
                        (vst3_host::ParameterEditKind::EndGesture, _) => Edit::End(e.id),
                        (_, Some(v)) => Edit::Value(e.id, v),
                        _ => continue,
                    };
                    edits.push((self.id, edit));
                }
                let _ = p.take_host_notifications();
            }
            PluginHandle::Clap(h) => {
                if let Ok(mut p) = h.try_lock() {
                    edits.extend(p.take_edits().into_iter().map(|e| (self.id, e)));
                }
            }
        }
    }

    fn is_open(&self) -> bool {
        match &self.window {
            Window::Vst3(w) => w.is_open(),
            #[cfg(windows)]
            Window::Clap(w) => w.is_open(),
        }
    }

    fn closed_by_user(&mut self) -> bool {
        match &mut self.window {
            Window::Vst3(w) => {
                let _ = w.service_platform_events();
                w.closed_by_user()
            }
            #[cfg(windows)]
            Window::Clap(w) => {
                w.service();
                w.closed_by_user()
            }
        }
    }

    fn close(&mut self) {
        match &mut self.window {
            Window::Vst3(w) => w.close(),
            #[cfg(windows)]
            Window::Clap(w) => w.close(),
        }
    }
}

impl App {
    pub(super) fn rt_for(&self, slot: &PluginSlot) -> Box<PluginRt> {
        PluginRt::new(slot.handle.clone(), self.shared.sample_rate as f64)
    }

    /// Tells the engine which plugin (if any) pad `i` plays.
    /// Gives the engine pad `i`'s instrument plugin (none while its track is frozen).
    pub fn push_plugin(&mut self, i: usize) {
        let plugin = self.pads[i].plugin.as_ref().filter(|_| !self.arrangement.is_frozen(i)).map(|s| self.rt_for(s));
        self.send(Cmd::SetPadPlugin { pad: i as u16, plugin });
    }

    pub fn open_browser(&mut self, target: PluginTarget) {
        self.browser.open = true;
        self.browser.query.clear();
        self.browser.target = target;
        self.browser.filter = match target {
            PluginTarget::Pad(_) => 1,
            PluginTarget::Fx(..) | PluginTarget::Analyzer(_) => 2,
        };
        // First visit on this machine: build the catalog in the background.
        if !self.catalog.scanned_once && !self.catalog.scanning {
            self.catalog.start_scan(false, &self.prefs.vst3_dirs);
        }
    }

    pub fn plugin_slot(&self, target: PluginTarget) -> Option<&PluginSlot> {
        match target {
            PluginTarget::Pad(i) => self.pads.get(i)?.plugin.as_ref(),
            PluginTarget::Fx(b, s) => self.fx_plugins.get(b * 3 + s)?.as_ref(),
            PluginTarget::Analyzer(k) => self.analyzers.get(k)?.0.as_ref(),
        }
    }

    /// Every loaded plugin instance: instruments, bus effects and analyzers.
    pub(super) fn all_plugins(&self) -> impl Iterator<Item = &PluginSlot> {
        self.pads
            .iter()
            .filter_map(|p| p.plugin.as_ref())
            .chain(self.fx_plugins.iter().flatten())
            .chain(self.analyzers.iter().filter_map(|a| a.0.as_ref()))
    }

    /// Tells the engine which plugin analyzer slot `k` holds and what it watches.
    pub(super) fn push_analyzer(&mut self, k: usize) {
        let (slot, source) = &self.analyzers[k];
        let plugin = slot.as_ref().map(|s| self.rt_for(s));
        let source = *source as u8;
        self.send(Cmd::SetAnalyzer { slot: k as u8, plugin, source });
    }

    /// Points analyzer slot `k` at another bus.
    pub fn set_analyzer_source(&mut self, k: usize, bus: usize) {
        if self.analyzers[k].1 != bus {
            self.analyzers[k].1 = bus;
            self.push_analyzer(k);
        }
    }

    /// Instantiates a plugin and puts it on a pad or in a bus slot, replacing what was there.
    /// `class_id` picks one plugin out of a file that holds several; `None` takes the first.
    pub fn load_plugin(
        &mut self,
        path: &std::path::Path,
        class_id: Option<&str>,
        target: PluginTarget,
        state: Option<&[u8]>,
    ) -> bool {
        let rate = self.shared.sample_rate as f64;
        let (handle, name) = if crate::clap::is_clap(path) {
            match crate::clap::ClapPlugin::load(path, class_id, rate, crate::engine::MAX_BLOCK) {
                Ok(mut p) => {
                    if let Some(bytes) = state {
                        // A stale or foreign state is not fatal; the plugin keeps its defaults.
                        let _ = p.load_state(bytes);
                    }
                    p.set_transport(self.bpm as f64, self.playing());
                    let name = p.name.clone();
                    (PluginHandle::Clap(Arc::new(Mutex::new(p))), name)
                }
                Err(e) => {
                    self.status = format!("Could not load {}: {e}", path.display());
                    return false;
                }
            }
        } else {
            match self.load_vst3(path, class_id, state) {
                Some(loaded) => loaded,
                None => return false,
            }
        };
        let params = Arc::new(handle.parameters());
        let slot = PluginSlot { path: path.to_path_buf(), class_id: class_id.map(str::to_string), name, handle, params };
        self.place_plugin(slot, target)
    }

    fn load_vst3(&mut self, path: &std::path::Path, class_id: Option<&str>, state: Option<&[u8]>) -> Option<(PluginHandle, String)> {
        let rate = self.shared.sample_rate as f64;
        if self.host.is_none() {
            let built = vst3_host::Vst3Host::builder()
                .sample_rate(rate)
                .block_size(crate::engine::MAX_BLOCK)
                .tempo(self.bpm as f64)
                .build();
            match built {
                Ok(h) => self.host = Some(h),
                Err(e) => {
                    self.status = format!("VST3 host failed to start: {e}");
                    return None;
                }
            }
        }
        let host = self.host.as_mut().unwrap();
        let loaded = match class_id {
            Some(id) => host.load_plugin_class(path, id),
            None => host.load_plugin(path),
        };
        let loaded = loaded.and_then(|mut p| {
            if let Some(bytes) = state {
                // A stale or foreign state is not fatal; the plugin just keeps its defaults.
                let _ = p.load_state(bytes);
            }
            p.start_processing()?;
            Ok(p)
        });
        let mut plugin = match loaded {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("Could not load {}: {e}", path.display());
                return None;
            }
        };
        let _ = plugin.set_tempo(self.bpm as f64);
        let name = plugin.info().name.clone();
        Some((PluginHandle::Vst3(Arc::new(Mutex::new(plugin))), name))
    }

    /// Puts a loaded plugin on a pad or in a bus slot, replacing what was there.
    fn place_plugin(&mut self, slot: PluginSlot, target: PluginTarget) -> bool {
        let name = slot.name.clone();
        match target {
            PluginTarget::Pad(i) if i < self.pads.len() => {
                let p = &mut self.pads[i];
                p.cfg.kind = PadKind::Midi;
                p.cfg.target = MidiTarget::Plugin;
                p.name = name.clone();
                p.plugin = Some(slot);
                self.resync_pad(i);
            }
            PluginTarget::Fx(b, s) if b < NUM_BUSES && s < 3 => {
                let rt = self.rt_for(&slot);
                self.fx_plugins[b * 3 + s] = Some(slot);
                self.send(Cmd::SetFxPlugin { slot: (b * 3 + s) as u8, plugin: Some(rt) });
                self.buses[b].slots[s] = SlotCfg::new(FxKind::Vst, true);
                self.push_bus(b);
            }
            PluginTarget::Analyzer(k) if k < self.analyzers.len() => {
                self.analyzers[k].0 = Some(slot);
                self.push_analyzer(k);
            }
            _ => return false,
        }
        self.status = format!("Loaded {name}");
        true
    }

    pub fn remove_plugin(&mut self, target: PluginTarget) {
        match target {
            PluginTarget::Pad(i) => {
                if self.pads[i].plugin.take().is_some() {
                    self.pads[i].cfg.target = MidiTarget::Synth;
                    self.resync_pad(i);
                }
            }
            PluginTarget::Fx(b, s) => {
                if self.fx_plugins[b * 3 + s].take().is_some() {
                    self.send(Cmd::SetFxPlugin { slot: (b * 3 + s) as u8, plugin: None });
                }
                if self.buses[b].slots[s].kind == FxKind::Vst {
                    self.buses[b].slots[s].kind = FxKind::None;
                    self.push_bus(b);
                }
            }
            PluginTarget::Analyzer(k) => {
                if self.analyzers[k].0.take().is_some() {
                    self.push_analyzer(k);
                }
            }
        }
    }

    /// Opens the plugin's own editor in a native window (or raises nothing if it has none).
    pub fn open_editor(&mut self, target: PluginTarget) {
        let Some(slot) = self.plugin_slot(target) else { return };
        let (id, name, handle) = (slot.id(), slot.name.clone(), slot.handle.clone());
        if self.editors.iter().any(|e| e.id == id && e.is_open()) {
            return;
        }
        self.editors.retain(|e| e.id != id);
        let window = match &handle {
            PluginHandle::Vst3(h) => {
                let mut window = vst3_host::PluginWindow::new(h.clone());
                window.open().map(|()| Window::Vst3(window)).map_err(|e| e.to_string())
            }
            #[cfg(windows)]
            PluginHandle::Clap(h) => crate::clap::ClapEditor::open(h.clone(), &name).map(Window::Clap),
            #[cfg(not(windows))]
            PluginHandle::Clap(_) => Err("CLAP editors are only shown on Windows so far".to_string()),
        };
        match window {
            Ok(window) => self.editors.push(EditorWin { id, window, handle }),
            Err(e) => self.status = format!("{name}: {e}"),
        }
    }

    /// Serialised plugin state for saving, if the plugin will give one.
    pub fn plugin_state(slot: &PluginSlot) -> Option<Vec<u8>> {
        slot.handle.save_state()
    }

    /// The instruments and bus effects, with where each is: the plugins whose parameters
    /// automation can move.
    pub fn automatable_plugins(&self) -> Vec<(PluginTarget, &PluginSlot)> {
        let pads = self.pads.iter().enumerate().filter_map(|(i, p)| Some((PluginTarget::Pad(i), p.plugin.as_ref()?)));
        let fx = self.fx_plugins.iter().enumerate().filter_map(|(i, s)| Some((PluginTarget::Fx(i / 3, i % 3), s.as_ref()?)));
        pads.chain(fx).collect()
    }

    pub(super) fn service_plugins(&mut self) {
        self.catalog.poll();
        for slot in self.all_plugins() {
            slot.handle.service();
        }
        let mut edits = Vec::new();
        if !self.editors.is_empty() {
            // An editor outliving its slot (pad cleared, bank removed) would keep the
            // plugin alive with nothing playing it.
            let live: Vec<usize> = self.all_plugins().map(|s| s.id()).collect();
            self.editors.retain_mut(|e| {
                let keep = live.contains(&e.id) && !e.closed_by_user();
                if keep {
                    e.feed(&mut edits);
                } else {
                    e.close();
                }
                keep
            });
        }
        self.plugin_edits(edits);
        // Tempo-synced plugins follow the transport.
        let now = (self.bpm, self.playing());
        if now != self.synced_transport {
            self.synced_transport = now;
            for slot in self.all_plugins() {
                slot.handle.set_transport(now.0 as f64, now.1);
            }
        }
    }
}
