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
    Ok(Some(Prepared {
        id,
        change: change.clone(),
        meta,
        mtime: stat.modified()?.duration_since(UNIX_EPOCH)?.as_secs() as i64,
        size: stat.len() as i64,
    }))
}

/// Write prepared changes in one transaction, so renames that exchange names
/// either all land or none do.
fn commit(db: &Db, pending: &[Prepared]) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    db.with(|conn| {
        let tx = conn.transaction()?;
        // Release all old names within the transaction before assigning the
        // final names, including offline chains that exchanged two filenames.
        for p in pending {
            if p.change.old_path != p.change.new_path {
                tx.execute(
                    "UPDATE tracks SET path = ?1 WHERE id = ?2",
                    rusqlite::params![format!("forte://pending/{}", p.id), p.id],
                )?;
            }
        }
        for p in pending {
            let new = Path::new(&p.change.new_path);
            tx.execute(
                "UPDATE tracks SET path = ?1 WHERE id = ?2",
                rusqlite::params![p.change.new_path, p.id],
            )?;
            let mut stmt = tx.prepare(scanner::UPSERT_TRACK)?;
            scanner::upsert_row(
                &mut stmt,
                &p.change.new_path,
                &new.parent().unwrap_or(Path::new("")).to_string_lossy(),
                &new.file_name().unwrap_or_default().to_string_lossy(),
                p.mtime,
                p.size,
                &p.meta,
            )?;
        }
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
    for path in &paths {
        let notice = std::fs::read(path)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| Ok(serde_json::from_slice::<Notice>(&bytes)?));
        let notice = match notice {
            Ok(notice) => notice,
            Err(error) => {
                // Possibly still being written. Notices are applied in order,
                // so wait for it rather than apply the ones after it.
                let age = std::fs::metadata(path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                    .unwrap_or_default();
                if age < UNREADABLE_NOTICE_GRACE {
                    return Ok(outcome);
                }
                set_aside_file(inbox, path)?;
                outcome
                    .set_aside
                    .push(format!("{}: unreadable notice: {error:#}", path.display()));
                continue;
            }
        };
        for change in notice.changes {
            let original = aliases.remove(&change.old_path).unwrap_or(change.old_path);
            aliases.insert(change.new_path.clone(), original.clone());
            combined.insert(original, change.new_path);
        }
    }

    let mut prepared = Vec::new();
    for (old_path, new_path) in combined {
        let change = FileChange { old_path, new_path };
        match prepare(db, &change) {
            Ok(Some(p)) => prepared.push(p),
            Ok(None) => {}
            // A busy database is worth another try; a missing or unreadable
            // file is not.
            Err(error) if error.downcast_ref::<rusqlite::Error>().is_some() => return Err(error),
            Err(error) => failed.push((change, format!("{error:#}"))),
        }
    }
    match commit(db, &prepared) {
        Ok(()) => outcome.changed = !prepared.is_empty(),
        Err(error) if !is_constraint_violation(&error) => return Err(error),
        Err(_) => {
            // One destination already belongs to another track, typically
            // because a scan reached the renamed file first. Apply the others
            // one at a time; only names exchanged within this batch needed to
            // land together, and those fail here as well. Returning early
            // leaves the notices for another round, which skips changes that
            // already landed: their old path no longer has a track.
            for p in prepared {
                match commit(db, std::slice::from_ref(&p)) {
                    Ok(()) => outcome.changed = true,
                    Err(error) if is_constraint_violation(&error) => {
                        failed.push((p.change, format!("{error:#}")))
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }

    if !failed.is_empty() {
        write_failed(inbox, &failed)?;
        outcome.set_aside.extend(
            failed.into_iter().map(|(change, error)| {
                format!("{} → {}: {error}", change.old_path, change.new_path)
            }),
        );
    }
    for path in paths {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
    }
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
fn write_failed(inbox: &Path, failed: &[(FileChange, String)]) -> Result<()> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
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
        failed_dir(inbox)?.join(format!("{stamp}.json")),
        serde_json::to_vec_pretty(&serde_json::json!({ "changes": record }))?,
    )?;
    Ok(())
}
