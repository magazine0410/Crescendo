use super::{
    queries::{track_from_row, TRACK_COLS},
    Db,
};
use crate::model::Track;
use anyhow::Result;
use serde::Serialize;

#[derive(Serialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub played_at: i64,
    pub track: Track,
}

impl Db {
    /// Keyset pagination keeps old pages stable when a new play arrives.
    pub fn song_history(&self, before: Option<i64>, limit: usize) -> Result<Vec<HistoryEntry>> {
        self.with(|conn| {
            let cols = TRACK_COLS
                .split(',')
                .map(|c| format!("t.{}", c.trim()))
                .collect::<Vec<_>>()
                .join(",");
            let mut stmt = conn.prepare(&format!(
                "SELECT {cols}, h.id, h.played_at FROM play_history h \
                 JOIN tracks t ON t.id = h.track_id WHERE h.id < ?1 \
                 ORDER BY h.id DESC LIMIT ?2"
            ))?;
            let n = TRACK_COLS.split(',').count();
            let rows = stmt.query_map(
                rusqlite::params![before.unwrap_or(i64::MAX), limit.clamp(1, 100) as i64],
                |r| {
                    Ok(HistoryEntry {
                        id: r.get(n)?,
                        played_at: r.get(n + 1)?,
                        track: track_from_row(r)?,
                    })
                },
            )?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }
}
