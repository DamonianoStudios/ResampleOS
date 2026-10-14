//! A host for CLAP plugins: loading, processing, parameters, state and (on Windows) the
//! plugin's own editor window.
//!
//! The plugin sits behind a mutex shared by the UI and the audio thread, like a VST3 one
//! (see [`crate::hosted`]). Everything the plugin asks of the host from another thread
//! (a main-thread callback, a resize, its editor closing) only raises a flag here, and
//! the UI acts on it next frame.

#[cfg(windows)]
mod window;
#[cfg(windows)]
pub use window::ClapEditor;

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::*;
use clap_sys::ext::audio_ports::{CLAP_EXT_AUDIO_PORTS, clap_audio_port_info, clap_host_audio_ports, clap_plugin_audio_ports};
use clap_sys::ext::gui::{CLAP_EXT_GUI, clap_host_gui, clap_plugin_gui};
use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_host_latency};
use clap_sys::ext::log::{CLAP_EXT_LOG, clap_host_log, clap_log_severity};
use clap_sys::ext::params::*;
use clap_sys::ext::state::{CLAP_EXT_STATE, clap_host_state, clap_plugin_state};
use clap_sys::ext::thread_check::{CLAP_EXT_THREAD_CHECK, clap_host_thread_check};
use clap_sys::factory::plugin_factory::{CLAP_PLUGIN_FACTORY_ID, clap_plugin_factory};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{CLAP_PROCESS_ERROR, clap_process};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::{CLAP_VERSION, clap_version_is_compatible};
use crate::hosted::Edit;
use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::ThreadId;

/// Most channels a port may have that we feed or read.
const MAX_CHANNELS: usize = 8;
/// Room for events in one block: notes from the pads, automation, transport.
const MAX_EVENTS: usize = 512;

/// A loaded `.clap` file. One per file however many plugins are made from it: the entry
/// point may be initialised only once at a time.
pub struct Library {
    entry: *const clap_plugin_entry,
    _lib: libloading::Library,
}

// The entry point is a table of thread-safe functions.
unsafe impl Send for Library {}
unsafe impl Sync for Library {}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe {
            if let Some(deinit) = (*self.entry).deinit {
                deinit();
            }
        }
    }
}

static LIBRARIES: Mutex<Vec<(PathBuf, Weak<Library>)>> = Mutex::new(Vec::new());

/// What a scan finds of one plugin in a file.
pub struct Descriptor {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub features: Vec<String>,
}

fn text(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

impl Library {
    pub fn open(path: &Path) -> Result<Arc<Library>, String> {
        let mut libs = LIBRARIES.lock().map_err(|_| "plugin list poisoned".to_string())?;
        libs.retain(|(_, w)| w.strong_count() > 0);
        if let Some(lib) = libs.iter().find(|(p, _)| p == path).and_then(|(_, w)| w.upgrade()) {
            return Ok(lib);
        }
        let lib = unsafe { libloading::Library::new(path) }.map_err(|e| e.to_string())?;
        let entry = unsafe { lib.get::<*const clap_plugin_entry>(b"clap_entry\0") }.map_err(|_| "not a CLAP plugin (no clap_entry)".to_string())?;
        let entry: *const clap_plugin_entry = *entry;
        if entry.is_null() {
            return Err("not a CLAP plugin".into());
        }
        unsafe {
            if !clap_version_is_compatible((*entry).clap_version) {
                return Err("made for an incompatible CLAP version".into());
            }
            let c_path = CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
            let init = (*entry).init.ok_or("plugin has no init")?;
            if !init(c_path.as_ptr()) {
                return Err("the plugin failed to initialise".into());
            }
        }
        let lib = Arc::new(Library { entry, _lib: lib });
        libs.push((path.to_path_buf(), Arc::downgrade(&lib)));
        Ok(lib)
    }

    fn factory(&self) -> Option<&clap_plugin_factory> {
        unsafe {
            let get = (*self.entry).get_factory?;
            let f = get(CLAP_PLUGIN_FACTORY_ID.as_ptr()) as *const clap_plugin_factory;
            f.as_ref()
        }
    }

    /// Every plugin the file offers.
    pub fn descriptors(&self) -> Vec<Descriptor> {
        let Some(f) = self.factory() else { return Vec::new() };
        let (Some(count), Some(get)) = (f.get_plugin_count, f.get_plugin_descriptor) else { return Vec::new() };
        let mut out = Vec::new();
        unsafe {
            for i in 0..count(f) {
                let Some(d) = get(f, i).as_ref() else { continue };
                let mut features = Vec::new();
                let mut fp = d.features;
                while !fp.is_null() && !(*fp).is_null() {
                    features.push(text(*fp));
                    fp = fp.add(1);
                }
                out.push(Descriptor { id: text(d.id), name: text(d.name), vendor: text(d.vendor), features });
            }
        }
        out
    }
}

/// Flags the plugin raises from any thread; the UI handles them on its next frame.
#[repr(C)]
struct HostData {
    /// First, so a `*const clap_host` is also a pointer to this.
    host: clap_host,
    main: ThreadId,
    callback: AtomicBool,
    restart: AtomicBool,
    rescan: AtomicBool,
    gui_closed: AtomicBool,
    /// Size the editor asked for, `width << 32 | height`; 0 for none.
    resize: AtomicU64,
}

unsafe fn data<'a>(host: *const clap_host) -> &'a HostData {
    unsafe { &*(host as *const HostData) }
}

unsafe extern "C" fn get_extension(_: *const clap_host, id: *const c_char) -> *const c_void {
    if id.is_null() {
        return null();
    }
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_PARAMS {
        &HOST_PARAMS as *const _ as *const c_void
    } else if id == CLAP_EXT_GUI {
        &HOST_GUI as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE {
        &HOST_STATE as *const _ as *const c_void
    } else if id == CLAP_EXT_LATENCY {
        &HOST_LATENCY as *const _ as *const c_void
    } else if id == CLAP_EXT_LOG {
        &HOST_LOG as *const _ as *const c_void
    } else if id == CLAP_EXT_THREAD_CHECK {
        &HOST_THREAD_CHECK as *const _ as *const c_void
    } else if id == CLAP_EXT_AUDIO_PORTS {
        &HOST_AUDIO_PORTS as *const _ as *const c_void
    } else {
        null()
    }
}
unsafe extern "C" fn request_restart(h: *const clap_host) {
    unsafe { data(h) }.restart.store(true, Ordering::Relaxed);
}
unsafe extern "C" fn request_process(_: *const clap_host) {
    // Plugins are processed on every block anyway.
}
unsafe extern "C" fn request_callback(h: *const clap_host) {
    unsafe { data(h) }.callback.store(true, Ordering::Relaxed);
}

unsafe extern "C" fn params_rescan(h: *const clap_host, _: clap_param_rescan_flags) {
    unsafe { data(h) }.rescan.store(true, Ordering::Relaxed);
}
unsafe extern "C" fn params_clear(_: *const clap_host, _: u32, _: clap_param_clear_flags) {}
unsafe extern "C" fn params_flush(_: *const clap_host) {}
static HOST_PARAMS: clap_host_params = clap_host_params { rescan: Some(params_rescan), clear: Some(params_clear), request_flush: Some(params_flush) };

unsafe extern "C" fn gui_hints(_: *const clap_host) {}
unsafe extern "C" fn gui_resize(h: *const clap_host, w: u32, ht: u32) -> bool {
    unsafe { data(h) }.resize.store((w as u64) << 32 | ht as u64, Ordering::Relaxed);
    true
}
unsafe extern "C" fn gui_show(_: *const clap_host) -> bool {
    true
}
unsafe extern "C" fn gui_closed(h: *const clap_host, _: bool) {
    unsafe { data(h) }.gui_closed.store(true, Ordering::Relaxed);
}
static HOST_GUI: clap_host_gui = clap_host_gui {
    resize_hints_changed: Some(gui_hints),
    request_resize: Some(gui_resize),
    request_show: Some(gui_show),
    request_hide: Some(gui_show),
    closed: Some(gui_closed),
};

unsafe extern "C" fn mark_dirty(_: *const clap_host) {}
static HOST_STATE: clap_host_state = clap_host_state { mark_dirty: Some(mark_dirty) };

unsafe extern "C" fn latency_changed(_: *const clap_host) {}
static HOST_LATENCY: clap_host_latency = clap_host_latency { changed: Some(latency_changed) };

unsafe extern "C" fn log(_: *const clap_host, _severity: clap_log_severity, msg: *const c_char) {
    if cfg!(debug_assertions) {
        eprintln!("[clap] {}", text(msg));
    }
}
static HOST_LOG: clap_host_log = clap_host_log { log: Some(log) };

unsafe extern "C" fn is_main(h: *const clap_host) -> bool {
    std::thread::current().id() == unsafe { data(h) }.main
}
unsafe extern "C" fn is_audio(h: *const clap_host) -> bool {
    std::thread::current().id() != unsafe { data(h) }.main
}
static HOST_THREAD_CHECK: clap_host_thread_check = clap_host_thread_check { is_main_thread: Some(is_main), is_audio_thread: Some(is_audio) };

unsafe extern "C" fn ports_flag(_: *const clap_host, _: u32) -> bool {
    false
}
unsafe extern "C" fn ports_rescan(_: *const clap_host, _: u32) {}
static HOST_AUDIO_PORTS: clap_host_audio_ports = clap_host_audio_ports { is_rescan_flag_supported: Some(ports_flag), rescan: Some(ports_rescan) };

const HOST_NAME: &CStr = c"ResampleOS";
const HOST_VENDOR: &CStr = c"ResampleOS";
const HOST_URL: &CStr = c"https://github.com/DamonianoStudios/ResampleOS";
const HOST_VERSION: &CStr = match CStr::from_bytes_with_nul(concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes()) {
    Ok(v) => v,
    Err(_) => c"0",
};

/// One event of any kind we send, laid out so a list of them can be handed over as is.
#[repr(C)]
#[derive(Clone, Copy)]
union Event {
    header: clap_event_header,
    note: clap_event_note,
    param: clap_event_param_value,
}

unsafe extern "C" fn in_size(list: *const clap_input_events) -> u32 {
    unsafe { (*((*list).ctx as *const Vec<Event>)).len() as u32 }
}
unsafe extern "C" fn in_get(list: *const clap_input_events, i: u32) -> *const clap_event_header {
    let evs = unsafe { &*((*list).ctx as *const Vec<Event>) };
    evs.get(i as usize).map_or(null(), |e| unsafe { &e.header } as *const _)
}


struct OutSink<'a> {
    edits: &'a mut Vec<Edit>,
    ranges: &'a [Range],
}

unsafe extern "C" fn out_push(list: *const clap_output_events, ev: *const clap_event_header) -> bool {
    let sink = unsafe { &mut *((*list).ctx as *mut OutSink) };
    let ev = unsafe { &*ev };
    if ev.space_id != CLAP_CORE_EVENT_SPACE_ID || sink.edits.len() >= sink.edits.capacity() {
        return true;
    }
    match ev.type_ {
        CLAP_EVENT_PARAM_VALUE => {
            let p = unsafe { &*(ev as *const _ as *const clap_event_param_value) };
            let v = range_of(sink.ranges, p.param_id).map_or(p.value, |r| r.normalize(p.value));
            sink.edits.push(Edit::Value(p.param_id, v));
        }
        CLAP_EVENT_PARAM_GESTURE_END => {
            let p = unsafe { &*(ev as *const _ as *const clap_event_param_gesture) };
            sink.edits.push(Edit::End(p.param_id));
        }
        _ => {}
    }
    true
}

/// A parameter's plain range, for converting to and from lane values, and the cookie
/// the plugin wants back with every event for it (some, such as JUCE-made plugins,
/// rely on it rather than the id).
#[derive(Clone, Copy)]
struct Range {
    id: u32,
    min: f64,
    max: f64,
    stepped: bool,
    cookie: *mut c_void,
}

impl Range {
    fn normalize(&self, plain: f64) -> f64 {
        if self.max > self.min { ((plain - self.min) / (self.max - self.min)).clamp(0.0, 1.0) } else { 0.0 }
    }
    fn plain(&self, v: f64) -> f64 {
        let x = self.min + v.clamp(0.0, 1.0) * (self.max - self.min);
        if self.stepped { x.round() } else { x }
    }
}

fn range_of(ranges: &[Range], id: u32) -> Option<&Range> {
    ranges.binary_search_by_key(&id, |r| r.id).ok().map(|i| &ranges[i])
}

/// A parameter as the UI lists it.
#[derive(Clone, Debug)]
pub struct ParamInfo {
    pub id: u32,
    pub name: String,
    pub automatable: bool,
}

/// One instance of a CLAP plugin.
pub struct ClapPlugin {
    plugin: *const clap_plugin,
    host: Box<HostData>,
    params: *const clap_plugin_params,
    state: *const clap_plugin_state,
    pub(crate) gui: *const clap_plugin_gui,
    pub name: String,
    ranges: Vec<Range>,
    /// Channel counts of the plugin's audio ports.
    ins: Vec<u32>,
    outs: Vec<u32>,
    /// Buffers for the channels and ports the engine does not feed or read.
    spare: Vec<Vec<f32>>,
    events: Vec<Event>,
    edits: Vec<Edit>,
    sample_rate: f64,
    active: bool,
    started: bool,
    steady: i64,
    tempo: f64,
    playing: bool,
    /// Kept last, so the library outlives the instance made from it.
    _lib: Arc<Library>,
}

// The plugin is only touched with the mutex it lives behind held, from the UI thread
// (as the CLAP main thread) or the audio thread.
unsafe impl Send for ClapPlugin {}

impl ClapPlugin {
    pub fn load(path: &Path, id: Option<&str>, sample_rate: f64, max_block: usize) -> Result<ClapPlugin, String> {
        let lib = Library::open(path)?;
        let descs = lib.descriptors();
        let desc = match id {
            Some(id) => descs.iter().find(|d| d.id == id),
            None => descs.first(),
        }
        .ok_or("the file holds no such plugin")?;
        let factory = lib.factory().ok_or("the file has no plugin factory")?;
        let mut host = Box::new(HostData {
            host: clap_host {
                clap_version: CLAP_VERSION,
                host_data: null_mut(),
                name: HOST_NAME.as_ptr(),
                vendor: HOST_VENDOR.as_ptr(),
                url: HOST_URL.as_ptr(),
                version: HOST_VERSION.as_ptr(),
                get_extension: Some(get_extension),
                request_restart: Some(request_restart),
                request_process: Some(request_process),
                request_callback: Some(request_callback),
            },
            main: std::thread::current().id(),
            callback: AtomicBool::new(false),
            restart: AtomicBool::new(false),
            rescan: AtomicBool::new(false),
            gui_closed: AtomicBool::new(false),
            resize: AtomicU64::new(0),
        });
        host.host.host_data = &mut *host as *mut HostData as *mut c_void;
        let c_id = CString::new(desc.id.clone()).map_err(|e| e.to_string())?;
        let plugin = unsafe {
            let create = factory.create_plugin.ok_or("factory cannot create plugins")?;
            create(factory, &host.host, c_id.as_ptr())
        };
        if plugin.is_null() {
            return Err("the plugin could not be created".into());
        }
        let ext = |id: &CStr| unsafe { (*plugin).get_extension.map_or(null(), |g| g(plugin, id.as_ptr())) };
        let mut me = ClapPlugin {
            plugin,
            params: null(),
            state: null(),
            gui: null(),
            host,
            name: desc.name.clone(),
            ranges: Vec::new(),
            ins: Vec::new(),
            outs: Vec::new(),
            spare: Vec::new(),
            events: Vec::with_capacity(MAX_EVENTS),
            edits: Vec::with_capacity(256),
            sample_rate,
            active: false,
            started: false,
            steady: 0,
            tempo: 120.0,
            playing: false,
            _lib: lib,
        };
        unsafe {
            let init = (*plugin).init.ok_or("plugin has no init")?;
            if !init(plugin) {
                // Destroyed by Drop, which knows it was never activated.
                return Err("the plugin failed to initialise".into());
            }
        }
        me.params = ext(CLAP_EXT_PARAMS) as *const clap_plugin_params;
        me.state = ext(CLAP_EXT_STATE) as *const clap_plugin_state;
        me.gui = ext(CLAP_EXT_GUI) as *const clap_plugin_gui;
        let ports = ext(CLAP_EXT_AUDIO_PORTS) as *const clap_plugin_audio_ports;
        (me.ins, me.outs) = unsafe { port_layout(plugin, ports) };
        let spares = me.ins.iter().chain(&me.outs).map(|&c| c as usize).sum::<usize>();
        me.spare = (0..spares).map(|_| vec![0.0; max_block]).collect();
        me.scan_ranges();
        unsafe {
            let activate = (*plugin).activate.ok_or("plugin has no activate")?;
            if !activate(plugin, sample_rate, 1, max_block as u32) {
                return Err("the plugin refused to start".into());
            }
        }
        me.active = true;
        Ok(me)
    }

    fn scan_ranges(&mut self) {
        self.ranges = self.param_infos_raw().into_iter().map(|(i, _)| i).collect();
        self.ranges.sort_by_key(|r| r.id);
    }

    fn param_infos_raw(&self) -> Vec<(Range, ParamInfo)> {
        let Some(params) = (unsafe { self.params.as_ref() }) else { return Vec::new() };
        let (Some(count), Some(get_info)) = (params.count, params.get_info) else { return Vec::new() };
        let mut out = Vec::new();
        unsafe {
            for i in 0..count(self.plugin) {
                let mut info: clap_param_info = std::mem::zeroed();
                if !get_info(self.plugin, i, &mut info) {
                    continue;
                }
                let name = CStr::from_ptr(info.name.as_ptr()).to_string_lossy().into_owned();
                let range = Range { id: info.id, min: info.min_value, max: info.max_value, stepped: info.flags & CLAP_PARAM_IS_STEPPED != 0, cookie: info.cookie };
                let automatable = info.flags & CLAP_PARAM_IS_AUTOMATABLE != 0 && info.flags & (CLAP_PARAM_IS_READONLY | CLAP_PARAM_IS_HIDDEN) == 0;
                out.push((range, ParamInfo { id: info.id, name, automatable }));
            }
        }
        out
    }

    /// The plugin's parameters, in its own order.
    pub fn parameters(&self) -> Vec<ParamInfo> {
        self.param_infos_raw().into_iter().map(|(_, p)| p).collect()
    }

    /// A parameter's normalized value.
    pub fn get_parameter(&self, id: u32) -> Option<f64> {
        let params = unsafe { self.params.as_ref()? };
        let mut v = 0.0;
        unsafe { params.get_value?(self.plugin, id, &mut v) }.then_some(())?;
        Some(range_of(&self.ranges, id).map_or(v, |r| r.normalize(v)))
    }

    /// A normalized value as the plugin writes it: "440 Hz", "-6.0 dB".
    pub fn format_parameter(&self, id: u32, normalized: f64) -> Option<String> {
        let params = unsafe { self.params.as_ref()? };
        let plain = range_of(&self.ranges, id)?.plain(normalized);
        let mut buf = [0 as c_char; 64];
        unsafe { params.value_to_text?(self.plugin, id, plain, buf.as_mut_ptr(), buf.len() as u32) }.then_some(())?;
        Some(unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
    }

    /// Queues a note for the next block. Safe on the audio thread: never allocates.
    pub fn queue_note(&mut self, on: bool, key: u8, velocity: f64, offset: u32) {
        if self.events.len() >= MAX_EVENTS {
            return;
        }
        let size = std::mem::size_of::<clap_event_note>() as u32;
        let type_ = if on { CLAP_EVENT_NOTE_ON } else { CLAP_EVENT_NOTE_OFF };
        let header = clap_event_header { size, time: offset, space_id: CLAP_CORE_EVENT_SPACE_ID, type_, flags: 0 };
        self.events.push(Event { note: clap_event_note { header, note_id: -1, port_index: 0, channel: 0, key: key as i16, velocity } });
    }

    /// Queues a parameter change (normalized) for the start of the next block.
    pub fn queue_parameter(&mut self, id: u32, normalized: f64) {
        if self.events.len() >= MAX_EVENTS {
            return;
        }
        let Some(r) = range_of(&self.ranges, id) else { return };
        let size = std::mem::size_of::<clap_event_param_value>() as u32;
        let header = clap_event_header { size, time: 0, space_id: CLAP_CORE_EVENT_SPACE_ID, type_: CLAP_EVENT_PARAM_VALUE, flags: 0 };
        let param = clap_event_param_value { header, param_id: id, cookie: r.cookie, note_id: -1, port_index: -1, channel: -1, key: -1, value: r.plain(normalized) };
        self.events.push(Event { param });
    }

    pub fn set_transport(&mut self, tempo: f64, playing: bool) {
        self.tempo = tempo;
        self.playing = playing;
    }

    /// Renders one block. `inputs` and `outputs` are the engine's stereo pair; ports and
    /// channels beyond them get silence in and are thrown away out.
    pub fn process(&mut self, n: usize, inputs: &mut [Vec<f32>], outputs: &mut [Vec<f32>]) -> bool {
        if !self.active {
            return false;
        }
        unsafe {
            if !self.started {
                let Some(start) = (*self.plugin).start_processing else { return false };
                if !start(self.plugin) {
                    return false;
                }
                self.started = true;
            }
        }
        // Events must be in time order; notes come in order of arrival. Insertion sort:
        // the list is short, nearly sorted, and must not allocate.
        for i in 1..self.events.len() {
            let mut j = i;
            while j > 0 && unsafe { self.events[j - 1].header.time > self.events[j].header.time } {
                self.events.swap(j - 1, j);
                j -= 1;
            }
        }
        let mut spare = self.spare.iter_mut();
        let mut ptrs_in = [[null_mut::<f32>(); MAX_CHANNELS]; 4];
        let mut ptrs_out = [[null_mut::<f32>(); MAX_CHANNELS]; 4];
        let mut fill = |ptrs: &mut [[*mut f32; MAX_CHANNELS]; 4], ports: &[u32], main: &mut [Vec<f32>], zero: bool| {
            for (port, &channels) in ports.iter().enumerate().take(4) {
                for ch in 0..(channels as usize).min(MAX_CHANNELS) {
                    let buf = spare.next();
                    ptrs[port][ch] = if port == 0 && ch < main.len() {
                        main[ch].as_mut_ptr()
                    } else if let Some(b) = buf {
                        if zero {
                            b[..n].fill(0.0);
                        }
                        b.as_mut_ptr()
                    } else {
                        null_mut()
                    };
                }
            }
        };
        fill(&mut ptrs_in, &self.ins, inputs, true);
        fill(&mut ptrs_out, &self.outs, outputs, false);
        let buffer = |ptrs: &mut [*mut f32; MAX_CHANNELS], channels: u32| clap_audio_buffer {
            data32: ptrs.as_mut_ptr(),
            data64: null_mut(),
            channel_count: channels.min(MAX_CHANNELS as u32),
            latency: 0,
            constant_mask: 0,
        };
        let mut ins = [buffer(&mut ptrs_in[0], 0); 4];
        let mut outs = [buffer(&mut ptrs_out[0], 0); 4];
        for (k, &c) in self.ins.iter().enumerate().take(4) {
            ins[k] = buffer(&mut ptrs_in[k], c);
        }
        for (k, &c) in self.outs.iter().enumerate().take(4) {
            outs[k] = buffer(&mut ptrs_out[k], c);
        }
        let flags = CLAP_TRANSPORT_HAS_TEMPO | CLAP_TRANSPORT_HAS_TIME_SIGNATURE | if self.playing { CLAP_TRANSPORT_IS_PLAYING } else { 0 };
        let transport = clap_event_transport {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_transport>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_TRANSPORT,
                flags: 0,
            },
            flags,
            song_pos_beats: 0,
            song_pos_seconds: 0,
            tempo: self.tempo,
            tempo_inc: 0.0,
            loop_start_beats: 0,
            loop_end_beats: 0,
            loop_start_seconds: 0,
            loop_end_seconds: 0,
            bar_start: 0,
            bar_number: 0,
            tsig_num: 4,
            tsig_denom: 4,
        };
        let in_events = clap_input_events { ctx: &self.events as *const Vec<Event> as *mut c_void, size: Some(in_size), get: Some(in_get) };
        let mut sink = OutSink { edits: &mut self.edits, ranges: &self.ranges };
        let out_events = clap_output_events { ctx: &mut sink as *mut OutSink as *mut c_void, try_push: Some(out_push) };
        let process = clap_process {
            steady_time: self.steady,
            frames_count: n as u32,
            transport: &transport,
            audio_inputs: ins.as_ptr(),
            audio_outputs: outs.as_mut_ptr(),
            audio_inputs_count: self.ins.len().min(4) as u32,
            audio_outputs_count: self.outs.len().min(4) as u32,
            in_events: &in_events,
            out_events: &out_events,
        };
        let status = unsafe { (*self.plugin).process.map_or(CLAP_PROCESS_ERROR, |p| p(self.plugin, &process)) };
        self.events.clear();
        self.steady += n as i64;
        // A mono output feeds both sides.
        if self.outs.first() == Some(&1) && outputs.len() > 1 {
            let (l, r) = outputs.split_at_mut(1);
            r[0][..n].copy_from_slice(&l[0][..n]);
        }
        status != CLAP_PROCESS_ERROR
    }

    /// Ends processing; call on the audio thread, before the plugin is let go of there.
    /// Processing starts again by itself if the plugin is handed back.
    pub fn stop_processing(&mut self) {
        if self.started {
            unsafe {
                if let Some(stop) = (*self.plugin).stop_processing {
                    stop(self.plugin);
                }
            }
            self.started = false;
        }
    }

    /// Moves the plugin's editor made since the last call.
    pub fn take_edits(&mut self) -> Vec<Edit> {
        std::mem::replace(&mut self.edits, Vec::with_capacity(256))
    }

    pub fn save_state(&self) -> Option<Vec<u8>> {
        let state = unsafe { self.state.as_ref()? };
        let mut bytes: Vec<u8> = Vec::new();
        unsafe extern "C" fn write(s: *const clap_ostream, buf: *const c_void, size: u64) -> i64 {
            let out = unsafe { &mut *((*s).ctx as *mut Vec<u8>) };
            out.extend_from_slice(unsafe { std::slice::from_raw_parts(buf as *const u8, size as usize) });
            size as i64
        }
        let stream = clap_ostream { ctx: &mut bytes as *mut Vec<u8> as *mut c_void, write: Some(write) };
        unsafe { state.save?(self.plugin, &stream) }.then_some(bytes)
    }

    pub fn load_state(&mut self, bytes: &[u8]) -> bool {
        let Some(state) = (unsafe { self.state.as_ref() }) else { return false };
        let mut cursor: &[u8] = bytes;
        unsafe extern "C" fn read(s: *const clap_istream, buf: *mut c_void, size: u64) -> i64 {
            let cur = unsafe { &mut *((*s).ctx as *mut &[u8]) };
            let n = (size as usize).min(cur.len());
            unsafe { std::ptr::copy_nonoverlapping(cur.as_ptr(), buf as *mut u8, n) };
            *cur = &cur[n..];
            n as i64
        }
        let stream = clap_istream { ctx: &mut cursor as *mut &[u8] as *mut c_void, read: Some(read) };
        let ok = unsafe { state.load.is_some_and(|l| l(self.plugin, &stream)) };
        self.scan_ranges();
        ok
    }

    /// Main-thread housekeeping the plugin asked for since the last frame.
    pub fn service(&mut self) {
        if self.host.callback.swap(false, Ordering::Relaxed) {
            unsafe {
                if let Some(f) = (*self.plugin).on_main_thread {
                    f(self.plugin);
                }
            }
        }
        if self.host.rescan.swap(false, Ordering::Relaxed) {
            self.scan_ranges();
        }
        if self.host.restart.swap(false, Ordering::Relaxed) {
            // With the lock held the audio thread is not inside the plugin.
            unsafe {
                if self.started {
                    if let Some(stop) = (*self.plugin).stop_processing {
                        stop(self.plugin);
                    }
                    self.started = false;
                }
                if self.active
                    && let Some(d) = (*self.plugin).deactivate
                {
                    d(self.plugin);
                }
                if let Some(a) = (*self.plugin).activate {
                    self.active = a(self.plugin, self.sample_rate, 1, crate::engine::MAX_BLOCK as u32);
                }
            }
        }
    }

    pub(crate) fn raw(&self) -> *const clap_plugin {
        self.plugin
    }

    pub(crate) fn take_gui_closed(&self) -> bool {
        self.host.gui_closed.swap(false, Ordering::Relaxed)
    }

    pub(crate) fn take_resize(&self) -> Option<(u32, u32)> {
        let v = self.host.resize.swap(0, Ordering::Relaxed);
        (v != 0).then_some(((v >> 32) as u32, v as u32))
    }
}

impl Drop for ClapPlugin {
    fn drop(&mut self) {
        unsafe {
            if self.started
                && let Some(stop) = (*self.plugin).stop_processing
            {
                stop(self.plugin);
            }
            if self.active
                && let Some(d) = (*self.plugin).deactivate
            {
                d(self.plugin);
            }
            if let Some(d) = (*self.plugin).destroy {
                d(self.plugin);
            }
        }
    }
}

/// Channel counts of each input and output port.
unsafe fn port_layout(plugin: *const clap_plugin, ports: *const clap_plugin_audio_ports) -> (Vec<u32>, Vec<u32>) {
    let Some(ports) = (unsafe { ports.as_ref() }) else {
        // No port extension: assume a stereo effect.
        return (vec![2], vec![2]);
    };
    let list = |input: bool| {
        let (Some(count), Some(get)) = (ports.count, ports.get) else { return Vec::new() };
        unsafe {
            (0..count(plugin, input))
                .map(|i| {
                    let mut info: clap_audio_port_info = std::mem::zeroed();
                    if get(plugin, i, input, &mut info) { info.channel_count } else { 0 }
                })
                .collect()
        }
    };
    (list(true), list(false))
}

/// The standard CLAP folders, plus any in `CLAP_PATH`.
pub fn standard_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(paths) = std::env::var("CLAP_PATH") {
        dirs.extend(std::env::split_paths(&paths));
    }
    #[cfg(windows)]
    {
        if let Ok(common) = std::env::var("COMMONPROGRAMFILES") {
            dirs.push(PathBuf::from(common).join("CLAP"));
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Programs").join("Common").join("CLAP"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/CLAP"));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        dirs.push(PathBuf::from("/usr/lib/clap"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".clap"));
        }
    }
    dirs
}

/// Every `.clap` file (or bundle) under `roots`.
pub fn find_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let path = e.path();
            if is_clap(&path) {
                out.push(path);
            } else if depth < 6 && path.is_dir() {
                walk(&path, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    for root in roots {
        walk(root, 0, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

pub fn is_clap(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("clap"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loads the first CLAP instrument installed on this machine, plays a note through it
    /// and round-trips its state. Skipped where none is installed.
    #[test]
    fn hosts_an_installed_instrument() {
        let mut files = find_files(&standard_dirs());
        // Vital first where it is installed: a full synth with a large parameter set.
        files.sort_by_key(|f| !f.to_string_lossy().to_lowercase().contains("vital"));
        let found = files.iter().find_map(|f| {
            let lib = Library::open(f).ok()?;
            let d = lib.descriptors().into_iter().find(|d| d.features.iter().any(|x| x == "instrument"))?;
            Some((f.clone(), d.id))
        });
        let Some((path, id)) = found else {
            eprintln!("no CLAP instrument installed; skipped");
            return;
        };
        let mut p = ClapPlugin::load(&path, Some(&id), 48000.0, 512).expect("load");
        let params = p.parameters();
        assert!(!params.is_empty(), "{} lists no parameters", p.name);
        // A level control if there is one: some parameters (a tempo, say) follow the host.
        let first = params
            .iter()
            .filter(|q| q.automatable)
            .find(|q| q.name.to_lowercase().contains("volume"))
            .or_else(|| params.iter().find(|q| q.automatable))
            .expect("an automatable parameter")
            .clone();
        // Processing happens on another thread, as it does in the app.
        let (mut p, peak) = std::thread::spawn(move || {
            let (mut ins, mut outs) = (vec![vec![0f32; 512]; 2], vec![vec![0f32; 512]; 2]);
            p.queue_note(true, 60, 0.9, 0);
            let mut peak = 0f32;
            for block in 0..60 {
                for o in outs.iter_mut() {
                    o.fill(0.0);
                }
                if block == 30 {
                    p.queue_parameter(first.id, 0.25);
                }
                assert!(p.process(512, &mut ins, &mut outs));
                peak = outs.iter().flatten().fold(peak, |m, v| m.max(v.abs()));
            }
            p.stop_processing();
            (p, peak)
        })
        .join()
        .unwrap();
        assert!(peak > 1e-3, "{} made no sound (peak {peak})", p.name);
        let v = p.get_parameter(first.id).unwrap();
        assert!((v - 0.25).abs() < 0.02, "{} {}: set 0.25, reads {v}", p.name, first.name);
        let text = p.format_parameter(first.id, v).unwrap_or_default();
        let state = p.save_state().expect("state");
        assert!(p.load_state(&state));
        println!("{}: {} parameters, peak {peak:.3}, state {} bytes, {} = {text}", p.name, params.len(), state.len(), first.name);
    }
}
