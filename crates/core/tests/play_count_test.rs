use crescendo_core::db::Db;

fn test_db() -> (tempfile::TempDir, Db, i64) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    let id = db
        .with(|conn| {
            conn.execute(
                "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms)
                 VALUES ('/m/a.flac', '/m', 'a.flac', 0, 0, 'FLAC', 180000)",
                [],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .unwrap();
    (dir, db, id)
}

#[test]
fn record_play_starts_at_zero_and_increments() {
    let (_d, db, id) = test_db();

    // Fresh tracks have no plays.
    let t = db.track_by_id(id).unwrap().unwrap();
    assert_eq!(t.play_count, 0);

    // Each call bumps the counter and returns the new total.
    assert_eq!(db.record_play(id).unwrap(), 1);
    assert_eq!(db.record_play(id).unwrap(), 2);

    // The bump is persisted on the row.
    assert_eq!(db.track_by_id(id).unwrap().unwrap().play_count, 2);
}
