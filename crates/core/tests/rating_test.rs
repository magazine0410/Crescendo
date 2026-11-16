use crescendo_core::db::Db;
use crescendo_core::model::BrowseMode;
use crescendo_core::search;

/// Four tracks with ids 1..=4, all unrated.
fn make_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        for i in 1..=4 {
            conn.execute(
                "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, duration_ms)
                 VALUES (?1, ?2, '/m', ?3, 0, 0, 'FLAC', 180000)",
                rusqlite::params![i, format!("/m/{i}.flac"), format!("{i}.flac")],
            )?;
        }
        Ok(())
    })
    .unwrap();
    (dir, db)
}

fn ids(db: &Db, query: &str) -> Vec<i64> {
    let filter = search::compile(query).unwrap();
    let mut ids: Vec<i64> = db
        .tracks_range_exact(Some(&filter), BrowseMode::Artist, &[], None, 0, 100)
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect();
    ids.sort();
    ids
}

#[test]
fn half_and_whole_star_ratings_round_trip_through_track_rows() {
    let (_dir, db) = make_db();
    db.set_rating(1, Some(3.5)).unwrap();
    db.set_rating(2, Some(4.0)).unwrap();
    // The ordinary track query propagates errors, unlike track_by_id.
    let tracks = db
        .tracks_range_exact(None, BrowseMode::Artist, &[], None, 0, 100)
        .unwrap();
    let rating = |id: i64| tracks.iter().find(|t| t.id == id).unwrap().rating;
    assert_eq!(rating(1), Some(3.5));
    assert_eq!(rating(2), Some(4.0));
    assert_eq!(rating(3), None);
    assert_eq!(db.track_by_id(1).unwrap().unwrap().rating, Some(3.5));

    db.set_rating(1, None).unwrap();
    assert_eq!(db.track_by_id(1).unwrap().unwrap().rating, None);
    assert_eq!(db.ratings().unwrap().len(), 1);
}

#[test]
fn invalid_ratings_and_unknown_tracks_are_rejected() {
    let (_dir, db) = make_db();
    for bad in [0.0, 0.3, 5.5, f64::NAN] {
        assert!(db.set_rating(1, Some(bad)).is_err(), "{bad}");
    }
    assert!(db.set_rating(99, Some(3.0)).is_err());
    assert!(db.ratings().unwrap().is_empty());
}

#[test]
fn search_compares_stars_and_skips_unrated() {
    let (_dir, db) = make_db();
    db.set_rating(1, Some(3.5)).unwrap();
    db.set_rating(2, Some(4.0)).unwrap();
    db.set_rating(3, Some(1.0)).unwrap();
    assert_eq!(ids(&db, "rating>=3.5"), [1, 2]);
    assert_eq!(ids(&db, "rating=4"), [2]);
    assert_eq!(ids(&db, "rating<3"), [3], "unrated is not a low rating");
}

#[test]
fn custom_rating_filter_lists_and_selects_star_values() {
    let (_dir, db) = make_db();
    db.set_rating(1, Some(3.5)).unwrap();
    db.set_rating(2, Some(4.0)).unwrap();
    let mode = BrowseMode::Custom("rating".into());
    let values: Vec<String> = db
        .browse_values_exact(mode.clone(), None)
        .unwrap()
        .into_iter()
        .map(|v| v.value)
        .collect();
    assert!(values.contains(&"3.5".to_string()), "{values:?}");
    assert!(values.contains(&"4".to_string()), "{values:?}");
    let picked = db
        .tracks_range_exact(None, mode, &["3.5".to_string()], None, 0, 100)
        .unwrap();
    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0].id, 1);
}
