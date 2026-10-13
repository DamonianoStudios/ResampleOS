//! User preferences, stored per machine rather than per project.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Output device by name; `None` follows the system default.
    pub output_device: Option<String>,
    /// `None` uses the device's own default rate.
    pub sample_rate: Option<u32>,
    /// Requested device buffer in frames; `None` lets the driver choose.
    pub buffer_frames: Option<u32>,
    /// Length of the skip-back buffer. Costs about 23 MB per minute at 48 kHz.
    pub skipback_minutes: u32,
    pub ui_scale: f32,
    /// Folders scanned for VST3 plugins in addition to the standard ones.
    pub vst3_dirs: Vec<PathBuf>,
    /// How late recorded input arrives, in milliseconds. Vocal takes are moved earlier by
    /// this much so they line up with what was heard while singing.
    pub input_latency_ms: f32,
    /// How the input's channels are treated: 0 = auto, 1 = stereo, 2 = mono.
    pub input_mode: u8,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            output_device: None,
            sample_rate: None,
            buffer_frames: None,
            skipback_minutes: 5,
            ui_scale: 1.0,
            vst3_dirs: Vec::new(),
            input_latency_ms: 0.0,
            input_mode: 0,
        }
    }
}

/// Where preferences and caches live: `%APPDATA%\ResampleOS` on Windows.
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(base).join("ResampleOS"))
}

impl Prefs {
    pub fn load() -> Self {
        let mut p: Prefs = config_dir()
            .and_then(|d| std::fs::read_to_string(d.join("settings.json")).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        p.skipback_minutes = p.skipback_minutes.clamp(1, 15);
        p.ui_scale = p.ui_scale.clamp(0.7, 1.6);
        p
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = config_dir().ok_or("no settings folder available")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("settings.json"), json).map_err(|e| e.to_string())
    }

    /// True when the two differ in something the audio engine only reads at startup.
    pub fn needs_restart(&self, running: &Prefs) -> bool {
        self.output_device != running.output_device
            || self.sample_rate != running.sample_rate
            || self.buffer_frames != running.buffer_frames
            || self.skipback_minutes != running.skipback_minutes
    }
}
