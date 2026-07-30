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
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    pub year: Option<i64>,
    pub duration_ms: i64,
    pub format: String,
    pub album_id: Option<i64>,
    pub rating: Option<i64>,
    /// Times this track has been played (>50% heard). See `Db::record_play`.
    pub play_count: i64,
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
    Grouping,
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
            BrowseMode::Grouping => Some("grouping"),
            BrowseMode::Custom(tag) => crate::search::browse_column(tag),
        }
    }
}

/// What ordered set of tracks playback iterates over. Resolved to concrete
/// track ids in Rust so the frontend never ships id lists.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlayContext {
    Album { id: i64 },
    Playlist { id: i64 },
    LibraryView { query: Option<String>, sort: Option<String> },
    ValueTracks {
        mode: BrowseMode,
        value: String,
        query: Option<String>,
        /// Collapse multi-theme grouping tags under their primary theme (only
        /// meaningful when `mode == Grouping`).
        #[serde(default)]
        collapse: bool,
    },
}
