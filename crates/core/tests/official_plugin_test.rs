use crescendo_core::plugins::{
    PluginFacetTransform, PluginManifest, PluginMenuSlot, GROUPING_PLUGIN_ID,
    GROUPING_PLUGIN_SETTING,
};
use std::io::Read;

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
