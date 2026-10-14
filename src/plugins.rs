//! Plugin catalog (VST3 and CLAP) and scanning.
//!
//! A big plugin folder is hostile territory: some plugins take seconds to load, some pop
//! dialogs, some crash. So nothing is ever probed inside the app. Each plugin is inspected
//! by a short-lived copy of this executable (`--scan-vst3 <path>`), in parallel, with a
//! timeout, and the result is cached against the file's timestamp so the next launch only
//! looks at what changed.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SCAN_FLAG: &str = "--scan-vst3";
pub const SCAN_CLAP_FLAG: &str = "--scan-clap";
const SCAN_MARKER: &str = "RSOS-SCAN ";
const SCAN_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginEntry {
    pub path: PathBuf,
    /// Which plugin inside the file this is. One file can hold hundreds (Waves' WaveShell).
    #[serde(default)]
    pub class_id: Option<String>,
    pub name: String,
    pub vendor: String,
    pub category: String,
    pub instrument: bool,
    /// Why the plugin could not be read; such entries are listed but cannot be loaded.
    pub error: Option<String>,
    /// Modification time + size of the file when it was scanned.
    pub stamp: u64,
    /// Lowercased name and vendor, for searching.
    #[serde(skip)]
    pub key: String,
}

impl PluginEntry {
    fn finish(mut self) -> Self {
        self.key = format!("{} {}", self.name, self.vendor).to_lowercase();
        self
    }
}

#[derive(Serialize, Deserialize)]
struct ScanReply {
    class_id: String,
    name: String,
    vendor: String,
    category: String,
}

/// Entry point for the scanner child process. Prints one marked line and exits.
pub fn scan_child(flag: &str, path: &str) -> ! {
    if flag == SCAN_CLAP_FLAG {
        let reply = match crate::clap::Library::open(Path::new(path)) {
            Ok(lib) => {
                let replies: Vec<ScanReply> = lib
                    .descriptors()
                    .into_iter()
                    .map(|d| ScanReply { class_id: d.id, name: d.name, vendor: d.vendor, category: d.features.join("|") })
                    .collect();
                if replies.is_empty() { "ERR no plugin in this file".to_string() } else { serde_json::to_string(&replies).unwrap_or_default() }
            }
            Err(e) => format!("ERR {e}"),
        };
        println!("{SCAN_MARKER}{reply}");
        std::process::exit(0);
    }
    let reply = match vst3_host::discovery::list_audio_classes(Path::new(path)) {
        Ok(classes) if classes.is_empty() => "ERR no audio plugin in this file".to_string(),
        Ok(classes) => {
            let replies: Vec<ScanReply> = classes
                .into_iter()
                .map(|c| ScanReply { class_id: c.class_id, name: c.name, vendor: c.vendor, category: c.category })
                .collect();
            serde_json::to_string(&replies).unwrap_or_default()
        }
        Err(e) => format!("ERR {e}"),
    };
    println!("{SCAN_MARKER}{reply}");
    // Skip destructors: a plugin that hangs while unloading must not hold up the scan.
    std::process::exit(0);
}

fn stamp(path: &Path) -> u64 {
    // A bundle is a directory; its own timestamp changes when the plugin is reinstalled.
    std::fs::metadata(path)
        .map(|m| {
            let t = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            t ^ m.len().rotate_left(32)
        })
        .unwrap_or(0)
}

fn cache_file() -> Option<PathBuf> {
    Some(crate::prefs::config_dir()?.join("vst3_cache.json"))
}

/// Probes one file. The result is every plugin in it, or a single entry saying why not.
fn scan_one(exe: &Path, path: &Path, cancel: &AtomicBool) -> Vec<PluginEntry> {
    let fallback = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut entry = PluginEntry {
        path: path.to_path_buf(),
        class_id: None,
        name: fallback,
        vendor: String::new(),
        category: String::new(),
        instrument: false,
        error: None,
        stamp: stamp(path),
        key: String::new(),
    };
    let mut cmd = Command::new(exe);
    let flag = if crate::clap::is_clap(path) { SCAN_CLAP_FLAG } else { SCAN_FLAG };
    cmd.arg(flag).arg(path).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            entry.error = Some(format!("could not start scanner: {e}"));
            return vec![entry.finish()];
        }
    };
    // Read on another thread so a chatty plugin can't block on a full pipe.
    let out = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut o) = out {
            let _ = std::io::Read::read_to_string(&mut o, &mut s);
        }
        s
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if started.elapsed() > SCAN_TIMEOUT || cancel.load(Ordering::Relaxed) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(_) => break None,
        }
    };
    let text = reader.join().unwrap_or_default();
    let line = text.lines().rev().find_map(|l| l.strip_prefix(SCAN_MARKER));
    match (line, status) {
        (Some(l), _) if l.starts_with("ERR ") => entry.error = Some(l[4..].to_string()),
        (Some(l), _) => match serde_json::from_str::<Vec<ScanReply>>(l) {
            Ok(replies) => {
                return replies
                    .into_iter()
                    .map(|r| {
                        PluginEntry {
                            class_id: Some(r.class_id),
                            instrument: r.category.to_lowercase().contains("instrument"),
                            name: r.name,
                            vendor: r.vendor,
                            category: r.category,
                            ..entry.clone()
                        }
                        .finish()
                    })
                    .collect();
            }
            Err(e) => entry.error = Some(format!("unreadable scan result: {e}")),
        },
        (None, None) => entry.error = Some("timed out while loading".into()),
        (None, Some(s)) => entry.error = Some(format!("crashed while loading ({s})")),
    }
    vec![entry.finish()]
}

enum ScanMsg {
    Entries(Vec<PluginEntry>),
    Done,
}

pub struct Catalog {
    pub entries: Vec<PluginEntry>,
    pub scanning: bool,
    pub total: usize,
    pub done: Arc<AtomicUsize>,
    /// True once a scan has completed at least once on this machine.
    pub scanned_once: bool,
    rx: Option<Receiver<ScanMsg>>,
    cancel: Arc<AtomicBool>,
}

impl Catalog {
    pub fn load() -> Self {
        let mut entries: Vec<PluginEntry> = cache_file()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        entries = entries.into_iter().map(PluginEntry::finish).collect();
        // CLAP plugins the catalog has never seen (as after updating from a version that
        // only read VST3) also make the browser refresh it.
        let unseen_clap = crate::clap::find_files(&crate::clap::standard_dirs()).iter().any(|p| !entries.iter().any(|e| &e.path == p));
        Catalog {
            // A cache written before files were read plugin by plugin counts as no scan,
            // so the browser refreshes it by itself.
            scanned_once: !unseen_clap && !entries.is_empty() && entries.iter().all(|e| e.error.is_some() || e.class_id.is_some()),
            entries,
            scanning: false,
            total: 0,
            done: Arc::new(AtomicUsize::new(0)),
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    fn save(&self) {
        let Some(file) = cache_file() else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string(&self.entries) {
            let _ = std::fs::write(file, json);
        }
    }

    pub fn failed(&self) -> usize {
        self.entries.iter().filter(|e| e.error.is_some()).count()
    }

    /// Scans the standard VST3 and CLAP folders in the background. Unchanged plugins come straight
    /// from the cache; `retry_failed` also re-probes the ones that failed last time.
    pub fn start_scan(&mut self, retry_failed: bool, extra_dirs: &[PathBuf]) {
        if self.scanning {
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let mut roots = vst3_host::discovery::scan_standard_paths();
        roots.extend(extra_dirs.iter().cloned());
        let mut paths = vst3_host::discovery::scan_directories(&roots).unwrap_or_default();
        let mut clap_roots = crate::clap::standard_dirs();
        clap_roots.extend(extra_dirs.iter().cloned());
        paths.extend(crate::clap::find_files(&clap_roots));

        let mut known: HashMap<PathBuf, Vec<PluginEntry>> = HashMap::new();
        for e in self.entries.drain(..) {
            known.entry(e.path.clone()).or_default().push(e);
        }
        let mut todo = Vec::new();
        for path in paths {
            // Entries cached before files were read plugin by plugin have no class id and
            // describe only one plugin of the file, so those files are read again.
            let fresh = |e: &PluginEntry| {
                e.stamp == stamp(&path) && if e.error.is_some() { !retry_failed } else { e.class_id.is_some() }
            };
            match known.remove(&path) {
                Some(found) if found.iter().all(fresh) => self.entries.extend(found),
                _ => todo.push(path),
            }
        }
        self.sort();
        self.total = todo.len();
        self.done = Arc::new(AtomicUsize::new(0));
        self.cancel = Arc::new(AtomicBool::new(false));
        if todo.is_empty() {
            self.scanned_once = true;
            self.save();
            return;
        }
        self.scanning = true;
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let queue = Arc::new(Mutex::new(todo));
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get() / 2).clamp(2, 8);
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let (queue, tx, exe) = (queue.clone(), tx.clone(), exe.clone());
                let (done, cancel) = (self.done.clone(), self.cancel.clone());
                std::thread::spawn(move || worker(&exe, &queue, &tx, &done, &cancel))
            })
            .collect();
        std::thread::spawn(move || {
            for h in handles {
                let _ = h.join();
            }
            let _ = tx.send(ScanMsg::Done);
        });
    }

    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn sort(&mut self) {
        // The file path keeps the many versions of one shell plugin in a stable order.
        self.entries.sort_by(|a, b| a.key.cmp(&b.key).then_with(|| a.path.cmp(&b.path)));
    }

    /// Call once per frame: folds finished scans into the catalog.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        let mut got = false;
        let mut finished = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                ScanMsg::Entries(found) => {
                    self.entries.extend(found);
                    got = true;
                }
                ScanMsg::Done => finished = true,
            }
        }
        if got {
            self.sort();
        }
        if finished {
            self.scanning = false;
            self.scanned_once = true;
            self.rx = None;
            self.save();
        }
    }
}

fn worker(exe: &Path, queue: &Mutex<Vec<PathBuf>>, tx: &Sender<ScanMsg>, done: &AtomicUsize, cancel: &AtomicBool) {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Some(path) = queue.lock().ok().and_then(|mut q| q.pop()) else { return };
        let found = scan_one(exe, &path, cancel);
        done.fetch_add(1, Ordering::Relaxed);
        // A cancelled probe was killed by us, not by the plugin; don't record it as broken.
        if cancel.load(Ordering::Relaxed) && found.iter().any(|e| e.error.is_some()) {
            return;
        }
        if tx.send(ScanMsg::Entries(found)).is_err() {
            return;
        }
    }
}
