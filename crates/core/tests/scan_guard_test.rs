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
    let opts = ScanOptions {
        root: root.to_path_buf(),
        threads: 1,
        full: false,
    };
    scan(db, &opts, &|_| {}, &AtomicBool::new(false))?;
    Ok(())
}

#[test]
fn a_root_that_does_not_exist_fails_the_scan_and_keeps_the_library() {
    let home = tempfile::tempdir().unwrap();
    let (db, id) = db_with_one_track(home.path(), "/m/a.flac");

    let error = run(&db, &home.path().join("not-mounted"))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("music folder not found"),
        "unexpected error: {error}"
    );

    assert!(!db.track_by_id(id).unwrap().unwrap().missing);
}

#[test]
fn an_empty_root_fails_the_scan_and_keeps_the_library() {
    let home = tempfile::tempdir().unwrap();
    let music = home.path().join("music");
    let (db, id) = db_with_one_track(home.path(), music.join("a.flac").to_str().unwrap());
    std::fs::create_dir(&music).unwrap();

    let error = run(&db, &music).unwrap_err().to_string();
    assert!(
        error.contains("holds no audio files"),
        "unexpected error: {error}"
    );

    assert!(!db.track_by_id(id).unwrap().unwrap().missing);
}

#[test]
fn a_reachable_root_still_marks_deleted_files_missing() {
    let home = tempfile::tempdir().unwrap();
    let music = home.path().join("music");
    std::fs::create_dir(&music).unwrap();
    // Tags are unreadable, so the file enters the library under its filename.
    std::fs::write(music.join("kept.flac"), b"not really a flac").unwrap();
    let (db, gone) = db_with_one_track(home.path(), music.join("gone.flac").to_str().unwrap());

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

fn scan_at(db: &Db, root: &Path) -> anyhow::Result<crescendo_core::model::ScanProgress> {
    scan(
        db,
        &ScanOptions {
            root: root.to_owned(),
            threads: 1,
            full: false,
        },
        &|_| {},
        &AtomicBool::new(false),
    )
}
fn fixture() -> (tempfile::TempDir, Db, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("music");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("kept.flac"), b"fixture").unwrap();
    let db = Db::open(&tmp.path().join("library.db")).unwrap();
    (tmp, db, root)
}
#[test]
fn restored_unchanged_file_becomes_visible() {
    let (tmp, db, root) = fixture();
    let track = root.join("restored.flac");
    std::fs::write(&track, b"fixture").unwrap();
    scan_at(&db, &root).unwrap();
    std::fs::rename(&track, tmp.path().join("parked.flac")).unwrap();
    scan_at(&db, &root).unwrap();
    std::fs::rename(tmp.path().join("parked.flac"), &track).unwrap();
    scan_at(&db, &root).unwrap();
    assert_eq!(
        db.library_stats().unwrap().tracks,
        2,
        "restored file must be live again"
    );
}
#[test]
fn imported_file_outside_root_survives_scan() {
    let (tmp, db, root) = fixture();
    scan_at(&db, &root).unwrap();
    // A sibling whose name starts with the root's name is still outside it.
    let external_dir = tmp.path().join("music-downloads");
    std::fs::create_dir(&external_dir).unwrap();
    let external = external_dir.join("download.flac");
    std::fs::write(&external, b"fixture").unwrap();
    let playlist = db.playlist_create("Imported").unwrap();
    crescendo_core::import::add_paths_to_playlist(&db, playlist, &[external]).unwrap();
    scan_at(&db, &root).unwrap();
    assert!(
        !db.playlist_tracks(playlist).unwrap()[0].missing,
        "external file still exists"
    );
}
#[test]
fn unreadable_subdirectory_does_not_hide_its_tracks() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, db, root) = fixture();
    let sub = root.join("album");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("track.flac"), b"fixture").unwrap();
    scan_at(&db, &root).unwrap();
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0)).unwrap();
    let result = scan_at(&db, &root);
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.unwrap_err().to_string().contains("scan incomplete"));
    assert_eq!(db.library_stats().unwrap().tracks, 2);
    // Restoring access should take the unchanged-file path, with no repairs
    // needed because the failed traversal never marked the tracks missing.
    let recovered = scan_at(&db, &root).unwrap();
    assert_eq!(recovered.updated, 0);
    assert_eq!(db.library_stats().unwrap().tracks, 2);
}
