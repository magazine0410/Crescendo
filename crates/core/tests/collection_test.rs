//! Collections: grouping playlists under a name in the sidebar.

use crescendo_core::db::Db;

fn test_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        let mut ins = conn.prepare(
            "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, duration_ms)
             VALUES (?1, ?2, ?2, ?2, 0, 0, 'FLAC', 1000)",
        )?;
        for id in 1..=5i64 {
            ins.execute(rusqlite::params![id, format!("/m/{id}.flac")])?;
        }
        Ok(())
    })
    .unwrap();
    (dir, db)
}

#[test]
fn set_creates_the_collection_and_takes_its_members() {
    let (_d, db) = test_db();
    let a = db.playlist_create("Zelda").unwrap();
    let b = db.playlist_create("Mario").unwrap();
    let loose = db.playlist_create("Jazz").unwrap();

    let id = db.collection_set(&[a, b], "Video games").unwrap().unwrap();
    let cols = db.collection_list().unwrap();
    assert_eq!(cols.len(), 1);
    assert_eq!(cols[0].name, "Video games");
    assert_eq!(cols[0].playlist_count, 2);

    let list = db.playlist_list().unwrap();
    let of = |id: i64| list.iter().find(|p| p.id == id).unwrap().collection_id;
    assert_eq!(of(a), Some(id));
    assert_eq!(of(b), Some(id));
    assert_eq!(of(loose), None, "a playlist outside the group is untouched");
}

/// The name is matched case-insensitively, so "Video Games" joins the group
/// "Video games" instead of starting a near-duplicate beside it.
#[test]
fn set_reuses_a_collection_of_the_same_name() {
    let (_d, db) = test_db();
    let a = db.playlist_create("A").unwrap();
    let b = db.playlist_create("B").unwrap();

    let first = db.collection_set(&[a], "Video games").unwrap().unwrap();
    let second = db.collection_set(&[b], "  video GAMES  ").unwrap().unwrap();
    assert_eq!(first, second);
    assert_eq!(db.collection_list().unwrap().len(), 1);
}

#[test]
fn set_reuses_a_collection_with_unicode_case_variants() {
    let (_d, db) = test_db();
    let a = db.playlist_create("A").unwrap();
    let b = db.playlist_create("B").unwrap();

    let first = db.collection_set(&[a], "Écho").unwrap().unwrap();
    let second = db.collection_set(&[b], "éCHO").unwrap().unwrap();
    assert_eq!(first, second);
    assert_eq!(db.collection_list().unwrap().len(), 1);
}

/// Counts are a plain sum of the members: a track in two member playlists
/// counts twice.
#[test]
fn counts_sum_the_members_including_duplicates() {
    let (_d, db) = test_db();
    let a = db.playlist_create("A").unwrap();
    let b = db.playlist_create("B").unwrap();
    db.playlist_add_tracks(a, &[1, 2, 3]).unwrap();
    db.playlist_add_tracks(b, &[3, 4]).unwrap(); // 3 is in both
    db.collection_set(&[a, b], "Group").unwrap();

    let c = &db.collection_list().unwrap()[0];
    assert_eq!(c.track_count, 5, "3 + 2, not 4 distinct");
    assert_eq!(c.total_duration_ms, 5000);
}

#[test]
fn a_blank_name_takes_playlists_out_and_drops_the_empty_group() {
    let (_d, db) = test_db();
    let a = db.playlist_create("A").unwrap();
    let b = db.playlist_create("B").unwrap();
    db.collection_set(&[a, b], "Group").unwrap();

    assert!(db.collection_set(&[a], "").unwrap().is_none());
    assert_eq!(
        db.collection_list().unwrap()[0].playlist_count,
        1,
        "b is still in"
    );

    // Taking the last member out leaves nothing to group, so the row goes.
    db.collection_set(&[b], "").unwrap();
    assert!(db.collection_list().unwrap().is_empty());
}

#[test]
fn moving_the_last_member_elsewhere_drops_the_old_group() {
    let (_d, db) = test_db();
    let a = db.playlist_create("A").unwrap();
    db.collection_set(&[a], "First").unwrap();
    db.collection_set(&[a], "Second").unwrap();

    let cols = db.collection_list().unwrap();
    assert_eq!(cols.len(), 1);
    assert_eq!(cols[0].name, "Second");
}

#[test]
fn deleting_a_collection_keeps_its_playlists() {
    let (_d, db) = test_db();
    let a = db.playlist_create("A").unwrap();
    db.playlist_add_tracks(a, &[1, 2]).unwrap();
    let id = db.collection_set(&[a], "Group").unwrap().unwrap();

    db.collection_delete(id).unwrap();
    assert!(db.collection_list().unwrap().is_empty());
    let list = db.playlist_list().unwrap();
    assert_eq!(list.len(), 1, "the playlist survives its group");
    assert_eq!(list[0].collection_id, None);
    assert_eq!(list[0].track_count, 2, "and keeps its tracks");
}

#[test]
fn rename_and_collapse_persist() {
    let (_d, db) = test_db();
    let a = db.playlist_create("A").unwrap();
    let id = db.collection_set(&[a], "Group").unwrap().unwrap();

    db.collection_rename(id, "  Renamed  ").unwrap();
    db.collection_set_collapsed(id, true).unwrap();
    let c = &db.collection_list().unwrap()[0];
    assert_eq!(c.name, "Renamed");
    assert!(c.collapsed);

    db.collection_set_collapsed(id, false).unwrap();
    assert!(!db.collection_list().unwrap()[0].collapsed);
}

#[test]
fn rename_rejects_a_unicode_case_insensitive_duplicate() {
    let (_d, db) = test_db();
    let a = db.playlist_create("A").unwrap();
    let b = db.playlist_create("B").unwrap();
    let first = db.collection_set(&[a], "Écho").unwrap().unwrap();
    let second = db.collection_set(&[b], "Other").unwrap().unwrap();

    let error = db.collection_rename(second, "éCHO").unwrap_err();
    assert!(error.to_string().contains("already exists"));
    let collections = db.collection_list().unwrap();
    assert_eq!(collections.len(), 2);
    assert_eq!(
        collections.iter().find(|c| c.id == first).unwrap().name,
        "Écho"
    );
    assert_eq!(
        collections.iter().find(|c| c.id == second).unwrap().name,
        "Other"
    );
}

/// A database written before collections existed must gain the table and the
/// column when it is opened, not fail.
#[test]
fn an_older_database_migrates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    {
        // A v3-era database: playlists without collection_id, no collections
        // table. The tables the sidebar queries touch have to be there too —
        // migrate() skips its CREATE block once user_version is >= 1.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE playlists (
               id INTEGER PRIMARY KEY, name TEXT NOT NULL, position INTEGER NOT NULL);
             CREATE TABLE playlist_tracks (
               playlist_id INTEGER NOT NULL, position INTEGER NOT NULL,
               track_id INTEGER NOT NULL, PRIMARY KEY (playlist_id, position));
             CREATE TABLE tracks (
               id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, duration_ms INTEGER NOT NULL,
               albumartist TEXT, year INTEGER, album TEXT, disc_no INTEGER,
               track_no INTEGER, missing INTEGER NOT NULL DEFAULT 0);
             INSERT INTO playlists(id, name, position) VALUES (1, 'Old', 0);
             PRAGMA user_version = 3;",
        )
        .unwrap();
    }

    let db = Db::open(&path).unwrap();
    let list = db.playlist_list().unwrap();
    assert_eq!(list.len(), 1, "the old playlist survives the migration");
    assert_eq!(list[0].collection_id, None);
    assert!(db.collection_list().unwrap().is_empty());
    db.collection_set(&[1], "New group").unwrap();
    assert_eq!(db.collection_list().unwrap()[0].playlist_count, 1);
}

#[test]
fn collection_failure_rolls_back_and_leaves_the_connection_usable() {
    let (_d, db) = test_db();
    let a = db.playlist_create("A").unwrap();
    let b = db.playlist_create("B").unwrap();
    db.with(|conn| {
        conn.execute_batch(&format!(
            "CREATE TRIGGER fail_second_collection_update
             BEFORE UPDATE OF collection_id ON playlists
             WHEN NEW.id = {b}
             BEGIN
               SELECT RAISE(ABORT, 'forced collection failure');
             END;"
        ))?;
        Ok(())
    })
    .unwrap();

    let error = db.collection_set(&[a, b], "Group").unwrap_err();
    assert!(error.to_string().contains("forced collection failure"));
    assert!(
        db.collection_list().unwrap().is_empty(),
        "the new collection rolls back"
    );
    assert!(db
        .playlist_list()
        .unwrap()
        .iter()
        .all(|p| p.collection_id.is_none()));
    db.with(|conn| {
        assert!(
            conn.is_autocommit(),
            "the failed operation must not leave BEGIN active"
        );
        conn.execute_batch("DROP TRIGGER fail_second_collection_update")?;
        Ok(())
    })
    .unwrap();

    db.collection_set(&[a, b], "Group").unwrap();
    assert_eq!(db.collection_list().unwrap()[0].playlist_count, 2);
}
