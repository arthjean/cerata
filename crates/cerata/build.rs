fn main() {
    #[cfg(windows)]
    embed_windows_icon();
}

#[cfg(windows)]
fn embed_windows_icon() {
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
