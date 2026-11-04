//! A note's history (EP-018): every version of a note, addressed by the
//! BLAKE3 hash of its bytes, in the library's store.
//!
//! The `.md` file stays the truth and is always written first; a version
//! is inserted afterwards, by one recorder thread per library that groups
//! what arrives within [`BATCH_WINDOW`] into one transaction. A save of
//! Cerata's becomes a version once the note has not been written for
//! [`SAVE_PAUSE`] (the typing pause), or at once when its tab closes, the
//! library changes or the app quits ([`History::flush`]). A change made
//! outside Cerata is recorded at once, with the known head as its parent.
//!
//! Blobs are deltas (US-088, ADR 0009 addendum): zstd level 3 with the
//! previous version of the note as the dictionary, a full blob first and
//! after [`MAX_CHAIN`] deltas. Two identical successive contents of a note
//! make one version.

pub mod device;
pub mod gc;
pub mod retention;

use crate::epoch::{EpochCounter, EpochToken};
use crate::store::{ContentHash, Store, StoreError, content_hash};
use device::Device;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

/// Most deltas between a blob and a full one (US-088): decoding a version
/// never applies more than this many.
pub const MAX_CHAIN: usize = 32;
/// A save becomes a version once the note was not written for this long.
pub const SAVE_PAUSE: Duration = Duration::from_secs(30);
/// Versions arriving within this window share one transaction.
pub const BATCH_WINDOW: Duration = Duration::from_millis(500);
/// zstd level of every blob.
const LEVEL: i32 = 3;

/// Blob codecs.
const CODEC_ZSTD: i64 = 1;
const CODEC_DELTA: i64 = 2;

/// Schema of the history (store migration 2). `version.parent` is the
/// hash of the version before; `blob.base` the blob a delta applies to.
pub const SCHEMA: &str = "
    CREATE TABLE blob (
        hash BLOB PRIMARY KEY,
        codec INTEGER NOT NULL,
        base BLOB,
        bytes BLOB NOT NULL
    ) WITHOUT ROWID;
    CREATE INDEX blob_base ON blob(base) WHERE base IS NOT NULL;
    CREATE TABLE device (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL
    ) WITHOUT ROWID;
    CREATE TABLE version (
        note INTEGER NOT NULL REFERENCES note(id),
        seq INTEGER NOT NULL,
        hash BLOB NOT NULL REFERENCES blob(hash),
        parent BLOB,
        cause TEXT NOT NULL,
        device TEXT NOT NULL,
        ts INTEGER NOT NULL,
        pinned INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (note, seq)
    ) WITHOUT ROWID;
    CREATE INDEX version_hash ON version(hash);
    CREATE TABLE meta (
        key TEXT PRIMARY KEY,
        value
    ) WITHOUT ROWID;";

/// Why a version exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cause {
    Save,
    External,
    Restore,
    Rename,
    Merge,
    Conflict,
}

impl Cause {
    pub const ALL: [Cause; 6] = [
        Cause::Save,
        Cause::External,
        Cause::Restore,
        Cause::Rename,
        Cause::Merge,
        Cause::Conflict,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::External => "external",
            Self::Restore => "restore",
            Self::Rename => "rename",
            Self::Merge => "merge",
            Self::Conflict => "conflict",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|cause| cause.as_str() == text)
    }

    /// The label of the history panel (US-091).
    pub fn label(self) -> &'static str {
        match self {
            Self::Save => "Saved",
            Self::External => "Changed outside Cerata",
            Self::Restore => "Restored",
            Self::Rename => "Renamed",
            Self::Merge => "Merged",
            Self::Conflict => "Conflict copy",
        }
    }
}

/// One version of a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionInfo {
    pub note: i64,
    pub seq: i64,
    pub hash: ContentHash,
    pub parent: Option<ContentHash>,
    pub cause: Cause,
    pub device_id: String,
    pub device_name: String,
    /// Seconds since the Unix epoch.
    pub ts: i64,
    pub pinned: bool,
}

/// A deleted note that still has versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedNote {
    pub id: i64,
    /// Where it was, relative to the library root.
    pub path: String,
    pub deleted_at: i64,
    pub versions: usize,
    /// The hash of its last version.
    pub head: ContentHash,
}

/// One version to insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The note, relative to the library root, `/`-separated.
    pub path: String,
    /// The file's bytes.
    pub bytes: Vec<u8>,
    pub cause: Cause,
    /// Seconds since the Unix epoch.
    pub ts: i64,
    /// Inserted only while the note has no version at all.
    pub only_if_empty: bool,
    /// `Some` parent instead of the head (a conflict copy's branch point).
    pub parent: Option<ContentHash>,
}

impl Request {
    pub fn new(path: impl Into<String>, bytes: Vec<u8>, cause: Cause) -> Self {
        Self {
            path: path.into(),
            bytes,
            cause,
            ts: unix_now(),
            only_if_empty: false,
            parent: None,
        }
    }
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

fn sql_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

pub(crate) fn hash_of(bytes: Vec<u8>) -> Option<ContentHash> {
    ContentHash::try_from(bytes.as_slice()).ok()
}

// Blobs -----------------------------------------------------------------

/// Deltas between `hash` and its full blob.
fn chain_length(connection: &Connection, hash: &ContentHash) -> rusqlite::Result<usize> {
    let mut statement = connection.prepare_cached("SELECT base FROM blob WHERE hash = ?1")?;
    let mut current = *hash;
    let mut length = 0;
    while length <= MAX_CHAIN + 1 {
        let base: Option<Option<Vec<u8>>> = statement
            .query_row([current.as_slice()], |row| row.get(0))
            .optional()?;
        match base.flatten().and_then(hash_of) {
            Some(base) => {
                length += 1;
                current = base;
            }
            None => return Ok(length),
        }
    }
    Ok(length)
}

fn compress(bytes: &[u8], dictionary: Option<&[u8]>) -> rusqlite::Result<Vec<u8>> {
    let compressed = match dictionary {
        Some(dictionary) => zstd::bulk::Compressor::with_dictionary(LEVEL, dictionary)
            .and_then(|mut compressor| compressor.compress(bytes)),
        None => zstd::bulk::compress(bytes, LEVEL),
    };
    compressed.map_err(sql_error)
}

fn decompress(bytes: &[u8], dictionary: Option<&[u8]>) -> rusqlite::Result<Vec<u8>> {
    let mut out = Vec::new();
    let read = match dictionary {
        Some(dictionary) => zstd::stream::read::Decoder::with_dictionary(bytes, dictionary)
            .and_then(|mut decoder| decoder.read_to_end(&mut out)),
        None => zstd::stream::read::Decoder::new(bytes)
            .and_then(|mut decoder| decoder.read_to_end(&mut out)),
    };
    read.map_err(sql_error)?;
    Ok(out)
}

/// The bytes of blob `hash`, checked against it; `None` if absent.
pub fn read_blob(connection: &Connection, hash: &ContentHash) -> rusqlite::Result<Option<Vec<u8>>> {
    let mut statement =
        connection.prepare_cached("SELECT codec, base, bytes FROM blob WHERE hash = ?1")?;
    // From `hash` down to its full blob.
    let mut chain: Vec<(i64, Vec<u8>)> = Vec::new();
    let mut current = *hash;
    loop {
        let row: Option<(i64, Option<Vec<u8>>, Vec<u8>)> = statement
            .query_row([current.as_slice()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()?;
        let Some((codec, base, bytes)) = row else {
            if chain.is_empty() {
                return Ok(None);
            }
            return Err(sql_error(std::io::Error::other(
                "a delta's base is missing",
            )));
        };
        chain.push((codec, bytes));
        match (codec, base.and_then(hash_of)) {
            (CODEC_DELTA, Some(base)) if chain.len() <= MAX_CHAIN + 1 => current = base,
            (CODEC_ZSTD, _) => break,
            _ => return Err(sql_error(std::io::Error::other("unreadable blob"))),
        }
    }
    let mut content: Option<Vec<u8>> = None;
    for (_, bytes) in chain.iter().rev() {
        content = Some(decompress(bytes, content.as_deref())?);
    }
    let content = content.unwrap_or_default();
    if content_hash(&content) != *hash {
        return Err(sql_error(std::io::Error::other(
            "a blob does not match its hash",
        )));
    }
    Ok(Some(content))
}

/// Inserts the blob of `bytes` unless present: a delta on `base` (its
/// content given) while the chain stays within [`MAX_CHAIN`].
fn put_blob(
    transaction: &Transaction,
    hash: &ContentHash,
    bytes: &[u8],
    base: Option<(&ContentHash, &[u8])>,
) -> rusqlite::Result<()> {
    let present: Option<i64> = transaction
        .prepare_cached("SELECT 1 FROM blob WHERE hash = ?1")?
        .query_row([hash.as_slice()], |row| row.get(0))
        .optional()?;
    if present.is_some() {
        return Ok(());
    }
    let base = match base {
        Some((base, content)) if chain_length(transaction, base)? < MAX_CHAIN => {
            Some((base, content))
        }
        _ => None,
    };
    let (codec, base_hash, data) = match base {
        Some((base, content)) => (
            CODEC_DELTA,
            Some(base.to_vec()),
            compress(bytes, Some(content))?,
        ),
        None => (CODEC_ZSTD, None, compress(bytes, None)?),
    };
    transaction
        .prepare_cached("INSERT INTO blob (hash, codec, base, bytes) VALUES (?1, ?2, ?3, ?4)")?
        .execute(params![hash.as_slice(), codec, base_hash, data])?;
    Ok(())
}

/// Re-encodes blob `hash` as a delta on `base`, or alone without one.
pub(crate) fn reencode_blob(
    transaction: &Transaction,
    hash: &ContentHash,
    base: Option<&ContentHash>,
) -> rusqlite::Result<()> {
    let Some(content) = read_blob(transaction, hash)? else {
        return Ok(());
    };
    let base_content = match base {
        Some(base) if chain_length(transaction, base)? < MAX_CHAIN => {
            read_blob(transaction, base)?.map(|content| (*base, content))
        }
        _ => None,
    };
    let (codec, base_hash, data) = match &base_content {
        Some((base, content_of_base)) => (
            CODEC_DELTA,
            Some(base.to_vec()),
            compress(&content, Some(content_of_base))?,
        ),
        None => (CODEC_ZSTD, None, compress(&content, None)?),
    };
    transaction
        .prepare_cached("UPDATE blob SET codec = ?2, base = ?3, bytes = ?4 WHERE hash = ?1")?
        .execute(params![hash.as_slice(), codec, base_hash, data])?;
    Ok(())
}

// Versions --------------------------------------------------------------

/// The id of the note at `path`. A note that left `path` since the request
/// was made (a save pending through the typing pause) is found where it
/// went: the one live note holding these bytes (moved outside Cerata), else
/// the last deleted row at `path`. Only a note the index has not recorded
/// yet gets a new row (its next upsert completes it); the row has no hash,
/// so it names no content a blob would be kept for.
fn note_id(transaction: &Transaction, path: &str, bytes: &[u8]) -> rusqlite::Result<i64> {
    let id: Option<i64> = transaction
        .prepare_cached("SELECT id FROM note WHERE deleted_at IS NULL AND path = ?1")?
        .query_row([path], |row| row.get(0))
        .optional()?;
    if let Some(id) = id {
        return Ok(id);
    }
    let hash = content_hash(bytes);
    let moved: Vec<i64> = transaction
        .prepare_cached("SELECT id FROM note WHERE deleted_at IS NULL AND hash = ?1 LIMIT 2")?
        .query_map([hash.as_slice()], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if let [id] = moved.as_slice() {
        return Ok(*id);
    }
    let deleted: Option<i64> = transaction
        .prepare_cached(
            "SELECT id FROM note WHERE deleted_at IS NOT NULL AND path = ?1
             ORDER BY deleted_at DESC, id DESC LIMIT 1",
        )?
        .query_row([path], |row| row.get(0))
        .optional()?;
    if let Some(id) = deleted {
        return Ok(id);
    }
    transaction
        .prepare_cached("INSERT INTO note (path, size, mtime_ns) VALUES (?1, ?2, 0)")?
        .execute(params![
            path,
            i64::try_from(bytes.len()).unwrap_or(i64::MAX)
        ])?;
    Ok(transaction.last_insert_rowid())
}

/// The newest sequence number of `note`, and the hash of its head: the
/// newest version that is not a conflict copy (a branch beside the note's
/// line, US-093).
fn head(
    connection: &Connection,
    note: i64,
) -> rusqlite::Result<(Option<i64>, Option<ContentHash>)> {
    let seq: Option<i64> = connection
        .prepare_cached("SELECT MAX(seq) FROM version WHERE note = ?1")?
        .query_row([note], |row| row.get(0))?;
    let hash: Option<Vec<u8>> = connection
        .prepare_cached(
            "SELECT hash FROM version WHERE note = ?1 AND cause != 'conflict'
             ORDER BY seq DESC LIMIT 1",
        )?
        .query_row([note], |row| row.get(0))
        .optional()?;
    Ok((seq, hash.and_then(hash_of)))
}

/// Whether `note` already has a conflict version holding `hash`.
fn conflict_known(
    connection: &Connection,
    note: i64,
    hash: &ContentHash,
) -> rusqlite::Result<bool> {
    let found: Option<i64> = connection
        .prepare_cached(
            "SELECT 1 FROM version WHERE note = ?1 AND hash = ?2 AND cause = 'conflict'",
        )?
        .query_row(params![note, hash.as_slice()], |row| row.get(0))
        .optional()?;
    Ok(found.is_some())
}

/// The last content inserted per note, so a delta does not decode its
/// base again. Bounded: cleared past [`Self::LIMIT`] notes.
#[derive(Default)]
struct HeadCache(HashMap<i64, (ContentHash, Vec<u8>)>);

impl HeadCache {
    const LIMIT: usize = 64;

    fn content(
        &mut self,
        connection: &Connection,
        note: i64,
        hash: &ContentHash,
    ) -> rusqlite::Result<Option<Vec<u8>>> {
        if let Some((known, content)) = self.0.get(&note)
            && known == hash
        {
            return Ok(Some(content.clone()));
        }
        read_blob(connection, hash)
    }

    fn insert(&mut self, note: i64, hash: ContentHash, content: Vec<u8>) {
        if self.0.len() >= Self::LIMIT && !self.0.contains_key(&note) {
            self.0.clear();
        }
        self.0.insert(note, (hash, content));
    }
}

/// Inserts `request` as the next version of its note, unless it holds the
/// head's content. Returns whether a version was added.
fn insert_version(
    transaction: &Transaction,
    device: &Device,
    cache: &mut HeadCache,
    request: &Request,
) -> rusqlite::Result<bool> {
    let note = note_id(transaction, &request.path, &request.bytes)?;
    let (last_seq, head) = head(transaction, note)?;
    if request.only_if_empty && last_seq.is_some() {
        return Ok(false);
    }
    let hash = content_hash(&request.bytes);
    // A restore is a gesture worth listing even when it brings back the
    // head's text (a deleted note); other causes skip an unchanged text.
    let always = matches!(request.cause, Cause::Conflict | Cause::Restore);
    if !always && head == Some(hash) {
        return Ok(false);
    }
    if request.cause == Cause::Conflict && conflict_known(transaction, note, &hash)? {
        return Ok(false);
    }
    let parent = request.parent.or(head);
    let base_content = match parent {
        Some(parent) => cache.content(transaction, note, &parent)?,
        None => None,
    };
    put_blob(
        transaction,
        &hash,
        &request.bytes,
        parent.as_ref().zip(base_content.as_deref()),
    )?;
    let seq = last_seq.map_or(1, |seq| seq + 1);
    transaction
        .prepare_cached(
            "INSERT INTO version (note, seq, hash, parent, cause, device, ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?
        .execute(params![
            note,
            seq,
            hash.to_vec(),
            parent.map(|parent| parent.to_vec()),
            request.cause.as_str(),
            device.id,
            request.ts
        ])?;
    cache.insert(note, hash, request.bytes.clone());
    Ok(true)
}

fn version_row(row: &rusqlite::Row) -> rusqlite::Result<VersionInfo> {
    let hash: Vec<u8> = row.get(2)?;
    let parent: Option<Vec<u8>> = row.get(3)?;
    let cause: String = row.get(4)?;
    Ok(VersionInfo {
        note: row.get(0)?,
        seq: row.get(1)?,
        hash: hash_of(hash).unwrap_or_default(),
        parent: parent.and_then(hash_of),
        cause: Cause::parse(&cause).unwrap_or(Cause::Save),
        device_id: row.get(5)?,
        device_name: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
        ts: row.get(7)?,
        pinned: row.get::<_, i64>(8)? != 0,
    })
}

const VERSION_COLUMNS: &str = "v.note, v.seq, v.hash, v.parent, v.cause, v.device, d.name, v.ts, \
                               v.pinned FROM version v LEFT JOIN device d ON d.id = v.device";

/// The versions of note `id`, newest first.
pub fn versions_of_note(store: &Store, id: i64) -> Result<Vec<VersionInfo>, StoreError> {
    store.read(|connection| {
        let mut statement = connection.prepare(&format!(
            "SELECT {VERSION_COLUMNS} WHERE v.note = ?1 ORDER BY v.seq DESC"
        ))?;
        let rows = statement.query_map([id], version_row)?;
        rows.collect()
    })
}

/// The live note at `path` and its versions, newest first.
pub fn versions_of_path(
    store: &Store,
    path: &str,
) -> Result<Option<(i64, Vec<VersionInfo>)>, StoreError> {
    let Some(note) = store.note(path)? else {
        return Ok(None);
    };
    Ok(Some((note.id, versions_of_note(store, note.id)?)))
}

/// The bytes of a version.
pub fn content(store: &Store, hash: &ContentHash) -> Result<Option<Vec<u8>>, StoreError> {
    let hash = *hash;
    store.read(move |connection| read_blob(connection, &hash))
}

/// The last version of the live note at `path` written before `ts`.
pub fn version_before(
    store: &Store,
    path: &str,
    ts: i64,
) -> Result<Option<VersionInfo>, StoreError> {
    let Some(note) = store.note(path)? else {
        return Ok(None);
    };
    store.read(|connection| {
        connection
            .query_row(
                &format!(
                    "SELECT {VERSION_COLUMNS} WHERE v.note = ?1 AND v.ts <= ?2
                       AND v.cause != 'conflict'
                     ORDER BY v.seq DESC LIMIT 1"
                ),
                params![note.id, ts],
                version_row,
            )
            .optional()
    })
}

/// Deleted notes that still have versions, most recently deleted first.
pub fn deleted_notes(store: &Store) -> Result<Vec<DeletedNote>, StoreError> {
    store.read(|connection| {
        let mut statement = connection.prepare(
            "SELECT n.id, n.path, n.deleted_at, COUNT(v.seq),
                    (SELECT hash FROM version WHERE note = n.id ORDER BY seq DESC LIMIT 1)
             FROM note n JOIN version v ON v.note = n.id
             WHERE n.deleted_at IS NOT NULL
             GROUP BY n.id ORDER BY n.deleted_at DESC, n.id DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(DeletedNote {
                id: row.get(0)?,
                path: row.get(1)?,
                deleted_at: row.get(2)?,
                versions: usize::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
                head: hash_of(row.get(4)?).unwrap_or_default(),
            })
        })?;
        rows.collect()
    })
}

/// Live notes whose newest version is not their last known content: a
/// crash came between a write and its version (US-089).
pub fn unversioned_heads(store: &Store) -> Result<Vec<String>, StoreError> {
    store.read(|connection| {
        let mut statement = connection.prepare(
            "SELECT n.path FROM note n
             JOIN version v ON v.note = n.id
             WHERE n.deleted_at IS NULL
               AND v.seq = (SELECT MAX(seq) FROM version
                            WHERE note = n.id AND cause != 'conflict')
               AND (n.hash IS NULL OR v.hash != n.hash)
             ORDER BY n.path",
        )?;
        let rows = statement.query_map([], |row| row.get(0))?;
        rows.collect()
    })
}

/// The `conflict` version of the conflict copy at `copy` (`relative` to
/// the library root), when its original is next to it: a branch of the
/// original whose parent is the original's last version written before
/// the copy (US-093).
pub fn conflict_version(
    store: &Store,
    root: &std::path::Path,
    copy: &std::path::Path,
    bytes: Vec<u8>,
) -> Option<Request> {
    let original = crate::conflict_copy::original_of(copy)?;
    let relative = crate::index::relative_slash_path(root, &original);
    let modified = std::fs::metadata(copy)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or_else(unix_now, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        });
    let parent = version_before(store, &relative, modified)
        .ok()
        .flatten()
        .map(|version| version.hash);
    let mut request = Request::new(relative, bytes, Cause::Conflict);
    request.parent = parent;
    Some(request)
}

/// Gives deleted note `id` back its life at `path` (relative), so a note
/// restored from "Deleted Notes" keeps its history. Nothing happens when a
/// live note already holds `path`. Answers whether the row was revived.
pub fn undelete(
    store: &Store,
    token: &EpochToken,
    id: i64,
    path: String,
) -> Receiver<Result<bool, StoreError>> {
    store.write(token, move |transaction| {
        let taken: Option<i64> = transaction
            .query_row(
                "SELECT id FROM note WHERE deleted_at IS NULL AND path = ?1",
                [&path],
                |row| row.get(0),
            )
            .optional()?;
        if taken.is_none() {
            transaction.execute(
                "UPDATE note SET deleted_at = NULL, path = ?2 WHERE id = ?1",
                params![id, path],
            )?;
        }
        Ok(())
    })
}

/// Pins or unpins a version (kept by every retention tier).
pub fn set_pinned(
    store: &Store,
    token: &EpochToken,
    note: i64,
    seq: i64,
    pinned: bool,
) -> Receiver<Result<bool, StoreError>> {
    store.write(token, move |transaction| {
        transaction.execute(
            "UPDATE version SET pinned = ?3 WHERE note = ?1 AND seq = ?2",
            params![note, seq, pinned],
        )?;
        Ok(())
    })
}

// The recorder ----------------------------------------------------------

/// The first retention check waits this long after the library opens.
pub const RETENTION_DELAY: Duration = Duration::from_secs(60);
/// Then whether a pass is due is checked this often.
pub const RETENTION_CHECK: Duration = Duration::from_secs(3600);

/// How long the recorder waits; shortened by tests.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub save_pause: Duration,
    pub batch_window: Duration,
    pub retention_delay: Duration,
    pub retention_check: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            save_pause: SAVE_PAUSE,
            batch_window: BATCH_WINDOW,
            retention_delay: RETENTION_DELAY,
            retention_check: RETENTION_CHECK,
        }
    }
}

enum Command {
    Record(Request),
    Saved(Request),
    Flush {
        path: Option<String>,
        done: Option<Sender<()>>,
        /// The library closes: a retention pass follows.
        closing: bool,
    },
    Renamed {
        from: String,
        to: String,
    },
}

/// Told once per session when a version could not be written.
pub type HistoryNotice = Arc<dyn Fn(String) + Send + Sync>;

/// The message of a version that could not be written (US-089).
pub fn failure_message(error: &StoreError) -> String {
    let detail = match error {
        StoreError::Sqlite(error) => error.to_string(),
        StoreError::Io { source, .. } => source.to_string(),
        other => other.to_string(),
    };
    format!("Cerata couldn't save this note's history: {detail}")
}

/// The history of one library: versions go to its store through one
/// recorder thread. Clones share it; it stops once the last is dropped,
/// after writing what it holds.
#[derive(Clone)]
pub struct History {
    store: Store,
    token: EpochToken,
    /// Never stale: retention passes and purges finish even once the
    /// library is closed in the window.
    maintenance: EpochToken,
    cache: Arc<parking_lot::Mutex<HeadCache>>,
    commands: Sender<Command>,
}

impl std::fmt::Debug for History {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("History")
            .field("store", &self.store)
            .finish()
    }
}

impl History {
    pub fn start(store: Store, token: EpochToken, device: Device, notice: HistoryNotice) -> Self {
        Self::start_with(store, token, device, notice, Timing::default())
    }

    pub fn start_with(
        store: Store,
        token: EpochToken,
        device: Device,
        notice: HistoryNotice,
        timing: Timing,
    ) -> Self {
        let (commands, receiver) = std::sync::mpsc::channel();
        let maintenance = EpochCounter::new().token();
        let cache = Arc::new(parking_lot::Mutex::new(HeadCache::default()));
        let recorder = Recorder {
            store: store.clone(),
            token: token.clone(),
            maintenance: maintenance.clone(),
            device,
            notice,
            noticed: AtomicBool::new(false),
            timing,
            cache: cache.clone(),
        };
        let spawned = std::thread::Builder::new()
            .name("cerata-history".into())
            .spawn(move || recorder.run(&receiver));
        if let Err(error) = spawned {
            log::error!("history unavailable: {error}");
        }
        Self {
            store,
            token,
            maintenance,
            cache,
            commands,
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn token(&self) -> &EpochToken {
        &self.token
    }

    /// Records a version within the batch window.
    pub fn record(&self, request: Request) {
        let _ = self.commands.send(Command::Record(request));
    }

    /// Cerata wrote `bytes` to `path`: a `save` version once the note has
    /// not been written for [`SAVE_PAUSE`], or on [`Self::flush`].
    pub fn saved(&self, path: impl Into<String>, bytes: Vec<u8>) {
        let _ = self
            .commands
            .send(Command::Saved(Request::new(path, bytes, Cause::Save)));
    }

    /// Writes the pending save of `path` (every one with `None`) now.
    pub fn flush(&self, path: Option<&str>) {
        let _ = self.commands.send(Command::Flush {
            path: path.map(str::to_owned),
            done: None,
            closing: false,
        });
    }

    /// The library closes: every pending save is written (waited for up
    /// to `timeout`), then a retention pass runs on the store's thread.
    pub fn close(&self, timeout: Duration) -> bool {
        let (done, wait) = std::sync::mpsc::channel();
        let sent = self.commands.send(Command::Flush {
            path: None,
            done: Some(done),
            closing: true,
        });
        sent.is_ok() && wait.recv_timeout(timeout).is_ok()
    }

    /// How many versions `purge` would delete, read on the calling thread.
    pub fn purge_count(&self, purge: gc::Purge) -> Result<usize, StoreError> {
        self.store
            .read(move |connection| gc::purge_count(connection, purge))
    }

    /// Deletes the versions of `purge` for good: their rows and blobs, then
    /// a vacuum and an emptied WAL leave none of their bytes in the files.
    /// The notes are never touched.
    pub fn purge(&self, purge: gc::Purge) -> Receiver<Result<gc::Removed, StoreError>> {
        let (answer, receiver) = std::sync::mpsc::channel();
        let (counted, removed) = std::sync::mpsc::channel();
        let deleted = self.store.write(&self.maintenance, move |transaction| {
            let _ = counted.send(gc::purge_versions(transaction, purge)?);
            Ok(())
        });
        let store = self.store.clone();
        let cache = self.cache.clone();
        let spawned = std::thread::Builder::new()
            .name("cerata-purge".into())
            .spawn(move || {
                let result = match deleted.recv() {
                    Ok(Ok(_)) => {
                        cache.lock().0.clear();
                        let removed = removed.try_recv().unwrap_or_default();
                        match store.maintain(|connection| gc::vacuum(connection)).recv() {
                            Ok(Ok(())) => Ok(removed),
                            Ok(Err(error)) => Err(error),
                            Err(_) => Err(StoreError::Closed),
                        }
                    }
                    Ok(Err(error)) => Err(error),
                    Err(_) => Err(StoreError::Closed),
                };
                let _ = answer.send(result);
            });
        if let Err(error) = spawned {
            log::error!("purge not started: {error}");
        }
        receiver
    }

    /// [`Self::flush`], waiting up to `timeout` for the versions to be
    /// committed. `false` at the deadline.
    pub fn flush_and_wait(&self, path: Option<&str>, timeout: Duration) -> bool {
        let (done, wait) = std::sync::mpsc::channel();
        let sent = self.commands.send(Command::Flush {
            path: path.map(str::to_owned),
            done: Some(done),
            closing: false,
        });
        sent.is_ok() && wait.recv_timeout(timeout).is_ok()
    }

    /// A note Cerata moved: its pending save follows it.
    pub fn renamed(&self, from: &str, to: &str) {
        let _ = self.commands.send(Command::Renamed {
            from: from.to_owned(),
            to: to.to_owned(),
        });
    }
}

struct Pending {
    request: Request,
    due: Instant,
}

struct Recorder {
    store: Store,
    token: EpochToken,
    maintenance: EpochToken,
    device: Device,
    notice: HistoryNotice,
    noticed: AtomicBool,
    timing: Timing,
    cache: Arc<parking_lot::Mutex<HeadCache>>,
}

impl Recorder {
    fn run(&self, commands: &Receiver<Command>) {
        // A purge interrupted during its vacuum vacuums again.
        let owed = self
            .store
            .read(|connection| gc::meta_get(connection, gc::VACUUM_OWED))
            .ok()
            .flatten()
            .is_some();
        if owed {
            log::info!("store: finishing a purge interrupted during its vacuum");
            let _ = self
                .store
                .maintain(|connection| gc::vacuum(connection))
                .recv();
        }
        let mut pending: HashMap<String, Pending> = HashMap::new();
        let mut batch: Vec<Request> = Vec::new();
        let mut deadline: Option<Instant> = None;
        let mut retention_at = Instant::now() + self.timing.retention_delay;
        loop {
            let now = Instant::now();
            if retention_at <= now {
                self.retention(false);
                retention_at = now + self.timing.retention_check;
            }
            let next = pending
                .values()
                .map(|pending| pending.due)
                .chain(deadline)
                .chain(Some(retention_at))
                .min();
            let received = match next {
                Some(at) => commands.recv_timeout(at.saturating_duration_since(now)),
                None => commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            let mut waiting: Vec<Sender<()>> = Vec::new();
            let mut now_please = false;
            let mut close = false;
            match received {
                Ok(Command::Record(request)) => {
                    // A save still pending for the note came first.
                    if let Some(earlier) = pending.remove(&request.path) {
                        batch.push(earlier.request);
                    }
                    batch.push(request);
                }
                Ok(Command::Saved(request)) => {
                    let due = Instant::now() + self.timing.save_pause;
                    pending.insert(request.path.clone(), Pending { request, due });
                }
                Ok(Command::Flush {
                    path,
                    done,
                    closing,
                }) => {
                    close |= closing;
                    let paths: Vec<String> = match path {
                        Some(path) => vec![path],
                        None => pending.keys().cloned().collect(),
                    };
                    for path in paths {
                        if let Some(pending) = pending.remove(&path) {
                            batch.push(pending.request);
                        }
                    }
                    waiting.extend(done);
                    now_please = true;
                }
                Ok(Command::Renamed { from, to }) => {
                    if let Some(mut moved) = pending.remove(&from) {
                        moved.request.path = to.clone();
                        pending.insert(to, moved);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    batch.extend(pending.drain().map(|(_, pending)| pending.request));
                    self.commit(std::mem::take(&mut batch));
                    return;
                }
            }
            let now = Instant::now();
            let due: Vec<String> = pending
                .iter()
                .filter(|(_, pending)| pending.due <= now)
                .map(|(path, _)| path.clone())
                .collect();
            for path in due {
                if let Some(pending) = pending.remove(&path) {
                    batch.push(pending.request);
                }
            }
            if !batch.is_empty() && deadline.is_none() {
                deadline = Some(now + self.timing.batch_window);
            }
            if now_please || deadline.is_some_and(|at| at <= now) {
                self.commit(std::mem::take(&mut batch));
                deadline = None;
            }
            for done in waiting {
                let _ = done.send(());
            }
            if close {
                self.retention(true);
            }
        }
    }

    /// A retention pass, when one is due or `always` (the library closes).
    fn retention(&self, always: bool) {
        let now = unix_now();
        let cache = self.cache.clone();
        let result = self
            .store
            .write(&self.maintenance, move |transaction| {
                if always || gc::pass_due(transaction, now)? {
                    let removed = gc::retention_pass(transaction, now)?;
                    if removed.versions > 0 {
                        cache.lock().0.clear();
                        log::info!(
                            "history: retention removed {} versions and {} blobs",
                            removed.versions,
                            removed.blobs
                        );
                    }
                }
                Ok(())
            })
            .recv();
        if let Ok(Err(error)) = result {
            log::warn!("history: retention pass failed: {error}");
        }
    }

    /// Inserts `batch` in one transaction and waits for it.
    fn commit(&self, mut batch: Vec<Request>) {
        if batch.is_empty() {
            return;
        }
        // Oldest first, so each note's versions keep their order; the sort
        // is stable for requests of the same second.
        batch.sort_by_key(|request| request.ts);
        let device = self.device.clone();
        let cache = self.cache.clone();
        let result = self
            .store
            .write(&self.token, move |transaction| {
                transaction
                    .prepare_cached(
                        "INSERT INTO device (id, name) VALUES (?1, ?2)
                         ON CONFLICT (id) DO UPDATE SET name = excluded.name",
                    )?
                    .execute(params![device.id, device.name])?;
                let mut cache = cache.lock();
                for request in &batch {
                    insert_version(transaction, &device, &mut cache, request)?;
                }
                Ok(())
            })
            .recv();
        match result {
            Ok(Err(error)) => {
                // The `.md` files are written already; only versions miss.
                self.cache.lock().0.clear();
                if !self.noticed.swap(true, Ordering::SeqCst) {
                    (self.notice)(failure_message(&error));
                }
            }
            // Rolled back: the cache may hold what was not committed.
            Ok(Ok(false)) => self.cache.lock().0.clear(),
            Ok(Ok(true)) => {}
            Err(_) => log::warn!("history: the store writer stopped"),
        }
    }
}

#[cfg(test)]
mod tests;
