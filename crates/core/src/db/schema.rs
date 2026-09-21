use rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 12;

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
              composer     TEXT,
              arranger     TEXT,
              lyricist     TEXT,
              vocalist     TEXT,
              catalognumber TEXT,
              event        TEXT,
              bpm          INTEGER,
              track_no     INTEGER,
              disc_no      INTEGER,
              year         INTEGER,
              date         TEXT,
              rating       INTEGER,
              has_embedded_art INTEGER NOT NULL DEFAULT 0,
              album_id     INTEGER,
              search_blob  TEXT NOT NULL DEFAULT '',
              missing      INTEGER NOT NULL DEFAULT 0,
              play_count   INTEGER NOT NULL DEFAULT 0,
              last_played  INTEGER
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

            CREATE TABLE IF NOT EXISTS play_history (
              id         INTEGER PRIMARY KEY,
              track_id   INTEGER NOT NULL,
              played_at  INTEGER NOT NULL,
              year       INTEGER NOT NULL,
              month      INTEGER NOT NULL,
              day        INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_play_history_track ON play_history(track_id, played_at);
            CREATE INDEX IF NOT EXISTS idx_play_history_at    ON play_history(played_at);
            CREATE INDEX IF NOT EXISTS idx_play_history_date  ON play_history(year, month, day);

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

            CREATE TABLE IF NOT EXISTS plugin_state (
              plugin_id TEXT PRIMARY KEY,
              desired_enabled INTEGER NOT NULL DEFAULT 0,
              failure_count INTEGER NOT NULL DEFAULT 0,
              last_error TEXT
            );
            CREATE TABLE IF NOT EXISTS plugin_pending (
              plugin_id TEXT PRIMARY KEY,
              operation TEXT NOT NULL,
              staged_path TEXT NOT NULL,
              target_version TEXT,
              package_hash TEXT,
              remove_data INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS plugin_settings (
              plugin_id TEXT NOT NULL,
              setting_key TEXT NOT NULL,
              value_json TEXT NOT NULL,
              PRIMARY KEY(plugin_id, setting_key)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS plugin_grants (
              plugin_id TEXT PRIMARY KEY,
              permissions_json TEXT NOT NULL,
              network_domains_json TEXT NOT NULL,
              version TEXT NOT NULL,
              package_hash TEXT NOT NULL,
              granted_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS plugin_storage (
              plugin_id TEXT NOT NULL,
              storage_key TEXT NOT NULL,
              value TEXT NOT NULL,
              PRIMARY KEY(plugin_id, storage_key)
            ) WITHOUT ROWID;
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
        tx.execute_batch("ALTER TABLE tracks ADD COLUMN play_count INTEGER NOT NULL DEFAULT 0;")?;
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
    if version < 6 && version >= 1 {
        // v6: one row per play, with the local calendar date it happened on, and
        // a `tracks.last_played` cache of the newest of them. The cache is what
        // the Last-played filter and sort read, so neither has to touch the
        // history table for 136k rows. Fresh databases get both from the CREATE
        // block above; a pre-v6 database has no history to backfill from, so
        // `last_played` starts NULL even where `play_count` is not 0.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS play_history (
               id         INTEGER PRIMARY KEY,
               track_id   INTEGER NOT NULL,
               played_at  INTEGER NOT NULL,
               year       INTEGER NOT NULL,
               month      INTEGER NOT NULL,
               day        INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_play_history_track ON play_history(track_id, played_at);
             CREATE INDEX IF NOT EXISTS idx_play_history_at    ON play_history(played_at);
             CREATE INDEX IF NOT EXISTS idx_play_history_date  ON play_history(year, month, day);
             ALTER TABLE tracks ADD COLUMN last_played INTEGER;",
        )?;
    }
    if version < 7 && version >= 1 {
        // v7: credit and release tags the scanner previously read past
        // (COMPOSER/ARRANGER/LYRICIST/VOCALIST/CATALOGNUMBER/EVENT/BPM). Fresh
        // databases get them from the CREATE block above. Existing rows stay
        // NULL until a *full* rescan re-reads their tags — an incremental scan
        // skips files whose mtime and size are unchanged, which is all of them.
        tx.execute_batch(
            "ALTER TABLE tracks ADD COLUMN composer TEXT;
             ALTER TABLE tracks ADD COLUMN arranger TEXT;
             ALTER TABLE tracks ADD COLUMN lyricist TEXT;
             ALTER TABLE tracks ADD COLUMN vocalist TEXT;
             ALTER TABLE tracks ADD COLUMN catalognumber TEXT;
             ALTER TABLE tracks ADD COLUMN event TEXT;
             ALTER TABLE tracks ADD COLUMN bpm INTEGER;",
        )?;
    }
    if version < 8 && version >= 1 {
        // v8: installed plugin lifecycle state, crash-safe deferred package
        // operations, and namespaced manifest-declared setting values. Package
        // metadata itself remains in each installed manifest.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS plugin_state (
               plugin_id TEXT PRIMARY KEY,
               desired_enabled INTEGER NOT NULL DEFAULT 0,
               failure_count INTEGER NOT NULL DEFAULT 0,
               last_error TEXT
             );
             CREATE TABLE IF NOT EXISTS plugin_pending (
               plugin_id TEXT PRIMARY KEY,
               operation TEXT NOT NULL,
               staged_path TEXT NOT NULL,
               target_version TEXT,
               package_hash TEXT,
               remove_data INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS plugin_settings (
               plugin_id TEXT NOT NULL,
               setting_key TEXT NOT NULL,
               value_json TEXT NOT NULL,
               PRIMARY KEY(plugin_id, setting_key)
             ) WITHOUT ROWID;",
        )?;
    }
    if version < 9 && version >= 1 {
        // v9: the former built-in grouping projection moves to the optional
        // official plugin. Existing databases that used exact values need no
        // notice; every other existing install gets the one-time offer. Fresh
        // databases skip migration blocks and therefore get no marker.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS prefs (
               key TEXT PRIMARY KEY,
               value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS meta (
               key TEXT PRIMARY KEY,
               value TEXT
             );
             INSERT INTO meta(key, value)
             SELECT 'grouping_plugin_migration',
                    CASE WHEN EXISTS(
                      SELECT 1 FROM prefs
                      WHERE key = 'grouping_split_multi' AND value = 'true'
                    ) THEN 'completed' ELSE 'pending' END
             WHERE NOT EXISTS(
               SELECT 1 FROM meta WHERE key = 'grouping_plugin_migration'
             );
             DELETE FROM prefs
             WHERE key = 'grouping_split_multi'
               AND EXISTS(
                 SELECT 1 FROM meta
                 WHERE key = 'grouping_plugin_migration' AND value = 'completed'
               );",
        )?;
    }
    if version < 10 && version >= 1 {
        // v10: Stage 5 capability grants (pinned to the exact approved package)
        // and namespaced key/value storage for sandboxed Wasm plugins. Grant
        // approval is an explicit user action, so an upgrading database starts
        // with no grants; Wasm plugins that request permissions stay disabled
        // until approved.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS plugin_grants (
               plugin_id TEXT PRIMARY KEY,
               permissions_json TEXT NOT NULL,
               network_domains_json TEXT NOT NULL,
               version TEXT NOT NULL,
               package_hash TEXT NOT NULL,
               granted_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS plugin_storage (
               plugin_id TEXT NOT NULL,
               storage_key TEXT NOT NULL,
               value TEXT NOT NULL,
               PRIMARY KEY(plugin_id, storage_key)
             ) WITHOUT ROWID;",
        )?;
    }
    if version < 11 {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS lastfm_outbox (
               id INTEGER PRIMARY KEY,
               account TEXT NOT NULL,
               track_json TEXT NOT NULL,
               started_at INTEGER NOT NULL,
               state TEXT NOT NULL DEFAULT 'pending',
               message TEXT,
               UNIQUE(account, started_at, track_json)
             );
             CREATE INDEX IF NOT EXISTS idx_lastfm_outbox_pending ON lastfm_outbox(state, id);",
        )?;
    }
    if version < 12 {
        // Match the default library order so arbitrary scrollbar jumps do not
        // sort the entire library again for every visible page.
        tx.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_tracks_library_order ON tracks(
               albumartist COLLATE NOCASE, year, album COLLATE NOCASE,
               disc_no, track_no, path
             ) WHERE missing = 0;",
        )?;
    }
    tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{migrate, SCHEMA_VERSION};
    use rusqlite::Connection;

    // Older migration fixtures omit unrelated track metadata, but must include
    // the columns indexed by the library-order migration.
    fn legacy_tracks(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE tracks(id INTEGER PRIMARY KEY, albumartist TEXT, year INTEGER,
             album TEXT, disc_no INTEGER, track_no INTEGER, path TEXT, missing INTEGER);",
        )
        .unwrap();
    }

    #[test]
    fn version_nine_database_gains_grant_and_storage_tables() {
        let mut conn = Connection::open_in_memory().unwrap();
        legacy_tracks(&conn);
        conn.execute_batch(
            "CREATE TABLE prefs(key TEXT PRIMARY KEY, value TEXT NOT NULL);
              CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT);
              CREATE TABLE plugin_state(plugin_id TEXT PRIMARY KEY, desired_enabled INTEGER NOT NULL DEFAULT 0, failure_count INTEGER NOT NULL DEFAULT 0, last_error TEXT);
              CREATE TABLE plugin_pending(plugin_id TEXT PRIMARY KEY, operation TEXT NOT NULL, staged_path TEXT NOT NULL, target_version TEXT, package_hash TEXT, remove_data INTEGER NOT NULL DEFAULT 0);
              CREATE TABLE plugin_settings(plugin_id TEXT NOT NULL, setting_key TEXT NOT NULL, value_json TEXT NOT NULL, PRIMARY KEY(plugin_id, setting_key)) WITHOUT ROWID;
              PRAGMA user_version = 9;",
        )
        .unwrap();

        migrate(&mut conn).unwrap();

        for table in ["plugin_grants", "plugin_storage"] {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "{table} was not created by the v10 migration");
        }
        // Approval is deliberate: an upgrading database starts with no grants.
        let grants: i64 = conn
            .query_row("SELECT COUNT(*) FROM plugin_grants", [], |row| row.get(0))
            .unwrap();
        assert_eq!(grants, 0);
    }

    #[test]
    fn version_seven_database_gains_plugin_tables() {
        let mut conn = Connection::open_in_memory().unwrap();
        legacy_tracks(&conn);
        conn.execute_batch(
            "CREATE TABLE prefs(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT);
             PRAGMA user_version = 7;",
        )
        .unwrap();

        migrate(&mut conn).unwrap();

        for table in ["plugin_state", "plugin_pending", "plugin_settings"] {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "{table} was not created by the v8 migration");
        }
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn existing_collapse_users_get_one_time_grouping_migration_but_fresh_databases_do_not() {
        let mut old = Connection::open_in_memory().unwrap();
        legacy_tracks(&old);
        old.execute_batch(
            "CREATE TABLE prefs(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE plugin_state(plugin_id TEXT PRIMARY KEY, desired_enabled INTEGER NOT NULL DEFAULT 0, failure_count INTEGER NOT NULL DEFAULT 0, last_error TEXT);
             CREATE TABLE plugin_pending(plugin_id TEXT PRIMARY KEY, operation TEXT NOT NULL, staged_path TEXT NOT NULL, target_version TEXT, package_hash TEXT, remove_data INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE plugin_settings(plugin_id TEXT NOT NULL, setting_key TEXT NOT NULL, value_json TEXT NOT NULL, PRIMARY KEY(plugin_id, setting_key)) WITHOUT ROWID;
             PRAGMA user_version = 8;",
        )
        .unwrap();
        migrate(&mut old).unwrap();
        let state: String = old
            .query_row(
                "SELECT value FROM meta WHERE key = 'grouping_plugin_migration'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "pending");

        let mut fresh = Connection::open_in_memory().unwrap();
        migrate(&mut fresh).unwrap();
        let count: i64 = fresh
            .query_row(
                "SELECT COUNT(*) FROM meta WHERE key = 'grouping_plugin_migration'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn version_eleven_gains_an_index_that_supplies_library_order() {
        let mut conn = Connection::open_in_memory().unwrap();
        legacy_tracks(&conn);
        conn.execute_batch("PRAGMA user_version = 11;").unwrap();
        migrate(&mut conn).unwrap();
        let sql = format!(
            "EXPLAIN QUERY PLAN SELECT id FROM tracks WHERE missing = 0 \
             ORDER BY {} LIMIT 200 OFFSET 150000",
            crate::db::queries::LIBRARY_SORT,
        );
        let plan = conn
            .prepare(&sql)
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join("\n");
        assert!(plan.contains("idx_tracks_library_order"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
        migrate(&mut conn).unwrap();
    }

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
