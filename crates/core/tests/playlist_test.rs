use crescendo_core::db::Db;

fn test_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        let mut ins = conn.prepare(
            "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, duration_ms,
                                albumartist, artist, album)
             VALUES (?1, ?2, ?2, ?2, 0, 0, 'FLAC', 1000, 'A', 'A', 'Al')",
        )?;
        for id in 1..=5i64 {
            ins.execute(rusqlite::params![id, format!("/m/{id}.flac")])?;
        }
        Ok(())
    })
    .unwrap();
    (dir, db)
}

fn ids(db: &Db, pid: i64) -> Vec<i64> {
    db.playlist_tracks(pid).unwrap().iter().map(|t| t.id).collect()
}

#[test]
fn add_tracks_skips_existing_members() {
    let (_d, db) = test_db();
    let pid = db.playlist_create("P").unwrap();

    db.playlist_add_tracks(pid, &[1, 2, 3]).unwrap();
    assert_eq!(ids(&db, pid), vec![1, 2, 3]);

    // 2 and 3 already present -> only 4 is appended, order preserved.
    db.playlist_add_tracks(pid, &[2, 3, 4]).unwrap();
    assert_eq!(ids(&db, pid), vec![1, 2, 3, 4]);

    // Re-adding only duplicates is a no-op.
    db.playlist_add_tracks(pid, &[1, 4]).unwrap();
    assert_eq!(ids(&db, pid), vec![1, 2, 3, 4]);
}

#[test]
fn ids_with_track_reports_membership() {
    let (_d, db) = test_db();
    let a = db.playlist_create("A").unwrap();
    let b = db.playlist_create("B").unwrap();
    db.playlist_add_tracks(a, &[1, 2]).unwrap();
    db.playlist_add_tracks(b, &[2, 3]).unwrap();

    let mut in2 = db.playlist_ids_with_track(2).unwrap();
    in2.sort();
    assert_eq!(in2, vec![a, b]);
    assert_eq!(db.playlist_ids_with_track(1).unwrap(), vec![a]);
    assert!(db.playlist_ids_with_track(5).unwrap().is_empty());
}
