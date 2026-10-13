//! VST3 plugin catalog and scanning.
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
const SCAN_MARKER: &str = "RSOS-SCAN ";
const SCAN_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginEntry {
    pub path: PathBuf,
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
    name: String,
    vendor: String,
    category: String,
}

/// Entry point for the scanner child process. Prints one marked line and exits.
pub fn scan_child(path: &str) -> ! {
    let reply = match vst3_host::discovery::get_plugin_info(Path::new(path)) {
        Ok(i) => serde_json::to_string(&ScanReply { name: i.name, vendor: i.vendor, category: i.category })
            .unwrap_or_default(),
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

fn scan_one(exe: &Path, path: &Path, cancel: &AtomicBool) -> PluginEntry {
    let fallback = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut entry = PluginEntry {
        path: path.to_path_buf(),
        name: fallback,
        vendor: String::new(),
        category: String::new(),
        instrument: false,
        error: None,
        stamp: stamp(path),
        key: String::new(),
    };
    let mut cmd = Command::new(exe);
    cmd.arg(SCAN_FLAG).arg(path).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            entry.error = Some(format!("could not start scanner: {e}"));
            return entry.finish();
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
        (Some(l), _) => match serde_json::from_str::<ScanReply>(l) {
            Ok(r) => {
                entry.instrument = r.category.contains("Instrument");
                entry.name = r.name;
                entry.vendor = r.vendor;
                entry.category = r.category;
            }
            Err(e) => entry.error = Some(format!("unreadable scan result: {e}")),
        },
        (None, None) => entry.error = Some("timed out while loading".into()),
        (None, Some(s)) => entry.error = Some(format!("crashed while loading ({s})")),
    }
    entry.finish()
}

enum ScanMsg {
    Entry(PluginEntry),
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
        Catalog {
            scanned_once: !entries.is_empty(),
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

    /// Scans the standard VST3 folders in the background. Unchanged plugins come straight
    /// from the cache; `retry_failed` also re-probes the ones that failed last time.
    pub fn start_scan(&mut self, retry_failed: bool, extra_dirs: &[PathBuf]) {
        if self.scanning {
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let mut roots = vst3_host::discovery::scan_standard_paths();
        roots.extend(extra_dirs.iter().cloned());
        let paths = vst3_host::discovery::scan_directories(&roots).unwrap_or_default();

        let known: HashMap<PathBuf, PluginEntry> = self.entries.drain(..).map(|e| (e.path.clone(), e)).collect();
        let mut todo = Vec::new();
        for path in paths {
            match known.get(&path) {
                Some(e) if e.stamp == stamp(&path) && !(retry_failed && e.error.is_some()) => {
                    self.entries.push(e.clone())
                }
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
        self.entries.sort_by(|a, b| a.key.cmp(&b.key));
    }

    /// Call once per frame: folds finished scans into the catalog.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        let mut got = false;
        let mut finished = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                ScanMsg::Entry(e) => {
                    self.entries.push(e);
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
        let entry = scan_one(exe, &path, cancel);
        done.fetch_add(1, Ordering::Relaxed);
        // A cancelled probe was killed by us, not by the plugin; don't record it as broken.
        if cancel.load(Ordering::Relaxed) && entry.error.is_some() {
            return;
        }
        if tx.send(ScanMsg::Entry(entry)).is_err() {
            return;
        }
    }
}
