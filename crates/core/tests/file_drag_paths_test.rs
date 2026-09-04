use crescendo_core::db::Db;
use crescendo_core::model::{BrowseMode, PlayContext};

fn test_db(track_count: i64) -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        let tx = conn.transaction()?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, duration_ms,
                                    albumartist, artist, album, album_id, grouping, track_no)
                 VALUES (?1, ?2, '/music', ?3, 0, 0, 'FLAC', 1000,
                         'Artist', 'Artist', 'Album', 1, 'Typo Group', ?1)",
            )?;
            for id in 1..=track_count {
                let filename = format!("{id:04}.flac");
                insert.execute(rusqlite::params![
                    id,
                    format!("/music/{filename}"),
                    filename
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    })
    .unwrap();
    (dir, db)
}

#[test]
fn library_and_value_context_paths_are_ordered_and_bounded() {
    let (_dir, db) = test_db(2_005);
    let library = PlayContext::LibraryView {
        query: None,
        projection_revision: 0,
        sort: None,
    };
    let paths = db.resolve_context_paths_exact(&library, 2_001).unwrap();
    assert_eq!(paths.len(), 2_001);
    assert_eq!(paths.first().unwrap(), "/music/0001.flac");
    assert_eq!(paths.last().unwrap(), "/music/2001.flac");

    let value = PlayContext::ValueTracks {
        mode: BrowseMode::Custom("grouping".into()),
        values: vec!["Typo Group".into()],
        query: Some("artist=Artist".into()),
        projection_revision: 0,
        sort: None,
    };
    let paths = db.resolve_context_paths_exact(&value, 3).unwrap();
    assert_eq!(
        paths,
        ["/music/0001.flac", "/music/0002.flac", "/music/0003.flac"]
    );
}

#[test]
fn explicit_ids_keep_selection_order_and_omit_missing_files() {
    let (_dir, db) = test_db(5);
    db.with(|conn| {
        conn.execute("UPDATE tracks SET missing = 1 WHERE id = 3", [])?;
        Ok(())
    })
    .unwrap();

    let paths = db.track_paths_by_ids(&[5, 3, 2, 1], 3).unwrap();
    assert_eq!(paths, ["/music/0005.flac", "/music/0002.flac"]);
}

#[test]
fn playlist_paths_share_one_limit_and_skip_missing_members() {
    let (_dir, db) = test_db(5);
    let first = db.playlist_create("First").unwrap();
    let second = db.playlist_create("Second").unwrap();
    db.playlist_add_tracks(first, &[4, 2, 1]).unwrap();
    db.playlist_add_tracks(second, &[5, 3]).unwrap();
    db.with(|conn| {
        conn.execute("UPDATE tracks SET missing = 1 WHERE id = 2", [])?;
        Ok(())
    })
    .unwrap();

    let context = PlayContext::Playlists {
        ids: vec![first, second],
    };
    let paths = db.resolve_context_paths_exact(&context, 3).unwrap();
    assert_eq!(
        paths,
        ["/music/0004.flac", "/music/0001.flac", "/music/0005.flac"]
    );
}
