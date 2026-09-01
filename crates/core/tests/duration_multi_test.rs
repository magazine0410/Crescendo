use crescendo_core::db::Db;
use crescendo_core::model::{BrowseMode, BrowseValue, PlayContext};

fn custom(tag: &str) -> BrowseMode {
    BrowseMode::Custom(tag.into())
}

fn browse_value(value: &str, count: i64) -> BrowseValue {
    BrowseValue { value: value.into(), count, sort_value: None }
}

/// Tracks spanning several per-minute duration buckets across two artists.
fn test_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        let mut ins = conn.prepare(
            "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms,
                                title, albumartist, artist, album, album_id, disc_no,
                                track_no, year)
             VALUES (?1, ?2, ?3, 0, 0, 'FLAC', ?4, ?5, ?6, ?6, 'Album', 1, 1, ?7, 2020)",
        )?;
        // (title, artist, duration_ms) — buckets 0, 2, 2, 3, 10
        let rows: Vec<(&str, &str, i64)> = vec![
            ("Short", "Artist A", 59_000),   // 0:59  -> bucket 0
            ("TwoA", "Artist A", 120_000),   // 2:00  -> bucket 2
            ("TwoB", "Artist B", 179_999),   // 2:59  -> bucket 2
            ("Three", "Artist B", 236_000),  // 3:56  -> bucket 3
            ("Long", "Artist B", 605_000),   // 10:05 -> bucket 10
        ];
        for (i, (title, artist, dur)) in rows.iter().enumerate() {
            ins.execute(rusqlite::params![
                format!("/m/{i}.flac"),
                "/m",
                format!("{i}.flac"),
                dur,
                title,
                artist,
                i as i64 + 1,
            ])?;
        }
        // The album row browse_albums joins against (normally scanner-built).
        conn.execute(
            "INSERT INTO albums(id, key, album, albumartist, year, track_count)
             VALUES (1, 'k', 'Album', 'Artist A', 2020, 5)",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    (dir, db)
}

#[test]
fn duration_browse_buckets_per_minute() {
    let (_d, db) = test_db();
    let values = db.browse_values_exact(custom("duration"), None).unwrap();
    // One row per whole minute, numerically ordered ("10" after "3", not "1x").
    assert_eq!(
        values,
        vec![
            browse_value("0", 1),
            browse_value("2", 2),
            browse_value("3", 1),
            browse_value("10", 1),
        ]
    );
    // The %length% alias buckets identically.
    assert_eq!(db.browse_values_exact(custom("length"), None).unwrap(), values);
}

#[test]
fn duration_value_selects_bucket_tracks() {
    let (_d, db) = test_db();
    assert_eq!(db.track_count_exact(None, custom("duration"), &["2".into()]).unwrap(), 2);
    let rows = db
        .tracks_range_exact(None, custom("duration"), &["2".into()], None, 0, 100)
        .unwrap();
    let titles: Vec<_> = rows.into_iter().map(|t| t.title.unwrap()).collect();
    assert_eq!(titles, ["TwoA", "TwoB"]);
}

#[test]
fn multi_select_unions_values() {
    let (_d, db) = test_db();
    // Two duration buckets at once (Ctrl-multi-select).
    let both = &["2".to_string(), "3".to_string()];
    assert_eq!(db.track_count_exact(None, custom("duration"), both).unwrap(), 3);
    let rows = db
        .tracks_range_exact(None, custom("duration"), both, None, 0, 100)
        .unwrap();
    let titles: Vec<_> = rows.into_iter().map(|t| t.title.unwrap()).collect();
    assert_eq!(titles, ["TwoA", "TwoB", "Three"]);

    // Same for a plain text mode: two album artists at once.
    let artists = &["Artist A".to_string(), "Artist B".to_string()];
    assert_eq!(db.track_count_exact(None, BrowseMode::Albumartist, artists).unwrap(), 5);
    assert_eq!(
        db.track_count_exact(None, BrowseMode::Albumartist, &["Artist A".into()]).unwrap(),
        2
    );

    // Empty selection = "All": no narrowing.
    assert_eq!(db.track_count_exact(None, BrowseMode::Albumartist, &[]).unwrap(), 5);
}

#[test]
fn multi_select_playback_context_resolves_union() {
    let (_d, db) = test_db();
    let ctx = PlayContext::ValueTracks {
        mode: custom("duration"),
        values: vec!["2".into(), "3".into()],
        query: None,
        projection_revision: 0,
        sort: None,
    };
    let ids = db.resolve_context_exact(&ctx).unwrap();
    assert_eq!(ids.len(), 3);
    let view = db
        .tracks_range_exact(None, custom("duration"), &["2".into(), "3".into()], None, 0, 100)
        .unwrap();
    assert_eq!(ids, view.iter().map(|t| t.id).collect::<Vec<_>>());
}

#[test]
fn multi_select_albums_union() {
    let (_d, db) = test_db();
    // browse_albums honors multiple values too (all tracks share one album here,
    // so any matching bucket lists it once).
    let albums = db.browse_albums_exact(custom("duration"), &["0".into(), "10".into()], None);
    assert_eq!(albums.unwrap().len(), 1);
    let none = db.browse_albums_exact(custom("duration"), &["7".into()], None);
    assert_eq!(none.unwrap().len(), 0);
}
