use crescendo_core::db::Db;
use crescendo_core::model::PlayContext;

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
fn set_order_rewrites_the_sidebar_order() {
    let (_d, db) = test_db();
    let a = db.playlist_create("A").unwrap();
    let b = db.playlist_create("B").unwrap();
    let c = db.playlist_create("C").unwrap();
    let listed = |db: &Db| -> Vec<i64> { db.playlist_list().unwrap().iter().map(|p| p.id).collect() };
    assert_eq!(listed(&db), vec![a, b, c], "created order to begin with");

    db.playlists_set_order(&[c, a, b]).unwrap();
    assert_eq!(listed(&db), vec![c, a, b]);

    // A playlist made afterwards goes to the end, not into the middle.
    let d = db.playlist_create("D").unwrap();
    assert_eq!(listed(&db), vec![c, a, b, d]);
}

#[test]
fn set_order_leaves_unlisted_playlists_in_the_list() {
    let (_d, db) = test_db();
    let a = db.playlist_create("A").unwrap();
    let b = db.playlist_create("B").unwrap();
    let c = db.playlist_create("C").unwrap();

    // A stale list that predates C must not drop it.
    db.playlists_set_order(&[b, a]).unwrap();
    let mut got: Vec<i64> = db.playlist_list().unwrap().iter().map(|p| p.id).collect();
    got.sort();
    let mut want = vec![a, b, c];
    want.sort();
    assert_eq!(got, want);
}

/// The combined view of several playlists concatenates them in the order
/// given, and playback has to resolve to exactly the same list — a row's
/// index in the view is the index the player is sent.
#[test]
fn multi_selection_view_and_playback_agree_on_order() {
    let (_d, db) = test_db();
    let a = db.playlist_create("A").unwrap();
    let b = db.playlist_create("B").unwrap();
    db.playlist_add_tracks(a, &[3, 1]).unwrap();
    db.playlist_add_tracks(b, &[5, 2, 4]).unwrap();

    let view: Vec<i64> = db
        .playlist_tracks_multi(&[b, a])
        .unwrap()
        .iter()
        .flat_map(|g| g.tracks.iter().map(|t| t.id))
        .collect();
    assert_eq!(view, vec![5, 2, 4, 3, 1], "playlist order, then position inside each");

    let played = db.resolve_context(&PlayContext::Playlists { ids: vec![b, a] }).unwrap();
    assert_eq!(played, view, "playback order must match the view row for row");
}

#[test]
fn multi_selection_keeps_a_group_per_id() {
    let (_d, db) = test_db();
    let a = db.playlist_create("A").unwrap();
    let empty = db.playlist_create("Empty").unwrap();
    db.playlist_add_tracks(a, &[1, 2]).unwrap();

    // An empty playlist still gets a group, so the view's grouping cannot
    // shift onto the wrong playlist.
    let groups = db.playlist_tracks_multi(&[a, empty, 999]).unwrap();
    assert_eq!(groups.len(), 3);
    assert_eq!(groups[0].playlist_id, a);
    assert_eq!(groups[0].tracks.len(), 2);
    assert!(groups[1].tracks.is_empty());
    assert!(groups[2].tracks.is_empty(), "an unknown id is empty, not an error");
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
