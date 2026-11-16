use crescendo_core::db::Db;
use crescendo_core::m3u::{
    export_playlist, export_playlists_to_folder, file_stem_for, import, with_m3u_extension,
};
use std::path::Path;

/// (path, title, artist, duration_ms, missing)
type Row = (String, Option<&'static str>, Option<&'static str>, i64, bool);

/// A library whose "music folder" is `root`, plus one track outside it.
/// Returns the track ids in insertion order.
fn library(db: &Db, root: &Path) -> Vec<i64> {
    let rows: [Row; 4] = [
        (format!("{}/A/01 Song.flac", root.display()), Some("Song"), Some("Band"), 245_400, false),
        (format!("{}/東方/幽雅に咲かせ.flac", root.display()), Some("幽雅に咲かせ"), Some("ZUN"), 180_000, false),
        ("/elsewhere/untitled.mp3".into(), None, None, 0, false),
        (format!("{}/B/gone.flac", root.display()), Some("Gone"), Some("Line\nBreak"), 61_000, true),
    ];
    db.with(|conn| {
        let mut ids = Vec::new();
        for (path, title, artist, duration, missing) in &rows {
            let p = Path::new(path);
            conn.execute(
                "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms,
                                    title, artist, missing)
                 VALUES (?1, ?2, ?3, 0, 0, 'FLAC', ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    path,
                    p.parent().unwrap().to_string_lossy(),
                    p.file_name().unwrap().to_string_lossy(),
                    duration,
                    title,
                    artist,
                    *missing as i64
                ],
            )?;
            ids.push(conn.last_insert_rowid());
        }
        Ok(ids)
    })
    .unwrap()
}

#[test]
fn a_playlist_exports_as_extended_m3u_and_imports_back_the_same() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Music");
    std::fs::create_dir(&root).unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    let ids = library(&db, &root);
    let playlist = db.playlist_create("Mix").unwrap();
    db.playlist_add_tracks(playlist, &ids).unwrap();

    // No extension given: .m3u is added.
    let result = export_playlist(&db, playlist, &root.join("Mix")).unwrap();
    let file = root.join("Mix.m3u");
    assert_eq!(result.files, [file.to_string_lossy().to_string()]);
    assert_eq!((result.tracks, result.missing), (4, 1));

    let text = std::fs::read_to_string(&file).unwrap();
    assert_eq!(
        text,
        "#EXTM3U\n\
         #EXTINF:245,Band - Song\n\
         A/01 Song.flac\n\
         #EXTINF:180,ZUN - 幽雅に咲かせ\n\
         東方/幽雅に咲かせ.flac\n\
         #EXTINF:-1,untitled\n\
         /elsewhere/untitled.mp3\n\
         #EXTINF:61,Line Break - Gone\n\
         B/gone.flac\n"
    );
    assert!(
        !std::fs::read_dir(&root).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with("crescendo-tmp")),
        "no temporary file is left behind"
    );

    let back = import(&db, &file).unwrap();
    assert_eq!(back.added, 4);
    assert!(back.missing.is_empty());
    let reimported: Vec<i64> = db
        .playlist_tracks(back.playlist_id)
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(reimported, ids, "same songs, same order");
}

#[test]
fn exporting_again_to_the_same_file_replaces_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    let ids = library(&db, dir.path());
    let playlist = db.playlist_create("Mix").unwrap();
    db.playlist_add_tracks(playlist, &ids[..1]).unwrap();
    let file = dir.path().join("mix.m3u");
    std::fs::write(&file, "old contents that are longer than the new ones").unwrap();
    export_playlist(&db, playlist, &file).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "#EXTM3U\n#EXTINF:245,Band - Song\nA/01 Song.flac\n"
    );
}

#[test]
fn a_folder_export_names_files_after_playlists_and_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    let ids = library(&db, dir.path());
    let chill = db.playlist_create("Chill").unwrap();
    let odd = db.playlist_create("../AC/DC").unwrap();
    db.playlist_add_tracks(chill, &ids[..2]).unwrap();
    db.playlist_add_tracks(odd, &ids[2..3]).unwrap();
    std::fs::write(out.join("Chill.m3u"), "keep me").unwrap();

    let result = export_playlists_to_folder(&db, &[chill, odd], &out).unwrap();
    assert_eq!(result.tracks, 3);
    let mut names: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    assert_eq!(names, ["Chill (2).m3u", "Chill.m3u", "_AC_DC.m3u"], "no way out of the folder");
    assert_eq!(std::fs::read_to_string(out.join("Chill.m3u")).unwrap(), "keep me");
    assert!(export_playlists_to_folder(&db, &[chill], &out.join("nope")).is_err());
}

#[test]
fn file_names_and_extensions() {
    assert_eq!(file_stem_for("  .hidden  "), "hidden");
    assert_eq!(file_stem_for("a/b\\c\td"), "a_b_c_d");
    assert_eq!(file_stem_for(" / "), "_");
    assert_eq!(file_stem_for(""), "Playlist");
    assert_eq!(with_m3u_extension(Path::new("/x/Mix")), Path::new("/x/Mix.m3u"));
    assert_eq!(with_m3u_extension(Path::new("/x/Mix.M3U")), Path::new("/x/Mix.M3U"));
    assert_eq!(with_m3u_extension(Path::new("/x/Mix.txt")), Path::new("/x/Mix.txt.m3u"));
}
