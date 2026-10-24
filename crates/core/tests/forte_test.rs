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
    assert!(
        crescendo_core::forte::refresh_pending(&db, &inbox)
            .unwrap()
            .changed
    );
    let track = db.track_by_id(id).unwrap().unwrap();
    assert_eq!(track.path, final_path.to_string_lossy());
    assert_eq!(track.title.as_deref(), Some("Edited offline"));
    assert_eq!(std::fs::read_dir(inbox).unwrap().count(), 0);
}

fn notice(inbox: &Path, name: &str, from: &Path, to: &Path) {
    std::fs::write(
        inbox.join(name),
        serde_json::json!({"changes": [{"old_path": from, "new_path": to}]}).to_string(),
    )
    .unwrap();
}

fn pending_notices(inbox: &Path) -> usize {
    std::fs::read_dir(inbox)
        .unwrap()
        .filter(|e| e.as_ref().unwrap().path().extension().is_some())
        .count()
}

/// A change that can never apply used to fail every retry and hold back
/// every later Forte save. It is now set aside and the rest still land.
#[test]
fn a_change_that_cannot_apply_does_not_block_the_inbox() {
    let temp = tempfile::tempdir().unwrap();
    let db = Db::open(&temp.path().join("library.db")).unwrap();
    let gone = temp.path().join("gone.wav");
    let taken = temp.path().join("taken.wav");
    let other = temp.path().join("other.wav");
    let kept = temp.path().join("kept.wav");
    let renamed = temp.path().join("renamed.wav");
    for (path, title) in [
        (&gone, "Gone"),
        (&taken, "Taken"),
        (&other, "Other"),
        (&kept, "Kept"),
    ] {
        wave(path, title);
    }
    let playlist = db.playlist_create("Test").unwrap();
    add_paths_to_playlist(
        &db,
        playlist,
        &[gone.clone(), taken.clone(), other.clone(), kept.clone()],
    )
    .unwrap();
    let kept_id = db.playlist_tracks(playlist).unwrap()[3].id;

    let inbox = temp.path().join("forte-inbox");
    std::fs::create_dir(&inbox).unwrap();
    // The edited file was deleted before Crescendo saw the notice.
    std::fs::remove_file(&gone).unwrap();
    notice(&inbox, "01.json", &gone, &gone);
    // A scan already gave the new name to another track.
    notice(&inbox, "02.json", &taken, &other);
    // A good save after both.
    std::fs::rename(&kept, &renamed).unwrap();
    wave(&renamed, "Renamed");
    notice(&inbox, "03.json", &kept, &renamed);

    let refresh = crescendo_core::forte::refresh_pending(&db, &inbox).unwrap();
    assert!(refresh.changed);
    assert_eq!(refresh.set_aside.len(), 2, "{:?}", refresh.set_aside);
    let track = db.track_by_id(kept_id).unwrap().unwrap();
    assert_eq!(track.path, renamed.to_string_lossy());
    assert_eq!(track.title.as_deref(), Some("Renamed"));
    assert_eq!(pending_notices(&inbox), 0);
    assert_eq!(std::fs::read_dir(inbox.join("failed")).unwrap().count(), 1);

    // Nothing is retried.
    let again = crescendo_core::forte::refresh_pending(&db, &inbox).unwrap();
    assert!(!again.changed && again.set_aside.is_empty());
}

#[test]
fn an_unreadable_notice_waits_then_is_set_aside() {
    let temp = tempfile::tempdir().unwrap();
    let db = Db::open(&temp.path().join("library.db")).unwrap();
    let old = temp.path().join("old.wav");
    let new = temp.path().join("new.wav");
    wave(&old, "Old");
    let playlist = db.playlist_create("Test").unwrap();
    add_paths_to_playlist(&db, playlist, &[old.clone()]).unwrap();
    let id = db.playlist_tracks(playlist).unwrap()[0].id;
    std::fs::rename(&old, &new).unwrap();

    let inbox = temp.path().join("forte-inbox");
    std::fs::create_dir(&inbox).unwrap();
    std::fs::write(inbox.join("01.json"), "{\"changes\": [").unwrap();
    notice(&inbox, "02.json", &old, &new);

    // Fresh: it may still be being written, so nothing after it is applied.
    let refresh = crescendo_core::forte::refresh_pending(&db, &inbox).unwrap();
    assert!(!refresh.changed);
    assert_eq!(pending_notices(&inbox), 2);

    let stale = std::time::SystemTime::now() - std::time::Duration::from_secs(120);
    std::fs::File::options()
        .write(true)
        .open(inbox.join("01.json"))
        .unwrap()
        .set_modified(stale)
        .unwrap();
    let refresh = crescendo_core::forte::refresh_pending(&db, &inbox).unwrap();
    assert!(refresh.changed);
    assert_eq!(refresh.set_aside.len(), 1);
    assert_eq!(
        db.track_by_id(id).unwrap().unwrap().path,
        new.to_string_lossy()
    );
    assert!(inbox.join("failed").join("01.json").exists());
    assert_eq!(pending_notices(&inbox), 0);
}
