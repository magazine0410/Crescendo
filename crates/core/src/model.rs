use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: i64,
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub albumartist: Option<String>,
    pub titlesort: Option<String>,
    pub artistsort: Option<String>,
    pub albumartistsort: Option<String>,
    pub genre: Option<String>,
    pub grouping: Option<String>,
    pub composer: Option<String>,
    pub arranger: Option<String>,
    pub lyricist: Option<String>,
    pub vocalist: Option<String>,
    pub catalognumber: Option<String>,
    pub event: Option<String>,
    pub bpm: Option<i64>,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    pub year: Option<i64>,
    pub duration_ms: i64,
    pub format: String,
    pub album_id: Option<i64>,
    pub rating: Option<i64>,
    /// Times this track has been played (>50% heard). See `Db::record_play`.
    pub play_count: i64,
    /// The file was not on disk at the last scan. Playlists keep such tracks
    /// and mark them; library views filter them out.
    pub missing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub id: i64,
    pub album: Option<String>,
    pub albumartist: Option<String>,
    pub albumartist_sort: Option<String>,
    pub year: Option<i64>,
    pub genre: Option<String>,
    pub track_count: i64,
    pub total_duration_ms: i64,
}

/// One album run in the Track-list view: a maximal block of consecutive tracks
/// (in LIBRARY_SORT order) sharing an album_id. `count` is the number of tracks
/// in this run for the current filter, so the runs' counts sum to `track_count`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackGroup {
    pub album_id: Option<i64>,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub artist_sort: Option<String>,
    pub year: Option<i64>,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryStats {
    pub tracks: i64,
    pub albums: i64,
    pub total_duration_ms: i64,
    pub formats: std::collections::HashMap<String, i64>,
}

/// One row in the Library tab's value facet. `sort_value` is separate from the
/// visible/filter value so a future projection can order or annotate a label
/// without changing the value that round-trips through selection queries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowseValue {
    pub value: String,
    pub count: i64,
    pub sort_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanProgress {
    pub scanned: u64,
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    pub errors: u64,
    pub current_path: String,
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowseMode {
    Genre,
    Artist,
    Album,
    Albumartist,
    /// User-defined left-panel filter: browse a single tag the user typed
    /// (e.g. `year`). Serializes as `{"custom": "year"}`. Only tags that map to
    /// a real, stored column are browsable — anything else resolves to `None`
    /// below and the value list comes back empty.
    Custom(String),
}

impl BrowseMode {
    /// Column backing this browse mode on `tracks`, or `None` when a custom tag
    /// does not resolve to a real, browsable column (→ empty value list). The
    /// returned name is always a hardcoded literal, never the raw user input.
    pub fn column(&self) -> Option<&'static str> {
        match self {
            BrowseMode::Genre => Some("genre"),
            BrowseMode::Artist => Some("artist"),
            BrowseMode::Album => Some("album"),
            BrowseMode::Albumartist => Some("albumartist"),
            BrowseMode::Custom(_) if self.is_duration() => Some("duration_ms"),
            BrowseMode::Custom(_) if self.is_last_played() => Some("last_played"),
            BrowseMode::Custom(_) if self.is_play_count() => Some("play_count"),
            BrowseMode::Custom(tag) => crate::search::browse_column(tag),
        }
    }

    /// Whether this is the custom `%duration%` / `%length%` filter. Duration has
    /// no browsable text column: the value list buckets tracks per whole minute
    /// (`duration_ms / 60000`, see queries.rs) and values are bucket numbers as
    /// strings ("0", "3", …); the UI renders them as "3 minutes (3:00 - 3:59)".
    pub fn is_duration(&self) -> bool {
        matches!(self, BrowseMode::Custom(tag)
            if matches!(tag.trim().to_lowercase().as_str(), "duration" | "length"))
    }

    /// Whether this is the custom `%lastplayed%` filter. Like duration it has no
    /// browsable text column: the value list buckets tracks by how long ago they
    /// were last played (see `queries::last_played_bucket`) and values are
    /// bucket numbers as strings ("0" = today … "5" = never played); the UI
    /// renders them as "Today", "This week", …
    pub fn is_last_played(&self) -> bool {
        matches!(self, BrowseMode::Custom(tag)
            if matches!(tag.trim().to_lowercase().as_str(), "lastplayed" | "last_played"))
    }

    /// Whether this is the custom `%playcount%` filter. Values are the play
    /// counts themselves as strings ("0", "1", …), listed most-played first, so
    /// the tracks nothing has ever played sit at the bottom. Unlike the other
    /// two special filters it groups a plain column; only the ordering (and the
    /// UI's "3 plays" labelling) sets it apart.
    pub fn is_play_count(&self) -> bool {
        matches!(self, BrowseMode::Custom(tag)
            if matches!(tag.trim().to_lowercase().as_str(),
                        "playcount" | "play_count" | "plays"))
    }
}

/// A user-selectable sort key for the Track-list view. Maps to a hardcoded
/// ORDER BY expression in `queries::order_sql` — never raw user input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    Albumartist,
    Artist,
    Album,
    Genre,
    Grouping,
    Year,
    Title,
    Duration,
    /// Newest play first when descending. Never-played tracks have no time at
    /// all: they sort as the oldest, so descending leaves them at the bottom.
    LastPlayed,
}

/// Explicit Track-list sort: a primary key with its direction, and an optional
/// secondary key whose direction either follows the primary
/// (`secondary_descending: None`) or is set on its own. `None` in the
/// `Option<TrackSort>` spots means the default `LIBRARY_SORT` order. Remaining
/// ties always fall back to the LIBRARY_SORT chain minus the keys already
/// chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackSort {
    pub key: SortKey,
    #[serde(default)]
    pub secondary: Option<SortKey>,
    #[serde(default)]
    pub descending: bool,
    #[serde(default)]
    pub secondary_descending: Option<bool>,
}

/// What ordered set of tracks playback iterates over. Resolved to concrete
/// track ids in Rust so the frontend never ships id lists.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlayContext {
    Album {
        id: i64,
    },
    Playlist {
        id: i64,
    },
    /// A playlist narrowed by the simple text filter used by PlaylistView.
    /// Playback resolves the matches again in Rust so track-id lists never
    /// cross IPC.
    FilteredPlaylist {
        id: i64,
        query: String,
    },
    /// Several playlists shown as one list (Ctrl/Shift-multi-select in the
    /// sidebar). Their tracks play back to back, in the order given — the
    /// same order the view draws them in.
    Playlists {
        ids: Vec<i64>,
    },
    /// The filtered form of `Playlists`, preserving playlist order first and
    /// position within each playlist second.
    FilteredPlaylists {
        ids: Vec<i64>,
        query: String,
    },
    LibraryView {
        query: Option<String>,
        /// Revision of the projection used to render the clicked row.
        projection_revision: u64,
        #[serde(default)]
        sort: Option<TrackSort>,
    },
    ValueTracks {
        mode: BrowseMode,
        /// The left-panel selection (one or more Ctrl-multi-selected values);
        /// tracks matching any of them play.
        values: Vec<String>,
        query: Option<String>,
        /// Revision of the projection used to render the clicked row.
        projection_revision: u64,
        #[serde(default)]
        sort: Option<TrackSort>,
    },
}
