use rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 3;

pub fn migrate(conn: &Connection) -> anyhow::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    if version < 1 {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS tracks (
              id           INTEGER PRIMARY KEY,
              path         TEXT NOT NULL UNIQUE,
              dir          TEXT NOT NULL,
              filename     TEXT NOT NULL,
              mtime        INTEGER NOT NULL,
              size         INTEGER NOT NULL,
              format       TEXT NOT NULL,
              duration_ms  INTEGER NOT NULL DEFAULT 0,
              bitrate      INTEGER,
              sample_rate  INTEGER,
              bit_depth    INTEGER,
              title        TEXT,
              artist       TEXT,
              album        TEXT,
              albumartist  TEXT,
              titlesort    TEXT,
              artistsort   TEXT,
              albumartistsort TEXT,
              genre        TEXT,
              grouping     TEXT,
              track_no     INTEGER,
              disc_no      INTEGER,
              year         INTEGER,
              date         TEXT,
              rating       INTEGER,
              has_embedded_art INTEGER NOT NULL DEFAULT 0,
              album_id     INTEGER,
              search_blob  TEXT NOT NULL DEFAULT '',
              missing      INTEGER NOT NULL DEFAULT 0,
              play_count   INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_tracks_album    ON tracks(album_id, disc_no, track_no);
            CREATE INDEX IF NOT EXISTS idx_tracks_artist   ON tracks(artist COLLATE NOCASE);
            CREATE INDEX IF NOT EXISTS idx_tracks_genre    ON tracks(genre COLLATE NOCASE);
            CREATE INDEX IF NOT EXISTS idx_tracks_grouping ON tracks(grouping COLLATE NOCASE);
            CREATE INDEX IF NOT EXISTS idx_tracks_dir      ON tracks(dir);

            CREATE TABLE IF NOT EXISTS albums (
              id INTEGER PRIMARY KEY,
              key TEXT NOT NULL UNIQUE,
              album TEXT,
              albumartist TEXT,
              albumartist_sort TEXT,
              year INTEGER,
              genre TEXT,
              track_count INTEGER NOT NULL DEFAULT 0,
              total_duration_ms INTEGER NOT NULL DEFAULT 0,
              dir TEXT,
              cover_source TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_albums_artist ON albums(albumartist COLLATE NOCASE, year);

            CREATE TABLE IF NOT EXISTS playlists (
              id INTEGER PRIMARY KEY,
              name TEXT NOT NULL,
              position INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS playlist_tracks (
              playlist_id INTEGER NOT NULL,
              position INTEGER NOT NULL,
              track_id INTEGER NOT NULL,
              PRIMARY KEY (playlist_id, position)
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS lastfm_cache (
              key TEXT PRIMARY KEY,
              json TEXT NOT NULL,
              fetched_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS prefs (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS meta (
              key TEXT PRIMARY KEY,
              value TEXT
            );
            "#,
        )?;
    }
    if version < 2 && version >= 1 {
        // v2: romanized sort tags. Fresh v1-block databases already include
        // these columns via the CREATE below being versioned together; only
        // pre-existing v1 databases need the ALTERs.
        conn.execute_batch(
            "ALTER TABLE tracks ADD COLUMN titlesort TEXT;
             ALTER TABLE tracks ADD COLUMN artistsort TEXT;
             ALTER TABLE tracks ADD COLUMN albumartistsort TEXT;
             ALTER TABLE albums ADD COLUMN albumartist_sort TEXT;",
        )?;
    }
    if version < 3 && version >= 1 {
        // v3: per-track play counter (a track counts as played once >50% of it
        // has been heard). Fresh databases get the column from the CREATE above.
        conn.execute_batch(
            "ALTER TABLE tracks ADD COLUMN play_count INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
    Ok(())
}
