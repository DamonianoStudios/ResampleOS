//! A hosted plugin of either format, as the UI and the engine see it.
//!
//! Each instance sits behind a mutex shared by the UI (editor, state, parameters) and the
//! audio thread (processing). The format-specific halves are `vst3_host` and
//! [`crate::clap`]; this only routes between them.

use crate::clap::ClapPlugin;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub enum PluginHandle {
    Vst3(Arc<Mutex<vst3_host::Plugin>>),
    Clap(Arc<Mutex<ClapPlugin>>),
}

/// A parameter as the automation menus list it.
#[derive(Clone, Debug)]
pub struct Param {
    pub id: u32,
    pub name: String,
}

/// A move of a parameter in the plugin's own editor.
#[derive(Clone, Copy, Debug)]
pub enum Edit {
    /// Normalized value.
    Value(u32, f64),
    /// The gesture on this parameter is over.
    End(u32),
}

impl PluginHandle {
    /// Identity of the instance.
    pub fn id(&self) -> usize {
        match self {
            PluginHandle::Vst3(h) => Arc::as_ptr(h) as usize,
            PluginHandle::Clap(h) => Arc::as_ptr(h) as usize,
        }
    }

    pub fn save_state(&self) -> Option<Vec<u8>> {
        match self {
            PluginHandle::Vst3(h) => h.lock().ok()?.save_state().ok(),
            PluginHandle::Clap(h) => h.lock().ok()?.save_state(),
        }
    }

    pub fn set_transport(&self, bpm: f64, playing: bool) {
        match self {
            PluginHandle::Vst3(h) => {
                if let Ok(mut p) = h.lock() {
                    let _ = p.set_tempo(bpm);
                    let _ = p.set_playing(playing);
                }
            }
            PluginHandle::Clap(h) => {
                if let Ok(mut p) = h.lock() {
                    p.set_transport(bpm, playing);
                }
            }
        }
    }

    /// The parameters a lane can move: automatable, writable and not hidden.
    pub fn parameters(&self) -> Vec<Param> {
        match self {
            PluginHandle::Vst3(h) => h
                .lock()
                .ok()
                .and_then(|p| p.get_parameters().ok())
                .unwrap_or_default()
                .into_iter()
                .filter(|p| p.can_automate && !p.is_read_only && !p.is_bypass)
                .map(|p| Param { id: p.id, name: p.name })
                .collect(),
            PluginHandle::Clap(h) => h
                .lock()
                .map(|p| p.parameters())
                .unwrap_or_default()
                .into_iter()
                .filter(|p| p.automatable)
                .map(|p| Param { id: p.id, name: p.name })
                .collect(),
        }
    }

    /// A parameter's normalized value. Gives up rather than wait if the audio thread has
    /// the plugin.
    pub fn parameter(&self, id: u32) -> Option<f64> {
        match self {
            PluginHandle::Vst3(h) => h.try_lock().ok()?.get_parameter(id).ok(),
            PluginHandle::Clap(h) => h.try_lock().ok()?.get_parameter(id),
        }
    }

    /// A normalized value as the plugin writes it, if the plugin is free to ask.
    pub fn format(&self, id: u32, normalized: f64) -> Option<String> {
        match self {
            PluginHandle::Vst3(h) => h.try_lock().ok()?.format_parameter(id, normalized).ok(),
            PluginHandle::Clap(h) => h.try_lock().ok()?.format_parameter(id, normalized),
        }
    }

    /// Main-thread upkeep, once a frame: CLAP plugins' callbacks and requests.
    pub fn service(&self) {
        if let PluginHandle::Clap(h) = self
            && let Ok(mut p) = h.try_lock()
        {
            p.service();
        }
    }
}
