//! Settings tests, through the public API the app calls.
//!
//! Ported from Writer `config.rs` (21 tests), `commands/settings.rs` (1 test)
//! and `tests/settings-write-order.test.ts` (7 tests); each port names its
//! Writer original. Adapted expectations are marked "Adapted".

use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::paths;

fn open(dir: &Path) -> Settings {
    Settings::open(dir).unwrap().0
}

fn open_with(dir: &Path, text: &str) -> (Settings, Vec<ConfigWarning>) {
    fs::write(dir.join("config"), text).unwrap();
    Settings::open(dir).unwrap()
}

fn config(dir: &Path) -> String {
    fs::read_to_string(dir.join("config")).unwrap_or_default()
}

fn number(value: f64) -> ConfigValue {
    ConfigValue::Number(value)
}

fn text(value: &str) -> ConfigValue {
    ConfigValue::String(value.into())
}

fn list(items: &[&str]) -> ConfigValue {
    ConfigValue::List(items.iter().map(|item| item.to_string()).collect())
}

/// Replaces the global config file with a directory so the next read fails.
fn break_file(path: &Path) {
    let _ = fs::remove_file(path);
    fs::create_dir(path).unwrap();
}

fn repair_file(path: &Path) {
    fs::remove_dir(path).unwrap();
}

// ---------- Criterion 1: schema and defaults ----------

#[test]
fn schema_loads_all_43_keys_with_writer_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    let schema = settings.schema();
    assert_eq!(schema.definitions().len(), 43);
    assert_eq!(settings.merged().len(), 43);

    assert_eq!(settings.get("editor.font-size"), Some(number(16.0)));
    let width = schema.get("editor.content-width").unwrap();
    assert_eq!(width.value_type, ValueType::Range);
    assert_eq!((width.min, width.max), (Some(480.0), Some(1600.0)));
    assert_eq!(settings.get("editor.content-width"), Some(number(734.0)));
    let zoom = schema.get("editor.zoom").unwrap();
    assert_eq!((zoom.min, zoom.max), (Some(50.0), Some(300.0)));
    assert!(zoom.is_global_only());
    assert_eq!(settings.get("editor.zoom"), Some(number(100.0)));
    assert_eq!(settings.get("editor.line-height"), Some(number(1.875)));
    assert_eq!(settings.get("appearance.theme"), Some(text("system")));
    assert_eq!(
        settings.get("files.associations"),
        Some(list(&["*.md", "*.mdx", "*.markdown"]))
    );
    assert_eq!(
        settings.get("telemetry.enabled"),
        Some(ConfigValue::Bool(false))
    );
    assert_eq!(settings.get("not.a-key"), None);
}

/// Ported from Writer `tests/settings-schema.test.ts` (typography and
/// default terminal contracts).
#[test]
fn schema_keeps_writer_typography_and_terminal_contracts() {
    let schema = Schema::embedded().unwrap();
    let fonts: Vec<(&str, &str, &str, ValueType)> = schema
        .definitions()
        .iter()
        .filter(|definition| definition.key.starts_with("fonts."))
        .map(|d| {
            (
                d.key.as_str(),
                d.label.as_str(),
                d.category.as_str(),
                d.value_type,
            )
        })
        .collect();
    assert_eq!(
        fonts,
        [
            ("fonts.ui", "UI font", "Typography", ValueType::Font),
            ("fonts.editor", "Editor font", "Typography", ValueType::Font),
            ("fonts.mono", "Code font", "Typography", ValueType::Font),
        ]
    );
    let prose = r#"-apple-system-body, ui-sans-serif, -apple-system, system-ui, "Segoe UI", Helvetica, "Apple Color Emoji", Arial, sans-serif, "Segoe UI Emoji", "Segoe UI Symbol""#;
    let mono = r#"ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace"#;
    let default_of = |key: &str| schema.get(key).unwrap().default.clone();
    assert_eq!(
        default_of("fonts.ui"),
        text(&format!("\"SF Pro\", {prose}"))
    );
    assert_eq!(
        default_of("fonts.editor"),
        text(&format!("\"SF Pro\", {prose}"))
    );
    assert_eq!(
        default_of("fonts.mono"),
        text(&format!("\"SF Mono\", {mono}"))
    );

    let terminal = schema.get("workspace.default-terminal").unwrap();
    assert_eq!(terminal.value_type, ValueType::String);
    assert_eq!(terminal.placeholder.as_deref(), Some("Platform default"));
    assert_eq!(terminal.normalize, Some(Normalization::Trim));
    assert_eq!(terminal.scope, Some(Scope::Global));
    assert_eq!(terminal.default, text(""));
}

#[test]
fn every_schema_default_matches_its_type_and_bounds() {
    let schema = Schema::embedded().unwrap();
    for definition in schema.definitions() {
        let default = &definition.default;
        let ok = match definition.value_type {
            ValueType::Boolean => default.as_bool().is_some(),
            ValueType::Number => default.as_f64().is_some(),
            ValueType::Range => default
                .as_f64()
                .is_some_and(|value| definition.clamp(value) == value),
            ValueType::Enum => default.as_str().is_some_and(|value| {
                definition
                    .options
                    .as_ref()
                    .is_some_and(|options| options.iter().any(|option| option == value))
            }),
            ValueType::String | ValueType::Color | ValueType::Font => default.as_str().is_some(),
            ValueType::List => default.as_list().is_some(),
        };
        assert!(ok, "{} default {default:?}", definition.key);
    }
}

// ---------- Criterion 2: layers ----------

/// Ported from Writer `test_settings_merge_order`.
#[test]
fn test_settings_merge_order() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    assert_eq!(settings.get("editor.font-size"), Some(number(16.0)));

    settings
        .set("editor.font-size", number(18.0), Layer::Global)
        .unwrap();
    assert_eq!(settings.get("editor.font-size"), Some(number(18.0)));

    settings.load_workspace(workspace.path()).unwrap();
    settings
        .set("editor.font-size", number(20.0), Layer::Workspace)
        .unwrap();
    assert_eq!(settings.get("editor.font-size"), Some(number(20.0)));
    assert_eq!(
        settings.get_global_or_default("editor.font-size"),
        Some(number(18.0))
    );

    settings.clear_workspace();
    assert_eq!(settings.get("editor.font-size"), Some(number(18.0)));
}

/// Ported from Writer `test_settings_workspace_override`.
#[test]
fn test_settings_workspace_override() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let settings = open(dir.path());

    settings
        .set("editor.font-size", number(18.0), Layer::Global)
        .unwrap();
    settings.load_workspace(workspace.path()).unwrap();
    settings
        .set("editor.font-size", number(20.0), Layer::Workspace)
        .unwrap();

    assert_eq!(settings.get("editor.font-size"), Some(number(20.0)));
    assert_eq!(
        fs::read_to_string(paths::workspace_config_path(workspace.path())).unwrap(),
        "editor.font-size = 20\n"
    );
}

/// Ported from Writer `global_setting_lookup_ignores_workspace_overrides`.
/// Adapted: the workspace line is also reported as a `GlobalOnly` warning,
/// and the rejected write is a typed `SettingsError::GlobalOnly`.
#[test]
fn global_setting_lookup_ignores_workspace_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    settings
        .set("workspace.default-terminal", text("Ghostty"), Layer::Global)
        .unwrap();
    let workspace_config = paths::workspace_config_path(workspace.path());
    fs::create_dir_all(workspace_config.parent().unwrap()).unwrap();
    fs::write(
        &workspace_config,
        "workspace.default-terminal = OtherTerminal\n",
    )
    .unwrap();

    let warnings = settings.load_workspace(workspace.path()).unwrap();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].line, 1);
    assert_eq!(warnings[0].kind, WarningKind::GlobalOnly);

    assert_eq!(
        settings.get_global_or_default("workspace.default-terminal"),
        Some(text("Ghostty"))
    );
    assert!(matches!(
        settings.set(
            "workspace.default-terminal",
            text("OtherTerminal"),
            Layer::Workspace
        ),
        Err(SettingsError::GlobalOnly { .. })
    ));
    assert_eq!(
        settings.get("workspace.default-terminal"),
        Some(text("Ghostty"))
    );
    let merged = settings.merged();
    let terminal = merged
        .iter()
        .find(|(key, _)| *key == "workspace.default-terminal");
    assert_eq!(terminal.map(|(_, value)| value), Some(&text("Ghostty")));
}

#[test]
fn workspace_write_without_workspace_is_a_typed_error() {
    let dir = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    assert!(matches!(
        settings.set("editor.font-size", number(20.0), Layer::Workspace),
        Err(SettingsError::NoWorkspace)
    ));
    assert!(matches!(
        settings.set("nope", number(1.0), Layer::Global),
        Err(SettingsError::UnknownKey(_))
    ));
}

// ---------- Criterion 3: format round trip ----------

/// Ported from Writer `test_parse_simple`. Adapted: Writer's schema-less
/// `key`, `number` and `bool` keys become schema keys of those types.
#[test]
fn test_parse_simple() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, warnings) = open_with(
        dir.path(),
        "workspace.default-terminal = value\neditor.tab-size = 42\nappearance.sidebar-visible = true\n",
    );
    assert!(warnings.is_empty());
    assert_eq!(
        settings.get("workspace.default-terminal"),
        Some(text("value"))
    );
    assert_eq!(settings.get("editor.tab-size"), Some(number(42.0)));
    assert_eq!(
        settings.get("appearance.sidebar-visible"),
        Some(ConfigValue::Bool(true))
    );
}

/// Ported from Writer `test_parse_comments_and_blanks`.
#[test]
fn test_parse_comments_and_blanks() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, warnings) = open_with(
        dir.path(),
        "# comment\n\nappearance.theme = dark\n# another comment\n",
    );
    assert!(warnings.is_empty());
    assert_eq!(settings.get("appearance.theme"), Some(text("dark")));
}

/// Ported from Writer `test_parse_dotted_keys`.
#[test]
fn test_parse_dotted_keys() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, _) = open_with(
        dir.path(),
        "editor.font-size = 16\nappearance.theme = dark\n",
    );
    assert_eq!(settings.get("editor.font-size"), Some(number(16.0)));
    assert_eq!(settings.get("appearance.theme"), Some(text("dark")));
}

/// Ported from Writer `test_parse_list_values`. Adapted: `files.exclude` is
/// not a schema key; `files.associations` is the schema's list.
#[test]
fn test_parse_list_values() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, _) = open_with(
        dir.path(),
        "files.associations = *.md\nfiles.associations = *.txt\nfiles.associations = *.org\n",
    );
    assert_eq!(
        settings.get("files.associations"),
        Some(list(&["*.md", "*.txt", "*.org"]))
    );
}

#[test]
fn list_writes_one_line_per_item_and_round_trips_an_empty_list() {
    let dir = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    settings
        .set(
            "files.associations",
            list(&["*.md", " *.txt "]),
            Layer::Global,
        )
        .unwrap();
    assert_eq!(
        config(dir.path()),
        "files.associations = *.md\nfiles.associations = *.txt\n"
    );
    settings
        .set("files.associations", list(&[]), Layer::Global)
        .unwrap();
    assert_eq!(config(dir.path()), "files.associations =\n");
    assert_eq!(open(dir.path()).get("files.associations"), Some(list(&[])));
}

/// Ported from Writer `test_serialize_preserves_comments`. Adapted: asserts
/// the exact text, so order is checked too.
#[test]
fn test_serialize_preserves_comments() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, _) = open_with(
        dir.path(),
        "# My settings\nappearance.theme = dark\n\n# Font settings\neditor.font-size = 14\n",
    );
    settings
        .set("appearance.theme", text("light"), Layer::Global)
        .unwrap();
    settings
        .set("editor.font-size", number(16.0), Layer::Global)
        .unwrap();
    assert_eq!(
        config(dir.path()),
        "# My settings\nappearance.theme = light\n\n# Font settings\neditor.font-size = 16\n"
    );
}

/// Ported from Writer `test_serialize_appends_new_keys`. Adapted: a new key
/// goes to its schema position, not to the end (`editor.tab-size` precedes
/// `appearance.theme` in the schema).
#[test]
fn test_serialize_appends_new_keys() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, _) = open_with(dir.path(), "appearance.theme = dark\n");
    settings
        .set("editor.tab-size", number(4.0), Layer::Global)
        .unwrap();
    settings
        .set("search.max-results", number(20.0), Layer::Global)
        .unwrap();
    assert_eq!(
        config(dir.path()),
        "editor.tab-size = 4\nappearance.theme = dark\nsearch.max-results = 20\n"
    );
}

/// Defect fix: Writer appended new keys in `HashMap` order. Cerata inserts
/// them in schema order whatever the write order, keeps a comment attached
/// to the key below it, and leaves unknown lines alone.
#[test]
fn new_keys_are_inserted_in_schema_order() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, _) = open_with(
        dir.path(),
        "# Theme\nappearance.theme = dark\nlegacy.key = kept\n",
    );
    for (key, value) in [
        ("search.max-results", number(20.0)),
        ("editor.font-size", number(18.0)),
        ("editor.zoom", number(120.0)),
        ("statusbar.show-words", ConfigValue::Bool(false)),
    ] {
        settings.set(key, value, Layer::Global).unwrap();
    }
    assert_eq!(
        config(dir.path()),
        "editor.font-size = 18\neditor.zoom = 120\nstatusbar.show-words = false\n\
         # Theme\nappearance.theme = dark\nsearch.max-results = 20\nlegacy.key = kept\n"
    );
}

/// Ported from Writer `test_remove_key`.
#[test]
fn test_remove_key() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, _) = open_with(
        dir.path(),
        "appearance.theme = dark\neditor.font-size = 14\neditor.line-height = 1.6\n",
    );
    settings.reset("editor.font-size", Layer::Global).unwrap();
    assert_eq!(
        config(dir.path()),
        "appearance.theme = dark\neditor.line-height = 1.6\n"
    );
}

/// Ported from Writer `test_roundtrip`. Adapted: rewriting a key to its
/// current value leaves the file byte-identical, and a change touches only
/// that key's line.
#[test]
fn test_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let original = "# Cerata\neditor.font-size = 16\n\n  # indented note\nappearance.theme = system\nwhat is this\n";
    let (settings, _) = open_with(dir.path(), original);
    let before = settings.merged();

    settings
        .set("editor.font-size", number(16.0), Layer::Global)
        .unwrap();
    assert_eq!(config(dir.path()), original);
    assert_eq!(open(dir.path()).merged(), before);

    settings
        .set("appearance.theme", text("dark"), Layer::Global)
        .unwrap();
    assert_eq!(
        config(dir.path()),
        original.replace("appearance.theme = system", "appearance.theme = dark")
    );
}

#[test]
fn crlf_line_endings_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, _) = open_with(dir.path(), "# note\r\nappearance.theme = dark\r\n");
    settings
        .set("search.max-results", number(20.0), Layer::Global)
        .unwrap();
    assert_eq!(
        config(dir.path()),
        "# note\r\nappearance.theme = dark\r\nsearch.max-results = 20\r\n"
    );
}

/// Ported from Writer `test_value_with_equals_sign`. Adapted: Writer's
/// `template` key is not in the schema, and its value had no `=` in it.
#[test]
fn test_value_with_equals_sign() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, _) = open_with(dir.path(), "workspace.default-terminal = a=b = c\n");
    assert_eq!(
        settings.get("workspace.default-terminal"),
        Some(text("a=b = c"))
    );
}

/// Ported from Writer `test_parse_false_value`. Adapted: `editor.spell-check`
/// is not in the schema.
#[test]
fn test_parse_false_value() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, _) = open_with(dir.path(), "files.insert-final-newline = false\n");
    assert_eq!(
        settings.get("files.insert-final-newline"),
        Some(ConfigValue::Bool(false))
    );
}

/// Ported from Writer `test_parse_float`.
#[test]
fn test_parse_float() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, _) = open_with(dir.path(), "editor.line-height = 1.6\n");
    assert_eq!(settings.get("editor.line-height"), Some(number(1.6)));
}

/// Ported from Writer `terminal_string_setting_preserves_lexical_value_across_reload_and_reset`.
#[test]
fn terminal_string_setting_preserves_lexical_value_across_reload_and_reset() {
    for value in ["TRUE", "00123", "1e3"] {
        let dir = tempfile::tempdir().unwrap();
        let settings = open(dir.path());
        settings
            .set("workspace.default-terminal", text(value), Layer::Global)
            .unwrap();

        let reloaded = open(dir.path());
        assert_eq!(
            reloaded.get("workspace.default-terminal"),
            Some(text(value))
        );

        reloaded
            .reset("workspace.default-terminal", Layer::Global)
            .unwrap();
        let reset = open(dir.path());
        assert_eq!(reset.get("workspace.default-terminal"), Some(text("")));
    }
}

/// Ported from Writer `terminal_string_setting_trims_at_the_settings_boundary`.
#[test]
fn terminal_string_setting_trims_at_the_settings_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    settings
        .set(
            "workspace.default-terminal",
            text("  Ghostty  "),
            Layer::Global,
        )
        .unwrap();
    assert_eq!(
        settings.get("workspace.default-terminal"),
        Some(text("Ghostty"))
    );
    settings
        .set("workspace.default-terminal", text("   "), Layer::Global)
        .unwrap();
    assert_eq!(settings.get("workspace.default-terminal"), Some(text("")));
}

// ---------- Criterion 4: write ordering ----------

/// Ported from `settings-write-order.test.ts`: "serializes backend writes
/// for the same key while updating UI optimistically".
#[test]
fn serializes_writes_for_the_same_key_while_reading_optimistically() {
    let dir = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    let first = settings
        .begin_set("fonts.editor", text("First, serif"), Layer::Global)
        .unwrap();
    let second = settings
        .begin_set("fonts.editor", text("Second, serif"), Layer::Global)
        .unwrap();
    assert_eq!(settings.get("fonts.editor"), Some(text("Second, serif")));
    assert_eq!(config(dir.path()), "");

    assert_eq!(first.commit().unwrap(), WriteOutcome::Applied);
    assert_eq!(config(dir.path()), "fonts.editor = First, serif\n");
    assert_eq!(settings.get("fonts.editor"), Some(text("Second, serif")));

    let outcome = std::thread::spawn(move || second.commit()).join().unwrap();
    assert_eq!(outcome.unwrap(), WriteOutcome::Applied);
    assert_eq!(config(dir.path()), "fonts.editor = Second, serif\n");
    assert_eq!(settings.get("fonts.editor"), Some(text("Second, serif")));
}

/// Ported from `settings-write-order.test.ts`: "reconciles optimistic state
/// to the canonical value returned by the backend". Adapted: validation runs
/// before the optimistic update, so the canonical value is visible at once.
#[test]
fn optimistic_value_is_already_canonical() {
    let dir = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    let pending = settings
        .begin_set(
            "workspace.default-terminal",
            text("  Ghostty  "),
            Layer::Global,
        )
        .unwrap();
    assert_eq!(
        settings.get("workspace.default-terminal"),
        Some(text("Ghostty"))
    );
    pending.commit().unwrap();
    assert_eq!(
        settings.get("workspace.default-terminal"),
        Some(text("Ghostty"))
    );

    settings
        .set("editor.zoom", number(500.0), Layer::Global)
        .unwrap();
    assert_eq!(settings.get("editor.zoom"), Some(number(300.0)));
}

/// Ported from `settings-write-order.test.ts`: "does not republish settings
/// when the backend returns the optimistic value unchanged".
#[test]
fn unchanged_commit_does_not_bump_the_revision() {
    let dir = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    let start = settings.revision();
    let pending = settings
        .begin_set("workspace.default-terminal", text("Ghostty"), Layer::Global)
        .unwrap();
    assert_eq!(settings.revision(), start + 1);
    pending.commit().unwrap();
    assert_eq!(settings.revision(), start + 1);
}

/// Ported from `settings-write-order.test.ts`: "does not republish an
/// unchanged list returned as a fresh IPC array".
#[test]
fn unchanged_list_does_not_bump_the_revision() {
    let dir = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    let start = settings.revision();
    settings
        .set(
            "files.associations",
            list(&["md", "markdown"]),
            Layer::Global,
        )
        .unwrap();
    assert_eq!(settings.revision(), start + 1);
    settings
        .set(
            "files.associations",
            list(&["md", "markdown"]),
            Layer::Global,
        )
        .unwrap();
    assert_eq!(settings.revision(), start + 1);
}

/// Ported from `settings-write-order.test.ts`: "a stale failed write cannot
/// roll back a newer optimistic value".
#[test]
fn stale_failed_write_cannot_roll_back_a_newer_value() {
    let dir = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    let path = settings.global_path();
    let first = settings
        .begin_set("fonts.editor", text("First, serif"), Layer::Global)
        .unwrap();
    let second = settings
        .begin_set("fonts.editor", text("Second, serif"), Layer::Global)
        .unwrap();

    break_file(&path);
    assert!(matches!(first.commit(), Err(SettingsError::Io { .. })));
    assert_eq!(settings.get("fonts.editor"), Some(text("Second, serif")));

    repair_file(&path);
    assert_eq!(second.commit().unwrap(), WriteOutcome::Applied);
    assert_eq!(settings.get("fonts.editor"), Some(text("Second, serif")));
    assert_eq!(config(dir.path()), "fonts.editor = Second, serif\n");
}

/// Ported from `settings-write-order.test.ts`: "the latest failed write
/// rolls back to the last successfully persisted value".
#[test]
fn latest_failed_write_rolls_back_to_the_last_persisted_value() {
    let dir = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    let path = settings.global_path();
    settings
        .set("fonts.editor", text("First, serif"), Layer::Global)
        .unwrap();
    let second = settings
        .begin_set("fonts.editor", text("Second, serif"), Layer::Global)
        .unwrap();
    assert_eq!(settings.get("fonts.editor"), Some(text("Second, serif")));

    break_file(&path);
    assert!(second.commit().is_err());
    assert_eq!(settings.get("fonts.editor"), Some(text("First, serif")));
    repair_file(&path);
}

/// Ported from `settings-write-order.test.ts`: "reset stays ordered behind
/// pending writes and wins in both backend and UI".
#[test]
fn reset_stays_ordered_behind_pending_writes_and_wins() {
    let dir = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    let default = settings.get("fonts.editor");
    let first = settings
        .begin_set("fonts.editor", text("First, serif"), Layer::Global)
        .unwrap();
    let second = settings
        .begin_set("fonts.editor", text("Second, serif"), Layer::Global)
        .unwrap();
    let reset = settings.begin_reset("fonts.editor", Layer::Global).unwrap();
    assert_eq!(settings.get("fonts.editor"), Some(text("Second, serif")));

    first.commit().unwrap();
    assert_eq!(config(dir.path()), "fonts.editor = First, serif\n");
    second.commit().unwrap();
    assert_eq!(config(dir.path()), "fonts.editor = Second, serif\n");
    assert_eq!(settings.get("fonts.editor"), Some(text("Second, serif")));
    assert_eq!(reset.commit().unwrap(), WriteOutcome::Applied);
    assert_eq!(config(dir.path()), "");
    assert_eq!(settings.get("fonts.editor"), default);
}

/// The same invariant when commits arrive out of order (background threads
/// give no ordering guarantee): older writes behind the reset are skipped.
#[test]
fn reset_committed_first_supersedes_older_pending_writes() {
    let dir = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    let default = settings.get("fonts.editor");
    let first = settings
        .begin_set("fonts.editor", text("First, serif"), Layer::Global)
        .unwrap();
    let second = settings
        .begin_set("fonts.editor", text("Second, serif"), Layer::Global)
        .unwrap();
    let reset = settings.begin_reset("fonts.editor", Layer::Global).unwrap();

    assert_eq!(reset.commit().unwrap(), WriteOutcome::Applied);
    assert_eq!(settings.get("fonts.editor"), default);
    assert_eq!(second.commit().unwrap(), WriteOutcome::Superseded);
    assert_eq!(first.commit().unwrap(), WriteOutcome::Superseded);
    assert_eq!(config(dir.path()), "");
    assert_eq!(settings.get("fonts.editor"), default);
}

#[test]
fn dropped_pending_write_cancels_its_optimistic_value() {
    let dir = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    let pending = settings
        .begin_set("editor.font-size", number(30.0), Layer::Global)
        .unwrap();
    assert_eq!(settings.get("editor.font-size"), Some(number(30.0)));
    drop(pending);
    assert_eq!(settings.get("editor.font-size"), Some(number(16.0)));
    assert_eq!(config(dir.path()), "");
}

/// Ported from Writer `test_settings_reset`.
#[test]
fn test_settings_reset() {
    let dir = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    settings
        .set("editor.font-size", number(18.0), Layer::Global)
        .unwrap();
    assert_eq!(settings.get("editor.font-size"), Some(number(18.0)));
    settings.reset("editor.font-size", Layer::Global).unwrap();
    assert_eq!(settings.get("editor.font-size"), Some(number(16.0)));
}

// ---------- Criterion 5: external edits and atomic writes ----------

/// Ported from Writer `stale_settings_instances_merge_global_writes_and_resets`.
#[test]
fn stale_settings_instances_merge_global_writes_and_resets() {
    let dir = tempfile::tempdir().unwrap();
    let first_window = open(dir.path());
    let second_window = open(dir.path());

    first_window
        .set("workspace.default-terminal", text("Ghostty"), Layer::Global)
        .unwrap();
    second_window
        .set(
            "appearance.sidebar-visible",
            ConfigValue::Bool(false),
            Layer::Global,
        )
        .unwrap();

    let after_write = open(dir.path());
    assert_eq!(
        after_write.get("workspace.default-terminal"),
        Some(text("Ghostty"))
    );
    assert_eq!(
        after_write.get("appearance.sidebar-visible"),
        Some(ConfigValue::Bool(false))
    );

    second_window
        .reset("appearance.sidebar-visible", Layer::Global)
        .unwrap();
    let after_reset = open(dir.path());
    assert_eq!(
        after_reset.get("workspace.default-terminal"),
        Some(text("Ghostty"))
    );
}

/// Defect fix (global file): a write re-reads the file and keeps an edit
/// made outside Cerata, which the handle also picks up.
#[test]
fn write_keeps_external_edit_of_the_global_config() {
    let dir = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    settings
        .set("editor.font-size", number(18.0), Layer::Global)
        .unwrap();
    fs::write(
        dir.path().join("config"),
        "# hand edit\neditor.font-size = 18\neditor.tab-size = 8\n",
    )
    .unwrap();

    settings
        .set("editor.zoom", number(110.0), Layer::Global)
        .unwrap();
    assert_eq!(
        config(dir.path()),
        "# hand edit\neditor.font-size = 18\neditor.zoom = 110\neditor.tab-size = 8\n"
    );
    assert_eq!(settings.get("editor.tab-size"), Some(number(8.0)));
}

/// Defect fix: Writer's `set_workspace` rewrote the workspace config from a
/// stale in-memory copy, losing an external edit.
#[test]
fn workspace_write_keeps_external_edit() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    settings.load_workspace(workspace.path()).unwrap();
    settings
        .set("editor.font-size", number(20.0), Layer::Workspace)
        .unwrap();
    let path = paths::workspace_config_path(workspace.path());
    assert_eq!(settings.workspace_path(), Some(path.clone()));
    fs::write(
        &path,
        "editor.font-size = 20\n# team note\nappearance.theme = dark\n",
    )
    .unwrap();

    settings
        .set("editor.line-height", number(1.5), Layer::Workspace)
        .unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "editor.font-size = 20\neditor.line-height = 1.5\n# team note\nappearance.theme = dark\n"
    );
    settings
        .reset("editor.font-size", Layer::Workspace)
        .unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "editor.line-height = 1.5\n# team note\nappearance.theme = dark\n"
    );
    assert_eq!(settings.get("appearance.theme"), Some(text("dark")));
}

/// Ported from Writer `reload_global_treats_missing_as_empty_and_preserves_memory_on_read_error`.
#[test]
fn reload_global_treats_missing_as_empty_and_preserves_memory_on_read_error() {
    let dir = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    settings
        .set("workspace.default-terminal", text("Ghostty"), Layer::Global)
        .unwrap();

    fs::remove_file(settings.global_path()).unwrap();
    settings.reload_global().unwrap();
    assert_eq!(settings.get("workspace.default-terminal"), Some(text("")));

    settings
        .set("workspace.default-terminal", text("Ghostty"), Layer::Global)
        .unwrap();
    break_file(&settings.global_path());

    assert!(settings.reload_global().is_err());
    assert_eq!(
        settings.get("workspace.default-terminal"),
        Some(text("Ghostty"))
    );
    assert!(
        settings
            .set(
                "appearance.sidebar-visible",
                ConfigValue::Bool(false),
                Layer::Global
            )
            .is_err()
    );
    assert_eq!(
        settings.get("appearance.sidebar-visible"),
        Some(ConfigValue::Bool(true))
    );
    assert!(settings.global_path().is_dir());
}

/// Ported from Writer `commands/settings.rs`
/// `global_mutation_and_window_initialization_share_one_process_lock`.
/// Adapted: Writer blocked window initialization behind a writer to avoid
/// reading a half-written file. Atomic replacement makes such a read
/// impossible, so the invariant left to test is the lock's other job: two
/// windows writing the global config at the same time lose no update.
#[test]
fn two_windows_writing_the_global_config_concurrently_lose_no_update() {
    let dir = tempfile::tempdir().unwrap();
    let windows = [open(dir.path()), open(dir.path())];
    let keys = ["editor.font-size", "editor.tab-size"];
    let threads: Vec<_> = windows
        .into_iter()
        .zip(keys)
        .map(|(window, key)| {
            std::thread::spawn(move || {
                for value in 1..=25 {
                    window
                        .set(key, number(f64::from(value)), Layer::Global)
                        .unwrap();
                    // A window opened mid-write always parses a whole file.
                    let (_, warnings) = Settings::open(&global_dir(&window)).unwrap();
                    assert!(warnings.is_empty());
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let reopened = open(dir.path());
    assert_eq!(reopened.get("editor.font-size"), Some(number(25.0)));
    assert_eq!(reopened.get("editor.tab-size"), Some(number(25.0)));
}

fn global_dir(settings: &Settings) -> PathBuf {
    settings.global_path().parent().unwrap().to_path_buf()
}

// ---------- Criterion 6: invalid values ----------

#[test]
fn invalid_value_falls_back_to_default_with_a_line_numbered_warning() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, warnings) = open_with(
        dir.path(),
        "# comment\neditor.font-size = abc\neditor.tab-size = 4\n",
    );
    assert_eq!(
        warnings,
        [ConfigWarning {
            path: dir.path().join("config"),
            line: 2,
            key: "editor.font-size".into(),
            kind: WarningKind::InvalidValue {
                raw: "abc".into(),
                expected: ValueType::Number,
            },
        }]
    );
    assert!(
        warnings[0]
            .to_string()
            .contains(":2: `editor.font-size = abc`")
    );
    assert_eq!(settings.get("editor.font-size"), Some(number(16.0)));
    assert_eq!(settings.get("editor.tab-size"), Some(number(4.0)));
    // Loading never rewrites the file.
    assert_eq!(
        config(dir.path()),
        "# comment\neditor.font-size = abc\neditor.tab-size = 4\n"
    );
}

/// Writer's `lib/zoom.ts` clamps to the schema bounds and reads garbage as
/// the default; Cerata applies that to every range key, on load and write.
#[test]
fn range_values_are_clamped_to_schema_bounds() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, warnings) = open_with(
        dir.path(),
        "editor.zoom = 10\neditor.content-width = 5000\ntheme.dark.contrast = 50\n",
    );
    assert_eq!(settings.get("editor.zoom"), Some(number(50.0)));
    assert_eq!(settings.get("editor.content-width"), Some(number(1600.0)));
    assert_eq!(settings.get("theme.dark.contrast"), Some(number(50.0)));
    let lines: Vec<(usize, &WarningKind)> = warnings.iter().map(|w| (w.line, &w.kind)).collect();
    assert_eq!(
        lines,
        [
            (1, &WarningKind::OutOfRange { raw: "10".into() }),
            (2, &WarningKind::OutOfRange { raw: "5000".into() }),
        ]
    );

    settings
        .set("editor.content-width", number(100.0), Layer::Global)
        .unwrap();
    assert_eq!(settings.get("editor.content-width"), Some(number(480.0)));
}

#[test]
fn invalid_enum_boolean_and_type_mismatch_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, warnings) = open_with(
        dir.path(),
        "appearance.theme = sepia\nappearance.sidebar-visible = yes\n",
    );
    assert_eq!(warnings.len(), 2);
    assert_eq!(settings.get("appearance.theme"), Some(text("system")));
    assert_eq!(
        settings.get("appearance.sidebar-visible"),
        Some(ConfigValue::Bool(true))
    );

    assert!(matches!(
        settings.set("appearance.theme", text("sepia"), Layer::Global),
        Err(SettingsError::InvalidValue { .. })
    ));
    assert!(matches!(
        settings.set("editor.font-size", text("abc"), Layer::Global),
        Err(SettingsError::TypeMismatch {
            expected: ValueType::Number,
            ..
        })
    ));
    assert!(matches!(
        settings.set("editor.font-size", number(f64::NAN), Layer::Global),
        Err(SettingsError::InvalidValue { .. })
    ));
    assert!(matches!(
        settings.set("fonts.ui", text("a\nb = c"), Layer::Global),
        Err(SettingsError::InvalidValue { .. })
    ));
    assert_eq!(
        config(dir.path()),
        "appearance.theme = sepia\nappearance.sidebar-visible = yes\n"
    );
}

#[test]
fn malformed_unknown_and_duplicate_lines_warn_and_survive_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, warnings) = open_with(
        dir.path(),
        "not a pair\nmystery.key = 1\neditor.tab-size = 2\neditor.tab-size = 3\n",
    );
    let kinds: Vec<(usize, &WarningKind)> = warnings.iter().map(|w| (w.line, &w.kind)).collect();
    assert_eq!(
        kinds,
        [
            (1, &WarningKind::Malformed),
            (2, &WarningKind::UnknownKey),
            (4, &WarningKind::Duplicate { first_line: 3 }),
        ]
    );
    assert_eq!(settings.get("editor.tab-size"), Some(number(3.0)));

    settings
        .set("editor.tab-size", number(4.0), Layer::Global)
        .unwrap();
    assert_eq!(
        config(dir.path()),
        "not a pair\nmystery.key = 1\neditor.tab-size = 4\n"
    );
}

// ---------- Criterion 7: paths and skipped migrations ----------

#[test]
fn config_paths_come_from_cerata_paths() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let settings = open(dir.path());
    assert_eq!(settings.global_path(), dir.path().join(paths::CONFIG_FILE));
    assert_eq!(
        global_config_path(),
        paths::global_config_dir().map(|dir| dir.join("config"))
    );
    settings.load_workspace(workspace.path()).unwrap();
    assert_eq!(
        settings.workspace_path(),
        Some(workspace.path().join(".cerata").join("config"))
    );
}

/// Ported from Writer `test_theme_font_migration`. Adapted: the migration
/// is Writer-legacy and skipped. The old keys are unknown keys: they warn,
/// do not feed `fonts.*`, and stay in the file untouched.
#[test]
fn test_theme_font_migration() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = "theme.light.editor-font = Georgia, serif\n\
                  theme.dark.editor-font = Iowan Old Style, serif\n\
                  theme.dark.mono-font = Fira Code, monospace\n";
    let (settings, warnings) = open_with(dir.path(), legacy);
    assert_eq!(warnings.len(), 3);
    assert!(
        warnings
            .iter()
            .all(|warning| warning.kind == WarningKind::UnknownKey)
    );
    let schema = settings.schema();
    for key in ["fonts.ui", "fonts.editor", "fonts.mono"] {
        assert_eq!(
            settings.get(key).as_ref(),
            Some(&schema.get(key).unwrap().default)
        );
    }
    assert_eq!(settings.get("theme.light.editor-font"), None);
    settings
        .set("fonts.editor", text("Palatino, serif"), Layer::Global)
        .unwrap();
    assert!(config(dir.path()).starts_with(legacy));
}

/// Ported from Writer `test_editor_width_migration`. Adapted: skipped like
/// the font migration; `appearance.editor-width` is an unknown key and
/// `editor.content-width` keeps its default.
#[test]
fn test_editor_width_migration() {
    let dir = tempfile::tempdir().unwrap();
    let (settings, warnings) = open_with(dir.path(), "appearance.editor-width = narrow\n");
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].kind, WarningKind::UnknownKey);
    assert_eq!(settings.get("editor.content-width"), Some(number(734.0)));
    assert_eq!(settings.get("appearance.editor-width"), None);
    assert_eq!(config(dir.path()), "appearance.editor-width = narrow\n");
}

#[test]
fn settings_handle_is_shareable_across_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Settings>();
    assert_send_sync::<PendingWrite>();
}
