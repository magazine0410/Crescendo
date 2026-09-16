//! Refresh files saved by the Forte GUI without replacing track identities.
use crate::{scanner, Db};
use anyhow::{bail, Context, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::UNIX_EPOCH;

#[derive(Debug, Deserialize, Serialize)]
pub struct FileChange {
    pub old_path: String,
    pub new_path: String,
}

pub fn refresh_files(db: &Db, changes: &[FileChange]) -> Result<()> {
    let mut pending = Vec::new();
    for change in changes {
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
            continue;
        };
        let old = Path::new(&change.old_path);
        let new = Path::new(&change.new_path);
        if old.parent() != new.parent() || !scanner::is_audio_file(new) {
            bail!("Forte returned an invalid rename");
        }
        let meta = scanner::tags::read_meta(new)
            .with_context(|| format!("Read edited file {}", new.display()))?;
        let stat = new.metadata()?;
        pending.push((id, change, meta, stat));
    }
    if pending.is_empty() {
        return Ok(());
    }
    db.with(|conn| {
        let tx = conn.transaction()?;
        // Release all old names within the transaction before assigning the
        // final names, including offline chains that exchanged two filenames.
        for (id, change, _, _) in &pending {
            if change.old_path != change.new_path {
                tx.execute(
                    "UPDATE tracks SET path = ?1 WHERE id = ?2",
                    rusqlite::params![format!("forte://pending/{id}"), id],
                )?;
            }
        }
        for (id, change, meta, stat) in &pending {
            let new = Path::new(&change.new_path);
            tx.execute(
                "UPDATE tracks SET path = ?1 WHERE id = ?2",
                rusqlite::params![change.new_path, id],
            )?;
            let mut stmt = tx.prepare(scanner::UPSERT_TRACK)?;
            scanner::upsert_row(
                &mut stmt,
                &change.new_path,
                &new.parent().unwrap_or(Path::new("")).to_string_lossy(),
                &new.file_name().unwrap_or_default().to_string_lossy(),
                stat.modified()?.duration_since(UNIX_EPOCH)?.as_secs() as i64,
                stat.len() as i64,
                meta,
            )?;
        }
        scanner::albums::rebuild_in_transaction(&tx)?;
        tx.commit()?;
        Ok(())
    })
}

/// Forte leaves durable notifications when it saves independently or while
/// Crescendo is closed. Collapse successive renames before reading files.
pub fn refresh_pending(db: &Db, inbox: &Path) -> Result<bool> {
    if !inbox.is_dir() {
        return Ok(false);
    }
    let mut paths: Vec<_> = std::fs::read_dir(inbox)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    paths.sort();
    if paths.is_empty() {
        return Ok(false);
    }
    #[derive(Deserialize)]
    struct Notice {
        changes: Vec<FileChange>,
    }
    let mut aliases = std::collections::HashMap::new();
    let mut combined = std::collections::BTreeMap::new();
    for path in &paths {
        let notice: Notice = serde_json::from_reader(std::fs::File::open(path)?)?;
        for change in notice.changes {
            let original = aliases.remove(&change.old_path).unwrap_or(change.old_path);
            aliases.insert(change.new_path.clone(), original.clone());
            combined.insert(original, change.new_path);
        }
    }
    let changes: Vec<_> = combined
        .into_iter()
        .map(|(old_path, new_path)| FileChange { old_path, new_path })
        .collect();
    refresh_files(db, &changes)?;
    for path in paths {
        std::fs::remove_file(path)?;
    }
    Ok(true)
}
