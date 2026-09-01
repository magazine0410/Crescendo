use crescendo_core::plugins::{
    PluginFacetTransform, PluginManifest, PluginMenuSlot, GROUPING_PLUGIN_ID,
    GROUPING_PLUGIN_SETTING,
};
use crescendo_core::{plugins::tooling::pack_plugin_source, Db};
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

#[test]
fn bundled_grouping_package_matches_the_canonical_validated_manifest() {
    let source = include_str!("../../../plugins/dev.crescendo.primary-grouping/manifest.json");
    let bytes = include_bytes!(
        "../../../src-tauri/resources/plugins/dev.crescendo.primary-grouping.crescendo-plugin"
    );
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(archive.len(), 1);
    let mut packaged = String::new();
    archive
        .by_name("manifest.json")
        .unwrap()
        .read_to_string(&mut packaged)
        .unwrap();
    assert_eq!(packaged, source);

    let manifest: PluginManifest = serde_json::from_str(source).unwrap();
    manifest.validate(false).unwrap();
    assert_eq!(manifest.id, GROUPING_PLUGIN_ID);
    assert!(manifest
        .contributes
        .settings
        .iter()
        .any(|setting| { setting.key() == GROUPING_PLUGIN_SETTING }));
    assert!(matches!(
        &manifest.contributes.facet_transforms[0],
        PluginFacetTransform::TextFirstSegment {
            target_tag,
            delimiter,
            trim_whitespace: true,
            enabled_setting,
        } if target_tag == "grouping"
            && delimiter == ";"
            && enabled_setting == GROUPING_PLUGIN_SETTING
    ));
    assert_eq!(
        manifest.contributes.menu_controls[0].slot,
        PluginMenuSlot::LibraryFilter
    );
}

/// The bundled migration copy and the release asset are the same plugin at the
/// same version, and the manager shows a package hash for each. `pack` is
/// deterministic precisely so those two hashes agree, so the checked-in bundle
/// must be exactly what the packer produces from the canonical source. Repack
/// it with:
///
/// `cargo run -p crescendo-core --bin crescendo-plugin -- pack plugins/dev.crescendo.primary-grouping <path>`
#[test]
fn bundled_grouping_package_is_byte_identical_to_the_packer_output() {
    let bundled = include_bytes!(
        "../../../src-tauri/resources/plugins/dev.crescendo.primary-grouping.crescendo-plugin"
    );
    let directory = tempfile::tempdir().unwrap();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/dev.crescendo.primary-grouping");
    let repacked = directory.path().join("repacked.crescendo-plugin");
    pack_plugin_source(&source, &repacked).unwrap();
    assert_eq!(
        std::fs::read(&repacked).unwrap(),
        bundled.as_slice(),
        "the bundled package has drifted from the canonical source; repack it"
    );
}

#[test]
fn release_packer_output_installs_as_a_disabled_official_plugin() {
    let directory = tempfile::tempdir().unwrap();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/dev.crescendo.primary-grouping");
    let package = directory
        .path()
        .join("dev.crescendo.primary-grouping-1.0.0.crescendo-plugin");
    let report = pack_plugin_source(&source, &package).unwrap();
    assert_eq!(report.manifest.id, GROUPING_PLUGIN_ID);
    assert_eq!(report.sha256.len(), 64);

    let database = Arc::new(Db::open(&directory.path().join("library.db")).unwrap());
    let manager = crescendo_core::plugins::PluginManager::start(
        directory.path().join("plugins"),
        database,
        "0.8.0",
    )
    .unwrap();
    let installed = manager.install(&package).unwrap();
    let official = installed
        .snapshot
        .plugins
        .iter()
        .find(|plugin| plugin.id == GROUPING_PLUGIN_ID)
        .unwrap();
    assert!(!official.desired_enabled);
    assert!(!official.loaded);
    assert!(!installed.snapshot.restart_required);
}
