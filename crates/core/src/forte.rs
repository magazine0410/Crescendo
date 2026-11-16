//! Refresh files saved by the Forte GUI without replacing track identities.
use crate::scanner::tags::TrackMeta;
use crate::{scanner, Db};
use anyhow::{bail, Context, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileChange {
    pub old_path: String,
    pub new_path: String,
}

/// A change whose track and edited file have been read, ready to write.
struct Prepared {
    id: i64,
    change: FileChange,
    meta: TrackMeta,
    mtime: i64,
    size: i64,
    added_at: i64,
}

/// Look up the track a change belongs to and read its edited file. None when
/// no track has the old path, which leaves nothing to update.
fn prepare(db: &Db, change: &FileChange) -> Result<Option<Prepared>> {
    let id: Option<i64> = db.with(|conn| {
        Ok(conn
            .query_row(
                "SELECT id FROM tracks WHERE path = ?1",
                [&change.old_path],
                |row| row.get(0),
            )
            .optional()?)
    })?;
    let Some(id) = id else {
        return Ok(None);
    };
    let old = Path::new(&change.old_path);
    let new = Path::new(&change.new_path);
    if old.parent() != new.parent() || !scanner::is_audio_file(new) {
        bail!("Forte returned an invalid rename");
    }
    let meta = scanner::tags::read_meta(new)
        .with_context(|| format!("Read edited file {}", new.display()))?;
    let stat = new.metadata()?;
    let mtime = stat.modified()?.duration_since(UNIX_EPOCH)?.as_secs() as i64;
    Ok(Some(Prepared {
        id,
        change: change.clone(),
        meta,
        mtime,
        size: stat.len() as i64,
        // Kept only if the row has no date added yet.
        added_at: scanner::added_time(&stat, mtime),
    }))
}

/// Write prepared changes, which land together or fail together.
fn write_changes(conn: &rusqlite::Connection, pending: &[Prepared]) -> Result<()> {
    // Release all old names before assigning the final names, including offline
    // chains that exchanged two filenames.
    for p in pending {
        if p.change.old_path != p.change.new_path {
            conn.execute(
                "UPDATE tracks SET path = ?1 WHERE id = ?2",
                rusqlite::params![format!("forte://pending/{}", p.id), p.id],
            )?;
        }
    }
    for p in pending {
        let new = Path::new(&p.change.new_path);
        conn.execute(
            "UPDATE tracks SET path = ?1 WHERE id = ?2",
            rusqlite::params![p.change.new_path, p.id],
        )?;
        let mut stmt = conn.prepare(scanner::UPSERT_TRACK)?;
        scanner::upsert_row(
            &mut stmt,
            &p.change.new_path,
            &new.parent().unwrap_or(Path::new("")).to_string_lossy(),
            &new.file_name().unwrap_or_default().to_string_lossy(),
            p.mtime,
            p.size,
            p.added_at,
            &p.meta,
        )?;
    }
    Ok(())
}

fn commit(db: &Db, pending: &[Prepared]) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    db.with(|conn| {
        let tx = conn.transaction()?;
        write_changes(&tx, pending)?;
        scanner::albums::rebuild_in_transaction(&tx)?;
        tx.commit()?;
        Ok(())
    })
}

pub fn refresh_files(db: &Db, changes: &[FileChange]) -> Result<()> {
    let mut pending = Vec::new();
    for change in changes {
        if let Some(prepared) = prepare(db, change)? {
            pending.push(prepared);
        }
    }
    commit(db, &pending)
}

/// What [`refresh_pending`] did with the inbox.
#[derive(Debug, Default)]
pub struct PendingRefresh {
    /// At least one track was updated.
    pub changed: bool,
    /// One message per change that could not be applied. Each was moved to
    /// the inbox's `failed` folder and will not be retried.
    pub set_aside: Vec<String>,
}

/// A notice that still does not parse after this long was not caught mid-write.
const UNREADABLE_NOTICE_GRACE: Duration = Duration::from_secs(60);

/// A folder gone for this long is taken to be deleted rather than unmounted, so
/// its changes stop holding up every later save.
const MISSING_FOLDER_GRACE: Duration = Duration::from_secs(7 * 86400);

/// What became of one change while reading what it needs.
enum Preparation {
    Ready(Prepared),
    /// No track has the old path.
    NoTrack,
    /// It can never apply, such as a deleted file or a broken rename.
    Refused(anyhow::Error),
    /// Its file cannot be reached right now, which says nothing about the file.
    Unreachable(anyhow::Error),
}

fn prepare_for_inbox(db: &Db, change: &FileChange) -> Result<Preparation> {
    match prepare(db, change) {
        Ok(Some(prepared)) => Ok(Preparation::Ready(prepared)),
        Ok(None) => Ok(Preparation::NoTrack),
        // A busy database is worth another try, and not this change's fault.
        Err(error) if error.downcast_ref::<rusqlite::Error>().is_some() => Err(error),
        Err(error) => {
            let new = Path::new(&change.new_path);
            let folder_exists = new.parent().is_some_and(|folder| folder.is_dir());
            let unreachable = match new.metadata() {
                Ok(_) => false,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => !folder_exists,
                Err(_) => true,
            };
            Ok(if unreachable {
                Preparation::Unreachable(error)
            } else {
                Preparation::Refused(error)
            })
        }
    }
}

/// How long ago a notice was written. A timestamp from the future means the clock
/// moved, not that the notice is new, so it counts as old.
fn notice_age(path: &Path) -> Duration {
    match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(modified) => SystemTime::now()
            .duration_since(modified)
            .unwrap_or(Duration::MAX),
        Err(_) => Duration::ZERO,
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Finish what a crash interrupted: a notice with a mark was already applied, so
/// only its file is left to delete. A mark whose file is gone is simply forgotten.
fn clear_applied_notices(db: &Db, inbox: &Path) -> Result<()> {
    let marks: Vec<String> = db.with(|conn| {
        let mut stmt = conn.prepare("SELECT name FROM forte_applied_notices")?;
        let names = stmt
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(names)
    })?;
    // This runs on every poll, which must not take the write lock for nothing.
    if marks.is_empty() {
        return Ok(());
    }
    for name in &marks {
        match std::fs::remove_file(inbox.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    db.with(|conn| {
        conn.execute("DELETE FROM forte_applied_notices", [])?;
        Ok(())
    })
}

/// Forte leaves durable notifications when it saves independently or while
/// Crescendo is closed. Collapse successive renames before reading files.
///
/// One change that can never apply (its file was deleted, or a scan already
/// gave its new name to another track) must not hold up every later save, so
/// such changes are set aside and the rest are applied.
pub fn refresh_pending(db: &Db, inbox: &Path) -> Result<PendingRefresh> {
    let mut outcome = PendingRefresh::default();
    if !inbox.is_dir() {
        return Ok(outcome);
    }
    clear_applied_notices(db, inbox)?;
    let mut paths: Vec<_> = std::fs::read_dir(inbox)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    paths.sort();
    if paths.is_empty() {
        return Ok(outcome);
    }
    #[derive(Deserialize)]
    struct Notice {
        changes: Vec<FileChange>,
    }
    let mut failed: Vec<(FileChange, String)> = Vec::new();
    let mut aliases = std::collections::HashMap::new();
    let mut combined = std::collections::BTreeMap::new();
    let mut oldest_notice = Duration::ZERO;
    let mut set_aside_notice = false;
    for path in &paths {
        let notice = std::fs::read(path)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| Ok(serde_json::from_slice::<Notice>(&bytes)?));
        let notice = match notice {
            Ok(notice) => notice,
            Err(error) => {
                // Possibly still being written. Notices are applied in order,
                // so wait for it rather than apply the ones after it.
                let age = notice_age(path);
                if age < UNREADABLE_NOTICE_GRACE {
                    return Ok(outcome);
                }
                set_aside_file(inbox, path)?;
                set_aside_notice = true;
                outcome
                    .set_aside
                    .push(format!("{}: unreadable notice: {error:#}", path.display()));
                continue;
            }
        };
        oldest_notice = oldest_notice.max(notice_age(path));
        // One save's changes are simultaneous: each names a file before and after
        // that save, so all of them resolve against the state before this notice.
        let resolved: Vec<(String, String)> = notice
            .changes
            .iter()
            .map(|change| {
                let original = aliases
                    .get(&change.old_path)
                    .cloned()
                    .unwrap_or_else(|| change.old_path.clone());
                (original, change.new_path.clone())
            })
            .collect();
        for change in &notice.changes {
            aliases.remove(&change.old_path);
        }
        for (original, new_path) in resolved {
            aliases.insert(new_path.clone(), original.clone());
            combined.insert(original, new_path);
        }
    }

    let mut prepared = Vec::new();
    for (old_path, new_path) in combined {
        let change = FileChange { old_path, new_path };
        match prepare_for_inbox(db, &change)? {
            Preparation::Ready(p) => prepared.push(p),
            Preparation::NoTrack if set_aside_notice => {
                let reason = "no track has this name, and an earlier notice that may have \
                              renamed it could not be read";
                failed.push((change, reason.to_string()));
            }
            // Ordinary: Forte also edits files that were never in this library.
            Preparation::NoTrack => {}
            Preparation::Refused(error) => failed.push((change, format!("{error:#}"))),
            Preparation::Unreachable(error) if oldest_notice >= MISSING_FOLDER_GRACE => {
                failed.push((change, format!("{error:#}")));
            }
            // Later notices may depend on this one, so nothing applies until the
            // folder is back. The notices stay, and the caller reports the error.
            Preparation::Unreachable(error) => return Err(error),
        }
    }

    // Everything this pass does lands in one transaction with the marks saying so:
    // a crash either leaves all of it to redo, or none of it to apply twice.
    let names: Vec<String> = paths
        .iter()
        .filter(|path| path.exists())
        .map(|path| file_name(path))
        .collect();
    db.with(|conn| {
        let mut tx = conn.transaction()?;
        let together = {
            let savepoint = tx.savepoint()?;
            match write_changes(&savepoint, &prepared) {
                Ok(()) => {
                    savepoint.commit()?;
                    true
                }
                Err(error) if is_constraint_violation(&error) => false,
                Err(error) => return Err(error),
            }
        };
        if together {
            outcome.changed = !prepared.is_empty();
        } else {
            // One destination already belongs to another track, typically because
            // a scan reached the renamed file first. Apply the others one at a
            // time; only names exchanged within this batch needed to land together,
            // and those fail here as well.
            for p in &prepared {
                let savepoint = tx.savepoint()?;
                match write_changes(&savepoint, std::slice::from_ref(p)) {
                    Ok(()) => {
                        savepoint.commit()?;
                        outcome.changed = true;
                    }
                    Err(error) if is_constraint_violation(&error) => {
                        failed.push((p.change.clone(), format!("{error:#}")));
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        // A GROUP BY over the whole library, so only when something moved.
        if outcome.changed {
            scanner::albums::rebuild_in_transaction(&tx)?;
        }
        for name in &names {
            tx.execute(
                "INSERT OR IGNORE INTO forte_applied_notices(name) VALUES (?1)",
                [name],
            )?;
        }
        // Before the commit, and named after this pass's notices: a retry after a
        // failure overwrites the record instead of adding a second one.
        if !failed.is_empty() {
            write_failed(
                inbox,
                names.first().map_or("changes", String::as_str),
                &failed,
            )?;
        }
        tx.commit()?;
        Ok(())
    })?;
    outcome.set_aside.extend(
        failed
            .into_iter()
            .map(|(change, error)| format!("{} → {}: {error}", change.old_path, change.new_path)),
    );
    clear_applied_notices(db, inbox)?;
    Ok(outcome)
}

fn is_constraint_violation(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<rusqlite::Error>(),
        Some(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

fn failed_dir(inbox: &Path) -> Result<std::path::PathBuf> {
    let dir = inbox.join("failed");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn set_aside_file(inbox: &Path, path: &Path) -> Result<()> {
    let name = path.file_name().unwrap_or_default();
    std::fs::rename(path, failed_dir(inbox)?.join(name))?;
    Ok(())
}

/// Keep the changes that could not be applied, with the reason, for anyone
/// reconciling them by hand. The folder is outside the inbox's `*.json`
/// listing, so nothing reads them again.
fn write_failed(inbox: &Path, name: &str, failed: &[(FileChange, String)]) -> Result<()> {
    let record: Vec<_> = failed
        .iter()
        .map(|(change, error)| {
            serde_json::json!({
                "old_path": change.old_path,
                "new_path": change.new_path,
                "error": error,
            })
        })
        .collect();
    std::fs::write(
        failed_dir(inbox)?.join(name),
        serde_json::to_vec_pretty(&serde_json::json!({ "changes": record }))?,
    )?;
    Ok(())
}
