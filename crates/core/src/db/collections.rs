//! Collections: a named group a playlist can belong to, so a long sidebar
//! can be folded into a few tidy rows.
//!
//! A playlist belongs to at most one collection (`playlists.collection_id`).
//! Counts are plain sums of the member playlists — a track sitting in two of
//! them counts twice, which is what the sidebar's totals mean.

use super::Db;
use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CollectionInfo {
    pub id: i64,
    pub name: String,
    pub collapsed: bool,
    pub playlist_count: i64,
    /// Sum of the member playlists' track counts, duplicates included.
    pub track_count: i64,
    pub total_duration_ms: i64,
}

impl Db {
    pub fn collection_list(&self) -> Result<Vec<CollectionInfo>> {
        self.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT c.id, c.name, c.collapsed,
                        (SELECT COUNT(*) FROM playlists p WHERE p.collection_id = c.id),
                        (SELECT COUNT(*) FROM playlist_tracks pt
                           JOIN playlists p ON p.id = pt.playlist_id
                          WHERE p.collection_id = c.id),
                        (SELECT COALESCE(SUM(t.duration_ms), 0) FROM playlist_tracks pt
                           JOIN playlists p ON p.id = pt.playlist_id
                           JOIN tracks t ON t.id = pt.track_id
                          WHERE p.collection_id = c.id)
                 FROM collections c ORDER BY c.position, c.id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(CollectionInfo {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    collapsed: r.get::<_, i64>(2)? != 0,
                    playlist_count: r.get(3)?,
                    track_count: r.get(4)?,
                    total_duration_ms: r.get(5)?,
                })
            })?;
            rows.map(|r| Ok(r?)).collect()
        })
    }

    /// Put `playlist_ids` into the collection called `name`, creating it if no
    /// collection has that name yet (matched case-insensitively, so "Video
    /// games" and "Video Games" stay one group). A blank name takes the
    /// playlists out of whatever collection they were in.
    ///
    /// Returns the collection they ended up in, or None when removed.
    /// Collections left without members are deleted: they have no rows of
    /// their own to hold, and an empty group would just be a dead sidebar row.
    pub fn collection_set(&self, playlist_ids: &[i64], name: &str) -> Result<Option<i64>> {
        let name = name.trim();
        self.with(|conn| {
            conn.execute_batch("BEGIN")?;
            let target: Option<i64> = if name.is_empty() {
                None
            } else {
                let existing: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM collections WHERE name = ?1 COLLATE NOCASE",
                        [name],
                        |r| r.get(0),
                    )
                    .ok();
                Some(match existing {
                    Some(id) => id,
                    None => {
                        conn.execute(
                            "INSERT INTO collections(name, position)
                             VALUES (?1, (SELECT COALESCE(MAX(position), 0) + 1 FROM collections))",
                            [name],
                        )?;
                        conn.last_insert_rowid()
                    }
                })
            };
            {
                let mut upd =
                    conn.prepare("UPDATE playlists SET collection_id = ?2 WHERE id = ?1")?;
                for id in playlist_ids {
                    upd.execute(rusqlite::params![id, target])?;
                }
            }
            conn.execute(
                "DELETE FROM collections WHERE NOT EXISTS
                   (SELECT 1 FROM playlists p WHERE p.collection_id = collections.id)",
                [],
            )?;
            conn.execute_batch("COMMIT")?;
            Ok(target)
        })
    }

    pub fn collection_rename(&self, id: i64, name: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "UPDATE collections SET name = ?2 WHERE id = ?1",
                rusqlite::params![id, name.trim()],
            )?;
            Ok(())
        })
    }

    /// Drop the collection. Its playlists stay, loose again — deleting a group
    /// must never delete what it grouped.
    pub fn collection_delete(&self, id: i64) -> Result<()> {
        self.with(|conn| {
            conn.execute_batch("BEGIN")?;
            conn.execute("UPDATE playlists SET collection_id = NULL WHERE collection_id = ?1", [id])?;
            conn.execute("DELETE FROM collections WHERE id = ?1", [id])?;
            conn.execute_batch("COMMIT")?;
            Ok(())
        })
    }

    pub fn collection_set_collapsed(&self, id: i64, collapsed: bool) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "UPDATE collections SET collapsed = ?2 WHERE id = ?1",
                rusqlite::params![id, i64::from(collapsed)],
            )?;
            Ok(())
        })
    }
}
