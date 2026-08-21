//! An unreachable music folder must never empty the library.
//!
//! The library lives on a network share, so a scan can start while the share
//! is not mounted. Without the guards in `scanner::scan` the walk would find
//! nothing and mark every track `missing = 1`.

use crescendo_core::db::Db;
use crescendo_core::scanner::{scan, ScanOptions};
use std::path::Path;
use std::sync::atomic::AtomicBool;

fn db_with_one_track(dir: &Path, path: &str) -> (Db, i64) {
    let db = Db::open(&dir.join("t.db")).unwrap();
    let id = db
        .with(|conn| {
            conn.execute(
                "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms)
                 VALUES (?1, '/m', 'a.flac', 0, 0, 'FLAC', 180000)",
                [path],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .unwrap();
    (db, id)
}

fn run(db: &Db, root: &Path) -> anyhow::Result<()> {
    let opts = ScanOptions { root: root.to_path_buf(), threads: 1, full: false };
    scan(db, &opts, &|_| {}, &AtomicBool::new(false))?;
    Ok(())
}

#[test]
fn a_root_that_does_not_exist_fails_the_scan_and_keeps_the_library() {
    let home = tempfile::tempdir().unwrap();
    let (db, id) = db_with_one_track(home.path(), "/m/a.flac");

    let error = run(&db, &home.path().join("not-mounted")).unwrap_err().to_string();
    assert!(error.contains("music folder not found"), "unexpected error: {error}");

    assert!(!db.track_by_id(id).unwrap().unwrap().missing);
}

#[test]
fn an_empty_root_fails_the_scan_and_keeps_the_library() {
    let home = tempfile::tempdir().unwrap();
    let (db, id) = db_with_one_track(home.path(), "/m/a.flac");
    let music = home.path().join("music");
    std::fs::create_dir(&music).unwrap();

    let error = run(&db, &music).unwrap_err().to_string();
    assert!(error.contains("holds no audio files"), "unexpected error: {error}");

    assert!(!db.track_by_id(id).unwrap().unwrap().missing);
}

#[test]
fn a_reachable_root_still_marks_deleted_files_missing() {
    let home = tempfile::tempdir().unwrap();
    let music = home.path().join("music");
    std::fs::create_dir(&music).unwrap();
    // Tags are unreadable, so the file enters the library under its filename.
    std::fs::write(music.join("kept.flac"), b"not really a flac").unwrap();
    let (db, gone) = db_with_one_track(home.path(), "/m/gone.flac");

    run(&db, &music).unwrap();

    assert!(db.track_by_id(gone).unwrap().unwrap().missing);
}

#[test]
fn an_empty_root_is_allowed_when_the_library_is_empty_too() {
    let home = tempfile::tempdir().unwrap();
    let db = Db::open(&home.path().join("t.db")).unwrap();
    let music = home.path().join("music");
    std::fs::create_dir(&music).unwrap();

    run(&db, &music).unwrap();
}
