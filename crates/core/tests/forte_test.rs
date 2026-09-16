use crescendo_core::{
    forte::{refresh_files, FileChange},
    import::add_paths_to_playlist,
    Db,
};
use lofty::{
    config::WriteOptions,
    prelude::{Accessor, TagExt},
    tag::{Tag, TagType},
};
use std::path::Path;

fn wave(path: &Path, title: &str) {
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend(236_u32.to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16_u32.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(44100_u32.to_le_bytes());
    bytes.extend(88200_u32.to_le_bytes());
    bytes.extend(2_u16.to_le_bytes());
    bytes.extend(16_u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(200_u32.to_le_bytes());
    bytes.extend([0_u8; 200]);
    std::fs::write(path, bytes).unwrap();
    let mut tag = Tag::new(TagType::Id3v2);
    tag.set_title(title.into());
    tag.set_album("Album".into());
    tag.set_artist("Artist".into());
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

#[test]
fn editing_and_renaming_preserve_playlists_history_and_track_identity() {
    let temp = tempfile::tempdir().unwrap();
    let db = Db::open(&temp.path().join("library.db")).unwrap();
    let old = temp.path().join("old.wav");
    let new = temp.path().join("new.wav");
    wave(&old, "Old title");
    let playlist = db.playlist_create("Keep this playlist").unwrap();
    add_paths_to_playlist(&db, playlist, &[old.clone()]).unwrap();
    let id = db.playlist_tracks(playlist).unwrap()[0].id;
    db.record_play(id).unwrap();
    std::fs::rename(&old, &new).unwrap();
    wave(&new, "New title");
    refresh_files(
        &db,
        &[FileChange {
            old_path: old.to_string_lossy().into(),
            new_path: new.to_string_lossy().into(),
        }],
    )
    .unwrap();
    let track = db.track_by_id(id).unwrap().unwrap();
    assert_eq!(track.title.as_deref(), Some("New title"));
    assert_eq!(track.path, new.to_string_lossy());
    assert_eq!(track.play_count, 1);
    assert!(!track.missing);
    assert_eq!(db.playlist_tracks(playlist).unwrap()[0].id, id);
    db.with(|c| {
        assert_eq!(
            c.query_row("SELECT count(*) FROM tracks", [], |r| r.get::<_, i64>(0))?,
            1
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn destination_conflicts_roll_back_database_updates() {
    let temp = tempfile::tempdir().unwrap();
    let db = Db::open(&temp.path().join("library.db")).unwrap();
    let a = temp.path().join("a.wav");
    let b = temp.path().join("b.wav");
    wave(&a, "A");
    wave(&b, "B");
    let playlist = db.playlist_create("Test").unwrap();
    add_paths_to_playlist(&db, playlist, &[a.clone(), b.clone()]).unwrap();
    assert!(refresh_files(
        &db,
        &[FileChange {
            old_path: a.to_string_lossy().into(),
            new_path: b.to_string_lossy().into()
        }]
    )
    .is_err());
    let tracks = db.playlist_tracks(playlist).unwrap();
    assert_eq!(tracks[0].title.as_deref(), Some("A"));
    assert_eq!(tracks[0].path, a.to_string_lossy());
    assert_eq!(tracks[1].title.as_deref(), Some("B"));
}

#[test]
fn independent_editor_notifications_follow_offline_rename_chains() {
    let temp = tempfile::tempdir().unwrap();
    let db = Db::open(&temp.path().join("library.db")).unwrap();
    let old = temp.path().join("old.wav");
    let middle = temp.path().join("middle.wav");
    let final_path = temp.path().join("final.wav");
    wave(&old, "Old title");
    let playlist = db.playlist_create("Test").unwrap();
    add_paths_to_playlist(&db, playlist, &[old.clone()]).unwrap();
    let id = db.playlist_tracks(playlist).unwrap()[0].id;
    std::fs::rename(&old, &final_path).unwrap();
    wave(&final_path, "Edited offline");
    let inbox = temp.path().join("forte-inbox");
    std::fs::create_dir(&inbox).unwrap();
    for (name, from, to) in [
        ("01.json", &old, &middle),
        ("02.json", &middle, &final_path),
    ] {
        std::fs::write(
            inbox.join(name),
            serde_json::json!({"changes": [{"old_path": from, "new_path": to}]}).to_string(),
        )
        .unwrap();
    }
    assert!(crescendo_core::forte::refresh_pending(&db, &inbox).unwrap());
    let track = db.track_by_id(id).unwrap().unwrap();
    assert_eq!(track.path, final_path.to_string_lossy());
    assert_eq!(track.title.as_deref(), Some("Edited offline"));
    assert_eq!(std::fs::read_dir(inbox).unwrap().count(), 0);
}
