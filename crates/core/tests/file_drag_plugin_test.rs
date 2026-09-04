use crescendo_core::plugins::{
    tooling::pack_plugin_source, PluginFileDragContribution, PluginManifest, PLUGIN_API_VERSION,
};
use crescendo_core::Db;
use std::path::PathBuf;
use std::sync::Arc;

const FILE_DRAG_PLUGIN_ID: &str = "dev.crescendo.file-drag";

#[test]
fn official_file_drag_package_matches_its_validated_source() {
    let source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins/dev.crescendo.file-drag");
    let packaged =
        include_bytes!("../../../plugins/dev.crescendo.file-drag-1.0.0.crescendo-plugin");
    let directory = tempfile::tempdir().unwrap();
    let repacked = directory.path().join("repacked.crescendo-plugin");

    let report = pack_plugin_source(&source, &repacked).unwrap();
    assert_eq!(report.manifest.id, FILE_DRAG_PLUGIN_ID);
    assert_eq!(report.manifest.api_version, PLUGIN_API_VERSION);
    assert_eq!(
        report.manifest.contributes.file_drag,
        Some(PluginFileDragContribution {})
    );
    assert_eq!(std::fs::read(repacked).unwrap(), packaged.as_slice());
}

#[test]
fn official_file_drag_plugin_is_inert_until_enabled_and_restarted() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(Db::open(&directory.path().join("library.db")).unwrap());
    let package = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/dev.crescendo.file-drag-1.0.0.crescendo-plugin");
    let manager = crescendo_core::plugins::PluginManager::start(
        directory.path().join("plugins"),
        database.clone(),
        "0.9.0",
    )
    .unwrap();

    let installed = manager.install(&package).unwrap();
    assert_eq!(installed.snapshot.file_drag_provider, None);
    manager.set_enabled(FILE_DRAG_PLUGIN_ID, true).unwrap();
    assert_eq!(manager.file_drag_provider(), None);
    drop(manager);

    let restarted = crescendo_core::plugins::PluginManager::start(
        directory.path().join("plugins"),
        database,
        "0.9.0",
    )
    .unwrap();
    assert_eq!(restarted.file_drag_provider(), Some(FILE_DRAG_PLUGIN_ID));
}

#[test]
fn official_file_drag_manifest_is_declarative_and_permissionless() {
    let source = include_str!("../../../plugins/dev.crescendo.file-drag/manifest.json");
    let manifest: PluginManifest = serde_json::from_str(source).unwrap();
    manifest.validate(false).unwrap();
    assert!(manifest.permissions.is_empty());
    assert!(manifest.contributes.file_drag.is_some());
}
