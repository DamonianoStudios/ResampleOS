//! VST3 instances: loading, editor windows, and keeping them in step with the engine.

use super::*;

impl App {
    pub(super) fn rt_for(&self, slot: &PluginSlot) -> Box<PluginRt> {
        PluginRt::new(slot.handle.clone(), self.shared.sample_rate as f64)
    }

    /// Tells the engine which plugin (if any) pad `i` plays.
    pub fn push_plugin(&mut self, i: usize) {
        let plugin = self.pads[i].plugin.as_ref().map(|s| self.rt_for(s));
        self.send(Cmd::SetPadPlugin { pad: i as u16, plugin });
    }

    pub fn open_browser(&mut self, target: PluginTarget) {
        self.browser.open = true;
        self.browser.query.clear();
        self.browser.target = target;
        self.browser.filter = match target {
            PluginTarget::Pad(_) => 1,
            PluginTarget::Fx(..) => 2,
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
        }
    }

    /// Instantiates a plugin and puts it on a pad or in a bus slot, replacing what was there.
    pub fn load_plugin(&mut self, path: &std::path::Path, target: PluginTarget, state: Option<&[u8]>) -> bool {
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
                    return false;
                }
            }
        }
        let loaded = self.host.as_mut().unwrap().load_plugin(path).and_then(|mut p| {
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
                return false;
            }
        };
        let _ = plugin.set_tempo(self.bpm as f64);
        let name = plugin.info().name.clone();
        let slot = PluginSlot { path: path.to_path_buf(), name: name.clone(), handle: Arc::new(Mutex::new(plugin)) };
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
                self.buses[b].slots[s] = SlotCfg { kind: FxKind::Vst, params: FxKind::Vst.defaults(), on: true };
                self.push_bus(b);
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
        }
    }

    /// Opens the plugin's own editor in a native window (or raises nothing if it has none).
    pub fn open_editor(&mut self, target: PluginTarget) {
        let Some(slot) = self.plugin_slot(target) else { return };
        let (id, name, handle) = (slot.id(), slot.name.clone(), slot.handle.clone());
        if self.editors.iter().any(|(k, w)| *k == id && w.is_open()) {
            return;
        }
        self.editors.retain(|(k, _)| *k != id);
        let mut window = vst3_host::PluginWindow::new(handle);
        match window.open() {
            Ok(()) => self.editors.push((id, window)),
            Err(e) => self.status = format!("{name}: {e}"),
        }
    }

    /// Serialised plugin state for saving, if the plugin will give one.
    pub fn plugin_state(slot: &PluginSlot) -> Option<Vec<u8>> {
        slot.handle.lock().ok()?.save_state().ok()
    }

    pub(super) fn service_plugins(&mut self) {
        self.catalog.poll();
        if !self.editors.is_empty() {
            // An editor outliving its slot (pad cleared, bank removed) would keep the
            // plugin alive with nothing playing it.
            let live: Vec<usize> = self
                .pads
                .iter()
                .filter_map(|p| p.plugin.as_ref())
                .chain(self.fx_plugins.iter().flatten())
                .map(|s| s.id())
                .collect();
            self.editors.retain_mut(|(id, w)| {
                let _ = w.service_platform_events();
                let keep = live.contains(id) && !w.closed_by_user();
                if !keep {
                    w.close();
                }
                keep
            });
        }
        // Tempo-synced plugins follow the transport.
        let now = (self.bpm, self.playing());
        if now != self.synced_transport {
            self.synced_transport = now;
            for slot in self.pads.iter().filter_map(|p| p.plugin.as_ref()).chain(self.fx_plugins.iter().flatten()) {
                if let Ok(mut p) = slot.handle.lock() {
                    let _ = p.set_tempo(now.0 as f64);
                    let _ = p.set_playing(now.1);
                }
            }
        }
    }
}
