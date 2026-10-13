//! Project folders: `project.json` plus one WAV per pad holding its active take.

use crate::app::{App, ConformMode, MAX_CHAIN, MAX_SCENES, Scene};
use crate::engine::{BusCfg, Cmd, PadCfg, Pattern};
use crate::model::PadModel;
use crate::music::{Key, pad_label};
use crate::sample;
use crate::shared::{MAX_BANKS, NUM_BUSES, VOCAL};
use crate::vocal::{Clip, CompSeg, Lane, LaneKind, TakeAudio, VocalStack};
use serde::{Deserialize, Serialize};
use crate::app::PluginTarget;
use crate::model::PluginSlot;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Serialize, Deserialize, Clone)]
struct PluginFile {
    path: PathBuf,
    #[serde(default)]
    class_id: Option<String>,
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
    PluginFile { path: slot.path.clone(), class_id: slot.class_id.clone(), name: slot.name.clone(), state }
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
    /// Takes and comp of a vocal pad. `sample` is its render, so the pad still plays if
    /// the takes cannot be read.
    #[serde(default)]
    vocal: Option<StackFile>,
}

#[derive(Serialize, Deserialize)]
struct ClipFile {
    /// Index into the stack's `audio` list.
    audio: usize,
    offset: i64,
    start: i64,
    len: i64,
    muted: bool,
}

#[derive(Serialize, Deserialize)]
struct LaneFile {
    name: String,
    kind: LaneKind,
    gain: f32,
    pan: f32,
    semis: f32,
    mute: bool,
    clips: Vec<ClipFile>,
}

#[derive(Serialize, Deserialize)]
struct StackFile {
    /// Files under `vocals/`, one per recorded pass.
    audio: Vec<String>,
    lanes: Vec<LaneFile>,
    comp: Vec<CompSeg>,
    len: i64,
    hand_comped: bool,
    markers: Vec<(i64, u32)>,
    takes_made: u32,
}

/// Writes a stack's passes as WAV files and returns its description.
fn save_stack(dir: &Path, label: &str, stack: &VocalStack, rate: u32) -> Result<StackFile, String> {
    let folder = dir.join("vocals");
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let mut passes: Vec<Arc<TakeAudio>> = Vec::new();
    let mut lanes = Vec::new();
    for lane in &stack.lanes {
        let mut clips = Vec::new();
        for c in &lane.clips {
            // Clips cut from the same pass share its file.
            let audio = match passes.iter().position(|p| Arc::ptr_eq(p, &c.audio)) {
                Some(i) => i,
                None => {
                    passes.push(c.audio.clone());
                    passes.len() - 1
                }
            };
            clips.push(ClipFile { audio, offset: c.offset, start: c.start, len: c.len, muted: c.muted });
        }
        lanes.push(LaneFile { name: lane.name.clone(), kind: lane.kind, gain: lane.gain, pan: lane.pan, semis: lane.semis, mute: lane.mute, clips });
    }
    let mut audio = Vec::new();
    for (i, pass) in passes.iter().enumerate() {
        let file = format!("{label}_{}.wav", i + 1);
        sample::save_wav(&folder.join(&file), &pass.data, rate)?;
        audio.push(file);
    }
    Ok(StackFile { audio, lanes, comp: stack.comp.clone(), len: stack.len, hand_comped: stack.hand_comped, markers: stack.markers.clone(), takes_made: stack.takes_made })
}

fn load_stack(dir: &Path, file: &StackFile, rate: u32) -> Result<VocalStack, String> {
    let mut passes = Vec::new();
    for name in &file.audio {
        passes.push(TakeAudio::new(sample::load_file(&dir.join("vocals").join(name), rate)?));
    }
    let lanes = file
        .lanes
        .iter()
        .map(|l| Lane {
            name: l.name.clone(),
            kind: l.kind,
            gain: l.gain,
            pan: l.pan,
            semis: l.semis,
            mute: l.mute,
            clips: l
                .clips
                .iter()
                .filter_map(|c| Some(Clip { audio: passes.get(c.audio)?.clone(), offset: c.offset, start: c.start, len: c.len, muted: c.muted }))
                .collect(),
        })
        .collect();
    Ok(VocalStack { lanes, comp: file.comp.clone(), len: file.len, hand_comped: file.hand_comped, markers: file.markers.clone(), takes_made: file.takes_made })
}

/// Version 2 added the vocal bus between Bus 4 and the master.
const FILE_VERSION: u32 = 2;

/// The bus settings of a project file, in today's layout.
fn buses_of(file: &ProjectFile) -> [BusCfg; NUM_BUSES] {
    let mut buses = app_default_buses();
    let old = file.version < 2;
    for (i, cfg) in file.buses.iter().enumerate() {
        if let Some(slot) = buses.get_mut(bus_slot(i, old)) {
            *slot = *cfg;
        }
    }
    buses
}

/// Where bus `index` of a file lives now: in old files the master came fifth.
fn bus_slot(index: usize, old: bool) -> usize {
    if old && index >= VOCAL { index + 1 } else { index }
}

fn app_default_buses() -> [BusCfg; NUM_BUSES] {
    let mut buses = [BusCfg::default(); NUM_BUSES];
    buses[VOCAL] = BusCfg::vocal();
    buses
}

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    version: u32,
    bpm: f32,
    swing: f32,
    key: Key,
    banks: usize,
    buses: Vec<BusCfg>,
    conform_mode: ConformMode,
    auto_conform: bool,
    preserve_transients: bool,
    pads: Vec<PadFile>,
    #[serde(default)]
    fx_plugins: Vec<FxPluginFile>,
    /// Loops. The `pattern` stored with each pad above is the one for loop `scene`.
    /// Projects from before loops existed have neither field and open as a single loop.
    #[serde(default)]
    scenes: Vec<Scene>,
    #[serde(default)]
    scene: usize,
    #[serde(default)]
    chain: Vec<usize>,
    #[serde(default)]
    chain_on: bool,
    #[serde(default)]
    vocal_fx: crate::engine::VocalFx,
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
            vocal: match &p.vocal {
                Some(stack) if !stack.is_empty() => Some(save_stack(dir, &pad_label(index), stack, app.shared.sample_rate)?),
                _ => None,
            },
        });
    }
    let count = pads.len();
    let file = ProjectFile {
        version: FILE_VERSION,
        bpm: app.bpm,
        swing: app.swing,
        key: app.key,
        banks: app.banks,
        buses: app.buses.to_vec(),
        conform_mode: app.conform_mode,
        auto_conform: app.auto_conform,
        preserve_transients: app.preserve_transients,
        scenes: app.scenes_complete(),
        scene: app.scene,
        chain: app.chain.clone(),
        chain_on: app.chain_on,
        vocal_fx: app.vocal_fx,
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
        if pf.cfg.kind == crate::engine::PadKind::Vocal {
            p.vocal = Some(match &pf.vocal {
                Some(stack) => load_stack(dir, stack, rate)?,
                None => VocalStack::default(),
            });
        }
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
    app.send(Cmd::QueueScene(None));
    app.queued_scene = None;
    app.chain_pos = 0;
    app.scenes = file.scenes.iter().take(MAX_SCENES).cloned().collect();
    if app.scenes.is_empty() {
        // One loop, long enough for the longest pattern in the file.
        let ticks = file.pads.iter().filter(|p| !p.pattern.notes.is_empty()).map(|p| p.pattern.len_ticks).max();
        app.scenes.push(Scene::new(ticks.map_or(2, |t| t.div_ceil(crate::music::TICKS_PER_BAR))));
    }
    for scene in app.scenes.iter_mut() {
        scene.bars = scene.bars.clamp(1, 16);
    }
    app.scene = file.scene.min(app.scenes.len() - 1);
    app.chain = file.chain.iter().copied().filter(|s| *s < app.scenes.len()).take(MAX_CHAIN).collect();
    app.chain_on = file.chain_on && !app.chain.is_empty();
    app.shared.scene_index.store(app.scene as u32, std::sync::atomic::Ordering::Relaxed);
    // Unload the outgoing project's effect plugins before its bus settings are replaced.
    for b in 0..NUM_BUSES {
        for s in 0..3 {
            app.remove_plugin(PluginTarget::Fx(b, s));
        }
    }
    // The saved render of a vocal pad may predate its last edit; rebuild it from the takes.
    for i in 0..app.pads.len() {
        if app.pads[i].vocal.as_ref().is_some_and(|v| !v.is_empty()) {
            app.vocal_refresh(i);
        }
    }
    app.buses = buses_of(&file);
    app.vocal_fx = file.vocal_fx;
    app.vocal_disarm();
    app.vocal.selected = None;
    app.push_all();
    app.push_scene_len();

    // Plugins last: instantiating them is the slow, fallible part.
    let read_state = |pf: &PluginFile| pf.state.as_ref().and_then(|f| std::fs::read(dir.join("plugins").join(f)).ok());
    let mut missing = Vec::new();
    for pf in &file.pads {
        if let Some(pl) = &pf.plugin {
            if pf.index < app.pads.len() && !app.load_plugin(&pl.path, pl.class_id.as_deref(), PluginTarget::Pad(pf.index), read_state(pl).as_deref()) {
                missing.push(pl.name.clone());
            } else if pf.index < app.pads.len() {
                // load_plugin renames the pad after the plugin; keep the saved name.
                app.pads[pf.index].name = pf.name.clone();
            }
        }
    }
    for fx in &file.fx_plugins {
        let saved = file.buses.get(fx.bus).and_then(|b| b.slots.get(fx.slot)).copied();
        let bus = bus_slot(fx.bus, file.version < 2);
        if !app.load_plugin(&fx.plugin.path, fx.plugin.class_id.as_deref(), PluginTarget::Fx(bus, fx.slot), read_state(&fx.plugin).as_deref()) {
            missing.push(fx.plugin.name.clone());
        } else if let Some(saved) = saved {
            app.buses[bus].slots[fx.slot] = saved;
            app.push_bus(bus);
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
