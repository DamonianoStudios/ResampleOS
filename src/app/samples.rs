//! The sample browser: the user's sample folders, searched and auditioned inside the app,
//! and dragged (or sent with Enter) onto pads and the song timeline.
//!
//! Folders are kept in the preferences, with favourites and the files used lately. Each
//! folder is listed on a background thread when it is added or the app opens; the browser
//! works from that list, so searching a large library never touches the disk. Auditions
//! are decoded in the background too and play on the engine's preview voice, after the
//! master, so they are never processed, captured or resampled.

use super::*;
use std::path::{Path, PathBuf};

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

    /// Lists a folder in the background.
    fn scan_folder(&mut self, root: &Path) {
        self.samples.index.retain(|(p, _)| p != root);
        self.samples.index.push((root.to_path_buf(), None));
        let tx = self.jobs_tx.clone();
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let entries = list_folder(&root);
            let _ = tx.send(JobResult::SamplesListed { root, entries });
        });
    }

    pub(super) fn samples_listed(&mut self, root: PathBuf, entries: Vec<SampleEntry>) {
        if let Some(slot) = self.samples.index.iter_mut().find(|(p, _)| *p == root) {
            slot.1 = Some(entries);
        }
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

    /// Plays a file on the preview voice (decoded in the background).
    pub fn preview_sample(&mut self, path: &Path) {
        self.samples.preview_seq += 1;
        if crate::app::is_midi(path) {
            self.stop_preview();
            return;
        }
        let seq = self.samples.preview_seq;
        let (tx, rate, path) = (self.jobs_tx.clone(), self.shared.sample_rate, path.to_path_buf());
        std::thread::spawn(move || {
            if let Ok(data) = sample::load_file(&path, rate) {
                let _ = tx.send(JobResult::Previewed { seq, data });
            }
        });
    }

    pub(super) fn previewed(&mut self, seq: u64, data: SampleData) {
        if seq == self.samples.preview_seq && self.samples.open {
            let gain = 10f32.powf(self.prefs.preview_db / 20.0);
            self.send(Cmd::Preview(Some(Arc::new(data)), gain));
        }
    }

    pub fn stop_preview(&mut self) {
        self.samples.preview_seq += 1;
        self.send(Cmd::Preview(None, 1.0));
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
    /// folder open.
    pub fn sample_rows(&self) -> Vec<PathBuf> {
        let q = self.samples.query.trim();
        let entry_paths = |v: Vec<&SampleEntry>| v.into_iter().map(|e| e.path.clone()).collect::<Vec<_>>();
        match self.samples.list {
            SampleList::Favourites => self.prefs.favourites.iter().filter(|p| q.is_empty() || name_matches(p, q)).cloned().collect(),
            SampleList::Recent => self.prefs.recent_samples.iter().filter(|p| q.is_empty() || name_matches(p, q)).cloned().collect(),
            SampleList::Folder if !q.is_empty() => entry_paths(search(self.all_samples(), q)),
            SampleList::Folder => match self.folder_entries(self.samples.folder) {
                Some(entries) => entry_paths(folder_view(entries, &self.samples.dir).1),
                None => Vec::new(),
            },
        }
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
}
