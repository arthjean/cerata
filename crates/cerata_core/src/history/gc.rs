//! Retention passes and purges (US-090).
//!
//! A pass drops the versions the tiers no longer keep
//! ([`super::retention::kept`]), then the blobs no version and no note's
//! head refers to. A blob a kept delta was built on is re-encoded first on
//! the nearest blob that stays, so nothing kept depends on what goes.
//!
//! A purge deletes versions at the user's request and must leave none of
//! their bytes in the store's files: `secure_delete` zeroes the freed
//! pages, and the purge then truncates the WAL, vacuums and truncates it
//! again. A flag in `meta`, committed with the deletion, says a vacuum is
//! owed: an app closed during the vacuum vacuums at the next opening.

use super::retention::{Dated, kept};
use super::{hash_of, reencode_blob};
use crate::store::ContentHash;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::{HashMap, HashSet};

/// `meta` key: Unix time of the last retention pass.
pub const LAST_PASS: &str = "last_retention";
/// `meta` key: a purge committed its deletions but not yet its vacuum.
pub const VACUUM_OWED: &str = "vacuum_owed";

/// A pass runs at most this often.
pub const PASS_INTERVAL_SECS: i64 = 24 * 3600;

/// What a pass or a purge removed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Removed {
    pub versions: usize,
    pub blobs: usize,
}

pub fn meta_get(connection: &Connection, key: &str) -> rusqlite::Result<Option<i64>> {
    connection
        .prepare_cached("SELECT value FROM meta WHERE key = ?1")?
        .query_row([key], |row| row.get(0))
        .optional()
}

pub fn meta_set(connection: &Connection, key: &str, value: Option<i64>) -> rusqlite::Result<()> {
    match value {
        Some(value) => connection
            .prepare_cached(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            )?
            .execute(params![key, value])?,
        None => connection
            .prepare_cached("DELETE FROM meta WHERE key = ?1")?
            .execute([key])?,
    };
    Ok(())
}

/// Whether a pass is due at `now`.
pub fn pass_due(connection: &Connection, now: i64) -> rusqlite::Result<bool> {
    Ok(meta_get(connection, LAST_PASS)?.is_none_or(|last| now - last >= PASS_INTERVAL_SECS))
}

/// Applies the tiers at `now` to every note, then collects the blobs.
pub fn retention_pass(transaction: &Transaction, now: i64) -> rusqlite::Result<Removed> {
    let mut by_note: HashMap<i64, Vec<(i64, Dated)>> = HashMap::new();
    {
        let mut statement =
            transaction.prepare("SELECT note, seq, ts, pinned FROM version ORDER BY note, seq")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                Dated {
                    ts: row.get(2)?,
                    pinned: row.get::<_, i64>(3)? != 0,
                },
            ))
        })?;
        for row in rows {
            let (note, seq, dated) = row?;
            by_note.entry(note).or_default().push((seq, dated));
        }
    }
    let mut removed = Removed::default();
    let mut delete =
        transaction.prepare_cached("DELETE FROM version WHERE note = ?1 AND seq = ?2")?;
    for (note, versions) in by_note {
        let dated: Vec<Dated> = versions.iter().map(|(_, dated)| *dated).collect();
        for ((seq, _), keep) in versions.iter().zip(kept(&dated, now)) {
            if !keep {
                removed.versions += delete.execute(params![note, seq])?;
            }
        }
    }
    removed.blobs = collect_blobs(transaction)?;
    meta_set(transaction, LAST_PASS, Some(now))?;
    Ok(removed)
}

/// Deletes the blobs no version and no note's head refers to, after
/// re-encoding the kept deltas built on them. Returns how many went.
pub fn collect_blobs(transaction: &Transaction) -> rusqlite::Result<usize> {
    let orphans: HashSet<ContentHash> = {
        let mut statement = transaction.prepare(
            "SELECT hash FROM blob
             WHERE hash NOT IN (SELECT hash FROM version)
               AND hash NOT IN (SELECT hash FROM note WHERE hash IS NOT NULL)",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
        rows.filter_map(|row| row.map(hash_of).transpose())
            .collect::<rusqlite::Result<_>>()?
    };
    if orphans.is_empty() {
        return Ok(0);
    }
    // Kept blobs whose base goes, with the nearest base that stays.
    let mut base_of = transaction.prepare_cached("SELECT base FROM blob WHERE hash = ?1")?;
    let mut dependents = transaction.prepare_cached("SELECT hash FROM blob WHERE base = ?1")?;
    let mut rebased: Vec<(ContentHash, Option<ContentHash>)> = Vec::new();
    for orphan in &orphans {
        let children: Vec<Vec<u8>> = dependents
            .query_map([orphan.as_slice()], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let children: Vec<ContentHash> = children
            .into_iter()
            .filter_map(hash_of)
            .filter(|child| !orphans.contains(child))
            .collect();
        if children.is_empty() {
            continue;
        }
        let mut base = Some(*orphan);
        while let Some(current) = base.filter(|current| orphans.contains(current)) {
            base = base_of
                .query_row([current.as_slice()], |row| row.get::<_, Option<Vec<u8>>>(0))
                .optional()?
                .flatten()
                .and_then(hash_of);
        }
        rebased.extend(children.into_iter().map(|child| (child, base)));
    }
    drop(base_of);
    drop(dependents);
    for (child, base) in &rebased {
        reencode_blob(transaction, child, base.as_ref())?;
    }
    let mut delete = transaction.prepare_cached("DELETE FROM blob WHERE hash = ?1")?;
    let mut count = 0;
    for orphan in &orphans {
        count += delete.execute([orphan.as_slice()])?;
    }
    Ok(count)
}

/// Which versions a purge deletes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purge {
    /// Every version of one note.
    Note(i64),
    /// Every version of every note older than this Unix time.
    Before(i64),
}

impl Purge {
    fn filter(self) -> (&'static str, i64) {
        match self {
            Self::Note(id) => ("note = ?1", id),
            Self::Before(ts) => ("ts < ?1", ts),
        }
    }
}

/// How many versions `purge` would delete.
pub fn purge_count(connection: &Connection, purge: Purge) -> rusqlite::Result<usize> {
    let (filter, value) = purge.filter();
    let count: i64 = connection.query_row(
        &format!("SELECT COUNT(*) FROM version WHERE {filter}"),
        [value],
        |row| row.get(0),
    )?;
    Ok(usize::try_from(count).unwrap_or(0))
}

/// Deletes the versions of `purge` and their blobs, and records that a
/// vacuum is owed; [`vacuum`] must follow once committed.
pub fn purge_versions(transaction: &Transaction, purge: Purge) -> rusqlite::Result<Removed> {
    let (filter, value) = purge.filter();
    let versions = transaction.execute(&format!("DELETE FROM version WHERE {filter}"), [value])?;
    let blobs = collect_blobs(transaction)?;
    meta_set(transaction, VACUUM_OWED, Some(1))?;
    Ok(Removed { versions, blobs })
}

/// Rewrites the store without the freed pages and empties the WAL, then
/// clears the owed vacuum. Outside any transaction.
pub fn vacuum(connection: &Connection) -> rusqlite::Result<()> {
    checkpoint(connection)?;
    connection.execute_batch("VACUUM")?;
    checkpoint(connection)?;
    meta_set(connection, VACUUM_OWED, None)?;
    checkpoint(connection)
}

fn checkpoint(connection: &Connection) -> rusqlite::Result<()> {
    let busy: i64 =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
    if busy != 0 {
        log::warn!("store: the WAL could not be truncated, a reader holds it");
    }
    Ok(())
}
