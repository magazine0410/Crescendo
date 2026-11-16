//! Importing PLS and XSPF playlists, alongside M3U.

use crescendo_core::db::Db;
use crescendo_core::m3u::import;
use std::path::Path;

/// Tracks under `root`: "A/01 Song.flac", "東方/幽雅に咲かせ.flac" and
/// "B/Rock & Roll.flac". Returns their ids in that order.
fn library(db: &Db, root: &Path) -> Vec<i64> {
    let files = ["A/01 Song.flac", "東方/幽雅に咲かせ.flac", "B/Rock & Roll.flac"];
    db.with(|conn| {
        let mut ids = Vec::new();
        for file in files {
            let path = root.join(file);
            conn.execute(
                "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms)
                 VALUES (?1, ?2, ?3, 0, 0, 'FLAC', 1000)",
                rusqlite::params![
                    path.to_string_lossy(),
                    path.parent().unwrap().to_string_lossy(),
                    path.file_name().unwrap().to_string_lossy()
                ],
            )?;
            ids.push(conn.last_insert_rowid());
        }
        Ok(ids)
    })
    .unwrap()
}

fn imported_ids(db: &Db, playlist: i64) -> Vec<i64> {
    db.playlist_tracks(playlist)
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect()
}

#[test]
fn a_pls_playlist_imports_in_its_numbered_order() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    let ids = library(&db, dir.path());
    let file = dir.path().join("Road Trip.pls");
    std::fs::write(
        &file,
        format!(
            "[playlist]\n\
             NumberOfEntries=5\n\
             File3=file://{root}/B/Rock%20%26%20Roll.flac\n\
             Title3=Rock & Roll\n\
             File1=A/01 Song.flac\n\
             Length1=245\n\
             file2={root}/東方/幽雅に咲かせ.flac\n\
             File4=http://radio.example/stream\n\
             File5=gone.flac\n\
             Version=2\n",
            root = dir.path().display()
        ),
    )
    .unwrap();
    let result = import(&db, &file).unwrap();
    assert_eq!(result.name, "Road Trip");
    assert_eq!(result.added, 3);
    assert_eq!(imported_ids(&db, result.playlist_id), ids);
    assert_eq!(result.missing.len(), 2, "{:?}", result.missing);
    assert_eq!(result.missing[0], "http://radio.example/stream", "reported as written");
    assert!(result.missing[1].ends_with("gone.flac"));
}

#[test]
fn an_xspf_playlist_imports_its_locations_and_title() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    let ids = library(&db, dir.path());
    let file = dir.path().join("export.xspf");
    let root = url::Url::from_directory_path(dir.path()).unwrap();
    // Percent-encoded by the url crate, as another player would write it.
    let japanese = root.join("東方/幽雅に咲かせ.flac").unwrap();
    assert!(japanese.as_str().contains("%E6%9D%B1"));
    std::fs::write(
        &file,
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- written by another player -->
<playlist version="1" xmlns="http://xspf.org/ns/0/">
  <title>Evening &amp; Night</title>
  <trackList>
    <track>
      <title>Rock &#38; Roll</title>
      <location>B/Rock%20&amp;%20Roll.flac</location>
    </track>
    <track>
      <location><![CDATA[{root}A/01%20Song.flac]]></location>
      <location>file:///ignored/second/location.flac</location>
    </track>
    <track><location>{japanese}</location></track>
    <track><location>https://music.example/song.mp3</location></track>
    <track><title>Only a title</title></track>
  </trackList>
</playlist>"#
        ),
    )
    .unwrap();
    let result = import(&db, &file).unwrap();
    assert_eq!(result.name, "Evening & Night");
    assert_eq!(result.added, 3);
    let got = imported_ids(&db, result.playlist_id);
    assert_eq!(got, [ids[2], ids[0], ids[1]], "the file's order");
    assert_eq!(
        result.missing,
        ["https://music.example/song.mp3", "Only a title"]
    );
}

#[test]
fn m3u_still_imports_and_now_reads_file_uris() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    let ids = library(&db, dir.path());
    let file = dir.path().join("mix.m3u8");
    let root = url::Url::from_directory_path(dir.path()).unwrap();
    std::fs::write(
        &file,
        format!("#EXTM3U\n#EXTINF:1,x\nA/01 Song.flac\n{root}B/Rock%20%26%20Roll.flac\n"),
    )
    .unwrap();
    let result = import(&db, &file).unwrap();
    assert_eq!(imported_ids(&db, result.playlist_id), [ids[0], ids[2]]);
    assert!(result.missing.is_empty());
}

#[test]
fn broken_or_other_files_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    let broken = dir.path().join("broken.xspf");
    std::fs::write(&broken, "<playlist><trackList><track></playlist>").unwrap();
    assert!(import(&db, &broken).is_err());
    let other = dir.path().join("list.txt");
    std::fs::write(&other, "A/01 Song.flac\n").unwrap();
    let error = import(&db, &other).unwrap_err().to_string();
    assert!(error.contains(".pls and .xspf"), "{error}");
}
