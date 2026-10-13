//! Embeds the application icon in the Windows executable, so it shows in Explorer and on
//! the taskbar. The window's own icon is set at runtime in `main.rs`.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        // A missing resource compiler should not stop the build; the app just keeps the
        // default executable icon.
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the application icon: {e}");
        }
    }
}
