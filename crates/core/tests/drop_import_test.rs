//! Dropping files and folders from the file manager onto a playlist.
//!
//! The fixture files carry no real audio, so tag reading fails and every
//! import lands on the scanner's filename-only fallback — which is exactly
//! the path that must still produce a playable row.

use crescendo_core::db::Db;
use crescendo_core::import::add_paths_to_playlist;
use std::path::{Path, PathBuf};

fn touch(path: &Path) -> PathBuf {
    std::fs::write(path, b"not really audio").unwrap();
    path.to_path_buf()
}

/// An album folder of 3 "tracks", plus a stray non-audio file that must be
/// ignored, and one loose file outside the folder.
fn fixture() -> (tempfile::TempDir, Db, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    let album = dir.path().join("Album");
    std::fs::create_dir(&album).unwrap();
    for name in ["01 - a.flac", "02 - b.flac", "03 - c.mp3"] {
        touch(&album.join(name));
    }
    touch(&album.join("cover.jpg"));
    let loose = touch(&dir.path().join("loose.opus"));
    (dir, db, album, loose)
}

fn track_paths(db: &Db, playlist: i64) -> Vec<String> {
    db.playlist_tracks(playlist)
        .unwrap()
        .into_iter()
        .map(|t| t.path)
        .collect()
}

#[test]
fn folder_drop_adds_every_audio_file() {
    let (_tmp, db, album, _loose) = fixture();
    let pl = db.playlist_create("Test").unwrap();

    let r = add_paths_to_playlist(&db, pl, &[album.clone()]).unwrap();
    assert_eq!(r.added, 3, "3 audio files in the folder");
    assert_eq!(r.imported, 3, "none of them were in the library");
    assert_eq!(r.duplicates, 0);
    assert!(r.failed.is_empty());

    let paths = track_paths(&db, pl);
    assert_eq!(paths.len(), 3);
    assert!(
        paths.iter().all(|p| !p.ends_with("cover.jpg")),
        "non-audio must be ignored"
    );
}

#[test]
fn dropping_the_same_folder_twice_adds_nothing() {
    let (_tmp, db, album, _loose) = fixture();
    let pl = db.playlist_create("Test").unwrap();
    add_paths_to_playlist(&db, pl, &[album.clone()]).unwrap();

    let r = add_paths_to_playlist(&db, pl, &[album]).unwrap();
    assert_eq!(r.added, 0);
    assert_eq!(r.duplicates, 3, "all three are already in the playlist");
    assert_eq!(r.imported, 0, "already in the library on the second drop");
    assert_eq!(track_paths(&db, pl).len(), 3);
}

#[test]
fn partial_overlap_adds_only_the_new_tracks() {
    let (_tmp, db, album, loose) = fixture();
    let pl = db.playlist_create("Test").unwrap();
    // One track of the album is already in the playlist.
    add_paths_to_playlist(&db, pl, &[album.join("02 - b.flac")]).unwrap();

    let r = add_paths_to_playlist(&db, pl, &[album, loose]).unwrap();
    assert_eq!(r.added, 3, "2 remaining album tracks + the loose file");
    assert_eq!(r.duplicates, 1);
    assert_eq!(track_paths(&db, pl).len(), 4);
}

#[test]
fn a_file_listed_twice_in_one_drop_is_added_once() {
    let (_tmp, db, album, _loose) = fixture();
    let pl = db.playlist_create("Test").unwrap();
    let file = album.join("01 - a.flac");

    // The folder and one of its own files dropped together.
    let r = add_paths_to_playlist(&db, pl, &[album, file]).unwrap();
    assert_eq!(r.added, 3);
    assert_eq!(
        r.duplicates, 0,
        "the repeat is dropped during expansion, not counted"
    );
    assert_eq!(track_paths(&db, pl).len(), 3);
}

#[test]
fn known_tracks_are_reused_instead_of_reimported() {
    let (_tmp, db, album, _loose) = fixture();
    let first = db.playlist_create("First").unwrap();
    let second = db.playlist_create("Second").unwrap();
    add_paths_to_playlist(&db, first, &[album.clone()]).unwrap();

    let r = add_paths_to_playlist(&db, second, &[album]).unwrap();
    assert_eq!(r.added, 3);
    assert_eq!(
        r.imported, 0,
        "the files entered the library on the first drop"
    );

    let total: i64 = db
        .with(|c| Ok(c.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(total, 3, "no duplicate track rows");
}

#[test]
fn folder_drop_follows_album_order_not_filename_order() {
    let (_tmp, db, album, _loose) = fixture();
    let pl = db.playlist_create("Test").unwrap();
    add_paths_to_playlist(&db, pl, &[album.clone()]).unwrap();

    // Tag the imported rows so disc/track numbers disagree with the names.
    db.with(|conn| {
        conn.execute(
            "UPDATE tracks SET disc_no = 1, track_no = 3 WHERE filename = '01 - a.flac'",
            [],
        )?;
        conn.execute(
            "UPDATE tracks SET disc_no = 1, track_no = 1 WHERE filename = '02 - b.flac'",
            [],
        )?;
        conn.execute(
            "UPDATE tracks SET disc_no = 2, track_no = 1 WHERE filename = '03 - c.mp3'",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let fresh = db.playlist_create("Fresh").unwrap();
    add_paths_to_playlist(&db, fresh, &[album]).unwrap();
    let names: Vec<String> = track_paths(&db, fresh)
        .iter()
        .map(|p| {
            Path::new(p)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    assert_eq!(names, vec!["02 - b.flac", "01 - a.flac", "03 - c.mp3"]);
}

/// A parent folder holding two albums, each numbered from track 1, one of
/// them split over two disc subfolders. Sorting the whole tree by disc/track
/// would merge the albums into one interleaved run.
#[test]
fn parent_folder_keeps_each_album_together() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Db::open(&tmp.path().join("t.db")).unwrap();
    let parent = tmp.path().join("Soundtracks");
    let solo = parent.join("Summer");
    let disc1 = parent.join("[Rip] Air").join("Disc 1");
    let disc2 = parent.join("[Rip] Air").join("Disc 2");
    for d in [&solo, &disc1, &disc2] {
        std::fs::create_dir_all(d).unwrap();
    }
    for i in 1..=3 {
        touch(&solo.join(format!("{i}.flac")));
        touch(&disc1.join(format!("{i}.flac")));
        touch(&disc2.join(format!("{i}.flac")));
    }

    // Get the rows in, then tag them the way a real rip is tagged.
    let seed = db.playlist_create("Seed").unwrap();
    add_paths_to_playlist(&db, seed, &[parent.clone()]).unwrap();
    db.with(|conn| {
        conn.execute(
            "UPDATE tracks SET album = 'Summer', albumartist = 'VA', disc_no = 1, \
             track_no = CAST(replace(filename, '.flac', '') AS INTEGER) \
             WHERE dir LIKE '%Summer'",
            [],
        )?;
        for (disc, like) in [(1, "%Disc 1"), (2, "%Disc 2")] {
            conn.execute(
                "UPDATE tracks SET album = 'Air', albumartist = 'VA', disc_no = ?1, \
                 track_no = CAST(replace(filename, '.flac', '') AS INTEGER) \
                 WHERE dir LIKE ?2",
                rusqlite::params![disc, like],
            )?;
        }
        Ok(())
    })
    .unwrap();

    let pl = db.playlist_create("Test").unwrap();
    add_paths_to_playlist(&db, pl, &[parent]).unwrap();
    let order: Vec<(String, i64, i64)> = db
        .playlist_tracks(pl)
        .unwrap()
        .into_iter()
        .map(|t| (t.album.unwrap(), t.disc_no.unwrap(), t.track_no.unwrap()))
        .collect();

    // "Summer" sorts before "[Rip] Air" on disk, so it comes first; each
    // album stays in one run, and Air's two discs stay in disc order.
    assert_eq!(
        order,
        vec![
            ("Summer".into(), 1, 1),
            ("Summer".into(), 1, 2),
            ("Summer".into(), 1, 3),
            ("Air".into(), 1, 1),
            ("Air".into(), 1, 2),
            ("Air".into(), 1, 3),
            ("Air".into(), 2, 1),
            ("Air".into(), 2, 2),
            ("Air".into(), 2, 3),
        ]
    );
}

#[test]
fn dropping_only_non_audio_changes_nothing() {
    let (_tmp, db, album, _loose) = fixture();
    let pl = db.playlist_create("Test").unwrap();

    let r = add_paths_to_playlist(&db, pl, &[album.join("cover.jpg")]).unwrap();
    assert_eq!(r.added, 0);
    assert_eq!(r.imported, 0);
    assert!(
        r.failed.is_empty(),
        "a file that is not audio is ignored, not a failure"
    );
    assert!(track_paths(&db, pl).is_empty());
}

#[test]
fn an_audio_path_that_is_gone_is_reported() {
    let (tmp, db, _album, _loose) = fixture();
    let pl = db.playlist_create("Test").unwrap();

    let r = add_paths_to_playlist(&db, pl, &[tmp.path().join("gone.flac")]).unwrap();
    assert_eq!(r.added, 0);
    assert_eq!(r.failed.len(), 1, "the drop must not fail silently");
    assert!(track_paths(&db, pl).is_empty());
}

#[test]
fn unknown_playlist_is_an_error() {
    let (_tmp, db, album, _loose) = fixture();
    assert!(add_paths_to_playlist(&db, 999, &[album]).is_err());
}
