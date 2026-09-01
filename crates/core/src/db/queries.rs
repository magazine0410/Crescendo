use super::Db;
use crate::model::{
    Album, BrowseMode, BrowseValue, LibraryStats, PlayContext, SortKey, Track, TrackGroup,
    TrackSort,
};
use crate::search::SearchFilter;
use anyhow::Result;
use rusqlite::{params_from_iter, types::Value, Connection, Row};

pub const TRACK_COLS: &str = "id, path, title, artist, album, albumartist, titlesort, \
                              artistsort, albumartistsort, genre, grouping, \
                              track_no, disc_no, year, duration_ms, format, album_id, rating, \
                              play_count, missing, composer, arranger, lyricist, vocalist, \
                              catalognumber, event, bpm";

/// Stable order used everywhere a "whole library" listing appears.
pub const LIBRARY_SORT: &str = "albumartist COLLATE NOCASE, year, album COLLATE NOCASE, \
                                disc_no, track_no, path";

/// ORDER BY expression for one explicit sort key. With `romanized`, Title
/// honors the titlesort tag when present, matching how titles render (see
/// format.ts); without it, titles sort by the tag they show.
fn sort_key_sql(key: SortKey, romanized: bool) -> &'static str {
    match key {
        SortKey::Albumartist => "albumartist COLLATE NOCASE",
        SortKey::Artist => "artist COLLATE NOCASE",
        SortKey::Album => "album COLLATE NOCASE",
        SortKey::Genre => "genre COLLATE NOCASE",
        SortKey::Grouping => "grouping COLLATE NOCASE",
        SortKey::Year => "year",
        SortKey::Title if romanized => "COALESCE(NULLIF(titlesort, ''), title) COLLATE NOCASE",
        SortKey::Title => "title COLLATE NOCASE",
        SortKey::Duration => "duration_ms",
        SortKey::LastPlayed => "last_played",
    }
}

/// Full ORDER BY body for the Track-list view: `LIBRARY_SORT` when no explicit
/// sort is chosen, otherwise the chosen key(s) — each with its direction —
/// followed by the LIBRARY_SORT tiebreakers the chosen keys don't already cover.
/// `romanized` is the `prefer_romanized` pref; pass the same value to the view
/// and to `resolve_context_with_projection` or a row's index stops matching
/// its queue position.
pub fn order_sql(sort: Option<TrackSort>, romanized: bool) -> String {
    let Some(s) = sort else { return LIBRARY_SORT.into() };
    let dir = if s.descending { " DESC" } else { "" };
    let mut used = vec![s.key];
    let mut out = format!("{}{dir}", sort_key_sql(s.key, romanized));
    if let Some(sec) = s.secondary.filter(|k| *k != s.key) {
        used.push(sec);
        let sec_dir = if s.secondary_descending.unwrap_or(s.descending) { " DESC" } else { "" };
        out = format!("{out}, {}{sec_dir}", sort_key_sql(sec, romanized));
    }
    const CHAIN: [(Option<SortKey>, &str); 6] = [
        (Some(SortKey::Albumartist), "albumartist COLLATE NOCASE"),
        (Some(SortKey::Year), "year"),
        (Some(SortKey::Album), "album COLLATE NOCASE"),
        (None, "disc_no"),
        (None, "track_no"),
        (None, "path"),
    ];
    for (key, expr) in CHAIN {
        if key.is_some_and(|k| used.contains(&k)) {
            continue;
        }
        out = format!("{out}, {expr}");
    }
    out
}

/// Host-implemented facet transforms. Plugin manifests can select one of these
/// bounded descriptors, but cannot supply an expression or column name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FacetTransform {
    TextFirstSegment {
        delimiter: String,
        trim_whitespace: bool,
    },
}

/// One row of the read-only library page served to sandboxed Wasm plugins.
/// Paths are present only when the `library.paths` permission was granted.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginLibraryTrack {
    pub id: i64,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub albumartist: Option<String>,
    pub genre: Option<String>,
    pub grouping: Option<String>,
    pub duration_ms: i64,
    pub year: Option<i64>,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginLibraryPage {
    pub tracks: Vec<PluginLibraryTrack>,
    pub total: i64,
}

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
        missing: r.get::<_, i64>(19)? != 0,
        composer: r.get(20)?,
        arranger: r.get(21)?,
        lyricist: r.get(22)?,
        vocalist: r.get(23)?,
        catalognumber: r.get(24)?,
        event: r.get(25)?,
        bpm: r.get(26)?,
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

/// Whole-minute bucket of a track's duration, as TEXT — the value the custom
/// `%duration%` filter groups and matches by ("0" = under a minute, "3" =
/// 3:00–3:59, …). The UI formats the label; only the bucket number crosses IPC.
const DURATION_BUCKET: &str = "CAST(duration_ms / 60000 AS TEXT)";

/// Whole days between the day a track was last played and today, both taken as
/// local calendar days: `0` = today, `1` = yesterday. Collapsing each side to
/// its own midnight keeps the time of day (and any DST jump between them) from
/// moving a play across a bucket edge. NULL for a track that was never played.
const DAYS_SINCE_LAST_PLAY: &str =
    "CAST(ROUND(julianday('now', 'localtime', 'start of day') \
                - julianday(last_played, 'unixepoch', 'localtime', 'start of day')) AS INTEGER)";

/// How long ago a track was last played, as a TEXT bucket number — the value
/// the custom `%lastplayed%` filter groups and matches by. The spans are
/// rolling day counts rather than calendar weeks or months, and each track
/// falls in the narrowest one that holds it: "0" today, "1" within 7 days, "2"
/// within 30, "3" within 365, "4" longer ago, "5" never played. The UI supplies
/// the labels; only the bucket number crosses IPC.
fn last_played_bucket() -> String {
    format!(
        "CAST(CASE WHEN last_played IS NULL THEN 5 \
                   WHEN {d} <= 0 THEN 0 \
                   WHEN {d} < 7 THEN 1 \
                   WHEN {d} < 30 THEN 2 \
                   WHEN {d} < 365 THEN 3 \
                   ELSE 4 END AS TEXT)",
        d = DAYS_SINCE_LAST_PLAY
    )
}

/// Host-owned facet presentation metadata. It chooses the matching SQL order:
/// plain tags sort as text, duration buckets numerically, play counts
/// descending, and the six last-played buckets in their preassigned order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FacetPresentation {
    Text,
    DurationBucket,
    LastPlayedBucket,
    PlayCount,
}

impl FacetPresentation {
    fn order_sql(self, group_expr: &str) -> String {
        match self {
            Self::Text => "1 COLLATE NOCASE".into(),
            Self::DurationBucket | Self::LastPlayedBucket => group_expr.into(),
            Self::PlayCount => format!("{group_expr} DESC"),
        }
    }
}

/// The complete trusted specification of a Library facet. Every SQL fragment
/// comes from this module; neither custom tags nor future plugin manifests can
/// inject an expression. Value and selection expressions deliberately live in
/// the same plan so a displayed row always round-trips to the same track set.
/// Search filters remain outside this projection: `grouping:...` and all other
/// field searches continue to match the raw stored metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetPlan {
    target_column: &'static str,
    value_expr: String,
    group_expr: String,
    selection_expr: String,
    sort_value_expr: Option<String>,
    presentation: FacetPresentation,
}

impl FacetPlan {
    pub fn resolve(
        mode: &BrowseMode,
        transform: Option<&FacetTransform>,
        romanized: bool,
    ) -> Option<Self> {
        let target_column = mode.column()?;
        let (base_expr, base_group_expr, base_presentation) = match mode {
            BrowseMode::Artist => {
                let shown = displayed_name_expr("artist", "artistsort", romanized);
                (shown.clone(), shown, FacetPresentation::Text)
            }
            BrowseMode::Albumartist => {
                let shown = displayed_name_expr("albumartist", "albumartistsort", romanized);
                (shown.clone(), shown, FacetPresentation::Text)
            }
            _ if mode.is_duration() => (
                DURATION_BUCKET.into(),
                "duration_ms / 60000".into(),
                FacetPresentation::DurationBucket,
            ),
            _ if mode.is_last_played() => {
                let bucket = last_played_bucket();
                (bucket.clone(), bucket, FacetPresentation::LastPlayedBucket)
            }
            _ if mode.is_play_count() => (
                "CAST(play_count AS TEXT)".into(),
                "play_count".into(),
                FacetPresentation::PlayCount,
            ),
            _ => {
                let value = col_expr(target_column);
                (value.clone(), value, FacetPresentation::Text)
            }
        };
        // Compose a contributed transform over the host's normal visible
        // expression. This keeps transforms meaningful for artist facets too:
        // when romanization is enabled they see the same label the user sees,
        // rather than silently being ignored or operating on a hidden raw tag.
        let (value_expr, group_expr, presentation) = match transform {
            Some(transform) => {
                let projected = transform_expr(&base_expr, transform);
                (projected.clone(), projected, FacetPresentation::Text)
            }
            None => (base_expr, base_group_expr, base_presentation),
        };
        Some(Self {
            target_column,
            selection_expr: value_expr.clone(),
            value_expr,
            group_expr,
            // No current facet needs a distinct visible sort value. Keep the
            // slot explicit rather than sending the old dead empty string.
            sort_value_expr: None,
            presentation,
        })
    }

    pub fn exact(mode: &BrowseMode, romanized: bool) -> Option<Self> {
        Self::resolve(mode, None, romanized)
    }

    fn values_sql(&self, where_sql: &str) -> String {
        let sort_value = self.sort_value_expr.as_deref().unwrap_or("NULL");
        let order = self.presentation.order_sql(&self.group_expr);
        format!(
            "SELECT {}, COUNT(*), {sort_value} FROM tracks WHERE {where_sql} \
             GROUP BY {} ORDER BY {order}",
            self.value_expr, self.group_expr
        )
    }
}

fn transform_expr(input_expr: &str, transform: &FacetTransform) -> String {
    match transform {
        FacetTransform::TextFirstSegment {
            delimiter,
            trim_whitespace,
        } => {
            // The delimiter is data, not SQL. Quoting it here is the only path
            // from a validated descriptor into the trusted host expression.
            let delimiter = format!("'{}'", delimiter.replace('\'', "''"));
            let split = format!(
                "CASE WHEN INSTR({input_expr}, {delimiter}) > 0 \
                 THEN SUBSTR({input_expr}, 1, INSTR({input_expr}, {delimiter}) - 1) \
                 ELSE {input_expr} END"
            );
            if *trim_whitespace {
                format!("TRIM({split})")
            } else {
                split
            }
        }
    }
}

/// Append a facet selection to a `track_where` fragment. A track matches when
/// its projected value is one of the selected rows. Empty values mean "All";
/// an unresolved custom tag with a non-empty selection deliberately matches
/// nothing. This clause is shared by albums, counts, pages, indexes, groups,
/// and playback resolution.
fn push_values_clause(
    where_sql: &mut String,
    params: &mut Vec<Value>,
    plan: Option<&FacetPlan>,
    values: &[String],
) {
    if values.is_empty() {
        return;
    }
    match plan {
        Some(plan) => {
            let mut placeholders = Vec::with_capacity(values.len());
            for v in values {
                params.push(Value::from(v.clone()));
                placeholders.push(format!("?{}", params.len()));
            }
            *where_sql = format!(
                "{where_sql} AND {} IN ({})",
                plan.selection_expr,
                placeholders.join(", ")
            );
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

fn displayed_name_expr(name: &str, sort: &str, romanized: bool) -> String {
    let raw = col_expr(name);
    if !romanized {
        return raw;
    }
    format!(
        "CASE WHEN NULLIF(COALESCE({sort}, ''), '') IS NOT NULL AND has_cjk({raw}) \
              THEN {sort} ELSE {raw} END"
    )
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

    /// Distinct values for the Library tab's left column, with track counts.
    /// Artist modes return the final visible label as the value, merging raw
    /// CJK names that render through the same sort tag. NULL values group under
    /// "" (rendered as "?" by the UI).
    /// **Ignores plugin facet transforms.** It builds a `FacetPlan::exact` from
    /// the stored `prefer_romanized` preference, so a plugin projecting this tag
    /// has no effect here. The app never calls it — the Tauri commands use
    /// `browse_values_with_plan` under a held projection guard, which is what
    /// keeps the value list, counts, rows and playback on one revision. This
    /// exists for tests, examples, and headless harnesses that have no plugin
    /// registry.
    pub fn browse_values_exact(
        &self,
        mode: BrowseMode,
        filter: Option<&SearchFilter>,
    ) -> Result<Vec<BrowseValue>> {
        let romanized = self.prefer_romanized();
        let plan = FacetPlan::exact(&mode, romanized);
        self.browse_values_with_plan(plan.as_ref(), filter)
    }

    /// Projection-explicit form used by the Tauri boundary while it holds the
    /// matching plugin projection read guard.
    pub fn browse_values_with_plan(
        &self,
        plan: Option<&FacetPlan>,
        filter: Option<&SearchFilter>,
    ) -> Result<Vec<BrowseValue>> {
        // Unresolvable custom tag (unknown / not a stored column) → empty list.
        let Some(plan) = plan else {
            return Ok(vec![]);
        };
        let (where_sql, params) = track_where(filter);
        let sql = plan.values_sql(&where_sql);
        self.with(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
                Ok(BrowseValue {
                    value: r.get(0)?,
                    count: r.get(1)?,
                    sort_value: r.get(2)?,
                })
            })?;
            rows.map(|r| Ok(r?)).collect()
        })
    }

    /// Albums that contain at least one track matching (mode in values AND search).
    /// **Ignores plugin facet transforms.** It builds a `FacetPlan::exact` from
    /// the stored `prefer_romanized` preference, so a plugin projecting this tag
    /// has no effect here. The app never calls it — the Tauri commands use
    /// `browse_albums_with_plan` under a held projection guard, which is what
    /// keeps the value list, counts, rows and playback on one revision. This
    /// exists for tests, examples, and headless harnesses that have no plugin
    /// registry.
    pub fn browse_albums_exact(
        &self,
        mode: BrowseMode,
        values: &[String],
        filter: Option<&SearchFilter>,
    ) -> Result<Vec<Album>> {
        let romanized = self.prefer_romanized();
        let plan = FacetPlan::exact(&mode, romanized);
        self.browse_albums_with_plan(plan.as_ref(), values, filter, romanized)
    }

    pub fn browse_albums_with_plan(
        &self,
        plan: Option<&FacetPlan>,
        values: &[String],
        filter: Option<&SearchFilter>,
        romanized: bool,
    ) -> Result<Vec<Album>> {
        let (mut where_sql, mut params) = track_where(filter);
        push_values_clause(&mut where_sql, &mut params, plan, values);
        // Read before locking: `with` holds the connection mutex.
        let artist_order = displayed_name_expr("a.albumartist", "a.albumartist_sort", romanized);
        self.with(|conn| {
            let sql = format!(
                "SELECT a.id, a.album, a.albumartist, a.albumartist_sort, a.year, a.genre, \
                        a.track_count, a.total_duration_ms \
                 FROM albums a WHERE a.id IN \
                   (SELECT DISTINCT album_id FROM tracks WHERE {where_sql}) \
                 ORDER BY {artist_order} \
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
    ///
    /// Writes the play three ways in one transaction: a dated row in
    /// `play_history`, the running `play_count`, and the `last_played` cache the
    /// Last-played filter and sort read. The calendar parts come from SQLite's
    /// `localtime`, so a play is filed under the day the listener saw on the
    /// clock, not the UTC day.
    pub fn record_play(&self, id: i64) -> Result<i64> {
        self.with(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO play_history(track_id, played_at, year, month, day) \
                 SELECT ?1, unixepoch(), \
                        CAST(strftime('%Y', 'now', 'localtime') AS INTEGER), \
                        CAST(strftime('%m', 'now', 'localtime') AS INTEGER), \
                        CAST(strftime('%d', 'now', 'localtime') AS INTEGER)",
                [id],
            )?;
            let n = tx.query_row(
                "UPDATE tracks SET play_count = play_count + 1, \
                        last_played = (SELECT MAX(played_at) FROM play_history \
                                       WHERE track_id = ?1) \
                 WHERE id = ?1 RETURNING play_count",
                [id],
                |r| r.get::<_, i64>(0),
            )?;
            tx.commit()?;
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

    /// Count of live tracks matching (search filter AND optional mode values).
    /// **Ignores plugin facet transforms.** It builds a `FacetPlan::exact` from
    /// the stored `prefer_romanized` preference, so a plugin projecting this tag
    /// has no effect here. The app never calls it — the Tauri commands use
    /// `track_count_with_plan` under a held projection guard, which is what keeps
    /// the value list, counts, rows and playback on one revision. This exists for
    /// tests, examples, and headless harnesses that have no plugin registry.
    pub fn track_count_exact(
        &self,
        filter: Option<&SearchFilter>,
        mode: BrowseMode,
        values: &[String],
    ) -> Result<i64> {
        let plan = FacetPlan::exact(&mode, self.prefer_romanized());
        self.track_count_with_plan(filter, plan.as_ref(), values)
    }

    pub fn track_count_with_plan(
        &self,
        filter: Option<&SearchFilter>,
        plan: Option<&FacetPlan>,
        values: &[String],
    ) -> Result<i64> {
        let (mut where_sql, mut params) = track_where(filter);
        push_values_clause(&mut where_sql, &mut params, plan, values);
        self.with(|conn| {
            Ok(conn.query_row(
                &format!("SELECT COUNT(*) FROM tracks WHERE {where_sql}"),
                params_from_iter(params.iter()),
                |r| r.get(0),
            )?)
        })
    }

    /// Windowed page of the library view (search filter + optional mode values +
    /// stable sort). The sort matches context resolution so a row's index is its
    /// index in the resolved play queue.
    /// **Ignores plugin facet transforms.** It builds a `FacetPlan::exact` from
    /// the stored `prefer_romanized` preference, so a plugin projecting this tag
    /// has no effect here. The app never calls it — the Tauri commands use
    /// `tracks_range_with_plan` under a held projection guard, which is what
    /// keeps the value list, counts, rows and playback on one revision. This
    /// exists for tests, examples, and headless harnesses that have no plugin
    /// registry.
    pub fn tracks_range_exact(
        &self,
        filter: Option<&SearchFilter>,
        mode: BrowseMode,
        values: &[String],
        sort: Option<TrackSort>,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<Track>> {
        let romanized = self.prefer_romanized();
        let plan = FacetPlan::exact(&mode, romanized);
        self.tracks_range_with_plan(
            filter,
            plan.as_ref(),
            values,
            romanized,
            sort,
            offset,
            limit,
        )
    }

    pub fn tracks_range_with_plan(
        &self,
        filter: Option<&SearchFilter>,
        plan: Option<&FacetPlan>,
        values: &[String],
        romanized: bool,
        sort: Option<TrackSort>,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<Track>> {
        let (mut where_sql, mut params) = track_where(filter);
        push_values_clause(&mut where_sql, &mut params, plan, values);
        let order = order_sql(sort, romanized);
        self.with(|conn| {
            let sql = format!(
                "SELECT {TRACK_COLS} FROM tracks WHERE {where_sql} \
                 ORDER BY {order} LIMIT ?{} OFFSET ?{}",
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

    /// Count of live tracks for the sandboxed plugin `library.read` host
    /// interface. No filter, no facet values — plugins see the whole library
    /// through bounded pages only.
    pub fn plugin_library_count(&self) -> Result<i64> {
        self.with(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM tracks WHERE missing = 0",
                [],
                |row| row.get(0),
            )?)
        })
    }

    /// One id-ordered page of live tracks for the plugin `library.read` host
    /// interface. `include_paths` is passed only when the separate
    /// `library.paths` permission was granted; otherwise `path` is `None`.
    /// Page size is clamped by the caller (the host interface) to 2000 rows.
    pub fn plugin_library_page(
        &self,
        offset: i64,
        limit: i64,
        include_paths: bool,
    ) -> Result<PluginLibraryPage> {
        self.with(|conn| {
            let total: i64 = conn.query_row(
                "SELECT COUNT(*) FROM tracks WHERE missing = 0",
                [],
                |row| row.get(0),
            )?;
            let path_column = if include_paths { "path" } else { "NULL" };
            let mut stmt = conn.prepare(&format!(
                "SELECT id, title, artist, album, albumartist, genre, grouping, \
                 duration_ms, year, track_no, disc_no, {path_column}
                 FROM tracks WHERE missing = 0 ORDER BY id LIMIT ?1 OFFSET ?2"
            ))?;
            let rows = stmt.query_map(rusqlite::params![limit, offset], |row| {
                Ok(PluginLibraryTrack {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    artist: row.get(2)?,
                    album: row.get(3)?,
                    albumartist: row.get(4)?,
                    genre: row.get(5)?,
                    grouping: row.get(6)?,
                    duration_ms: row.get(7)?,
                    year: row.get(8)?,
                    track_no: row.get(9)?,
                    disc_no: row.get(10)?,
                    path: row.get(11)?,
                })
            })?;
            let tracks = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(PluginLibraryPage { tracks, total })
        })
    }

    /// 0-based index of `track_id` within the filtered + ordered view (same
    /// order as `tracks_range`), or `None` when the track isn't in that set.
    /// Lets the Track-list view scroll to the now-playing track after a filter
    /// change (e.g. clearing the search).
    /// **Ignores plugin facet transforms.** It builds a `FacetPlan::exact` from
    /// the stored `prefer_romanized` preference, so a plugin projecting this tag
    /// has no effect here. The app never calls it — the Tauri commands use
    /// `track_index_with_plan` under a held projection guard, which is what keeps
    /// the value list, counts, rows and playback on one revision. This exists for
    /// tests, examples, and headless harnesses that have no plugin registry.
    pub fn track_index_exact(
        &self,
        filter: Option<&SearchFilter>,
        mode: BrowseMode,
        values: &[String],
        sort: Option<TrackSort>,
        track_id: i64,
    ) -> Result<Option<i64>> {
        let romanized = self.prefer_romanized();
        let plan = FacetPlan::exact(&mode, romanized);
        self.track_index_with_plan(filter, plan.as_ref(), values, romanized, sort, track_id)
    }

    pub fn track_index_with_plan(
        &self,
        filter: Option<&SearchFilter>,
        plan: Option<&FacetPlan>,
        values: &[String],
        romanized: bool,
        sort: Option<TrackSort>,
        track_id: i64,
    ) -> Result<Option<i64>> {
        let (mut where_sql, mut params) = track_where(filter);
        push_values_clause(&mut where_sql, &mut params, plan, values);
        let order = order_sql(sort, romanized);
        self.with(|conn| {
            let sql = format!(
                "SELECT rn FROM ( \
                   SELECT id, ROW_NUMBER() OVER (ORDER BY {order}) - 1 AS rn \
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
    /// the same filtered + ordered set as `tracks_range`. Uses the
    /// gaps-and-islands pattern so a re-appearing album_id (e.g. split by a year
    /// change) and scattered NULL-album tracks each form their own run.
    /// **Ignores plugin facet transforms.** It builds a `FacetPlan::exact` from
    /// the stored `prefer_romanized` preference, so a plugin projecting this tag
    /// has no effect here. The app never calls it — the Tauri commands use
    /// `track_groups_with_plan` under a held projection guard, which is what
    /// keeps the value list, counts, rows and playback on one revision. This
    /// exists for tests, examples, and headless harnesses that have no plugin
    /// registry.
    pub fn track_groups_exact(
        &self,
        filter: Option<&SearchFilter>,
        mode: BrowseMode,
        values: &[String],
        sort: Option<TrackSort>,
    ) -> Result<Vec<TrackGroup>> {
        let romanized = self.prefer_romanized();
        let plan = FacetPlan::exact(&mode, romanized);
        self.track_groups_with_plan(filter, plan.as_ref(), values, romanized, sort)
    }

    pub fn track_groups_with_plan(
        &self,
        filter: Option<&SearchFilter>,
        plan: Option<&FacetPlan>,
        values: &[String],
        romanized: bool,
        sort: Option<TrackSort>,
    ) -> Result<Vec<TrackGroup>> {
        let (mut where_sql, mut params) = track_where(filter);
        push_values_clause(&mut where_sql, &mut params, plan, values);
        let order = order_sql(sort, romanized);
        self.with(|conn| {
            let sql = format!(
                "WITH ordered AS ( \
                   SELECT album_id, album, \
                          COALESCE(albumartist, artist) AS artist, \
                          COALESCE(albumartistsort, artistsort) AS artist_sort, year, \
                          ROW_NUMBER() OVER (ORDER BY {order}) AS rn \
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
    /// **Ignores plugin facet transforms.** It builds a `FacetPlan::exact` from
    /// the stored `prefer_romanized` preference, so a plugin projecting this tag
    /// has no effect here. The app never calls it — the Tauri commands use
    /// `resolve_context_with_projection` under a held projection guard, which is
    /// what keeps the value list, counts, rows and playback on one revision. This
    /// exists for tests, examples, and headless harnesses that have no plugin
    /// registry.
    pub fn resolve_context_exact(&self, ctx: &PlayContext) -> Result<Vec<i64>> {
        let romanized = self.prefer_romanized();
        let plan = match ctx {
            PlayContext::ValueTracks { mode, .. } => FacetPlan::exact(mode, romanized),
            _ => None,
        };
        self.resolve_context_with_projection(ctx, plan.as_ref(), romanized)
    }

    /// Projection-explicit context resolution used before the Tauri command
    /// hands concrete IDs to the player thread.
    pub fn resolve_context_with_projection(
        &self,
        ctx: &PlayContext,
        plan: Option<&FacetPlan>,
        romanized: bool,
    ) -> Result<Vec<i64>> {
        match ctx {
            PlayContext::Album { id } => self.with(|conn| {
                collect_ids(
                    conn,
                    "SELECT id FROM tracks WHERE album_id = ?1 AND missing = 0 \
                     ORDER BY disc_no, track_no, path",
                    vec![Value::from(*id)],
                )
            }),
            // Missing files stay in: a playlist view shows them (marked), and
            // dropping them here would shift every later row's index. The
            // player skips a file it cannot open and moves on.
            PlayContext::Playlist { id } => {
                self.with(|conn| collect_playlist_ids(conn, std::slice::from_ref(id), None))
            }
            PlayContext::FilteredPlaylist { id, query } => self.with(|conn| {
                collect_playlist_ids(conn, std::slice::from_ref(id), Some(query))
            }),
            // Playlist by playlist, so the ids line up with the combined view
            // the user double-clicked in.
            PlayContext::Playlists { ids } => {
                self.with(|conn| collect_playlist_ids(conn, ids, None))
            }
            PlayContext::FilteredPlaylists { ids, query } => {
                self.with(|conn| collect_playlist_ids(conn, ids, Some(query)))
            }
            PlayContext::LibraryView { query, sort, .. } => {
                let filter = compile_optional_query(query.as_deref())?;
                let (where_sql, params) = track_where(filter.as_ref());
                let order = order_sql(*sort, romanized);
                self.with(|conn| {
                    collect_ids(
                        conn,
                        &format!("SELECT id FROM tracks WHERE {where_sql} ORDER BY {order}"),
                        params,
                    )
                })
            }
            PlayContext::ValueTracks {
                values, query, sort, ..
            } => {
                let filter = compile_optional_query(query.as_deref())?;
                let (mut where_sql, mut params) = track_where(filter.as_ref());
                push_values_clause(&mut where_sql, &mut params, plan, values);
                let order = order_sql(*sort, romanized);
                self.with(|conn| {
                    collect_ids(
                        conn,
                        &format!("SELECT id FROM tracks WHERE {where_sql} ORDER BY {order}"),
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

    /// Expired Last.fm responses are never read again, so remove them at app
    /// startup instead of letting one row per viewed artist/album accumulate
    /// for the lifetime of the library database.
    pub fn lastfm_cache_prune(&self, max_age_secs: i64) -> Result<()> {
        self.with(|conn| {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'lastfm_cache')",
                [],
                |row| row.get(0),
            )?;
            if !exists {
                return Ok(());
            }
            conn.execute(
                "DELETE FROM lastfm_cache WHERE fetched_at < ?1",
                rusqlite::params![now_unix() - max_age_secs],
            )?;
            Ok(())
        })
    }

    /// The cached online-lyrics row for a track, as `(text, source, age in
    /// seconds)`. An empty `text` records a previous miss so unknown tracks
    /// are not re-queried; the caller applies the age limit, because a miss
    /// caused by a failed request expires far sooner than a real one (see
    /// `lyrics::for_track`).
    pub fn lyrics_cache_get(&self, track_id: i64) -> Result<Option<(String, String, i64)>> {
        self.with(|conn| {
            let now = now_unix();
            Ok(conn
                .query_row(
                    "SELECT text, source, fetched_at FROM lyrics_cache WHERE track_id = ?1",
                    rusqlite::params![track_id],
                    |r| Ok((r.get(0)?, r.get(1)?, now - r.get::<_, i64>(2)?)),
                )
                .ok())
        })
    }

    /// Drop lyrics cache rows past their useful life. Anything this deletes
    /// would be re-fetched and overwritten on the track's next play anyway,
    /// so the table stays bounded by "tracks played recently" rather than by
    /// the size of the library.
    pub fn lyrics_cache_prune(&self, max_age_secs: i64) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "DELETE FROM lyrics_cache WHERE fetched_at < ?1",
                rusqlite::params![now_unix() - max_age_secs],
            )?;
            Ok(())
        })
    }

    pub fn lyrics_cache_put(&self, track_id: i64, text: &str, source: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO lyrics_cache(track_id, text, source, fetched_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(track_id) DO UPDATE SET text = excluded.text, \
                 source = excluded.source, fetched_at = excluded.fetched_at",
                rusqlite::params![track_id, text, source, now_unix()],
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

/// Resolve one or more playlists in the same concatenated order PlaylistView
/// uses. Its filter is deliberately simple rather than the library's search
/// language: a Unicode-insensitive substring over the five fields shown by
/// that view's existing client-side filter.
fn collect_playlist_ids(
    conn: &Connection,
    playlist_ids: &[i64],
    query: Option<&str>,
) -> Result<Vec<i64>> {
    const UNFILTERED: &str =
        "SELECT pt.track_id FROM playlist_tracks pt \
         JOIN tracks t ON t.id = pt.track_id \
         WHERE pt.playlist_id = ?1 ORDER BY pt.position";
    const FILTERED: &str =
        "SELECT pt.track_id FROM playlist_tracks pt \
         JOIN tracks t ON t.id = pt.track_id \
         WHERE pt.playlist_id = ?1 AND (\
           ucontains(t.title, ?2) OR ucontains(t.artist, ?2) OR \
           ucontains(t.album, ?2) OR ucontains(t.albumartist, ?2) OR \
           ucontains(t.genre, ?2)\
         ) ORDER BY pt.position";

    let query = query.map(str::trim).filter(|query| !query.is_empty());
    let mut out = Vec::new();
    match query {
        Some(query) => {
            let mut stmt = conn.prepare(FILTERED)?;
            for id in playlist_ids {
                for row in stmt.query_map(rusqlite::params![id, query], |row| row.get(0))? {
                    out.push(row?);
                }
            }
        }
        None => {
            let mut stmt = conn.prepare(UNFILTERED)?;
            for id in playlist_ids {
                for row in stmt.query_map([id], |row| row.get(0))? {
                    out.push(row?);
                }
            }
        }
    }
    Ok(out)
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
