//! Project folders: `project.json` plus one WAV per pad holding its active take.

use crate::app::{App, ConformMode};
use crate::engine::{BusCfg, Cmd, PadCfg, Pattern};
use crate::model::PadModel;
use crate::music::{Key, pad_label};
use crate::sample;
use crate::shared::{MAX_BANKS, NUM_BUSES};
use serde::{Deserialize, Serialize};
use crate::app::PluginTarget;
use crate::model::PluginSlot;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Serialize, Deserialize, Clone)]
struct PluginFile {
    path: PathBuf,
    name: String,
    /// File under `plugins/` holding the plugin's own state blob.
    state: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct FxPluginFile {
    bus: usize,
    slot: usize,
    plugin: PluginFile,
}

fn save_plugin(dir: &Path, label: &str, slot: &PluginSlot) -> PluginFile {
    let state = App::plugin_state(slot).and_then(|bytes| {
        let file = format!("{label}.vstate");
        let folder = dir.join("plugins");
        std::fs::create_dir_all(&folder).ok()?;
        std::fs::write(folder.join(&file), bytes).ok()?;
        Some(file)
    });
    PluginFile { path: slot.path.clone(), name: slot.name.clone(), state }
}

#[derive(Serialize, Deserialize)]
struct PadFile {
    index: usize,
    name: String,
    cfg: PadCfg,
    pattern: Pattern,
    transients: Vec<u32>,
    sample: Option<String>,
    #[serde(default)]
    plugin: Option<PluginFile>,
}

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    version: u32,
    bpm: f32,
    swing: f32,
    key: Key,
    banks: usize,
    buses: [BusCfg; NUM_BUSES],
    conform_mode: ConformMode,
    auto_conform: bool,
    preserve_transients: bool,
    pads: Vec<PadFile>,
    #[serde(default)]
    fx_plugins: Vec<FxPluginFile>,
}

pub fn save(app: &App, dir: &Path) -> Result<usize, String> {
    let samples = dir.join("samples");
    std::fs::create_dir_all(&samples).map_err(|e| e.to_string())?;
    let mut pads = Vec::new();
    for (index, p) in app.pads.iter().enumerate() {
        if p.is_empty() && p.cfg == PadCfg::default() && p.pattern.notes.is_empty() {
            continue;
        }
        let sample = match p.sample() {
            Some(s) => {
                let file = format!("{}.wav", pad_label(index));
                sample::save_wav(&samples.join(&file), s, app.shared.sample_rate)?;
                Some(file)
            }
            None => None,
        };
        pads.push(PadFile {
            index,
            name: p.name.clone(),
            cfg: p.cfg,
            pattern: p.pattern.clone(),
            transients: p.transients.clone(),
            sample,
            plugin: p.plugin.as_ref().map(|s| save_plugin(dir, &pad_label(index), s)),
        });
    }
    let count = pads.len();
    let file = ProjectFile {
        version: 1,
        bpm: app.bpm,
        swing: app.swing,
        key: app.key,
        banks: app.banks,
        buses: app.buses,
        conform_mode: app.conform_mode,
        auto_conform: app.auto_conform,
        preserve_transients: app.preserve_transients,
        pads,
        fx_plugins: app
            .fx_plugins
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let s = s.as_ref()?;
                Some(FxPluginFile { bus: i / 3, slot: i % 3, plugin: save_plugin(dir, &format!("bus{}_{}", i / 3 + 1, i % 3 + 1), s) })
            })
            .collect(),
    };
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("project.json"), json).map_err(|e| e.to_string())?;
    Ok(count)
}

pub fn load(app: &mut App, dir: &Path) -> Result<usize, String> {
    let json = std::fs::read_to_string(dir.join("project.json")).map_err(|e| e.to_string())?;
    let file: ProjectFile = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    let rate = app.shared.sample_rate;

    // Decode everything first so a broken file leaves the current project untouched.
    let banks = file.banks.clamp(1, MAX_BANKS);
    let mut pads: Vec<PadModel> = (0..banks * 16).map(|_| PadModel::default()).collect();
    for pf in &file.pads {
        if pf.index >= pads.len() {
            continue;
        }
        let p = &mut pads[pf.index];
        p.name = pf.name.clone();
        p.pattern = pf.pattern.clone();
        if let Some(name) = &pf.sample {
            let s = sample::load_file(&dir.join("samples").join(name), rate)?;
            p.push_take(Arc::new(s));
        }
        // After push_take, which resets the play region.
        p.cfg = pf.cfg;
        p.transients = pf.transients.clone();
    }

    app.send(Cmd::Play(false));
    app.send(Cmd::Panic);
    for i in 0..app.pads.len() {
        if app.pads[i].is_empty() && app.pads[i].cfg == PadCfg::default() {
            continue;
        }
        app.reset_engine_pad(i);
    }
    app.pads = pads;
    app.banks = banks;
    app.bpm = file.bpm;
    app.swing = file.swing;
    app.key = file.key;
    app.conform_mode = file.conform_mode;
    app.auto_conform = file.auto_conform;
    app.preserve_transients = file.preserve_transients;
    app.bank = 0;
    app.selected = 0;
    // Unload the outgoing project's effect plugins before its bus settings are replaced.
    for b in 0..NUM_BUSES {
        for s in 0..3 {
            app.remove_plugin(PluginTarget::Fx(b, s));
        }
    }
    app.buses = file.buses;
    app.push_all();

    // Plugins last: instantiating them is the slow, fallible part.
    let read_state = |pf: &PluginFile| pf.state.as_ref().and_then(|f| std::fs::read(dir.join("plugins").join(f)).ok());
    let mut missing = Vec::new();
    for pf in &file.pads {
        if let Some(pl) = &pf.plugin {
            if pf.index < app.pads.len() && !app.load_plugin(&pl.path, PluginTarget::Pad(pf.index), read_state(pl).as_deref()) {
                missing.push(pl.name.clone());
            } else if pf.index < app.pads.len() {
                // load_plugin renames the pad after the plugin; keep the saved name.
                app.pads[pf.index].name = pf.name.clone();
            }
        }
    }
    for fx in &file.fx_plugins {
        let saved = file.buses.get(fx.bus).and_then(|b| b.slots.get(fx.slot)).copied();
        if !app.load_plugin(&fx.plugin.path, PluginTarget::Fx(fx.bus, fx.slot), read_state(&fx.plugin).as_deref()) {
            missing.push(fx.plugin.name.clone());
        } else if let Some(saved) = saved {
            app.buses[fx.bus].slots[fx.slot] = saved;
            app.push_bus(fx.bus);
        }
    }
    // A different project is a fresh start for undo.
    app.reset_history();
    if !missing.is_empty() {
        return Err(format!("project opened, but these plugins could not be loaded: {}", missing.join(", ")));
    }
    Ok(file.pads.len())
}

pub fn save_dialog(app: &mut App) {
    let dir = match app.project_dir.clone() {
        Some(d) => Some(d),
        None => rfd::FileDialog::new().set_title("Choose a folder for this project").pick_folder(),
    };
    let Some(dir) = dir else { return };
    app.status = match save(app, &dir) {
        Ok(n) => format!("Saved {n} pads to {}", dir.display()),
        Err(e) => format!("Save failed: {e}"),
    };
    app.project_dir = Some(dir);
}

pub fn open_dialog(app: &mut App) {
    let Some(dir) = rfd::FileDialog::new().set_title("Open project folder").pick_folder() else { return };
    app.status = match load(app, &dir) {
        Ok(n) => format!("Opened {} ({n} pads)", dir.display()),
        Err(e) => format!("Open failed: {e}"),
    };
    if app.status.starts_with("Opened") {
        app.project_dir = Some(dir);
    }
}
