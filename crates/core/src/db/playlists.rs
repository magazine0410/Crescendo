use super::Db;
use crate::db::queries::{track_from_row, TRACK_COLS};
use crate::model::Track;
use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct PlaylistInfo {
    pub id: i64,
    pub name: String,
    pub track_count: i64,
    pub total_duration_ms: i64,
    /// Collection this playlist belongs to, if any.
    pub collection_id: Option<i64>,
}

/// One playlist's tracks, kept in its own group so a view of several
/// playlists can head each run and map a row back to the playlist it
/// belongs to.
#[derive(Debug, Clone, Serialize)]
pub struct PlaylistTracks {
    pub playlist_id: i64,
    pub tracks: Vec<Track>,
}

impl Db {
    pub fn playlist_list(&self) -> Result<Vec<PlaylistInfo>> {
        self.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT p.id, p.name,
                        COUNT(pt.track_id),
                        COALESCE(SUM(t.duration_ms), 0),
                        p.collection_id
                 FROM playlists p
                 LEFT JOIN playlist_tracks pt ON pt.playlist_id = p.id
                 LEFT JOIN tracks t ON t.id = pt.track_id
                 GROUP BY p.id ORDER BY p.position, p.id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(PlaylistInfo {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    track_count: r.get(2)?,
                    total_duration_ms: r.get(3)?,
                    collection_id: r.get(4)?,
                })
            })?;
            rows.map(|r| Ok(r?)).collect()
        })
    }

    pub fn playlist_create(&self, name: &str) -> Result<i64> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO playlists(name, position)
                 VALUES (?1, (SELECT COALESCE(MAX(position), 0) + 1 FROM playlists))",
                [name],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// Store the order of the playlists themselves (the sidebar order), by
    /// writing `position` in the order the ids are given. Ids that are not
    /// listed keep their old position, so a stale list cannot lose a playlist.
    pub fn playlists_set_order(&self, ids: &[i64]) -> Result<()> {
        self.with(|conn| {
            let tx = conn.transaction()?;
            {
                let mut upd = tx.prepare("UPDATE playlists SET position = ?2 WHERE id = ?1")?;
                for (i, id) in ids.iter().enumerate() {
                    upd.execute(rusqlite::params![id, i as i64])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
    }

    pub fn playlist_rename(&self, id: i64, name: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "UPDATE playlists SET name = ?2 WHERE id = ?1",
                rusqlite::params![id, name],
            )?;
            Ok(())
        })
    }

    pub fn playlist_delete(&self, id: i64) -> Result<()> {
        self.with(|conn| {
            conn.execute("DELETE FROM playlist_tracks WHERE playlist_id = ?1", [id])?;
            conn.execute("DELETE FROM playlists WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    pub fn playlist_tracks(&self, id: i64) -> Result<Vec<Track>> {
        self.with(|conn| {
            let sql = format!(
                "SELECT {TRACK_COLS} FROM playlist_tracks pt
                 JOIN tracks ON tracks.id = pt.track_id
                 WHERE pt.playlist_id = ?1 ORDER BY pt.position"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([id], track_from_row)?;
            rows.map(|r| Ok(r?)).collect()
        })
    }

    /// Tracks of several playlists at once, one group per id and in the order
    /// the ids are given. Ids that no longer exist come back as empty groups,
    /// so the caller's grouping never shifts under it.
    pub fn playlist_tracks_multi(&self, ids: &[i64]) -> Result<Vec<PlaylistTracks>> {
        self.with(|conn| {
            let sql = format!(
                "SELECT {TRACK_COLS} FROM playlist_tracks pt
                 JOIN tracks ON tracks.id = pt.track_id
                 WHERE pt.playlist_id = ?1 ORDER BY pt.position"
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                let tracks = stmt
                    .query_map([id], track_from_row)?
                    .collect::<rusqlite::Result<Vec<Track>>>()?;
                out.push(PlaylistTracks {
                    playlist_id: *id,
                    tracks,
                });
            }
            Ok(out)
        })
    }

    pub fn playlist_add_tracks(&self, id: i64, track_ids: &[i64]) -> Result<()> {
        self.with(|conn| {
            let tx = conn.transaction()?;
            append_tracks(&tx, id, track_ids)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Playlist ids that already contain `track_id` (used to grey out
    /// "Add to playlist" targets a song is already a member of).
    /// Track ids in playlist order, windowed in SQL. The plugin host uses this
    /// instead of `playlist_tracks`: loading every full `Track` and then
    /// slicing defeats the window and would block the single Wasm runtime
    /// thread on a large playlist.
    pub fn playlist_track_ids_page(&self, id: i64, offset: i64, limit: i64) -> Result<Vec<i64>> {
        self.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1 \
                 ORDER BY position LIMIT ?2 OFFSET ?3",
            )?;
            let rows = stmt.query_map(rusqlite::params![id, limit, offset], |row| row.get(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<i64>>>()?)
        })
    }

    /// Append tracks, refusing unknown playlist or track ids in the *same*
    /// transaction that inserts them. `playlist_tracks` carries no foreign
    /// keys, so without this a caller can persist rows pointing at nothing —
    /// reachable from a `playlists.write` plugin, which supplies raw ids.
    ///
    /// Checking in one transaction and inserting in another would leave a gap
    /// in which a concurrent delete removes the playlist between the two, so
    /// both halves run under a single `BEGIN`.
    pub fn playlist_add_tracks_checked(&self, id: i64, track_ids: &[i64]) -> Result<()> {
        self.with(|conn| {
            let tx = conn.transaction()?;
            let playlist_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM playlists WHERE id = ?1)",
                [id],
                |row| row.get(0),
            )?;
            if !playlist_exists {
                anyhow::bail!("playlist {id} does not exist");
            }
            {
                let mut check = tx.prepare("SELECT EXISTS(SELECT 1 FROM tracks WHERE id = ?1)")?;
                for track_id in track_ids {
                    let exists: bool = check.query_row([track_id], |row| row.get(0))?;
                    if !exists {
                        anyhow::bail!("track {track_id} does not exist");
                    }
                }
            }
            append_tracks(&tx, id, track_ids)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Rename, refusing an unknown playlist in the same transaction that
    /// writes the new name. Checking first and renaming afterwards would let a
    /// concurrent delete land in between — and SQLite reuses row ids, so the
    /// write could even reach a different playlist created since the check.
    pub fn playlist_rename_checked(&self, id: i64, name: &str) -> Result<()> {
        self.with(|conn| {
            let tx = conn.transaction()?;
            let changed = tx.execute(
                "UPDATE playlists SET name = ?2 WHERE id = ?1",
                rusqlite::params![id, name],
            )?;
            if changed == 0 {
                anyhow::bail!("playlist {id} does not exist");
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// Remove positions, refusing an unknown playlist in the same transaction.
    /// Positions that do not exist are ignored, as they are for the UI's own
    /// removal, but a missing playlist is an error rather than a silent no-op.
    pub fn playlist_remove_tracks_checked(&self, id: i64, positions: &[i64]) -> Result<()> {
        self.with(|conn| {
            let tx = conn.transaction()?;
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM playlists WHERE id = ?1)",
                [id],
                |row| row.get(0),
            )?;
            if !exists {
                anyhow::bail!("playlist {id} does not exist");
            }
            {
                let mut del = tx.prepare(
                    "DELETE FROM playlist_tracks WHERE playlist_id = ?1 AND position = ?2",
                )?;
                for position in positions {
                    del.execute(rusqlite::params![id, position])?;
                }
            }
            renumber(&tx, id)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Whether a playlist exists, for host-boundary checks that must not
    /// silently no-op on a bad id.
    pub fn playlist_exists(&self, id: i64) -> Result<bool> {
        self.with(|conn| {
            Ok(conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM playlists WHERE id = ?1)",
                [id],
                |row| row.get(0),
            )?)
        })
    }

    pub fn playlist_ids_with_track(&self, track_id: i64) -> Result<Vec<i64>> {
        self.with(|conn| {
            let mut stmt = conn
                .prepare("SELECT DISTINCT playlist_id FROM playlist_tracks WHERE track_id = ?1")?;
            let ids = stmt
                .query_map([track_id], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<i64>>>()?;
            Ok(ids)
        })
    }

    pub fn playlist_remove_tracks(&self, id: i64, positions: &[i64]) -> Result<()> {
        self.with(|conn| {
            let tx = conn.transaction()?;
            {
                let mut del = tx.prepare(
                    "DELETE FROM playlist_tracks WHERE playlist_id = ?1 AND position = ?2",
                )?;
                for pos in positions {
                    del.execute(rusqlite::params![id, pos])?;
                }
            }
            renumber(&tx, id)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Move the tracks at `from` positions (sorted ascending) so the block
    /// starts at `to` (position after removal).
    pub fn playlist_reorder(&self, id: i64, from: &[i64], to: i64) -> Result<()> {
        self.with(|conn| {
            let tx = conn.transaction()?;
            let all: Vec<(i64, i64)> = {
                let mut stmt = tx.prepare(
                    "SELECT position, track_id FROM playlist_tracks \
                     WHERE playlist_id = ?1 ORDER BY position",
                )?;
                let rows =
                    stmt.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
                rows.collect::<rusqlite::Result<_>>()?
            };

            let moving: Vec<i64> = all
                .iter()
                .filter(|(pos, _)| from.contains(pos))
                .map(|&(_, tid)| tid)
                .collect();
            let mut rest: Vec<i64> = all
                .iter()
                .filter(|(pos, _)| !from.contains(pos))
                .map(|&(_, tid)| tid)
                .collect();
            let insert_at = (to.max(0) as usize).min(rest.len());
            for (i, tid) in moving.into_iter().enumerate() {
                rest.insert(insert_at + i, tid);
            }

            tx.execute("DELETE FROM playlist_tracks WHERE playlist_id = ?1", [id])?;
            {
                let mut ins = tx.prepare(
                    "INSERT INTO playlist_tracks(playlist_id, position, track_id) \
                     VALUES (?1, ?2, ?3)",
                )?;
                for (i, tid) in rest.iter().enumerate() {
                    ins.execute(rusqlite::params![id, i as i64, tid])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
    }
}

fn renumber(conn: &rusqlite::Connection, id: i64) -> Result<()> {
    // Compact positions to 0..n keeping order.
    let mut stmt = conn
        .prepare("SELECT position FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position")?;
    let positions: Vec<i64> = stmt
        .query_map([id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut update = conn.prepare(
        "UPDATE playlist_tracks SET position = ?3 \
         WHERE playlist_id = ?1 AND position = ?2",
    )?;
    for (i, pos) in positions.iter().enumerate() {
        if *pos != i as i64 {
            update.execute(rusqlite::params![id, pos, i as i64])?;
        }
    }
    Ok(())
}

/// Append tracks inside a caller's transaction, skipping songs the playlist
/// already holds so the same track is never appended twice (deduped against
/// the playlist's current members; repeats within `track_ids` itself are
/// preserved). Split out of [`Db::playlist_add_tracks`] so the checked variant
/// can validate and insert without ever committing in between.
fn append_tracks(tx: &rusqlite::Transaction<'_>, id: i64, track_ids: &[i64]) -> Result<()> {
    let existing: std::collections::HashSet<i64> = {
        let mut stmt = tx.prepare("SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1")?;
        let set = stmt
            .query_map([id], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<_>>()?;
        set
    };
    let mut next: i64 = tx.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_tracks \
         WHERE playlist_id = ?1",
        [id],
        |r| r.get(0),
    )?;
    let mut insert = tx.prepare(
        "INSERT INTO playlist_tracks(playlist_id, position, track_id) \
         VALUES (?1, ?2, ?3)",
    )?;
    for tid in track_ids {
        if existing.contains(tid) {
            continue;
        }
        insert.execute(rusqlite::params![id, next, tid])?;
        next += 1;
    }
    Ok(())
}
