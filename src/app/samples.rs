//! The sample browser: the user's sample folders, searched and auditioned inside the app,
//! and dragged (or sent with Enter) onto pads and the song timeline.
//!
//! Folders are kept in the preferences, with favourites and the files used lately. Each
//! folder is listed on a background thread when it is added or the app opens; the browser
//! works from that list, so searching a large library never touches the disk. Then each
//! file is analysed, one folder at a time (length, tempo, key, or a one-shot's note), and
//! the results are cached per folder in the settings folder, so it happens once per file.
//! Auditions are decoded in the background too and play on the engine's preview voice,
//! after the master, so they are never processed, captured or resampled; with Sync on, a
//! loop is stretched to the project tempo first and starts on the beat.

use super::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// Files larger than this are not analysed (a long recording, not a sample).
const MAX_ANALYSE_BYTES: u64 = 80_000_000;
/// What analysis finds of a file.
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct SampleInfo {
    pub secs: f32,
    /// 0 for a one-shot.
    pub bpm: f32,
    pub root: u8,
    pub minor: bool,
    pub key_conf: f32,
    /// The note a one-shot sounds, as a MIDI note.
    pub note: Option<u8>,
}

impl SampleInfo {
    pub fn is_loop(&self) -> bool {
        self.bpm > 0.0
    }

    /// "92 BPM · Am", or "C3 · 0.4 s" for a one-shot.
    pub fn label(&self) -> String {
        let n = crate::music::NOTE_NAMES;
        if self.is_loop() {
            let key = if self.key_conf > 0.3 { format!(" · {}{}", n[self.root as usize % 12], if self.minor { "m" } else { "" }) } else { String::new() };
            format!("{:.0} BPM{key}", self.bpm)
        } else {
            let note = self.note.map_or(String::new(), |m| format!("{}{} · ", n[m as usize % 12], m as i32 / 12 - 1));
            format!("{note}{:.1} s", self.secs)
        }
    }
}

/// Which files the browser shows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SampleFilter {
    /// 0 any, 1 one-shots, 2 loops.
    pub kind: u8,
    /// None any; Some(None) the project key; Some(Some((root, minor))) that key.
    pub key: Option<Option<(u8, bool)>>,
    pub bpm: (f32, f32),
    /// Longest, in seconds; 0 any.
    pub max_secs: f32,
}

impl Default for SampleFilter {
    fn default() -> Self {
        SampleFilter { kind: 0, key: None, bpm: (0.0, 0.0), max_secs: 0.0 }
    }
}

impl SampleFilter {
    pub fn active(&self) -> bool {
        *self != SampleFilter::default()
    }

    /// Whether a file with `info` (None: not analysed yet, or a MIDI file) is shown.
    pub fn matches(&self, info: Option<&SampleInfo>, project: crate::music::Key) -> bool {
        if !self.active() {
            return true;
        }
        let Some(i) = info else { return false };
        match self.kind {
            1 if i.is_loop() => return false,
            2 if !i.is_loop() => return false,
            _ => {}
        }
        if self.bpm.1 > 0.0 && (!i.is_loop() || i.bpm < self.bpm.0 || i.bpm > self.bpm.1) {
            return false;
        }
        if self.max_secs > 0.0 && i.secs > self.max_secs {
            return false;
        }
        let key = match self.key {
            None => return true,
            Some(None) => project,
            Some(Some((root, minor))) => crate::music::Key { root, scale: if minor { crate::music::Scale::Minor } else { crate::music::Scale::Major } },
        };
        key_matches(i, key)
    }
}

/// A loop fits a key when its own key shares at least six of seven notes with it (the
/// same key, its relative, or a mode a note away); a one-shot when its note is in it.
pub fn key_matches(i: &SampleInfo, key: crate::music::Key) -> bool {
    if i.is_loop() {
        if i.key_conf <= 0.3 {
            return false;
        }
        let own = crate::music::Key { root: i.root, scale: if i.minor { crate::music::Scale::Minor } else { crate::music::Scale::Major } };
        (own.mask() & key.mask()).count_ones() >= 6
    } else {
        i.note.is_some_and(|n| key.contains(n as i32))
    }
}

/// Files the browser lists.
const EXTENSIONS: [&str; 8] = ["wav", "mp3", "flac", "ogg", "aif", "aiff", "mid", "midi"];
/// Most files listed from one folder, so a whole drive added by mistake stays manageable.
const MAX_FILES: usize = 200_000;
/// Most results a search shows.
pub const MAX_RESULTS: usize = 500;
/// How many recently used files are remembered.
const RECENT: usize = 40;

/// A folder of samples, as kept in the preferences.
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct SampleFolder {
    pub name: String,
    pub path: PathBuf,
}

/// One file found in a folder.
#[derive(Clone, Debug)]
pub struct SampleEntry {
    pub path: PathBuf,
    /// The file's folder, relative to the sample folder's root.
    pub dir: PathBuf,
    /// Lower-case name, for searching.
    key: String,
}

/// Which list the browser shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SampleList {
    #[default]
    Folder,
    Favourites,
    Recent,
    /// The audio files the open project uses (in its folder).
    Project,
}

#[derive(Default)]
pub struct SampleBrowser {
    pub open: bool,
    pub query: String,
    pub list: SampleList,
    /// The folder on show (an index into the preferences' folders) and where in it.
    pub folder: usize,
    pub dir: PathBuf,
    /// What each folder holds, by the folder's path; `None` while it is being listed.
    pub index: Vec<(PathBuf, Option<Vec<SampleEntry>>)>,
    pub selected: Option<PathBuf>,
    /// The list has the keyboard: arrows move through it, Enter uses the file.
    pub focused: bool,
    /// A file being dragged out of the browser.
    pub drag: Option<PathBuf>,
    /// Play a file as soon as it is selected.
    pub auto_play: bool,
    /// Counts auditions, so a slow decode that has been overtaken is dropped.
    preview_seq: u64,
    pub started: bool,
    /// What analysis has found, by file.
    pub info: HashMap<PathBuf, SampleInfo>,
    pub filter: SampleFilter,
    pub show_filter: bool,
    /// Audition loops at the project tempo, in time.
    pub sync: bool,
    /// Folders being analysed, with a flag to stop, and how many files are left in each.
    pub analysing: Vec<(PathBuf, Arc<AtomicBool>, usize)>,
}

/// Whether a file is one the browser lists.
pub fn is_sample(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Every file the browser lists under `root`, sorted by folder then name.
pub fn list_folder(root: &Path) -> Vec<SampleEntry> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = e.path();
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() && is_sample(&path) && out.len() < MAX_FILES {
                let rel = dir.strip_prefix(root).unwrap_or(Path::new("")).to_path_buf();
                out.push(SampleEntry { key: name.to_lowercase(), dir: rel, path });
            }
        }
    }
    out.sort_by(|a, b| a.dir.cmp(&b.dir).then_with(|| a.key.cmp(&b.key)));
    out
}

/// Entries whose name holds every word of `query`, case aside.
#[cfg(test)]
pub fn search<'a>(entries: impl Iterator<Item = &'a SampleEntry>, query: &str) -> Vec<&'a SampleEntry> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    entries.filter(|e| words.iter().all(|w| e.key.contains(w.as_str()))).take(MAX_RESULTS).collect()
}

/// The folders directly inside `dir` that hold listed files (at any depth), and the files
/// directly in it.
pub fn folder_view<'a>(entries: &'a [SampleEntry], dir: &Path) -> (Vec<String>, Vec<&'a SampleEntry>) {
    let mut subs: Vec<String> = Vec::new();
    let mut files = Vec::new();
    for e in entries {
        if e.dir == dir {
            files.push(e);
        } else if let Ok(rest) = e.dir.strip_prefix(dir)
            && let Some(first) = rest.components().next()
        {
            let s = first.as_os_str().to_string_lossy().into_owned();
            if subs.last() != Some(&s) && !subs.contains(&s) {
                subs.push(s);
            }
        }
    }
    subs.sort_by_key(|s| s.to_lowercase());
    (subs, files)
}

impl App {
    /// Opens or closes the browser; the first time, lists the folders.
    pub fn toggle_samples(&mut self) {
        self.samples.open ^= true;
        if self.samples.open && !self.samples.started {
            self.samples.started = true;
            self.samples.auto_play = true;
            for f in self.prefs.sample_folders.clone() {
                self.scan_folder(&f.path);
            }
        }
        if !self.samples.open {
            self.stop_preview();
        }
    }

    /// Lists a folder in the background, then analyses what is in it.
    fn scan_folder(&mut self, root: &Path) {
        self.samples.index.retain(|(p, _)| p != root);
        self.samples.index.push((root.to_path_buf(), None));
        self.stop_analysing(root);
        let cancel = Arc::new(AtomicBool::new(false));
        self.samples.analysing.push((root.to_path_buf(), cancel.clone(), usize::MAX));
        let tx = self.jobs_tx.clone();
        let root = root.to_path_buf();
        let rate = self.shared.sample_rate;
        std::thread::spawn(move || {
            let entries = list_folder(&root);
            let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();
            let _ = tx.send(JobResult::SamplesListed { root: root.clone(), entries });
            analyse_folder(&root, &paths, rate, &cancel, &tx);
        });
    }

    fn stop_analysing(&mut self, root: &Path) {
        for (p, cancel, _) in &self.samples.analysing {
            if p == root {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        self.samples.analysing.retain(|(p, _, _)| p != root);
    }

    pub(super) fn samples_listed(&mut self, root: PathBuf, entries: Vec<SampleEntry>) {
        if let Some(slot) = self.samples.index.iter_mut().find(|(p, _)| *p == root) {
            slot.1 = Some(entries);
        }
    }

    pub(super) fn samples_analysed(&mut self, root: PathBuf, items: Vec<(PathBuf, SampleInfo)>, left: usize) {
        self.samples.info.extend(items);
        if let Some(a) = self.samples.analysing.iter_mut().find(|a| a.0 == root) {
            a.2 = left;
        }
        if left == 0 {
            self.samples.analysing.retain(|a| a.0 != root);
        }
    }

    /// Files still to analyse, in every folder (None while a folder is being listed).
    pub fn samples_to_analyse(&self) -> Option<usize> {
        let mut n = 0;
        for a in &self.samples.analysing {
            if a.2 == usize::MAX {
                return None;
            }
            n += a.2;
        }
        Some(n)
    }

    /// What is known of folder `k`: `None` while it is still being listed.
    pub fn folder_entries(&self, k: usize) -> Option<&[SampleEntry]> {
        let root = &self.prefs.sample_folders.get(k)?.path;
        self.samples.index.iter().find(|(p, _)| p == root)?.1.as_deref()
    }

    /// Every file listed so far, in every folder.
    pub fn all_samples(&self) -> impl Iterator<Item = &SampleEntry> {
        self.samples.index.iter().filter_map(|(_, e)| e.as_ref()).flatten()
    }

    fn save_prefs_now(&mut self) {
        self.prefs_edit.sample_folders = self.prefs.sample_folders.clone();
        self.prefs_edit.favourites = self.prefs.favourites.clone();
        self.prefs_edit.recent_samples = self.prefs.recent_samples.clone();
        self.prefs_edit.preview_db = self.prefs.preview_db;
        if let Err(e) = self.prefs.save() {
            self.status = format!("Could not save the settings: {e}");
        }
    }

    /// Asks for a folder and adds it.
    pub fn add_sample_folder_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new().set_title("Add a sample folder").pick_folder() else { return };
        self.add_sample_folder(path);
    }

    pub fn add_sample_folder(&mut self, path: PathBuf) {
        if self.prefs.sample_folders.iter().any(|f| f.path == path) {
            return;
        }
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        self.prefs.sample_folders.push(SampleFolder { name, path: path.clone() });
        self.save_prefs_now();
        self.scan_folder(&path);
        self.samples.folder = self.prefs.sample_folders.len() - 1;
        self.samples.dir = PathBuf::new();
        self.samples.list = SampleList::Folder;
    }

    pub fn remove_sample_folder(&mut self, k: usize) {
        if k >= self.prefs.sample_folders.len() {
            return;
        }
        let f = self.prefs.sample_folders.remove(k);
        self.samples.index.retain(|(p, _)| *p != f.path);
        self.stop_analysing(&f.path);
        self.samples.folder = self.samples.folder.min(self.prefs.sample_folders.len().saturating_sub(1));
        self.samples.dir = PathBuf::new();
        self.save_prefs_now();
    }

    /// Lists folder `k` again (after files were added to it).
    pub fn rescan_sample_folder(&mut self, k: usize) {
        if let Some(f) = self.prefs.sample_folders.get(k).cloned() {
            self.scan_folder(&f.path);
        }
    }

    pub fn is_favourite(&self, path: &Path) -> bool {
        self.prefs.favourites.iter().any(|f| f == path)
    }

    pub fn toggle_favourite(&mut self, path: &Path) {
        match self.prefs.favourites.iter().position(|f| f == path) {
            Some(i) => drop(self.prefs.favourites.remove(i)),
            None => self.prefs.favourites.push(path.to_path_buf()),
        }
        self.save_prefs_now();
    }

    fn note_recent(&mut self, path: &Path) {
        self.prefs.recent_samples.retain(|p| p != path);
        self.prefs.recent_samples.insert(0, path.to_path_buf());
        self.prefs.recent_samples.truncate(RECENT);
        self.save_prefs_now();
    }

    /// Selects a file, and plays it if auditioning is on.
    pub fn select_sample(&mut self, path: &Path) {
        self.samples.selected = Some(path.to_path_buf());
        self.samples.focused = true;
        if self.samples.auto_play {
            self.preview_sample(path);
        }
    }

    /// Plays a file on the preview voice (decoded in the background). With Sync on, a loop
    /// is stretched to the project tempo first, goes round, and starts on the beat.
    pub fn preview_sample(&mut self, path: &Path) {
        self.samples.preview_seq += 1;
        if crate::app::is_midi(path) {
            self.stop_preview();
            return;
        }
        let seq = self.samples.preview_seq;
        let (tx, rate, path) = (self.jobs_tx.clone(), self.shared.sample_rate, path.to_path_buf());
        let sync = self.samples.info.get(&path).filter(|i| self.samples.sync && i.is_loop()).map(|i| (i.bpm, self.bpm, self.warp_mode));
        std::thread::spawn(move || {
            let Ok(data) = sample::load_file(&path, rate) else { return };
            let (data, synced) = match sync {
                Some((from, to, mode)) => (sync_loop(&data, from, to, mode, rate as f32), true),
                None => (data, false),
            };
            let _ = tx.send(JobResult::Previewed { seq, data, sync: synced });
        });
    }

    pub(super) fn previewed(&mut self, seq: u64, data: SampleData, sync: bool) {
        if seq == self.samples.preview_seq && self.samples.open {
            let gain = 10f32.powf(self.prefs.preview_db / 20.0);
            self.send(Cmd::Preview(Some(Arc::new(data)), gain, sync));
        }
    }

    pub fn stop_preview(&mut self) {
        self.samples.preview_seq += 1;
        self.send(Cmd::Preview(None, 1.0, false));
    }

    /// Uses a file where it makes sense: on the selected pad (or, with `next`, the next
    /// empty one) in the pad view; on the timeline at the insert marker, on the selected
    /// pad's track if it has one, in the song view.
    pub fn use_sample(&mut self, path: &Path, next: bool) {
        self.note_recent(path);
        let midi = crate::app::is_midi(path);
        if self.view == View::Arrange {
            let pad = Some(self.selected).filter(|&p| !self.arrangement.clips(p).is_empty());
            let at = self.arr.cursor;
            if midi {
                self.import_midi_to_song(path, pad, at);
            } else {
                self.load_song_audio(path.to_path_buf(), pad, at);
            }
            return;
        }
        let pad = if next { self.next_empty_pad().unwrap_or(self.selected) } else { self.selected };
        self.drop_sample_on_pad(path, pad);
    }

    /// A file dropped on a pad.
    pub fn drop_sample_on_pad(&mut self, path: &Path, pad: usize) {
        self.note_recent(path);
        if crate::app::is_midi(path) {
            self.import_midi_to_pads(path, pad);
        } else {
            self.select(pad);
            self.load_file(pad, path.to_path_buf());
        }
    }

    /// A file dropped on the timeline: on `pad`'s track (or a new one) at `at`.
    pub fn drop_sample_on_song(&mut self, path: &Path, pad: Option<usize>, at: u64) {
        self.note_recent(path);
        if crate::app::is_midi(path) {
            self.import_midi_to_song(path, pad, at);
        } else {
            self.load_song_audio(path.to_path_buf(), pad, at);
        }
    }

    /// The files the browser shows now, in order: search results, the list chosen, or the
    /// folder open; with a filter, only those that pass it.
    pub fn sample_rows(&self) -> Vec<PathBuf> {
        let q = self.samples.query.trim();
        let entry_paths = |v: Vec<&SampleEntry>| v.into_iter().map(|e| e.path.clone()).collect::<Vec<_>>();
        let filter = self.samples.filter;
        let rows: Vec<PathBuf> = match self.samples.list {
            SampleList::Favourites => self.prefs.favourites.iter().filter(|p| q.is_empty() || name_matches(p, q)).cloned().collect(),
            SampleList::Recent => self.prefs.recent_samples.iter().filter(|p| q.is_empty() || name_matches(p, q)).cloned().collect(),
            SampleList::Project => self.project_files().into_iter().filter(|p| q.is_empty() || name_matches(p, q)).collect(),
            // A filter searches every folder, as a search does.
            SampleList::Folder if !q.is_empty() || filter.active() => {
                let pass = |e: &&SampleEntry| filter.matches(self.samples.info.get(&e.path), self.key);
                let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
                self.all_samples().filter(|e| words.iter().all(|w| e.key.contains(w.as_str()))).filter(pass).take(MAX_RESULTS).map(|e| e.path.clone()).collect()
            }
            SampleList::Folder => match self.folder_entries(self.samples.folder) {
                Some(entries) => entry_paths(folder_view(entries, &self.samples.dir).1),
                None => Vec::new(),
            },
        };
        if filter.active() {
            rows.into_iter().filter(|p| filter.matches(self.samples.info.get(p), self.key)).collect()
        } else {
            rows
        }
    }

    /// The audio the open project holds: its pads' takes, the song's recordings, vocal takes.
    pub fn project_files(&self) -> Vec<PathBuf> {
        let Some(dir) = &self.project_dir else { return Vec::new() };
        let mut out = Vec::new();
        for sub in ["samples", "song", "vocals"] {
            let Ok(rd) = std::fs::read_dir(dir.join(sub)) else { continue };
            let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && is_sample(p)).collect();
            files.sort();
            out.extend(files);
        }
        out
    }

    /// Arrow keys and Enter while the browser has the keyboard. True if `key` was used.
    pub(super) fn samples_key(&mut self, key: egui::Key, shift: bool) -> bool {
        use egui::Key;
        if !(self.samples.open && self.samples.focused) {
            return false;
        }
        match key {
            Key::ArrowUp | Key::ArrowDown => {
                let rows = self.sample_rows();
                if rows.is_empty() {
                    return true;
                }
                let at = self.samples.selected.as_ref().and_then(|s| rows.iter().position(|r| r == s));
                let next = match (at, key) {
                    (None, _) => 0,
                    (Some(i), Key::ArrowUp) => i.saturating_sub(1),
                    (Some(i), _) => (i + 1).min(rows.len() - 1),
                };
                let path = rows[next].clone();
                self.select_sample(&path);
            }
            Key::Enter => {
                if let Some(path) = self.samples.selected.clone() {
                    self.use_sample(&path, shift);
                }
            }
            Key::Escape => {
                self.samples.focused = false;
                self.stop_preview();
            }
            _ => return false,
        }
        true
    }
}

/// A loop at `from` BPM stretched to `to`, ready to go round in time.
fn sync_loop(data: &SampleData, from: f32, to: f32, mode: crate::stretch::WarpMode, sr: f32) -> SampleData {
    if (from - to).abs() < 0.01 {
        return SampleData::new(data.l.clone(), data.r.clone());
    }
    let onsets = crate::analysis::detect_onsets(&data.mono(), sr, 0.5);
    let w = crate::stretch::Warp { mode, ratio: from as f64 / to as f64, semis: 0.0, formants: false };
    crate::stretch::warp(data, &onsets, w, sr)
}

/// One folder's cache: what analysis found of each file, with the file's size and time.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Cache {
    files: HashMap<String, (u64, u64, SampleInfo)>,
}

fn cache_path(root: &Path) -> Option<PathBuf> {
    // FNV-1a of the folder's path, for a file name.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in root.to_string_lossy().as_bytes() {
        h = (h ^ *b as u64).wrapping_mul(0x100_0000_01b3);
    }
    Some(crate::prefs::config_dir()?.join("sample_cache").join(format!("{h:016x}.json")))
}

/// Analyses every audio file of a folder (from the cache when it has not changed), sending
/// results back a few at a time. One folder at a time across the app.
fn analyse_folder(root: &Path, paths: &[PathBuf], rate: u32, cancel: &AtomicBool, tx: &std::sync::mpsc::Sender<JobResult>) {
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _turn = ONE_AT_A_TIME.lock();
    let file = cache_path(root);
    let mut cache: Cache = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let audio: Vec<&PathBuf> = paths.iter().filter(|p| !crate::app::is_midi(p)).collect();
    let mut batch = Vec::new();
    let mut fresh = 0;
    let mut sent = std::time::Instant::now();
    let total = audio.len();
    let save = |cache: &Cache| {
        if let Some(f) = &file {
            let _ = std::fs::create_dir_all(f.parent().unwrap_or(Path::new(".")));
            if let Ok(json) = serde_json::to_string(cache) {
                let _ = std::fs::write(f, json);
            }
        }
    };
    for (k, path) in audio.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(meta) = std::fs::metadata(path) else { continue };
        let size = meta.len();
        let time = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy().into_owned();
        let info = match cache.files.get(&rel) {
            Some((s, t, info)) if *s == size && *t == time => Some(*info),
            _ if size > MAX_ANALYSE_BYTES => None,
            _ => sample::load_file(path, rate).ok().map(|data| {
                fresh += 1;
                let a = crate::analysis::analyze(&data, rate as f32);
                let secs = data.len() as f32 / rate as f32;
                let info = SampleInfo { secs, bpm: a.bpm, root: a.key_root, minor: a.key_minor, key_conf: a.key_conf, note: a.pitch.map(|p| p.round().clamp(0.0, 127.0) as u8) };
                cache.files.insert(rel.clone(), (size, time, info));
                info
            }),
        };
        if let Some(info) = info {
            batch.push(((*path).clone(), info));
        }
        let left = total - k - 1;
        if left == 0 || batch.len() >= 400 || sent.elapsed().as_millis() > 400 {
            let _ = tx.send(JobResult::SamplesAnalysed { root: root.to_path_buf(), items: std::mem::take(&mut batch), left });
            sent = std::time::Instant::now();
        }
        if fresh > 0 && fresh % 200 == 0 {
            save(&cache);
        }
    }
    if total == 0 {
        let _ = tx.send(JobResult::SamplesAnalysed { root: root.to_path_buf(), items: Vec::new(), left: 0 });
    }
    if fresh > 0 {
        save(&cache);
    }
}

fn name_matches(path: &Path, query: &str) -> bool {
    let name = path.file_name().map_or(String::new(), |n| n.to_string_lossy().to_lowercase());
    query.split_whitespace().all(|w| name.contains(&w.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_listed_searched_and_browsed() {
        let root = std::env::temp_dir().join(format!("resampleos_samples_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, file) in [("Drums/Kicks", "Kick Deep.wav"), ("Drums/Kicks", "kick punchy.WAV"), ("Drums/Snares", "Snare 1.flac"), ("", "Bassline.mid"), ("Loops", "notes.txt")] {
            let d = root.join(dir);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(file), b"x").unwrap();
        }
        let entries = list_folder(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(entries.len(), 4, "audio and MIDI files only");
        // Search is by every word of the name, case aside.
        let names = |v: Vec<&SampleEntry>| v.iter().map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>();
        assert_eq!(names(search(entries.iter(), "KICK")), vec!["Kick Deep.wav", "kick punchy.WAV"]);
        assert_eq!(names(search(entries.iter(), "kick deep")), vec!["Kick Deep.wav"]);
        // Browsing: at the top, the Drums folder and the MIDI file; Loops holds nothing listed.
        let (subs, files) = folder_view(&entries, Path::new(""));
        assert_eq!(subs, vec!["Drums"]);
        assert_eq!(names(files), vec!["Bassline.mid"]);
        let (subs, files) = folder_view(&entries, Path::new("Drums"));
        assert_eq!(subs, vec!["Kicks", "Snares"]);
        assert!(files.is_empty());
    }

    #[test]
    fn filters_pick_by_kind_key_tempo_and_length() {
        use crate::music::{Key, Scale};
        let a_minor_loop = SampleInfo { secs: 4.0, bpm: 92.0, root: 9, minor: true, key_conf: 0.8, note: None };
        let f_sharp_loop = SampleInfo { secs: 4.0, bpm: 140.0, root: 6, minor: false, key_conf: 0.8, note: None };
        let kick_e = SampleInfo { secs: 0.4, bpm: 0.0, root: 0, minor: false, key_conf: 0.0, note: Some(40) };
        let c_major = Key { root: 0, scale: Scale::Major };
        let all = SampleFilter::default();
        assert!(all.matches(None, c_major), "no filter shows everything, analysed or not");
        let loops = SampleFilter { kind: 2, ..all };
        assert!(loops.matches(Some(&a_minor_loop), c_major) && !loops.matches(Some(&kick_e), c_major));
        assert!(!loops.matches(None, c_major), "a filter hides what is not analysed yet");
        // The project key, C major: A minor is its relative; F# major is not; an E one-shot is in it.
        let in_key = SampleFilter { key: Some(None), ..all };
        assert!(in_key.matches(Some(&a_minor_loop), c_major));
        assert!(!in_key.matches(Some(&f_sharp_loop), c_major));
        assert!(in_key.matches(Some(&kick_e), c_major));
        // A chosen key, and tempo and length ranges.
        let f_sharp = SampleFilter { key: Some(Some((6, false))), ..all };
        assert!(f_sharp.matches(Some(&f_sharp_loop), c_major) && !f_sharp.matches(Some(&a_minor_loop), c_major));
        let slow = SampleFilter { bpm: (85.0, 100.0), ..all };
        assert!(slow.matches(Some(&a_minor_loop), c_major) && !slow.matches(Some(&f_sharp_loop), c_major) && !slow.matches(Some(&kick_e), c_major));
        let short = SampleFilter { max_secs: 1.0, ..all };
        assert!(short.matches(Some(&kick_e), c_major) && !short.matches(Some(&a_minor_loop), c_major));
        assert_eq!(a_minor_loop.label(), "92 BPM · Am");
        assert_eq!(kick_e.label(), "E2 · 0.4 s");
    }
}
