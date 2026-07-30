use super::Db;
use crate::model::{Album, BrowseMode, LibraryStats, PlayContext, Track, TrackGroup};
use crate::search::SearchFilter;
use anyhow::Result;
use rusqlite::{params_from_iter, types::Value, Connection, Row};

pub const TRACK_COLS: &str = "id, path, title, artist, album, albumartist, titlesort, \
                              artistsort, albumartistsort, genre, grouping, \
                              track_no, disc_no, year, duration_ms, format, album_id, rating, \
                              play_count";

/// Stable order used everywhere a "whole library" listing appears.
pub const LIBRARY_SORT: &str = "albumartist COLLATE NOCASE, year, album COLLATE NOCASE, \
                                disc_no, track_no, path";

/// SQL for the "primary theme" of a grouping tag: the text before the first
/// ';' (trimmed). Used to collapse multi-theme grouping tags (Touhou remixes
/// tagged `A; B; C`) under one umbrella. `;` is ASCII so INSTR/SUBSTR stay
/// Unicode-safe on the CJK-heavy library; NULL/empty grouping folds to '' (the
/// "?" bucket the UI already renders).
const GROUPING_FIRST_SEG: &str =
    "TRIM(CASE WHEN INSTR(COALESCE(grouping, ''), ';') > 0 \
              THEN SUBSTR(COALESCE(grouping, ''), 1, INSTR(COALESCE(grouping, ''), ';') - 1) \
              ELSE COALESCE(grouping, '') END)";

pub fn track_from_row(r: &Row) -> rusqlite::Result<Track> {
    Ok(Track {
        id: r.get(0)?,
        path: r.get(1)?,
        title: r.get(2)?,
        artist: r.get(3)?,
        album: r.get(4)?,
        albumartist: r.get(5)?,
        titlesort: r.get(6)?,
        artistsort: r.get(7)?,
        albumartistsort: r.get(8)?,
        genre: r.get(9)?,
        grouping: r.get(10)?,
        track_no: r.get(11)?,
        disc_no: r.get(12)?,
        year: r.get(13)?,
        duration_ms: r.get(14)?,
        format: r.get(15)?,
        album_id: r.get(16)?,
        rating: r.get(17)?,
        play_count: r.get(18)?,
    })
}

fn album_from_row(r: &Row) -> rusqlite::Result<Album> {
    Ok(Album {
        id: r.get(0)?,
        album: r.get(1)?,
        albumartist: r.get(2)?,
        albumartist_sort: r.get(3)?,
        year: r.get(4)?,
        genre: r.get(5)?,
        track_count: r.get(6)?,
        total_duration_ms: r.get(7)?,
    })
}

/// WHERE fragment for live (non-missing) tracks plus an optional search filter.
fn track_where(filter: Option<&SearchFilter>) -> (String, Vec<Value>) {
    match filter {
        Some(f) => (format!("missing = 0 AND ({})", f.where_sql), f.params.clone()),
        None => ("missing = 0".into(), vec![]),
    }
}

/// Append a `<mode column> = value` equality to a `track_where` fragment, in place.
/// NULLs collate to "" (so selecting the "?" group matches untagged tracks) — this
/// is the single source of the left-panel value filter shared by browse_albums,
/// track_count/tracks_range, and the ValueTracks play context. When `collapse` is
/// set for grouping mode, the value is matched against the primary theme
/// (`GROUPING_FIRST_SEG`) instead of the whole tag, so an umbrella row selects
/// every track whose first theme equals `value`.
fn push_value_clause(
    where_sql: &mut String,
    params: &mut Vec<Value>,
    mode: &BrowseMode,
    value: &str,
    collapse: bool,
) {
    let lhs = if collapse && mode.column() == Some("grouping") {
        Some(GROUPING_FIRST_SEG.to_string())
    } else {
        mode.column().map(col_expr)
    };
    match lhs {
        Some(lhs) => {
            *where_sql = format!("{where_sql} AND {lhs} = ?{}", params.len() + 1);
            params.push(Value::from(value.to_string()));
        }
        // Custom tag that resolves to no real column: select nothing.
        None => where_sql.push_str(" AND 0"),
    }
}

/// GROUP BY / equality expression for a value-filterable column: coalesce NULL
/// to '' and force TEXT, so integer columns (year, rating, …) round-trip through
/// the string-based value list and still compare equal to the selected value.
fn col_expr(col: &str) -> String {
    format!("CAST(COALESCE({col}, '') AS TEXT)")
}

impl Db {
    pub fn library_stats(&self) -> Result<LibraryStats> {
        self.with(|conn| {
            let (tracks, total_duration_ms) = conn.query_row(
                "SELECT COUNT(*), COALESCE(SUM(duration_ms), 0) FROM tracks WHERE missing = 0",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let albums =
                conn.query_row("SELECT COUNT(*) FROM albums", [], |r| r.get(0))?;
            let mut formats = std::collections::HashMap::new();
            let mut stmt = conn.prepare(
                "SELECT format, COUNT(*) FROM tracks WHERE missing = 0 GROUP BY format",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            for row in rows {
                let (k, v) = row?;
                formats.insert(k, v);
            }
            Ok(LibraryStats { tracks, albums, total_duration_ms, formats })
        })
    }

    /// Distinct values for the Library tab's left column, with track counts
    /// and (for artist/album-artist modes) the romanized sort value used for
    /// ordering and display. NULL values group under "" (rendered as "?" by
    /// the UI).
    pub fn browse_values(
        &self,
        mode: BrowseMode,
        filter: Option<&SearchFilter>,
        collapse: bool,
    ) -> Result<Vec<(String, i64, String)>> {
        // Unresolvable custom tag (unknown / not a stored column) → empty list.
        let col = match mode.column() {
            Some(c) => c,
            None => return Ok(vec![]),
        };
        let (where_sql, params) = track_where(filter);
        let val = col_expr(col);
        let sql = match &mode {
            BrowseMode::Artist => format!(
                "SELECT COALESCE({col}, ''), COUNT(*), COALESCE(MAX(artistsort), '') \
                 FROM tracks WHERE {where_sql} GROUP BY COALESCE({col}, '') \
                 ORDER BY COALESCE(NULLIF(COALESCE(MAX(artistsort), ''), ''), \
                                   COALESCE({col}, '')) COLLATE NOCASE"
            ),
            BrowseMode::Albumartist => format!(
                "SELECT COALESCE({col}, ''), COUNT(*), COALESCE(MAX(albumartistsort), '') \
                 FROM tracks WHERE {where_sql} GROUP BY COALESCE({col}, '') \
                 ORDER BY COALESCE(NULLIF(COALESCE(MAX(albumartistsort), ''), ''), \
                                   COALESCE({col}, '')) COLLATE NOCASE"
            ),
            // Collapse multi-theme grouping tags under their primary theme.
            _ if col == "grouping" && collapse => format!(
                "SELECT {GROUPING_FIRST_SEG}, COUNT(*), '' FROM tracks WHERE {where_sql} \
                 GROUP BY {GROUPING_FIRST_SEG} ORDER BY 1 COLLATE NOCASE"
            ),
            // Genre / Album / non-collapsed Grouping / Custom. TEXT-cast so
            // integer columns (year, rating, …) come back as strings.
            _ => format!(
                "SELECT {val}, COUNT(*), '' FROM tracks WHERE {where_sql} \
                 GROUP BY {val} ORDER BY 1 COLLATE NOCASE"
            ),
        };
        self.with(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
            })?;
            rows.map(|r| Ok(r?)).collect()
        })
    }

    /// Albums that contain at least one track matching (mode=value AND search).
    pub fn browse_albums(
        &self,
        mode: BrowseMode,
        value: Option<&str>,
        filter: Option<&SearchFilter>,
        collapse: bool,
    ) -> Result<Vec<Album>> {
        let (mut where_sql, mut params) = track_where(filter);
        if let Some(v) = value {
            push_value_clause(&mut where_sql, &mut params, &mode, v, collapse);
        }
        self.with(|conn| {
            let sql = format!(
                "SELECT a.id, a.album, a.albumartist, a.albumartist_sort, a.year, a.genre, \
                        a.track_count, a.total_duration_ms \
                 FROM albums a WHERE a.id IN \
                   (SELECT DISTINCT album_id FROM tracks WHERE {where_sql}) \
                 ORDER BY COALESCE(NULLIF(a.albumartist_sort, ''), a.albumartist) \
                          COLLATE NOCASE, a.year, a.album COLLATE NOCASE"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(params.iter()), album_from_row)?;
            rows.map(|r| Ok(r?)).collect()
        })
    }

    pub fn album_tracks(&self, album_id: i64) -> Result<Vec<Track>> {
        self.with(|conn| {
            let sql = format!(
                "SELECT {TRACK_COLS} FROM tracks WHERE album_id = ?1 AND missing = 0 \
                 ORDER BY disc_no, track_no, path"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([album_id], track_from_row)?;
            rows.map(|r| Ok(r?)).collect()
        })
    }

    pub fn track_by_id(&self, id: i64) -> Result<Option<Track>> {
        self.with(|conn| {
            let sql = format!("SELECT {TRACK_COLS} FROM tracks WHERE id = ?1");
            Ok(conn.query_row(&sql, [id], track_from_row).ok())
        })
    }

    /// Credit one play to a track and return its new total. Called by the
    /// player once more than half of the track has been heard.
    pub fn record_play(&self, id: i64) -> Result<i64> {
        self.with(|conn| {
            let n = conn.query_row(
                "UPDATE tracks SET play_count = play_count + 1 WHERE id = ?1 \
                 RETURNING play_count",
                [id],
                |r| r.get::<_, i64>(0),
            )?;
            Ok(n)
        })
    }

    /// Fetch several tracks preserving the id order given.
    pub fn tracks_by_ids(&self, ids: &[i64]) -> Result<Vec<Track>> {
        self.with(|conn| {
            let sql = format!("SELECT {TRACK_COLS} FROM tracks WHERE id = ?1");
            let mut stmt = conn.prepare(&sql)?;
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                if let Ok(t) = stmt.query_row([id], track_from_row) {
                    out.push(t);
                }
            }
            Ok(out)
        })
    }

    /// Count of live tracks matching (search filter AND optional mode=value).
    pub fn track_count(
        &self,
        filter: Option<&SearchFilter>,
        mode: BrowseMode,
        value: Option<&str>,
        collapse: bool,
    ) -> Result<i64> {
        let (mut where_sql, mut params) = track_where(filter);
        if let Some(v) = value {
            push_value_clause(&mut where_sql, &mut params, &mode, v, collapse);
        }
        self.with(|conn| {
            Ok(conn.query_row(
                &format!("SELECT COUNT(*) FROM tracks WHERE {where_sql}"),
                params_from_iter(params.iter()),
                |r| r.get(0),
            )?)
        })
    }

    /// Windowed page of the library view (search filter + optional mode=value +
    /// stable sort). The sort matches `resolve_context` so a row's index is its
    /// index in the resolved play queue.
    pub fn tracks_range(
        &self,
        filter: Option<&SearchFilter>,
        mode: BrowseMode,
        value: Option<&str>,
        collapse: bool,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<Track>> {
        let (mut where_sql, mut params) = track_where(filter);
        if let Some(v) = value {
            push_value_clause(&mut where_sql, &mut params, &mode, v, collapse);
        }
        self.with(|conn| {
            let sql = format!(
                "SELECT {TRACK_COLS} FROM tracks WHERE {where_sql} \
                 ORDER BY {LIBRARY_SORT} LIMIT ?{} OFFSET ?{}",
                params.len() + 1,
                params.len() + 2
            );
            params.push(Value::from(limit));
            params.push(Value::from(offset));
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(params.iter()), track_from_row)?;
            rows.map(|r| Ok(r?)).collect()
        })
    }

    /// 0-based index of `track_id` within the filtered + LIBRARY_SORT-ordered
    /// view (same order as `tracks_range`), or `None` when the track isn't in
    /// that set. Lets the Track-list view scroll to the now-playing track after
    /// a filter change (e.g. clearing the search).
    pub fn track_index(
        &self,
        filter: Option<&SearchFilter>,
        mode: BrowseMode,
        value: Option<&str>,
        collapse: bool,
        track_id: i64,
    ) -> Result<Option<i64>> {
        let (mut where_sql, mut params) = track_where(filter);
        if let Some(v) = value {
            push_value_clause(&mut where_sql, &mut params, &mode, v, collapse);
        }
        self.with(|conn| {
            let sql = format!(
                "SELECT rn FROM ( \
                   SELECT id, ROW_NUMBER() OVER (ORDER BY {LIBRARY_SORT}) - 1 AS rn \
                   FROM tracks WHERE {where_sql} \
                 ) WHERE id = ?{}",
                params.len() + 1
            );
            params.push(Value::from(track_id));
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(params_from_iter(params.iter()))?;
            Ok(match rows.next()? {
                Some(r) => Some(r.get(0)?),
                None => None,
            })
        })
    }

    /// Album runs for the Track-list view: consecutive same-album_id blocks over
    /// the same filtered + LIBRARY_SORT-ordered set as `tracks_range`. Uses the
    /// gaps-and-islands pattern so a re-appearing album_id (e.g. split by a year
    /// change) and scattered NULL-album tracks each form their own run.
    pub fn track_groups(
        &self,
        filter: Option<&SearchFilter>,
        mode: BrowseMode,
        value: Option<&str>,
        collapse: bool,
    ) -> Result<Vec<TrackGroup>> {
        let (mut where_sql, mut params) = track_where(filter);
        if let Some(v) = value {
            push_value_clause(&mut where_sql, &mut params, &mode, v, collapse);
        }
        self.with(|conn| {
            let sql = format!(
                "WITH ordered AS ( \
                   SELECT album_id, album, \
                          COALESCE(albumartist, artist) AS artist, \
                          COALESCE(albumartistsort, artistsort) AS artist_sort, year, \
                          ROW_NUMBER() OVER (ORDER BY {LIBRARY_SORT}) AS rn \
                   FROM tracks WHERE {where_sql} \
                 ), islands AS ( \
                   SELECT *, rn - ROW_NUMBER() OVER (PARTITION BY album_id ORDER BY rn) AS grp \
                   FROM ordered \
                 ) \
                 SELECT album_id, MIN(album), MIN(artist), MIN(artist_sort), MIN(year), COUNT(*) \
                 FROM islands GROUP BY album_id, grp ORDER BY MIN(rn)"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
                Ok(TrackGroup {
                    album_id: r.get(0)?,
                    album: r.get(1)?,
                    artist: r.get(2)?,
                    artist_sort: r.get(3)?,
                    year: r.get(4)?,
                    count: r.get(5)?,
                })
            })?;
            rows.map(|r| Ok(r?)).collect()
        })
    }

    pub fn album_by_id(&self, id: i64) -> Result<Option<Album>> {
        self.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT id, album, albumartist, albumartist_sort, year, genre, \
                            track_count, total_duration_ms FROM albums WHERE id = ?1",
                    [id],
                    album_from_row,
                )
                .ok())
        })
    }

    /// Resolve a play context to the ordered track-id list playback walks.
    pub fn resolve_context(&self, ctx: &PlayContext) -> Result<Vec<i64>> {
        match ctx {
            PlayContext::Album { id } => self.with(|conn| {
                collect_ids(
                    conn,
                    "SELECT id FROM tracks WHERE album_id = ?1 AND missing = 0 \
                     ORDER BY disc_no, track_no, path",
                    vec![Value::from(*id)],
                )
            }),
            PlayContext::Playlist { id } => self.with(|conn| {
                collect_ids(
                    conn,
                    "SELECT pt.track_id FROM playlist_tracks pt \
                     JOIN tracks t ON t.id = pt.track_id AND t.missing = 0 \
                     WHERE pt.playlist_id = ?1 ORDER BY pt.position",
                    vec![Value::from(*id)],
                )
            }),
            PlayContext::LibraryView { query, .. } => {
                let filter = compile_optional_query(query.as_deref())?;
                let (where_sql, params) = track_where(filter.as_ref());
                self.with(|conn| {
                    collect_ids(
                        conn,
                        &format!(
                            "SELECT id FROM tracks WHERE {where_sql} ORDER BY {LIBRARY_SORT}"
                        ),
                        params,
                    )
                })
            }
            PlayContext::ValueTracks { mode, value, query, collapse } => {
                let filter = compile_optional_query(query.as_deref())?;
                let (mut where_sql, mut params) = track_where(filter.as_ref());
                push_value_clause(&mut where_sql, &mut params, mode, value, *collapse);
                self.with(|conn| {
                    collect_ids(
                        conn,
                        &format!(
                            "SELECT id FROM tracks WHERE {where_sql} ORDER BY {LIBRARY_SORT}"
                        ),
                        params,
                    )
                })
            }
        }
    }

    pub fn lastfm_cache_get(&self, key: &str, max_age_secs: i64) -> Result<Option<String>> {
        self.with(|conn| {
            let now = now_unix();
            Ok(conn
                .query_row(
                    "SELECT json FROM lastfm_cache WHERE key = ?1 AND fetched_at > ?2",
                    rusqlite::params![key, now - max_age_secs],
                    |r| r.get(0),
                )
                .ok())
        })
    }

    pub fn lastfm_cache_put(&self, key: &str, json: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO lastfm_cache(key, json, fetched_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET json = excluded.json, \
                 fetched_at = excluded.fetched_at",
                rusqlite::params![key, json, now_unix()],
            )?;
            Ok(())
        })
    }
}

fn collect_ids(conn: &Connection, sql: &str, params: Vec<Value>) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| r.get::<_, i64>(0))?;
    rows.map(|r| Ok(r?)).collect()
}

fn compile_optional_query(query: Option<&str>) -> Result<Option<SearchFilter>> {
    match query {
        Some(q) if !q.trim().is_empty() => Ok(Some(crate::search::compile(q)?)),
        _ => Ok(None),
    }
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
