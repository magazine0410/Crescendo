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
}

impl Db {
    pub fn playlist_list(&self) -> Result<Vec<PlaylistInfo>> {
        self.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT p.id, p.name,
                        COUNT(pt.track_id),
                        COALESCE(SUM(t.duration_ms), 0)
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

    pub fn playlist_rename(&self, id: i64, name: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute("UPDATE playlists SET name = ?2 WHERE id = ?1",
                         rusqlite::params![id, name])?;
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

    pub fn playlist_add_tracks(&self, id: i64, track_ids: &[i64]) -> Result<()> {
        self.with(|conn| {
            // Skip tracks already in the playlist so the same song is never
            // appended twice (deduped against the playlist's current members;
            // repeats within `track_ids` itself are preserved).
            let existing: std::collections::HashSet<i64> = {
                let mut stmt =
                    conn.prepare("SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1")?;
                let set = stmt
                    .query_map([id], |r| r.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<_>>()?;
                set
            };
            let mut next: i64 = conn.query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_tracks \
                 WHERE playlist_id = ?1",
                [id],
                |r| r.get(0),
            )?;
            let mut insert = conn.prepare(
                "INSERT INTO playlist_tracks(playlist_id, position, track_id) \
                 VALUES (?1, ?2, ?3)",
            )?;
            conn.execute_batch("BEGIN")?;
            for tid in track_ids {
                if existing.contains(tid) {
                    continue;
                }
                insert.execute(rusqlite::params![id, next, tid])?;
                next += 1;
            }
            conn.execute_batch("COMMIT")?;
            Ok(())
        })
    }

    /// Playlist ids that already contain `track_id` (used to grey out
    /// "Add to playlist" targets a song is already a member of).
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
            conn.execute_batch("BEGIN")?;
            {
                let mut del = conn.prepare(
                    "DELETE FROM playlist_tracks WHERE playlist_id = ?1 AND position = ?2",
                )?;
                for pos in positions {
                    del.execute(rusqlite::params![id, pos])?;
                }
            }
            renumber(conn, id)?;
            conn.execute_batch("COMMIT")?;
            Ok(())
        })
    }

    /// Move the tracks at `from` positions (sorted ascending) so the block
    /// starts at `to` (position after removal).
    pub fn playlist_reorder(&self, id: i64, from: &[i64], to: i64) -> Result<()> {
        self.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT position, track_id FROM playlist_tracks \
                 WHERE playlist_id = ?1 ORDER BY position",
            )?;
            let rows = stmt.query_map([id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })?;
            let all: Vec<(i64, i64)> = rows.collect::<rusqlite::Result<_>>()?;

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

            conn.execute_batch("BEGIN")?;
            conn.execute("DELETE FROM playlist_tracks WHERE playlist_id = ?1", [id])?;
            {
                let mut ins = conn.prepare(
                    "INSERT INTO playlist_tracks(playlist_id, position, track_id) \
                     VALUES (?1, ?2, ?3)",
                )?;
                for (i, tid) in rest.iter().enumerate() {
                    ins.execute(rusqlite::params![id, i as i64, tid])?;
                }
            }
            conn.execute_batch("COMMIT")?;
            Ok(())
        })
    }
}

fn renumber(conn: &rusqlite::Connection, id: i64) -> Result<()> {
    // Compact positions to 0..n keeping order.
    let mut stmt = conn.prepare(
        "SELECT position FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position",
    )?;
    let positions: Vec<i64> =
        stmt.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
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
