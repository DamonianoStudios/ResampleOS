//! A CLAP plugin's editor in a window of its own (Windows).
//!
//! The window is a plain Win32 top-level window that the plugin's view is parented into.
//! It lives on the UI thread, whose message loop (eframe's) also delivers its messages.
//! Closing it, and resizes from either side, only raise flags that [`ClapEditor::service`]
//! acts on each frame.

use super::ClapPlugin;
use clap_sys::ext::gui::{CLAP_WINDOW_API_WIN32, clap_window, clap_window_handle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const CLASS: &[u16] = &[
    b'R' as u16, b'S' as u16, b'O' as u16, b'S' as u16, b'C' as u16, b'l' as u16, b'a' as u16, b'p' as u16, 0,
];

/// What the window procedure reports back.
#[derive(Default)]
struct WinState {
    close: AtomicBool,
    resized: AtomicBool,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const WinState;
    if let Some(state) = unsafe { state.as_ref() } {
        match msg {
            WM_CLOSE => {
                // Torn down from the UI thread, with the plugin told first.
                state.close.store(true, Ordering::Relaxed);
                return 0;
            }
            WM_SIZE => state.resized.store(true, Ordering::Relaxed),
            _ => {}
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn register() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: CLASS.as_ptr(),
        };
        RegisterClassW(&wc);
    });
}

pub struct ClapEditor {
    hwnd: HWND,
    state: Box<WinState>,
    handle: Arc<Mutex<ClapPlugin>>,
    resizable: bool,
    open: bool,
}

/// The outer window size for a client area of `w` by `h`.
fn outer(style: u32, w: u32, h: u32) -> (i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 };
    unsafe { AdjustWindowRectEx(&mut r, style, 0, 0) };
    (r.right - r.left, r.bottom - r.top)
}

impl ClapEditor {
    pub fn open(handle: Arc<Mutex<ClapPlugin>>, title: &str) -> Result<ClapEditor, String> {
        let p = handle.lock().map_err(|_| "plugin lock poisoned")?;
        let gui = unsafe { p.gui.as_ref() }.ok_or("this plugin has no editor")?;
        let plugin = p.raw();
        let api = CLAP_WINDOW_API_WIN32.as_ptr();
        unsafe {
            if !gui.is_api_supported.is_some_and(|f| f(plugin, api, false)) {
                return Err("this plugin's editor cannot be embedded in a window".into());
            }
            if !gui.create.is_some_and(|f| f(plugin, api, false)) {
                return Err("the plugin could not create its editor".into());
            }
        }
        let (mut w, mut h) = (640u32, 400u32);
        let resizable = unsafe { gui.can_resize.is_some_and(|f| f(plugin)) };
        unsafe {
            if let Some(get) = gui.get_size {
                get(plugin, &mut w, &mut h);
            }
        }
        register();
        let style = if resizable { WS_OVERLAPPEDWINDOW } else { WS_OVERLAPPEDWINDOW & !(WS_THICKFRAME | WS_MAXIMIZEBOX) };
        let (ow, oh) = outer(style, w.max(64), h.max(64));
        let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let state = Box::new(WinState::default());
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                CLASS.as_ptr(),
                title.as_ptr(),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                ow,
                oh,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            unsafe {
                if let Some(f) = gui.destroy {
                    f(plugin);
                }
            }
            return Err("could not create a window for the editor".into());
        }
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, &*state as *const WinState as isize) };
        let window = clap_window { api, specific: clap_window_handle { win32: hwnd as *mut _ } };
        unsafe {
            if !gui.set_parent.is_some_and(|f| f(plugin, &window)) {
                if let Some(f) = gui.destroy {
                    f(plugin);
                }
                DestroyWindow(hwnd);
                return Err("the plugin's editor would not attach to its window".into());
            }
            if let Some(f) = gui.show {
                f(plugin);
            }
            ShowWindow(hwnd, SW_SHOW);
        }
        drop(p);
        state.resized.store(false, Ordering::Relaxed);
        Ok(ClapEditor { hwnd, state, handle, resizable, open: true })
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// True once the user has closed the window (or the plugin closed its editor).
    pub fn closed_by_user(&self) -> bool {
        !self.open
            || self.state.close.load(Ordering::Relaxed)
            || self.handle.try_lock().is_ok_and(|p| p.take_gui_closed())
    }

    /// Applies resizes asked for by the plugin or made by the user.
    pub fn service(&mut self) {
        if !self.open {
            return;
        }
        let Ok(p) = self.handle.try_lock() else { return };
        let Some(gui) = (unsafe { p.gui.as_ref() }) else { return };
        let plugin = p.raw();
        let style = unsafe { GetWindowLongW(self.hwnd, GWL_STYLE) } as u32;
        if let Some((w, h)) = p.take_resize() {
            let (ow, oh) = outer(style, w, h);
            unsafe {
                SetWindowPos(self.hwnd, std::ptr::null_mut(), 0, 0, ow, oh, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
                if let Some(f) = gui.set_size {
                    f(plugin, w, h);
                }
            }
            self.state.resized.store(false, Ordering::Relaxed);
        } else if self.resizable && self.state.resized.swap(false, Ordering::Relaxed) {
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            unsafe { GetClientRect(self.hwnd, &mut r) };
            let (mut w, mut h) = ((r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32);
            unsafe {
                if let Some(adjust) = gui.adjust_size {
                    adjust(plugin, &mut w, &mut h);
                }
                if let Some(f) = gui.set_size {
                    f(plugin, w, h);
                }
            }
        }
        drop(p);
    }

    pub fn close(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        if let Ok(p) = self.handle.lock()
            && let Some(gui) = unsafe { p.gui.as_ref() }
        {
            unsafe {
                if let Some(f) = gui.hide {
                    f(p.raw());
                }
                if let Some(f) = gui.destroy {
                    f(p.raw());
                }
            }
        }
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            DestroyWindow(self.hwnd);
        }
    }
}

impl Drop for ClapEditor {
    fn drop(&mut self) {
        self.close();
    }
}
