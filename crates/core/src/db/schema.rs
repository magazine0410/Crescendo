use rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 5;

pub fn migrate(conn: &mut Connection) -> anyhow::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    // SQLite commits each statement in execute_batch independently unless an
    // explicit transaction surrounds it. Keep every pending schema step and
    // the version marker atomic so a failed ALTER can be retried on next start.
    let tx = conn.transaction()?;
    if version < 1 {
        tx.execute_batch(
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

            CREATE TABLE IF NOT EXISTS collections (
              id INTEGER PRIMARY KEY,
              name TEXT NOT NULL,
              position INTEGER NOT NULL,
              collapsed INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS playlists (
              id INTEGER PRIMARY KEY,
              name TEXT NOT NULL,
              position INTEGER NOT NULL,
              collection_id INTEGER
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
            CREATE TABLE IF NOT EXISTS lyrics_cache (
              track_id INTEGER PRIMARY KEY,
              text TEXT NOT NULL,
              source TEXT NOT NULL,
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
        tx.execute_batch(
            "ALTER TABLE tracks ADD COLUMN titlesort TEXT;
             ALTER TABLE tracks ADD COLUMN artistsort TEXT;
             ALTER TABLE tracks ADD COLUMN albumartistsort TEXT;
             ALTER TABLE albums ADD COLUMN albumartist_sort TEXT;",
        )?;
    }
    if version < 3 && version >= 1 {
        // v3: per-track play counter (a track counts as played once >50% of it
        // has been heard). Fresh databases get the column from the CREATE above.
        tx.execute_batch(
            "ALTER TABLE tracks ADD COLUMN play_count INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    if version < 4 && version >= 1 {
        // v4: playlist collections — a named group a playlist can belong to.
        // Fresh databases get both from the CREATE block above; only existing
        // ones need the table and the column.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS collections (
               id INTEGER PRIMARY KEY,
               name TEXT NOT NULL,
               position INTEGER NOT NULL,
               collapsed INTEGER NOT NULL DEFAULT 0
             );
             ALTER TABLE playlists ADD COLUMN collection_id INTEGER;",
        )?;
    }
    if version < 5 && version >= 1 {
        // v5: cached online lyrics lookups (VocaDB/LRCLIB), keyed by track.
        // Fresh databases get the table from the CREATE block above.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS lyrics_cache (
               track_id INTEGER PRIMARY KEY,
               text TEXT NOT NULL,
               source TEXT NOT NULL,
               fetched_at INTEGER NOT NULL
             );",
        )?;
    }
    tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::migrate;
    use rusqlite::Connection;

    #[test]
    fn failed_migration_rolls_back_earlier_schema_changes() {
        let mut conn = Connection::open_in_memory().unwrap();
        // The first v2 ALTER (titlesort) can succeed, while the second one is
        // forced to fail because artistsort already exists.
        conn.execute_batch(
            "CREATE TABLE tracks (id INTEGER PRIMARY KEY, artistsort TEXT);
             CREATE TABLE albums (id INTEGER PRIMARY KEY);
             PRAGMA user_version = 1;",
        )
        .unwrap();

        assert!(migrate(&mut conn).is_err());
        let columns: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('tracks')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(columns.contains(&"artistsort".to_string()));
        assert!(
            !columns.contains(&"titlesort".to_string()),
            "the successful first ALTER must roll back with the failed batch"
        );
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
        assert!(conn.is_autocommit());
    }
}
