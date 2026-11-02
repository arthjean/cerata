//! Embeds the app icon into `cerata.exe`. GPUI's Windows backend registers its
//! window class with icon resource 1 of the running module, so this single
//! resource covers Explorer, the taskbar and the title bar.

fn main() {
    #[cfg(windows)]
    embed_windows_icon();
}

#[cfg(windows)]
fn embed_windows_icon() {
    // Absolute: `rc` runs from OUT_DIR, where a relative path would not resolve.
    const ICON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/brand/cerata.ico");
    println!("cargo:rerun-if-changed={ICON}");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ICON);
    if let Err(e) = res.compile() {
        println!(
            "cargo:warning=winresource: cannot embed {ICON} ({e}); the exe keeps the default icon"
        );
    }
}
