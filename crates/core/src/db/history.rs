use super::{
    queries::{track_from_row, TRACK_COLS},
    Db,
};
use crate::model::Track;
use crate::songlink::{Service, SINCE_KEY};
use anyhow::Result;
use serde::Serialize;

#[derive(Serialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub played_at: i64,
    pub track: Track,
    /// Whose page this play can link to: Last.fm's for a play from before
    /// ListenBrainz took over, ListenBrainz's after.
    pub service: Service,
    /// The Last.fm page, when one is saved for the song. A ListenBrainz page
    /// is looked up separately (`songlink::listenbrainz_links`), since that
    /// asks the network.
    pub url: Option<String>,
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
                "SELECT {cols}, h.id, h.played_at, \
                        h.played_at < COALESCE( \
                          (SELECT CAST(value AS INTEGER) FROM meta WHERE key = ?3), 0), \
                        l.url \
                 FROM play_history h \
                 JOIN tracks t ON t.id = h.track_id \
                 LEFT JOIN lastfm_links l ON l.artist = t.artist AND l.title = t.title \
                 WHERE h.id < ?1 \
                 ORDER BY h.id DESC LIMIT ?2"
            ))?;
            let n = TRACK_COLS.split(',').count();
            let rows = stmt.query_map(
                rusqlite::params![
                    before.unwrap_or(i64::MAX),
                    limit.clamp(1, 100) as i64,
                    SINCE_KEY
                ],
                |r| {
                    let lastfm: bool = r.get(n + 2)?;
                    Ok(HistoryEntry {
                        id: r.get(n)?,
                        played_at: r.get(n + 1)?,
                        track: track_from_row(r)?,
                        service: if lastfm {
                            Service::Lastfm
                        } else {
                            Service::Listenbrainz
                        },
                        url: if lastfm { r.get(n + 3)? } else { None },
                    })
                },
            )?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }
}
