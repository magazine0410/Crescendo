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

#[test]
fn each_play_is_recorded_with_its_calendar_date() {
    let (_d, db, id) = test_db();
    db.record_play(id).unwrap();
    db.record_play(id).unwrap();

    db.with(|conn| {
        // One row per play, not one row per track.
        let rows: i64 = conn.query_row(
            "SELECT COUNT(*) FROM play_history WHERE track_id = ?1",
            [id],
            |r| r.get(0),
        )?;
        assert_eq!(rows, 2);

        // Every row carries the local calendar date of the play, and it agrees
        // with the epoch stored beside it.
        let bad: i64 = conn.query_row(
            "SELECT COUNT(*) FROM play_history WHERE \
                 year  != CAST(strftime('%Y', played_at, 'unixepoch', 'localtime') AS INTEGER) \
              OR month != CAST(strftime('%m', played_at, 'unixepoch', 'localtime') AS INTEGER) \
              OR day   != CAST(strftime('%d', played_at, 'unixepoch', 'localtime') AS INTEGER)",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(bad, 0);

        // The track's cache points at the newest of them.
        let (last, newest): (i64, i64) = conn.query_row(
            "SELECT t.last_played, MAX(h.played_at) FROM tracks t \
             JOIN play_history h ON h.track_id = t.id WHERE t.id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!(last, newest);
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_failed_play_leaves_no_half_written_history() {
    let (_d, db, id) = test_db();
    // A play of a track that is not in the library: the UPDATE matches no row,
    // so record_play reports the failure and its history row rolls back with it.
    assert!(db.record_play(id + 999).is_err());
    db.with(|conn| {
        let rows: i64 = conn.query_row("SELECT COUNT(*) FROM play_history", [], |r| r.get(0))?;
        assert_eq!(rows, 0);
        Ok(())
    })
    .unwrap();
    // The real track still records normally afterwards.
    assert_eq!(db.record_play(id).unwrap(), 1);
}
