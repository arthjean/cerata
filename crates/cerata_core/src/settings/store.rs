//! The settings handle the app holds: layered values behind a lock, plus a
//! write coordinator that keeps optimistic reads, ordered writes and
//! rollback consistent.
//!
//! Ported from Writer `config.rs` (`Settings`: layers, `get`, `merged`,
//! `set_*`, `reset_*`, `reload_global`), `commands/settings.rs` (the
//! process-wide global config lock) and `src/stores/settings-store.ts`
//! (per-key write lanes, optimistic values, rollback by mutation version).
//! Changes from Writer: every write re-reads its file and replaces it
//! atomically (Writer's `set_workspace` and `reset_workspace` wrote from a
//! stale in-memory copy, losing external edits), and the ordering that the
//! TS store kept with promise chains lives here as per-key sequence numbers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use super::format::{self, ParsedLayer};
use super::schema::{Schema, SettingDef};
use super::{ConfigValue, ConfigWarning, Layer, SettingsError, atomic};
use crate::paths;

/// Serializes every config read-modify-write in the process, across all
/// [`Settings`] handles (one per window), so two windows writing the global
/// file never lose each other's update.
static FILE_IO: Mutex<()> = parking_lot::const_mutex(());

/// Path of the global config: `paths::global_config_dir()/config`.
pub fn global_config_path() -> Option<PathBuf> {
    paths::global_config_dir().map(|dir| dir.join(paths::CONFIG_FILE))
}

/// Shared handle to the merged settings of one window: schema defaults, the
/// global config, then the open workspace's config. Cloning is cheap and
/// every clone sees the same state.
#[derive(Clone)]
pub struct Settings {
    inner: Arc<Inner>,
}

struct Inner {
    schema: &'static Schema,
    state: RwLock<State>,
}

struct State {
    global: LayerState,
    workspace: Option<LayerState>,
    next_seq: u64,
    revision: u64,
}

struct LayerState {
    path: PathBuf,
    /// What the file held after the last successful load or write.
    persisted: HashMap<String, ConfigValue>,
    /// Optimistic value of the latest pending write per key.
    pending: HashMap<String, ConfigValue>,
    lanes: HashMap<String, Lane>,
}

/// Per-key ordering state. Sequence numbers come from one counter per
/// handle, so they order every mutation of a key.
#[derive(Default)]
struct Lane {
    /// Sequence of the newest write begun on this key.
    latest: u64,
    /// Highest sequence whose commit ran (successfully or not). An older
    /// commit arriving later is stale and is skipped.
    applied: u64,
}

impl LayerState {
    fn new(path: PathBuf, persisted: HashMap<String, ConfigValue>) -> Self {
        Self {
            path,
            persisted,
            pending: HashMap::new(),
            lanes: HashMap::new(),
        }
    }

    fn visible(&self, key: &str) -> Option<&ConfigValue> {
        self.pending.get(key).or_else(|| self.persisted.get(key))
    }
}

impl State {
    fn resolve<'a>(&'a self, definition: &'a SettingDef) -> &'a ConfigValue {
        let key = definition.key.as_str();
        let workspace = (!definition.is_global_only())
            .then(|| self.workspace.as_ref()?.visible(key))
            .flatten();
        workspace
            .or_else(|| self.global.visible(key))
            .unwrap_or(&definition.default)
    }

    fn global_or_default<'a>(&'a self, definition: &'a SettingDef) -> &'a ConfigValue {
        self.global
            .visible(&definition.key)
            .unwrap_or(&definition.default)
    }

    fn snapshot(&self, schema: &Schema) -> Vec<ConfigValue> {
        schema
            .definitions()
            .iter()
            .map(|definition| self.resolve(definition).clone())
            .collect()
    }

    /// Bumps the revision when any merged value differs from `before`.
    fn bump_if_changed(&mut self, schema: &Schema, before: &[ConfigValue]) {
        let changed = schema
            .definitions()
            .iter()
            .zip(before)
            .any(|(definition, old)| self.resolve(definition) != old);
        if changed {
            self.revision += 1;
        }
    }

    fn layer_mut(&mut self, layer: Layer) -> Option<&mut LayerState> {
        match layer {
            Layer::Global => Some(&mut self.global),
            Layer::Workspace => self.workspace.as_mut(),
        }
    }

    /// The layer a pending write targets, unless the workspace it was begun
    /// against has since been closed or replaced.
    fn layer_at(&self, layer: Layer, path: &Path) -> Option<&LayerState> {
        match layer {
            Layer::Global => Some(&self.global),
            Layer::Workspace => self.workspace.as_ref(),
        }
        .filter(|state| state.path == path)
    }

    fn layer_at_mut(&mut self, layer: Layer, path: &Path) -> Option<&mut LayerState> {
        self.layer_mut(layer).filter(|state| state.path == path)
    }
}

/// Result of a committed write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    /// The file now holds this write.
    Applied,
    /// A newer write on the same key already ran, so this one was skipped.
    Superseded,
}

impl Settings {
    /// Loads the schema defaults and the global config at
    /// `config_dir/config`. A missing file is an empty layer. Returns the
    /// parse warnings, which are also logged.
    pub fn open(config_dir: &Path) -> Result<(Self, Vec<ConfigWarning>), SettingsError> {
        let schema = Schema::embedded()?;
        let path = config_dir.join(paths::CONFIG_FILE);
        let parsed = {
            let _io = FILE_IO.lock();
            load_layer(&path, schema, Layer::Global)?
        };
        let state = State {
            global: LayerState::new(path, parsed.values),
            workspace: None,
            next_seq: 0,
            revision: 0,
        };
        let settings = Self {
            inner: Arc::new(Inner {
                schema,
                state: RwLock::new(state),
            }),
        };
        Ok((settings, parsed.warnings))
    }

    /// [`Settings::open`] on the OS config directory
    /// ([`global_config_path`]).
    pub fn open_default() -> Result<(Self, Vec<ConfigWarning>), SettingsError> {
        let dir = paths::global_config_dir().ok_or(SettingsError::NoConfigDir)?;
        Self::open(&dir)
    }

    /// The embedded schema, for rendering the preferences UI.
    pub fn schema(&self) -> &'static Schema {
        self.inner.schema
    }

    /// Loads `{root}/.cerata/config` as the workspace layer, replacing any
    /// previous one. On error the previous layer stays in place.
    pub fn load_workspace(&self, root: &Path) -> Result<Vec<ConfigWarning>, SettingsError> {
        let path = paths::workspace_config_path(root);
        let _io = FILE_IO.lock();
        let parsed = load_layer(&path, self.inner.schema, Layer::Workspace)?;
        self.mutate(|state| state.workspace = Some(LayerState::new(path, parsed.values)));
        Ok(parsed.warnings)
    }

    /// Drops the workspace layer.
    pub fn clear_workspace(&self) {
        self.mutate(|state| state.workspace = None);
    }

    /// Re-reads the global config. A missing file is an empty layer; on any
    /// other error the in-memory values are kept.
    pub fn reload_global(&self) -> Result<Vec<ConfigWarning>, SettingsError> {
        let _io = FILE_IO.lock();
        let path = self.global_path();
        let parsed = load_layer(&path, self.inner.schema, Layer::Global)?;
        self.mutate(|state| state.global.persisted = parsed.values);
        Ok(parsed.warnings)
    }

    /// Merged value of `key`: workspace, then global, then the schema
    /// default, with pending writes visible. A global-only key ignores the
    /// workspace. `None` for a key the schema does not declare.
    pub fn get(&self, key: &str) -> Option<ConfigValue> {
        let definition = self.inner.schema.get(key)?;
        Some(self.inner.state.read().resolve(definition).clone())
    }

    /// Value of `key` without any workspace override.
    pub fn get_global_or_default(&self, key: &str) -> Option<ConfigValue> {
        let definition = self.inner.schema.get(key)?;
        Some(
            self.inner
                .state
                .read()
                .global_or_default(definition)
                .clone(),
        )
    }

    /// Every merged value, in schema order.
    pub fn merged(&self) -> Vec<(&'static str, ConfigValue)> {
        let state = self.inner.state.read();
        self.inner
            .schema
            .definitions()
            .iter()
            .map(|definition| (definition.key.as_str(), state.resolve(definition).clone()))
            .collect()
    }

    /// Counter bumped whenever a merged value visibly changes: the UI
    /// re-renders when it moves, and only then.
    pub fn revision(&self) -> u64 {
        self.inner.state.read().revision
    }

    pub fn global_path(&self) -> PathBuf {
        self.inner.state.read().global.path.clone()
    }

    pub fn workspace_path(&self) -> Option<PathBuf> {
        let state = self.inner.state.read();
        state.workspace.as_ref().map(|layer| layer.path.clone())
    }

    /// Validates `value`, shows it immediately through [`Settings::get`] and
    /// returns the pending write. [`PendingWrite::commit`] performs the file
    /// I/O and may run on another thread; dropping it uncommitted cancels it.
    pub fn begin_set(
        &self,
        key: &str,
        value: ConfigValue,
        layer: Layer,
    ) -> Result<PendingWrite, SettingsError> {
        let definition = self.definition(key)?;
        if layer == Layer::Workspace && definition.is_global_only() {
            return Err(SettingsError::GlobalOnly {
                key: definition.key.clone(),
            });
        }
        let value = format::validate(definition, value)?;
        self.begin(definition, layer, Some(value))
    }

    /// Queues the removal of `key` from `layer`. Unlike a set it is not
    /// optimistic: the current value stays visible until the commit.
    pub fn begin_reset(&self, key: &str, layer: Layer) -> Result<PendingWrite, SettingsError> {
        let definition = self.definition(key)?;
        self.begin(definition, layer, None)
    }

    /// [`Settings::begin_set`] then [`PendingWrite::commit`].
    pub fn set(
        &self,
        key: &str,
        value: ConfigValue,
        layer: Layer,
    ) -> Result<WriteOutcome, SettingsError> {
        self.begin_set(key, value, layer)?.commit()
    }

    /// [`Settings::begin_reset`] then [`PendingWrite::commit`].
    pub fn reset(&self, key: &str, layer: Layer) -> Result<WriteOutcome, SettingsError> {
        self.begin_reset(key, layer)?.commit()
    }

    fn definition(&self, key: &str) -> Result<&'static SettingDef, SettingsError> {
        self.inner
            .schema
            .get(key)
            .ok_or_else(|| SettingsError::UnknownKey(key.to_string()))
    }

    fn begin(
        &self,
        definition: &'static SettingDef,
        layer: Layer,
        value: Option<ConfigValue>,
    ) -> Result<PendingWrite, SettingsError> {
        let schema = self.inner.schema;
        let key = definition.key.as_str();
        let mut state = self.inner.state.write();
        let before = state.snapshot(schema);
        let seq = state.next_seq + 1;
        let target = state.layer_mut(layer).ok_or(SettingsError::NoWorkspace)?;
        target.lanes.entry(key.to_string()).or_default().latest = seq;
        if let Some(value) = &value {
            target.pending.insert(key.to_string(), value.clone());
        }
        let path = target.path.clone();
        state.next_seq = seq;
        state.bump_if_changed(schema, &before);
        Ok(PendingWrite {
            settings: self.clone(),
            layer,
            path,
            key,
            value,
            seq,
            finished: false,
        })
    }

    fn mutate(&self, change: impl FnOnce(&mut State)) {
        let schema = self.inner.schema;
        let mut state = self.inner.state.write();
        let before = state.snapshot(schema);
        change(&mut state);
        state.bump_if_changed(schema, &before);
    }
}

/// A begun set or reset, waiting for its file write. Writes on one key take
/// effect in the order they were begun, whatever the order of their commits:
/// a commit that arrives after a newer one on the same key is skipped. When
/// the newest write on a key finishes, the optimistic value gives way to what
/// the file holds, so a failure rolls back to the last persisted value while
/// a stale failure leaves the newer value alone.
#[must_use = "a pending write is canceled when dropped"]
pub struct PendingWrite {
    settings: Settings,
    layer: Layer,
    path: PathBuf,
    key: &'static str,
    value: Option<ConfigValue>,
    seq: u64,
    finished: bool,
}

impl PendingWrite {
    /// Re-reads the config file, applies this write to its text and replaces
    /// the file atomically. Blocks while another config write is in flight.
    pub fn commit(mut self) -> Result<WriteOutcome, SettingsError> {
        self.finished = true;
        let inner = &self.settings.inner;
        let _io = FILE_IO.lock();

        let superseded = inner
            .state
            .read()
            .layer_at(self.layer, &self.path)
            .and_then(|layer| layer.lanes.get(self.key))
            .is_some_and(|lane| lane.applied > self.seq);
        if superseded {
            return Ok(WriteOutcome::Superseded);
        }

        let result = persist(
            &self.path,
            inner.schema,
            self.layer,
            self.key,
            self.value.as_ref(),
        );

        let mut state = inner.state.write();
        let before = state.snapshot(inner.schema);
        let outcome = match (state.layer_at_mut(self.layer, &self.path), result) {
            (Some(layer), result) => {
                let lane = layer.lanes.entry(self.key.to_string()).or_default();
                lane.applied = lane.applied.max(self.seq);
                let is_latest = lane.latest == self.seq;
                let outcome = result.map(|persisted| {
                    layer.persisted = persisted;
                    WriteOutcome::Applied
                });
                if is_latest {
                    layer.pending.remove(self.key);
                }
                outcome
            }
            // The workspace was closed or replaced meanwhile: the old file
            // was written, but it no longer feeds this handle.
            (None, result) => result.map(|_| WriteOutcome::Applied),
        };
        state.bump_if_changed(inner.schema, &before);
        outcome
    }
}

impl Drop for PendingWrite {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let (layer, key, seq) = (self.layer, self.key, self.seq);
        let path = self.path.clone();
        self.settings.mutate(|state| {
            if let Some(target) = state.layer_at_mut(layer, &path)
                && target.lanes.get(key).is_some_and(|lane| lane.latest == seq)
            {
                target.pending.remove(key);
            }
        });
    }
}

fn read_config(path: &Path) -> Result<String, SettingsError> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(raw),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(SettingsError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Reads and parses one layer, logging its warnings.
fn load_layer(path: &Path, schema: &Schema, layer: Layer) -> Result<ParsedLayer, SettingsError> {
    let raw = read_config(path)?;
    let parsed = format::parse(&raw, path, schema, layer);
    for warning in &parsed.warnings {
        log::warn!("{warning}");
    }
    Ok(parsed)
}

/// Read-modify-write of one key. Re-reading first keeps edits made to the
/// file outside Cerata. Returns the layer's values as now persisted.
fn persist(
    path: &Path,
    schema: &Schema,
    layer: Layer,
    key: &str,
    value: Option<&ConfigValue>,
) -> Result<HashMap<String, ConfigValue>, SettingsError> {
    let raw = read_config(path)?;
    let next = format::edit(&raw, key, value, schema);
    if next != raw {
        atomic::write_atomic(path, next.as_bytes()).map_err(|source| SettingsError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(format::parse(&next, path, schema, layer).values)
}
