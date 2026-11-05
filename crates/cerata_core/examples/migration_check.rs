use std::collections::BTreeMap;
use std::path::PathBuf;

use cerata_core::library::LibraryRegistry;
use cerata_core::pins::PinStore;
use cerata_core::session::{SessionStore, legacy_key};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = std::env::args().nth(1).map(PathBuf::from) else {
        return Err("usage: migration_check COPY_OF_DATA_DIR".into());
    };
    let read = |name: &str| -> BTreeMap<String, serde_json::Value> {
        std::fs::read_to_string(dir.join(name))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    };
    let recents: Vec<String> = std::fs::read_to_string(dir.join("recent_workspaces.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let sessions_before = read("sessions.json");
    let pins_before = read("pins.json");
    println!(
        "before: {} recent workspaces, {} session keys, {} pin keys",
        recents.len(),
        sessions_before.len(),
        pins_before.len()
    );

    let registry = LibraryRegistry::new(&dir);
    let libraries = registry.list()?;
    let missing = recents
        .iter()
        .filter(|path| !PathBuf::from(path).is_dir())
        .count();
    println!(
        "registry: {} libraries ({} recent folders gone, skipped)",
        libraries.len(),
        missing
    );

    let sessions = SessionStore::new(&dir);
    let pins = PinStore::new(&dir);
    for library in &libraries {
        let key = library.key();
        let legacy = legacy_key(&library.path);
        let tabs_before = sessions_before
            .get(&legacy)
            .and_then(|s| s.get("tabs"))
            .and_then(|t| t.as_array())
            .map_or(0, Vec::len);
        let pins_before = pins_before
            .get(&legacy)
            .and_then(|p| p.as_array())
            .map_or(0, Vec::len);
        let session = sessions.load(&key)?;
        let tabs_after = session.as_ref().map_or(0, |s| s.tabs.len());
        let restorable = session.as_ref().map_or(0, |s| {
            s.tabs
                .iter()
                .filter(|tab| {
                    tab.location
                        .payload
                        .get("path")
                        .and_then(|p| p.as_str())
                        .is_some_and(|p| PathBuf::from(p).is_file())
                })
                .count()
        });
        let pinned = pins.load(&key)?;
        println!(
            "- {} ({}): tabs {tabs_before} -> {tabs_after} ({restorable} files present), pins {pins_before} -> {}",
            library.name,
            library.id,
            pinned.len()
        );
    }
    let sessions_after = read("sessions.json");
    let pins_after = read("pins.json");
    let relative = |map: &BTreeMap<String, serde_json::Value>| {
        map.keys()
            .filter(|key| cerata_core::library::is_library_id(key))
            .count()
    };
    println!(
        "after: sessions {} keys ({} by library id), pins {} keys ({} by library id)",
        sessions_after.len(),
        relative(&sessions_after),
        pins_after.len(),
        relative(&pins_after)
    );
    Ok(())
}
