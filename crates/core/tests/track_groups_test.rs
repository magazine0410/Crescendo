use crescendo_core::db::Db;
use crescendo_core::model::BrowseMode;

/// Insert tracks whose LIBRARY_SORT order (albumartist, year, album, disc, track,
/// path) makes album_id 1 re-appear non-consecutively (split by a later year) and
/// includes a NULL-album track, so the gaps-and-islands grouping is exercised.
fn test_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        let mut ins = conn.prepare(
            "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms,
                                albumartist, artist, album, album_id, disc_no, track_no, year)
             VALUES (?1, ?2, ?3, 0, 0, 'FLAC', ?4, ?5, ?5, ?6, ?7, 1, ?8, ?9)",
        )?;
        // (album, album_id, track_no, year) under albumartist "Artist X"
        let rows: Vec<(&str, Option<i64>, i64, i64)> = vec![
            ("Alpha", Some(1), 1, 2000),
            ("Alpha", Some(1), 2, 2000),
            ("Beta", Some(2), 1, 2010),
            ("Beta", Some(2), 2, 2010),
            ("Alpha", Some(1), 3, 2020), // same album_id 1, later year -> sorts after Beta
        ];
        for (i, (album, album_id, tn, year)) in rows.iter().enumerate() {
            ins.execute(rusqlite::params![
                format!("/m/x/{i}.flac"),
                "/m/x",
                format!("{i}.flac"),
                180_000i64,
                "Artist X",
                album,
                album_id,
                tn,
                year,
            ])?;
        }
        // NULL-album track under a later-sorting artist
        ins.execute(rusqlite::params![
            "/m/y/0.flac",
            "/m/y",
            "0.flac",
            180_000i64,
            "Artist Y",
            Option::<String>::None,
            Option::<i64>::None,
            1i64,
            2005i64,
        ])?;
        Ok(())
    })
    .unwrap();
    (dir, db)
}

#[test]
fn groups_split_reappearing_album_id_and_isolate_null() {
    let (_d, db) = test_db();
    let groups = db.track_groups(None, BrowseMode::Artist, None, false).unwrap();
    let shape: Vec<(Option<i64>, i64)> = groups.iter().map(|g| (g.album_id, g.count)).collect();
    // album_id 1 appears as two separate runs (not merged), NULL is its own run.
    assert_eq!(shape, vec![(Some(1), 2), (Some(2), 2), (Some(1), 1), (None, 1)]);
    // The runs' counts partition the whole filtered track set exactly.
    let total: i64 = groups.iter().map(|g| g.count).sum();
    assert_eq!(total, db.track_count(None, BrowseMode::Artist, None, false).unwrap());
}

#[test]
fn groups_respect_value_filter() {
    let (_d, db) = test_db();
    // Narrow to album "Beta": one run of 2 tracks.
    let groups = db
        .track_groups(None, BrowseMode::Album, Some("Beta"), false)
        .unwrap();
    let shape: Vec<(Option<i64>, i64)> = groups.iter().map(|g| (g.album_id, g.count)).collect();
    assert_eq!(shape, vec![(Some(2), 2)]);
}
