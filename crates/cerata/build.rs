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
    // Read at run time, not with `env!`: a shared target dir reuses the compiled
    // build script across checkouts, which would bake in another checkout's path.
    let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        println!(
            "cargo:warning=winresource: CARGO_MANIFEST_DIR unset; the exe keeps the default icon"
        );
        return;
    };
    let icon = format!("{manifest_dir}/assets/brand/cerata.ico");
    println!("cargo:rerun-if-changed={icon}");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(&icon);
    if let Err(e) = res.compile() {
        println!(
            "cargo:warning=winresource: cannot embed {icon} ({e}); the exe keeps the default icon"
        );
    }
}
