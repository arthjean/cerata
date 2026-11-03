//! A library's local store (ADR 0009, EP-017): `store.sqlite` in
//! `data_local_dir()/cerata/libraries/<id>/`, never in the notes folder nor
//! in a folder a cloud service syncs.
//!
//! The `.md` files stay the only source of truth: the store keeps what can
//! be rebuilt from them (path, size, modified time, BLAKE3 hash and title of
//! every note), so deleting it loses nothing but history.
//!
//! One writer connection lives on its own thread; reads open their own
//! connection on the calling thread (`Connection` is `Send`, not `Sync`).
//! Every call blocks on SQLite, so none is made on the UI thread. Each
//! write runs in one transaction that commits only while its
//! [`EpochToken`] is current: a write of a closed library leaves nothing
//! partial behind.

use crate::epoch::EpochToken;
use crate::similarity::Sketch;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, TransactionBehavior, params};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, SystemTime};

/// File name of the store in its library folder.
pub const STORE_FILE: &str = "store.sqlite";

/// BLAKE3 hash of a file's bytes.
pub type ContentHash = [u8; 32];

pub fn content_hash(bytes: &[u8]) -> ContentHash {
    *blake3::hash(bytes).as_bytes()
}

/// Schema migrations, applied in order; the schema version is their count.
const MIGRATIONS: &[&str] = &[
    // 1: the note index (US-083). A deleted note keeps its row, marked by
    // `deleted_at`, so its id survives for history; only live paths are
    // unique. `sketch` holds the line chunks of the last known content
    // (similarity of renames, US-085).
    "CREATE TABLE note (
        id INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        size INTEGER NOT NULL,
        mtime_ns INTEGER NOT NULL,
        hash BLOB,
        title TEXT,
        sketch BLOB,
        offline INTEGER NOT NULL DEFAULT 0,
        deleted_at INTEGER
    );
    CREATE UNIQUE INDEX note_live_path ON note(path) WHERE deleted_at IS NULL;",
];

/// The schema version this build writes.
pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("store: {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the store writer has stopped")]
    Closed,
    #[error("store: {} is not a SQLite database", .0.display())]
    NotADatabase(PathBuf),
}

/// What opening the store tells the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreNotice {
    /// The store's schema is newer than this build: left untouched.
    Newer,
    /// The store was corrupt: set aside at `set_aside`, a new one started.
    Rebuilt { set_aside: PathBuf },
}

impl StoreNotice {
    pub fn message(&self) -> String {
        match self {
            Self::Newer => "This library's index was made by a newer Cerata. History is \
                            unavailable until you update."
                .into(),
            Self::Rebuilt { set_aside } => format!(
                "Cerata rebuilt this library's index. Older versions were set aside in {}.",
                set_aside.display()
            ),
        }
    }
}

/// Result of [`Store::open`]. Without a store the library still opens.
pub enum StoreOpen {
    Ready {
        store: Store,
        notice: Option<StoreNotice>,
    },
    Unavailable {
        notice: Option<StoreNotice>,
    },
}

/// One note as the store knows it. `path` is relative to the library
/// root, `/`-separated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteRecord {
    pub path: String,
    pub size: u64,
    pub mtime_ns: u64,
    pub hash: Option<ContentHash>,
    pub title: Option<String>,
    /// A cloud placeholder whose content is not on this device.
    pub offline: bool,
}

/// A live row of the `note` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredNote {
    pub id: i64,
    pub record: NoteRecord,
}

/// One change of the `note` table.
#[derive(Debug, Clone)]
pub enum NoteChange {
    /// Inserts or updates the live note at `record.path`. A `sketch` of
    /// `None` keeps the stored one.
    Upsert {
        record: NoteRecord,
        sketch: Option<Sketch>,
    },
    /// The live note at `from` now lives at `record.path`, keeping its id.
    Rename {
        from: String,
        record: NoteRecord,
        sketch: Option<Sketch>,
    },
    /// Marks the live note at `path` deleted, keeping its row.
    Delete { path: String },
}

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// Handle on a library's store; clones share the writer thread, which
/// ends once the last clone is dropped.
#[derive(Clone)]
pub struct Store {
    path: PathBuf,
    writer: Sender<Job>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("path", &self.path).finish()
    }
}

/// `data_local_dir()/cerata/libraries/<id>`, given `local_data_dir()/cerata`.
pub fn library_store_dir(local_data_dir: &Path, library_id: &str) -> PathBuf {
    local_data_dir.join("libraries").join(library_id)
}

impl Store {
    /// Opens or creates the store in `dir`. `synced_by` names the cloud
    /// service syncing a folder ([`crate::cloud::synced_by`]): a synced
    /// `dir` gets no store. A store of a newer schema is left untouched; a
    /// corrupt one is set aside with its `-wal` and `-shm` and replaced.
    pub fn open(dir: &Path, synced_by: &dyn Fn(&Path) -> Option<String>) -> StoreOpen {
        if let Some(service) = synced_by(dir) {
            log::warn!(
                "no store for this library: {} is synced by {service}",
                dir.display()
            );
            return StoreOpen::Unavailable { notice: None };
        }
        if let Err(error) = create_private_dir(dir) {
            log::warn!("no store for this library: {}: {error}", dir.display());
            return StoreOpen::Unavailable { notice: None };
        }
        let path = dir.join(STORE_FILE);
        // SQLite discards the `-wal` of a file that is not a database: such
        // a file is set aside before SQLite sees it.
        let opened = if has_foreign_header(&path) {
            Err(StoreError::NotADatabase(path.clone()))
        } else {
            open_writer(&path)
        };
        match opened {
            Ok(Opened::Ready(connection)) => match Self::start(path, connection) {
                Ok(store) => StoreOpen::Ready {
                    store,
                    notice: None,
                },
                Err(error) => unavailable(&error),
            },
            Ok(Opened::Newer(version)) => {
                log::warn!(
                    "{} has schema {version}, newer than {SCHEMA_VERSION}: left untouched",
                    path.display()
                );
                StoreOpen::Unavailable {
                    notice: Some(StoreNotice::Newer),
                }
            }
            Err(error) if is_corruption(&error) => {
                log::warn!("{} is corrupt: {error}", path.display());
                let set_aside = match quarantine(&path) {
                    Ok(set_aside) => set_aside,
                    Err(error) => return unavailable(&error),
                };
                match open_writer(&path) {
                    Ok(Opened::Ready(connection)) => match Self::start(path, connection) {
                        Ok(store) => StoreOpen::Ready {
                            store,
                            notice: Some(StoreNotice::Rebuilt { set_aside }),
                        },
                        Err(error) => unavailable(&error),
                    },
                    Ok(Opened::Newer(_)) => StoreOpen::Unavailable { notice: None },
                    Err(error) => unavailable(&error),
                }
            }
            Err(error) => unavailable(&error),
        }
    }

    fn start(path: PathBuf, mut connection: Connection) -> Result<Self, StoreError> {
        let (writer, jobs) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("cerata-store".into())
            .spawn(move || {
                for job in jobs {
                    job(&mut connection);
                }
            })
            .map_err(|source| StoreError::Io {
                path: path.clone(),
                source,
            })?;
        Ok(Self { path, writer })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Runs `read` on a new read-only connection, on the calling thread.
    pub fn read<T>(
        &self,
        read: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, StoreError> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let connection = Connection::open_with_flags(&self.path, flags)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        Ok(read(&connection)?)
    }

    /// Runs `change` in one transaction on the writer thread. It commits
    /// only if `token` is still current before and after `change`; the
    /// answer is `true` once committed. Drop the receiver not to wait.
    pub fn write(
        &self,
        token: &EpochToken,
        change: impl FnOnce(&rusqlite::Transaction) -> rusqlite::Result<()> + Send + 'static,
    ) -> Receiver<Result<bool, StoreError>> {
        let (answer, receiver) = std::sync::mpsc::channel();
        let token = token.clone();
        let job: Job = Box::new(move |connection| {
            let result = (|| {
                if token.is_stale() {
                    return Ok(false);
                }
                let transaction =
                    connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                change(&transaction)?;
                if token.is_stale() {
                    // Dropping the transaction rolls it back.
                    return Ok(false);
                }
                transaction.commit()?;
                Ok(true)
            })();
            if let Err(error) = &result {
                log::warn!("store write failed: {error}");
            }
            let _ = answer.send(result);
        });
        if self.writer.send(job).is_err() {
            let (answer, receiver) = std::sync::mpsc::channel();
            let _ = answer.send(Err(StoreError::Closed));
            return receiver;
        }
        receiver
    }

    /// Applies `changes` in one transaction (deletions, then renames, then
    /// upserts), committed only while `token` is current.
    pub fn apply(
        &self,
        token: &EpochToken,
        changes: Vec<NoteChange>,
    ) -> Receiver<Result<bool, StoreError>> {
        self.write(token, move |transaction| {
            apply_changes(transaction, changes)
        })
    }

    /// Every live note.
    pub fn live_notes(&self) -> Result<Vec<StoredNote>, StoreError> {
        self.read(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, path, size, mtime_ns, hash, title, offline
                 FROM note WHERE deleted_at IS NULL ORDER BY path",
            )?;
            let rows = statement.query_map([], stored_note)?;
            rows.collect()
        })
    }

    /// The live note at `path`.
    pub fn note(&self, path: &str) -> Result<Option<StoredNote>, StoreError> {
        self.read(|connection| {
            connection
                .query_row(
                    "SELECT id, path, size, mtime_ns, hash, title, offline
                     FROM note WHERE deleted_at IS NULL AND path = ?1",
                    [path],
                    stored_note,
                )
                .optional()
        })
    }

    /// Ids and deletion times of the deleted rows that were at `path`.
    pub fn deleted_at_path(&self, path: &str) -> Result<Vec<(i64, i64)>, StoreError> {
        self.read(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, deleted_at FROM note WHERE deleted_at IS NOT NULL AND path = ?1",
            )?;
            let rows = statement.query_map([path], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect()
        })
    }

    /// Stored sketches of the live notes at `paths`.
    pub fn sketches(&self, paths: &[String]) -> Result<HashMap<String, Sketch>, StoreError> {
        if paths.is_empty() {
            return Ok(HashMap::new());
        }
        self.read(|connection| {
            let mut statement = connection.prepare(
                "SELECT sketch FROM note WHERE deleted_at IS NULL AND path = ?1 AND sketch IS NOT NULL",
            )?;
            let mut found = HashMap::new();
            for path in paths {
                let sketch: Option<Vec<u8>> =
                    statement.query_row([path], |row| row.get(0)).optional()?;
                if let Some(sketch) = sketch {
                    found.insert(path.clone(), Sketch::from_bytes(&sketch));
                }
            }
            Ok(found)
        })
    }

    /// The schema version recorded in the file.
    pub fn schema_version(&self) -> Result<i64, StoreError> {
        self.read(|connection| {
            connection.pragma_query_value(None, "user_version", |row| row.get(0))
        })
    }

    /// Waits until every write queued so far has run.
    pub fn sync(&self) -> Result<(), StoreError> {
        let (done, wait) = std::sync::mpsc::channel();
        let job: Job = Box::new(move |_| {
            let _ = done.send(());
        });
        self.writer.send(job).map_err(|_| StoreError::Closed)?;
        wait.recv().map_err(|_| StoreError::Closed)
    }
}

fn unavailable(error: &dyn std::fmt::Display) -> StoreOpen {
    log::warn!("no store for this library: {error}");
    StoreOpen::Unavailable { notice: None }
}

enum Opened {
    Ready(Connection),
    Newer(i64),
}

/// Opens the writer connection: reads the schema version first, so a
/// newer store is never modified, then sets the pragmas and migrates.
fn open_writer(path: &Path) -> Result<Opened, StoreError> {
    let mut connection = Connection::open(path)?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Ok(Opened::Newer(version));
    }
    let mode: String =
        connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        log::warn!("{}: journal mode is {mode}, not WAL", path.display());
    }
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "secure_delete", "ON")?;
    migrate(&mut connection, version)?;
    Ok(Opened::Ready(connection))
}

/// Applies the migrations after `from` in one transaction.
fn migrate(connection: &mut Connection, from: i64) -> rusqlite::Result<()> {
    if from >= SCHEMA_VERSION {
        return Ok(());
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let start = usize::try_from(from).unwrap_or(0);
    for migration in MIGRATIONS.iter().skip(start) {
        transaction.execute_batch(migration)?;
    }
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()
}

fn is_corruption(error: &StoreError) -> bool {
    match error {
        StoreError::Sqlite(error) => matches!(
            error.sqlite_error_code(),
            Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
        ),
        StoreError::NotADatabase(_) => true,
        _ => false,
    }
}

/// `true` for a non-empty file that does not start with SQLite's header.
fn has_foreign_header(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut header = [0u8; 16];
    match file.read(&mut header) {
        Ok(0) | Err(_) => false,
        Ok(read) => read < 16 || header != *b"SQLite format 3\0",
    }
}

/// Moves `path`, `-wal` and `-shm` to `{name}.corrupt-{YYYYMMDD-HHMMSS}`
/// (UTC), without replacing an earlier quarantine. Returns the new path.
fn quarantine(path: &Path) -> Result<PathBuf, StoreError> {
    let stamp = crate::json_store::utc_stamp(SystemTime::now());
    let named = |suffix: &str, n: u32| {
        let mut name = path.as_os_str().to_owned();
        if n == 0 {
            name.push(format!(".corrupt-{stamp}{suffix}"));
        } else {
            name.push(format!(".corrupt-{stamp}-{n}{suffix}"));
        }
        PathBuf::from(name)
    };
    let n = (0..10_000)
        .find(|n| !named("", *n).exists())
        .ok_or_else(|| StoreError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other("no quarantine name is free"),
        })?;
    let target = named("", n);
    std::fs::rename(path, &target).map_err(|source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    for suffix in ["-wal", "-shm"] {
        let mut side = path.as_os_str().to_owned();
        side.push(suffix);
        let side = PathBuf::from(side);
        if side.exists()
            && let Err(error) = std::fs::rename(&side, named(suffix, n))
        {
            log::warn!("{} not set aside: {error}", side.display());
        }
    }
    Ok(target)
}

/// Creates `dir` and its parents; on Unix the store folders are `0700`.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    #[cfg(unix)]
    {
        // `mode` is masked by the umask and leaves existing folders alone.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn stored_note(row: &rusqlite::Row) -> rusqlite::Result<StoredNote> {
    let hash: Option<Vec<u8>> = row.get(4)?;
    Ok(StoredNote {
        id: row.get(0)?,
        record: NoteRecord {
            path: row.get(1)?,
            size: to_u64(row.get(2)?),
            mtime_ns: to_u64(row.get(3)?),
            hash: hash.and_then(|hash| ContentHash::try_from(hash.as_slice()).ok()),
            title: row.get(5)?,
            offline: row.get::<_, i64>(6)? != 0,
        },
    })
}

fn to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| to_i64(since.as_secs()))
}

fn apply_changes(
    transaction: &rusqlite::Transaction,
    changes: Vec<NoteChange>,
) -> rusqlite::Result<()> {
    let now = unix_seconds();
    let mut delete = transaction
        .prepare_cached("UPDATE note SET deleted_at = ?2 WHERE deleted_at IS NULL AND path = ?1")?;
    let mut rename = transaction.prepare_cached(
        "UPDATE note SET path = ?2, size = ?3, mtime_ns = ?4, hash = ?5, title = ?6,
             offline = ?7, sketch = COALESCE(?8, sketch)
         WHERE deleted_at IS NULL AND path = ?1",
    )?;
    let mut upsert = transaction.prepare_cached(
        "INSERT INTO note (path, size, mtime_ns, hash, title, offline, sketch)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (path) WHERE deleted_at IS NULL DO UPDATE SET
             size = excluded.size, mtime_ns = excluded.mtime_ns, hash = excluded.hash,
             title = excluded.title, offline = excluded.offline,
             sketch = COALESCE(excluded.sketch, note.sketch)",
    )?;
    let (mut deletes, mut renames, mut upserts) = (Vec::new(), Vec::new(), Vec::new());
    for change in changes {
        match change {
            NoteChange::Delete { .. } => deletes.push(change),
            NoteChange::Rename { .. } => renames.push(change),
            NoteChange::Upsert { .. } => upserts.push(change),
        }
    }
    for change in deletes.into_iter().chain(renames).chain(upserts) {
        match change {
            NoteChange::Delete { path } => {
                delete.execute(params![path, now])?;
            }
            NoteChange::Rename {
                from,
                record,
                sketch,
            } => {
                let sketch = sketch.map(|sketch| sketch.to_bytes());
                let moved = rename.execute(params![
                    from,
                    record.path,
                    to_i64(record.size),
                    to_i64(record.mtime_ns),
                    record.hash.map(|hash| hash.to_vec()),
                    record.title,
                    record.offline,
                    sketch,
                ])?;
                if moved == 0 {
                    upsert_record(&mut upsert, &record, sketch)?;
                }
            }
            NoteChange::Upsert { record, sketch } => {
                upsert_record(&mut upsert, &record, sketch.map(|sketch| sketch.to_bytes()))?;
            }
        }
    }
    Ok(())
}

fn upsert_record(
    statement: &mut rusqlite::CachedStatement,
    record: &NoteRecord,
    sketch: Option<Vec<u8>>,
) -> rusqlite::Result<usize> {
    statement.execute(params![
        record.path,
        to_i64(record.size),
        to_i64(record.mtime_ns),
        record.hash.map(|hash| hash.to_vec()),
        record.title,
        record.offline,
        sketch,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epoch::EpochCounter;
    use tempfile::TempDir;

    fn not_synced(_: &Path) -> Option<String> {
        None
    }

    fn open(dir: &Path) -> (Store, Option<StoreNotice>) {
        match Store::open(dir, &not_synced) {
            StoreOpen::Ready { store, notice } => (store, notice),
            StoreOpen::Unavailable { notice } => panic!("store unavailable: {notice:?}"),
        }
    }

    fn record(path: &str, title: &str) -> NoteRecord {
        NoteRecord {
            path: path.into(),
            size: 10,
            mtime_ns: 1_000,
            hash: Some(content_hash(path.as_bytes())),
            title: Some(title.into()),
            offline: false,
        }
    }

    fn upsert(record: NoteRecord) -> NoteChange {
        NoteChange::Upsert {
            record,
            sketch: None,
        }
    }

    #[test]
    fn a_new_store_is_created_with_its_pragmas_and_schema() {
        let dir = TempDir::new().unwrap();
        let store_dir = library_store_dir(dir.path(), "0123");
        let (store, notice) = open(&store_dir);
        assert_eq!(notice, None);
        assert_eq!(store.path(), store_dir.join(STORE_FILE));
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        let mode = store
            .read(|c| c.pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0)))
            .unwrap();
        assert_eq!(mode, "wal");
        // The other pragmas are per connection: checked on the writer.
        let (answer, wait) = std::sync::mpsc::channel();
        let token = EpochCounter::new().token();
        let _ = store.write(&token, move |t| {
            let sync: i64 = t.pragma_query_value(None, "synchronous", |r| r.get(0))?;
            let keys: i64 = t.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
            let secure: i64 = t.pragma_query_value(None, "secure_delete", |r| r.get(0))?;
            let _ = answer.send((sync, keys, secure));
            Ok(())
        });
        // NORMAL is 1.
        assert_eq!(wait.recv().unwrap(), (1, 1, 1));
    }

    #[test]
    fn notes_survive_a_reopen_and_keep_their_ids_across_renames() {
        let dir = TempDir::new().unwrap();
        let token = EpochCounter::new().token();
        let (store, _) = open(dir.path());
        assert!(
            store
                .apply(
                    &token,
                    vec![upsert(record("a.md", "A")), upsert(record("b.md", "B"))]
                )
                .recv()
                .unwrap()
                .unwrap()
        );
        let id = store.note("a.md").unwrap().unwrap().id;
        drop(store);

        let (store, notice) = open(dir.path());
        assert_eq!(notice, None);
        let mut titles: Vec<_> = store
            .live_notes()
            .unwrap()
            .into_iter()
            .map(|note| (note.record.path, note.record.title))
            .collect();
        titles.sort();
        assert_eq!(
            titles,
            [
                ("a.md".into(), Some("A".into())),
                ("b.md".into(), Some("B".into()))
            ]
        );
        let renamed = NoteChange::Rename {
            from: "a.md".into(),
            record: record("c.md", "A"),
            sketch: None,
        };
        let deleted = NoteChange::Delete {
            path: "b.md".into(),
        };
        store
            .apply(&token, vec![renamed, deleted])
            .recv()
            .unwrap()
            .unwrap();
        assert_eq!(store.note("c.md").unwrap().unwrap().id, id);
        assert_eq!(store.note("a.md").unwrap(), None);
        assert_eq!(store.note("b.md").unwrap(), None);
        assert_eq!(store.deleted_at_path("b.md").unwrap().len(), 1);
        // A new note at a deleted note's path gets a new id.
        store
            .apply(&token, vec![upsert(record("b.md", "B2"))])
            .recv()
            .unwrap()
            .unwrap();
        let reborn = store.note("b.md").unwrap().unwrap();
        assert_ne!(
            Some(reborn.id),
            store.deleted_at_path("b.md").unwrap().first().map(|d| d.0)
        );
    }

    #[test]
    fn sketches_are_kept_until_replaced() {
        let dir = TempDir::new().unwrap();
        let token = EpochCounter::new().token();
        let (store, _) = open(dir.path());
        let sketch = Sketch::of(b"one\ntwo\n");
        store
            .apply(
                &token,
                vec![NoteChange::Upsert {
                    record: record("a.md", "A"),
                    sketch: Some(sketch.clone()),
                }],
            )
            .recv()
            .unwrap()
            .unwrap();
        store
            .apply(&token, vec![upsert(record("a.md", "A2"))])
            .recv()
            .unwrap()
            .unwrap();
        let found = store.sketches(&["a.md".into(), "none.md".into()]).unwrap();
        assert_eq!(found.get("a.md"), Some(&sketch));
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn an_older_schema_is_migrated_in_place() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(STORE_FILE);
        // A version-0 file: an empty SQLite database.
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE unrelated (x);")
            .unwrap();
        let (store, notice) = open(dir.path());
        assert_eq!(notice, None);
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(store.live_notes().unwrap(), []);
    }

    #[test]
    fn a_store_of_a_newer_cerata_is_left_untouched() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(STORE_FILE);
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch("CREATE TABLE future (x); INSERT INTO future VALUES (1);")
                .unwrap();
            c.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                .unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        let StoreOpen::Unavailable { notice } = Store::open(dir.path(), &not_synced) else {
            panic!("a newer store must not open");
        };
        assert_eq!(notice, Some(StoreNotice::Newer));
        assert_eq!(
            notice.unwrap().message(),
            "This library's index was made by a newer Cerata. History is unavailable until you update."
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn a_corrupt_store_is_set_aside_with_its_journal_and_rebuilt() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(STORE_FILE);
        // Random bytes: SQLITE_NOTADB.
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let garbage: Vec<u8> = (0..8192)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        std::fs::write(&path, &garbage).unwrap();
        std::fs::write(dir.path().join("store.sqlite-wal"), b"wal").unwrap();
        std::fs::write(dir.path().join("store.sqlite-shm"), b"shm").unwrap();
        let (store, notice) = open(dir.path());
        let Some(StoreNotice::Rebuilt { set_aside }) = notice.clone() else {
            panic!("expected a rebuild notice, got {notice:?}");
        };
        assert_eq!(std::fs::read(&set_aside).unwrap(), garbage);
        let name = set_aside
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(name.starts_with("store.sqlite.corrupt-"), "{name}");
        let mut wal = set_aside.as_os_str().to_owned();
        wal.push("-wal");
        assert_eq!(std::fs::read(PathBuf::from(wal)).unwrap(), b"wal");
        let mut shm = set_aside.as_os_str().to_owned();
        shm.push("-shm");
        assert_eq!(std::fs::read(PathBuf::from(shm)).unwrap(), b"shm");
        assert_eq!(store.live_notes().unwrap(), []);
        assert!(
            notice.unwrap().message().starts_with(
                "Cerata rebuilt this library's index. Older versions were set aside in "
            )
        );
    }

    #[test]
    fn a_synced_folder_gets_no_store() {
        let dir = TempDir::new().unwrap();
        let store_dir = dir.path().join("synced");
        let synced = |_: &Path| Some("OneDrive".to_owned());
        let StoreOpen::Unavailable { notice } = Store::open(&store_dir, &synced) else {
            panic!("a synced folder must get no store");
        };
        assert_eq!(notice, None);
        assert!(!store_dir.exists());
    }

    #[test]
    fn a_write_of_a_stale_epoch_leaves_nothing() {
        let dir = TempDir::new().unwrap();
        let counter = EpochCounter::new();
        let (store, _) = open(dir.path());
        let stale = counter.token();
        counter.advance();
        assert!(
            !store
                .apply(&stale, vec![upsert(record("a.md", "A"))])
                .recv()
                .unwrap()
                .unwrap()
        );
        // Canceled mid-transaction: the rows already written roll back.
        let token = counter.token();
        let canceler = counter.clone();
        let committed = store
            .write(&token, move |t| {
                apply_changes(t, vec![upsert(record("b.md", "B"))])?;
                canceler.advance();
                Ok(())
            })
            .recv()
            .unwrap()
            .unwrap();
        assert!(!committed);
        assert_eq!(store.live_notes().unwrap(), []);
    }

    #[cfg(unix)]
    #[test]
    fn the_store_folder_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let store_dir = library_store_dir(dir.path(), "abc");
        let _ = open(&store_dir);
        let mode = std::fs::metadata(&store_dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
