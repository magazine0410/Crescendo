//! The lyrics cache stores three kinds of row — a hit, a "no lyrics exist"
//! miss, and a short-lived "the lookup failed" marker — so it reports each
//! row's age and lets `lyrics::for_track` apply the right limit.

use crescendo_core::db::Db;

fn open() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    (dir, db)
}

#[test]
fn get_returns_text_source_and_age() {
    let (_dir, db) = open();
    assert!(db.lyrics_cache_get(1).unwrap().is_none());

    db.lyrics_cache_put(1, "la la la", "LRCLIB").unwrap();
    let (text, source, age) = db.lyrics_cache_get(1).unwrap().unwrap();
    assert_eq!(text, "la la la");
    assert_eq!(source, "LRCLIB");
    assert!((0..5).contains(&age), "fresh row should be seconds old, got {age}");
}

#[test]
fn put_replaces_the_row_for_a_track() {
    let (_dir, db) = open();
    db.lyrics_cache_put(1, "", "").unwrap();
    db.lyrics_cache_put(1, "found later", "VocaDB · Romanized").unwrap();
    let (text, source, _) = db.lyrics_cache_get(1).unwrap().unwrap();
    assert_eq!(text, "found later");
    assert_eq!(source, "VocaDB · Romanized");
}

#[test]
fn prune_drops_only_rows_past_the_limit() {
    let (_dir, db) = open();
    db.lyrics_cache_put(1, "fresh", "LRCLIB").unwrap();
    db.lyrics_cache_put(2, "stale", "LRCLIB").unwrap();
    // Age row 2 by an hour.
    db.with(|conn| {
        conn.execute("UPDATE lyrics_cache SET fetched_at = fetched_at - 3600 WHERE track_id = 2", [])?;
        Ok(())
    })
    .unwrap();

    db.lyrics_cache_prune(1800).unwrap();
    assert!(db.lyrics_cache_get(1).unwrap().is_some());
    assert!(db.lyrics_cache_get(2).unwrap().is_none());
}
