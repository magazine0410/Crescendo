//! Exporting playlists through a plugin's playlist exporter (runtime API
//! 0.2.2), against the real Wasm runtime and the demo component, whose
//! `export_titles` command writes one line per chunk it is handed.

use crescendo_core::db::Db;
use crescendo_core::plugins::export::{
    export_playlist, export_playlists_to_folder, file_stem_for, with_extension, CHUNK_BYTES,
};
use crescendo_core::plugins::{
    PluginCommandDeclaration, PluginContributions, PluginKind, PluginManager, PluginManifest,
    PluginPlaylistExporter, WASM_API_VERSION,
};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const MODULE: &[u8] = include_bytes!("fixtures/wasm-demo-module.wasm");
const PLUGIN_ID: &str = "dev.crescendo.wasm-demo";

fn manifest(permissions: &[&str]) -> PluginManifest {
    PluginManifest {
        schema_version: 1,
        id: PLUGIN_ID.into(),
        name: "Wasm Demo".into(),
        version: "1.0.0".into(),
        description: "Export test plugin".into(),
        authors: vec!["Crescendo".into()],
        license: "MIT".into(),
        homepage: None,
        kind: PluginKind::Wasm,
        api_version: WASM_API_VERSION.into(),
        min_crescendo_version: "0.8.0".into(),
        permissions: permissions.iter().map(|p| (*p).to_string()).collect(),
        network_domains: vec![],
        contributes: PluginContributions {
            commands: vec![PluginCommandDeclaration {
                id: "export_titles".into(),
            }],
            playlist_exporters: vec![PluginPlaylistExporter {
                id: "titles".into(),
                label: "Demo chunk list".into(),
                extension: "txt".into(),
                command: "export_titles".into(),
            }],
            ..PluginContributions::default()
        },
    }
}

const EXPORTER_PERMISSIONS: [&str; 3] = ["playlists.read", "library.read", "library.paths"];

fn write_package(path: &Path, manifest: &PluginManifest) {
    let mut writer = ZipWriter::new(File::create(path).unwrap());
    let options = SimpleFileOptions::default();
    writer.start_file("manifest.json", options).unwrap();
    writer
        .write_all(&serde_json::to_vec(manifest).unwrap())
        .unwrap();
    writer.start_file("module.wasm", options).unwrap();
    writer.write_all(MODULE).unwrap();
    writer.finish().unwrap();
}

/// A library of `tracks` songs with long paths, a playlist "Mix/Live" holding
/// all of them, and the demo exporter installed, approved and loaded.
fn setup(tracks: i64) -> (TempDir, Arc<Db>, PluginManager, i64) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(&dir.path().join("library.db")).unwrap());
    let long = "x".repeat(400);
    db.with(|connection| {
        for id in 1..=tracks {
            connection.execute(
                "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, duration_ms, title, missing)
                 VALUES (?1, ?2, '/m', ?3, 0, 0, 'FLAC', 1000, ?4, ?5)",
                rusqlite::params![
                    id,
                    format!("/m/{long}/{id}.flac"),
                    format!("{id}.flac"),
                    format!("Title {id}"),
                    (id == 1) as i64
                ],
            )?;
        }
        Ok(())
    })
    .unwrap();
    let playlist = db.playlist_create("Mix/Live").unwrap();
    let ids: Vec<i64> = (1..=tracks).collect();
    db.playlist_add_tracks(playlist, &ids).unwrap();

    let plugins = dir.path().join("plugins");
    let manager = PluginManager::start(plugins.clone(), db.clone(), "0.8.0").unwrap();
    let package = dir.path().join("demo.crescendo-plugin");
    write_package(&package, &manifest(&EXPORTER_PERMISSIONS));
    manager.install(&package).unwrap();
    manager.approve_grants(PLUGIN_ID).unwrap();
    manager.set_enabled(PLUGIN_ID, true).unwrap();
    drop(manager);
    let manager = PluginManager::start(plugins, db.clone(), "0.8.0").unwrap();
    // The runtime instantiates the component on its own thread.
    let deadline = Instant::now() + Duration::from_secs(10);
    while manager
        .invoke(PLUGIN_ID, "export_titles", &serde_json::json!({}))
        .is_err()
    {
        assert!(
            Instant::now() < deadline,
            "the demo plugin never became ready"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    (dir, db, manager, playlist)
}

#[test]
fn a_loaded_exporter_appears_in_the_snapshot() {
    let (_dir, _db, manager, _) = setup(1);
    let exporters = manager.snapshot().unwrap().playlist_exporters;
    assert_eq!(exporters.len(), 1);
    assert_eq!(exporters[0].plugin_id, PLUGIN_ID);
    assert_eq!(exporters[0].extension, "txt");
    assert!(manager.playlist_exporter(PLUGIN_ID, "titles").is_some());
    assert!(manager.playlist_exporter(PLUGIN_ID, "other").is_none());
}

#[test]
fn a_long_playlist_is_exported_in_chunks_written_in_order() {
    // About 600 bytes of JSON per track: well over one chunk in all.
    let count = 2 * CHUNK_BYTES as i64 / 500;
    let (dir, _db, manager, playlist) = setup(count);
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();

    let result = export_playlist(
        &manager,
        &_db,
        PLUGIN_ID,
        "titles",
        playlist,
        &out.join("mix"),
    )
    .unwrap();
    let file = out.join("mix.txt");
    assert_eq!(result.files, [file.to_string_lossy().to_string()]);
    assert_eq!((result.tracks, result.missing), (count as usize, 1));

    let text = std::fs::read_to_string(&file).unwrap();
    let mut lines = text.lines();
    assert_eq!(
        lines.next().unwrap(),
        format!("# demo export into {}", out.display()),
        "the plugin is told which folder the file goes to"
    );
    let mut expected_offset = 0;
    let mut chunks = 0;
    for line in lines {
        let parts: Vec<&str> = line.split(' ').collect();
        assert_eq!(parts[0], "chunk");
        assert_eq!(parts[1].parse::<i64>().unwrap(), expected_offset);
        expected_offset += parts[2].parse::<i64>().unwrap();
        chunks += 1;
    }
    assert_eq!(
        expected_offset, count,
        "every track handed over once, in order"
    );
    assert!(chunks >= 2, "the playlist was split into chunks");
    assert!(
        !std::fs::read_dir(&out).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with("crescendo-tmp")),
        "no temporary file is left behind"
    );
}

#[test]
fn a_folder_export_never_overwrites_and_an_empty_playlist_still_gets_its_header() {
    let (dir, db, manager, playlist) = setup(2);
    let empty = db.playlist_create("Empty").unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    std::fs::write(out.join("Mix_Live.txt"), "keep me").unwrap();

    let result =
        export_playlists_to_folder(&manager, &db, PLUGIN_ID, "titles", &[playlist, empty], &out)
            .unwrap();
    assert_eq!(result.files.len(), 2);
    assert_eq!(
        std::fs::read_to_string(out.join("Mix_Live.txt")).unwrap(),
        "keep me"
    );
    assert!(out.join("Mix_Live (2).txt").is_file());
    let empty_text = std::fs::read_to_string(out.join("Empty.txt")).unwrap();
    assert_eq!(
        empty_text,
        format!("# demo export into {}\nchunk 0 0\n", out.display())
    );
    assert!(export_playlists_to_folder(
        &manager,
        &db,
        PLUGIN_ID,
        "titles",
        &[playlist],
        &out.join("no")
    )
    .is_err());
}

#[test]
fn an_unknown_exporter_is_refused_before_any_file_is_made() {
    let (dir, db, manager, playlist) = setup(1);
    let file = dir.path().join("nope.txt");
    assert!(export_playlist(&manager, &db, PLUGIN_ID, "other", playlist, &file).is_err());
    assert!(export_playlist(
        &manager,
        &db,
        "dev.example.absent",
        "titles",
        playlist,
        &file
    )
    .is_err());
    assert!(!file.exists());
}

#[test]
fn an_exporter_must_request_the_data_it_is_handed() {
    for missing in EXPORTER_PERMISSIONS {
        let permissions: Vec<&str> = EXPORTER_PERMISSIONS
            .into_iter()
            .filter(|p| *p != missing)
            .collect();
        let error = manifest(&permissions)
            .validate(true)
            .unwrap_err()
            .to_string();
        assert!(error.contains(missing), "{error}");
    }
    let mut old = manifest(&EXPORTER_PERMISSIONS);
    old.api_version = "0.2.1".into();
    assert!(
        old.validate(true).is_err(),
        "exporters need runtime API 0.2.2"
    );
}

#[test]
fn file_names_and_extensions() {
    assert_eq!(file_stem_for("  .hidden  "), "hidden");
    assert_eq!(file_stem_for("a/b\\c\td"), "a_b_c_d");
    assert_eq!(file_stem_for(""), "Playlist");
    assert_eq!(
        with_extension(Path::new("/x/Mix"), "m3u"),
        Path::new("/x/Mix.m3u")
    );
    assert_eq!(
        with_extension(Path::new("/x/Mix.M3U"), "m3u"),
        Path::new("/x/Mix.M3U")
    );
    assert_eq!(
        with_extension(Path::new("/x/Mix.txt"), "m3u"),
        Path::new("/x/Mix.txt.m3u")
    );
}
