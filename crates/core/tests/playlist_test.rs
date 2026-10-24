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
    db.playlist_tracks(pid)
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect()
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
    let listed =
        |db: &Db| -> Vec<i64> { db.playlist_list().unwrap().iter().map(|p| p.id).collect() };
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
    assert_eq!(
        view,
        vec![5, 2, 4, 3, 1],
        "playlist order, then position inside each"
    );

    let played = db
        .resolve_context_exact(&PlayContext::Playlists { ids: vec![b, a] })
        .unwrap();
    assert_eq!(
        played, view,
        "playback order must match the view row for row"
    );
}

#[test]
fn filtered_playlist_playback_matches_visible_rows() {
    let (_d, db) = test_db();
    db.with(|conn| {
        conn.execute("UPDATE tracks SET title = 'Alpha' WHERE id = 1", [])?;
        conn.execute(
            "UPDATE tracks SET title = 'Écho', missing = 1 WHERE id = 2",
            [],
        )?;
        conn.execute("UPDATE tracks SET title = 'Other' WHERE id IN (3, 5)", [])?;
        conn.execute(
            "UPDATE tracks SET title = 'Fourth', genre = 'Rock' WHERE id = 4",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let a = db.playlist_create("A").unwrap();
    let b = db.playlist_create("B").unwrap();
    db.playlist_add_tracks(a, &[1, 2, 3]).unwrap();
    db.playlist_add_tracks(b, &[4, 2, 5]).unwrap();

    let single = db
        .resolve_context_exact(&PlayContext::FilteredPlaylist {
            id: a,
            query: "éCH".into(),
        })
        .unwrap();
    assert_eq!(
        single,
        vec![2],
        "Unicode matching keeps a visible missing track"
    );

    let multi = db
        .resolve_context_exact(&PlayContext::FilteredPlaylists {
            ids: vec![b, a],
            query: "ÉCHO".into(),
        })
        .unwrap();
    assert_eq!(
        multi,
        vec![2, 2],
        "playlist order is preserved across matches"
    );

    let genre = db
        .resolve_context_exact(&PlayContext::FilteredPlaylist {
            id: b,
            query: "rock".into(),
        })
        .unwrap();
    assert_eq!(genre, vec![4]);

    // Rows show the romaji sort tag for a Japanese name, so it filters too.
    db.with(|conn| {
        conn.execute(
            "UPDATE tracks SET title = '時の過ぎゆくままに', \
             titlesort = 'Toki no Sugiyuku Mama ni' WHERE id = 5",
            [],
        )?;
        conn.execute(
            "UPDATE tracks SET artist = '上海アリス幻樂団', artistsort = 'ZUN' WHERE id = 4",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    for (query, expected) in [
        ("sugiyuku", vec![5]),
        ("zun", vec![4]),
        ("過ぎゆく", vec![5]),
    ] {
        let romaji = db
            .resolve_context_exact(&PlayContext::FilteredPlaylist {
                id: b,
                query: query.into(),
            })
            .unwrap();
        assert_eq!(romaji, expected, "{query}");
    }

    let blank = db
        .resolve_context_exact(&PlayContext::FilteredPlaylist {
            id: a,
            query: "  ".into(),
        })
        .unwrap();
    assert_eq!(
        blank,
        vec![1, 2, 3],
        "a blank filter falls back to the full playlist"
    );
}

/// A playlist keeps tracks whose file has gone missing, and shows them. If
/// playback dropped them, every row after one would play the wrong track.
#[test]
fn missing_tracks_keep_the_rows_and_playback_lined_up() {
    let (_d, db) = test_db();
    let pid = db.playlist_create("P").unwrap();
    db.playlist_add_tracks(pid, &[1, 2, 3, 4]).unwrap();
    db.with(|conn| {
        conn.execute("UPDATE tracks SET missing = 1 WHERE id IN (2, 3)", [])?;
        Ok(())
    })
    .unwrap();

    let view = db.playlist_tracks(pid).unwrap();
    assert_eq!(
        ids(&db, pid),
        vec![1, 2, 3, 4],
        "the view keeps missing tracks"
    );
    assert_eq!(
        view.iter().map(|t| t.missing).collect::<Vec<_>>(),
        vec![false, true, true, false],
        "and flags them, so the UI can mark them"
    );

    let played = db
        .resolve_context_exact(&PlayContext::Playlist { id: pid })
        .unwrap();
    assert_eq!(
        played,
        vec![1, 2, 3, 4],
        "row index N must resolve to row N"
    );
    // The last row is what breaks first when missing tracks are dropped: it
    // sat at index 3 in the view but would have been index 1 in playback.
    assert_eq!(played[3], view[3].id);

    let multi = db
        .resolve_context_exact(&PlayContext::Playlists { ids: vec![pid] })
        .unwrap();
    assert_eq!(multi, played, "multi-selection resolves the same way");
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
    assert!(
        groups[2].tracks.is_empty(),
        "an unknown id is empty, not an error"
    );
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

#[test]
fn reorder_failure_rolls_back_and_leaves_the_connection_usable() {
    let (_d, db) = test_db();
    let pid = db.playlist_create("P").unwrap();
    db.playlist_add_tracks(pid, &[1, 2, 3]).unwrap();
    db.with(|conn| {
        conn.execute_batch(&format!(
            "CREATE TRIGGER fail_second_reorder_insert
             BEFORE INSERT ON playlist_tracks
             WHEN NEW.playlist_id = {pid} AND NEW.position = 1
             BEGIN
               SELECT RAISE(ABORT, 'forced reorder failure');
             END;"
        ))?;
        Ok(())
    })
    .unwrap();

    let error = db.playlist_reorder(pid, &[0], 2).unwrap_err();
    assert!(error.to_string().contains("forced reorder failure"));
    assert_eq!(
        ids(&db, pid),
        vec![1, 2, 3],
        "the delete and partial rebuild roll back"
    );
    db.with(|conn| {
        assert!(
            conn.is_autocommit(),
            "the failed operation must not leave BEGIN active"
        );
        conn.execute_batch("DROP TRIGGER fail_second_reorder_insert")?;
        Ok(())
    })
    .unwrap();

    db.playlist_add_tracks(pid, &[4]).unwrap();
    assert_eq!(ids(&db, pid), vec![1, 2, 3, 4]);
}

#[test]
fn a_checked_add_that_fails_validation_inserts_nothing() {
    let (_d, db) = test_db();
    let pid = db.playlist_create("P").unwrap();
    db.playlist_add_tracks_checked(pid, &[1, 2]).unwrap();

    // One unknown id rejects the whole batch, and the valid ids in front of it
    // must not survive: validation and insertion share one transaction.
    assert!(db.playlist_add_tracks_checked(pid, &[3, 4_242, 4]).is_err());
    assert_eq!(ids(&db, pid), vec![1, 2]);

    assert!(db.playlist_add_tracks_checked(9_999, &[1]).is_err());
    assert_eq!(ids(&db, pid), vec![1, 2]);
}
