//! UI-side project model. The engine holds its own copy of everything it needs to play;
//! this is the editable source of truth that gets pushed to it.

use crate::analysis::{self, Analysis};
use crate::engine::{MidiTarget, PadCfg, PadKind, Pattern};
use crate::sample::SampleData;
use std::path::PathBuf;
use crate::hosted::{Param, PluginHandle};
use std::sync::Arc;

pub const MAX_TAKES: usize = 16;

/// A loaded plugin instance (VST3 or CLAP). The engine holds a second handle to it.
#[derive(Clone)]
pub struct PluginSlot {
    pub path: PathBuf,
    /// Which plugin inside the file, for files that hold several.
    pub class_id: Option<String>,
    pub name: String,
    pub handle: PluginHandle,
    /// The parameters automation can move, read when the plugin was loaded.
    pub params: Arc<Vec<Param>>,
}

impl PluginSlot {
    /// Identity of the instance, used to match editor windows to slots.
    pub fn id(&self) -> usize {
        self.handle.id()
    }

    pub fn param_name(&self, id: u32) -> Option<&str> {
        self.params.iter().find(|p| p.id == id).map(|p| p.name.as_str())
    }
}

#[derive(Default, Clone)]
pub struct PadModel {
    pub name: String,
    pub cfg: PadCfg,
    /// Revision stack: every destructive edit pushes a new buffer.
    pub takes: Vec<Arc<SampleData>>,
    pub take_idx: usize,
    pub pattern: Pattern,
    pub transients: Vec<u32>,
    pub analysis: Option<Analysis>,
    /// Cached mini-waveform for the pad face.
    pub mini: Vec<(f32, f32)>,
    /// VST3 instrument, when this is a MIDI pad targeting one.
    pub plugin: Option<PluginSlot>,
    /// Takes and comp of a vocal pad. Its sample (in `takes`) is the render of this.
    pub vocal: Option<crate::vocal::VocalStack>,
    /// The drum synth patch behind each take the drum synth baked, matched to the take
    /// by its buffer. Stepping back to such a take can bring the live synth back.
    pub drum_patches: Vec<(Arc<SampleData>, crate::drum::DrumParams)>,
    /// Onset envelope of one take (identified by its buffer address), kept so that
    /// dragging the sensitivity slider only re-runs the cheap peak-picking.
    onset_cache: Option<(usize, Arc<Vec<f32>>)>,
}

impl PadModel {
    pub fn sample(&self) -> Option<&Arc<SampleData>> {
        self.takes.get(self.take_idx)
    }

    pub fn is_empty(&self) -> bool {
        self.cfg.kind == PadKind::Audio && self.takes.is_empty()
    }

    pub fn refresh_mini(&mut self) {
        self.mini = match self.sample() {
            Some(s) => s.peaks(0, s.len(), 48),
            None => Vec::new(),
        };
    }

    /// Pushes a new revision, dropping any redo branch and the oldest take past the cap.
    pub fn push_take(&mut self, s: Arc<SampleData>) {
        if !self.takes.is_empty() {
            self.takes.truncate(self.take_idx + 1);
        }
        self.takes.push(s);
        if self.takes.len() > MAX_TAKES {
            self.takes.remove(0);
        }
        self.take_idx = self.takes.len() - 1;
        self.cfg.start = 0;
        self.cfg.end = 0;
        self.refresh_mini();
    }

    /// True when the two hold the same user-visible content. Buffers and plugins are
    /// compared by identity, so this is cheap enough to run every frame.
    ///
    /// `patterns` is false where the pattern is accounted for elsewhere: undo keeps patterns
    /// with their loops, not with the pads.
    pub fn same_as(&self, other: &PadModel, patterns: bool) -> bool {
        self.cfg == other.cfg
            && self.take_idx == other.take_idx
            && self.name == other.name
            && (!patterns || self.pattern == other.pattern)
            && self.transients == other.transients
            && self.takes.len() == other.takes.len()
            && self.takes.iter().zip(&other.takes).all(|(a, b)| Arc::ptr_eq(a, b))
            && self.plugin.as_ref().map(PluginSlot::id) == other.plugin.as_ref().map(PluginSlot::id)
            && self.vocal == other.vocal
    }

    /// A copy for another pad: the active take only, and no plugin, since an instance
    /// can't be shared (a pad that was playing one falls back to the internal synth).
    pub fn duplicate(&self) -> PadModel {
        let mut cfg = self.cfg;
        if cfg.target == MidiTarget::Plugin {
            cfg.target = MidiTarget::Synth;
        }
        PadModel {
            name: self.name.clone(),
            cfg,
            takes: self.sample().cloned().into_iter().collect(),
            take_idx: 0,
            pattern: self.pattern.clone(),
            transients: self.transients.clone(),
            analysis: self.analysis.clone(),
            mini: self.mini.clone(),
            plugin: None,
            vocal: self.vocal.clone(),
            drum_patches: self.drum_patches.iter().filter(|(s, _)| self.sample().is_some_and(|t| Arc::ptr_eq(s, t))).cloned().collect(),
            onset_cache: None,
        }
    }

    /// Re-detects transient markers on the active take.
    pub fn detect_transients(&mut self, sr: f32, sensitivity: f32) {
        let Some(s) = self.takes.get(self.take_idx) else {
            self.transients.clear();
            return;
        };
        let id = Arc::as_ptr(s) as usize;
        let mono = s.mono();
        if self.onset_cache.as_ref().is_none_or(|(of, _)| *of != id) {
            self.onset_cache = Some((id, Arc::new(analysis::onset_envelope(&mono, sr))));
        }
        let envelope = &self.onset_cache.as_ref().unwrap().1;
        self.transients = analysis::pick_onsets(envelope, &mono, sr, sensitivity);
    }

    /// Region actually played, in frames.
    pub fn region(&self) -> (usize, usize) {
        let len = self.sample().map_or(0, |s| s.len());
        let end = if self.cfg.end == 0 || self.cfg.end as usize > len { len } else { self.cfg.end as usize };
        ((self.cfg.start as usize).min(end), end)
    }
}
