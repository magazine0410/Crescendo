use super::manifest::PluginManifest;
use super::package::{
    create_operation_dir, normalize_archive_path, read_manifest, stage_archive, sync_directory,
    validate_declarative_assets, validate_package_layout, MAX_ENTRIES, MAX_EXTRACTED_BYTES,
    MAX_MANIFEST_BYTES,
};
use anyhow::{anyhow, bail, Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

pub const MANIFEST_V1_SCHEMA: &str = include_str!("../../../../plugin-sdk/manifest-v1.schema.json");

#[derive(Debug, Clone)]
pub struct PluginSourceReport {
    pub manifest: PluginManifest,
    pub entry_count: usize,
    pub extracted_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct PluginPackageReport {
    pub manifest: PluginManifest,
    pub sha256: String,
    pub entry_count: usize,
    pub extracted_bytes: u64,
    pub archive_bytes: u64,
}

#[derive(Debug)]
struct SourceEntry {
    archive_name: String,
    path: PathBuf,
    size: u64,
}

/// Validate an unpacked plugin source directory with the same manifest,
/// package-layout, size and declarative-content rules as the runtime loader.
pub fn validate_plugin_source(source: &Path) -> Result<PluginSourceReport> {
    validate_shipped_schema()?;
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("could not inspect {}", source.display()))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!("plugin source must be a regular directory");
    }

    let mut entries = Vec::new();
    let mut extracted_bytes = 0u64;
    for entry in WalkDir::new(source).follow_links(false) {
        let entry = entry?;
        if entry.path() == source {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink()
            || (!metadata.file_type().is_file() && !metadata.file_type().is_dir())
        {
            bail!("plugin source cannot contain links or special files");
        }
        let relative = entry.path().strip_prefix(source)?;
        if is_source_only_file(relative, metadata.is_dir()) {
            continue;
        }
        let raw_name = relative
            .to_str()
            .ok_or_else(|| anyhow!("plugin source paths must be valid UTF-8"))?
            .replace(std::path::MAIN_SEPARATOR, "/");
        let (normalized, _) = normalize_archive_path(&raw_name)?;
        if normalized != relative {
            bail!("plugin source contains a path that does not normalize exactly");
        }
        validate_package_layout(relative, metadata.is_dir())?;
        if !metadata.is_file() {
            continue;
        }
        if entries.len() >= MAX_ENTRIES {
            bail!("plugin packages cannot contain more than {MAX_ENTRIES} files");
        }
        if relative == Path::new("manifest.json") && metadata.len() > MAX_MANIFEST_BYTES {
            bail!("manifest.json exceeds 256 KiB");
        }
        extracted_bytes = extracted_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| anyhow!("plugin source is too large"))?;
        if extracted_bytes > MAX_EXTRACTED_BYTES {
            bail!("plugin source exceeds the 64 MiB extracted-size limit");
        }
        entries.push(SourceEntry {
            archive_name: raw_name,
            path: entry.path().to_path_buf(),
            size: metadata.len(),
        });
    }
    if entries.is_empty() {
        bail!("plugin source is empty");
    }

    entries.sort_by(|left, right| {
        let left_key = (left.archive_name != "manifest.json", &left.archive_name);
        let right_key = (right.archive_name != "manifest.json", &right.archive_name);
        left_key.cmp(&right_key)
    });
    let manifest_path = source.join("manifest.json");
    validate_manifest_against_shipped_schema(&manifest_path)?;
    let manifest = read_manifest(&manifest_path)?;
    let has_module = source.join("module.wasm").is_file();
    manifest.validate(has_module)?;
    validate_declarative_assets(source, &manifest)?;

    Ok(PluginSourceReport {
        manifest,
        entry_count: entries.len(),
        extracted_bytes,
    })
}

/// Validate a finished package through the runtime staging path, including
/// the immutable-copy, ZIP, containment, manifest and content-policy checks.
pub fn validate_plugin_package(package: &Path) -> Result<PluginPackageReport> {
    validate_shipped_schema()?;
    let validation_base = std::env::temp_dir().join("crescendo-plugin-validation");
    fs::create_dir_all(&validation_base)?;
    let (_, operation_dir) = create_operation_dir(&validation_base)?;
    let result = (|| {
        let staged = stage_archive(package, &operation_dir)?;
        validate_manifest_against_shipped_schema(&staged.content_dir.join("manifest.json"))?;
        let mut entry_count = 0usize;
        let mut extracted_bytes = 0u64;
        for entry in WalkDir::new(&staged.content_dir).follow_links(false) {
            let entry = entry?;
            if !entry.file_type().is_file()
                || entry.file_name().to_str() == Some(".crescendo-install.json")
            {
                continue;
            }
            entry_count += 1;
            extracted_bytes = extracted_bytes
                .checked_add(entry.metadata()?.len())
                .ok_or_else(|| anyhow!("plugin package is too large"))?;
        }
        Ok(PluginPackageReport {
            manifest: staged.manifest,
            sha256: staged.package_hash,
            entry_count,
            extracted_bytes,
            archive_bytes: fs::metadata(package)?.len(),
        })
    })();
    let _ = fs::remove_dir_all(&operation_dir);
    result
}

/// Create a reproducible `.crescendo-plugin` archive. Paths are sorted,
/// timestamps and Unix modes are fixed, directory entries are omitted, and
/// the completed archive is accepted only if the runtime validator accepts it.
pub fn pack_plugin_source(source: &Path, output: &Path) -> Result<PluginPackageReport> {
    let source_report = validate_plugin_source(source)?;
    if output.extension().and_then(|value| value.to_str()) != Some("crescendo-plugin") {
        bail!("plugin package output must end in .crescendo-plugin");
    }
    if output.try_exists()? {
        bail!(
            "refusing to overwrite existing package {}",
            output.display()
        );
    }
    let output_parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let source_root = fs::canonicalize(source)?;
    let output_parent = fs::canonicalize(output_parent).with_context(|| {
        format!(
            "plugin package output directory {} does not exist",
            output_parent.display()
        )
    })?;
    let output_name = output
        .file_name()
        .ok_or_else(|| anyhow!("plugin package output needs a file name"))?;
    let final_output = output_parent.join(output_name);
    if final_output.starts_with(&source_root) {
        bail!("plugin package output cannot be inside its source directory");
    }

    let (_, operation_dir) = create_operation_dir(&output_parent)?;
    let temporary = operation_dir.join("package.crescendo-plugin");
    let result = (|| {
        let mut entries = collect_source_entries(source)?;
        entries.sort_by(|left, right| {
            let left_key = (left.archive_name != "manifest.json", &left.archive_name);
            let right_key = (right.archive_name != "manifest.json", &right.archive_name);
            left_key.cmp(&right_key)
        });

        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let mut archive = ZipWriter::new(file);
        let timestamp = DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0)
            .map_err(|_| anyhow!("could not construct the deterministic ZIP timestamp"))?;
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(timestamp)
            .unix_permissions(0o100644);
        for entry in entries {
            archive.start_file(&entry.archive_name, options)?;
            let file = File::open(&entry.path)?;
            let mut reader = BufReader::new(file).take(entry.size + 1);
            let written = std::io::copy(&mut reader, &mut archive)?;
            if written != entry.size {
                bail!("plugin source changed while it was being packed");
            }
        }
        let output_file = archive.finish()?;
        output_file.sync_all()?;

        let report = validate_plugin_package(&temporary)?;
        if report.manifest != source_report.manifest {
            bail!("packed manifest does not match the validated source manifest");
        }
        fs::hard_link(&temporary, &final_output)
            .with_context(|| format!("could not create {}", final_output.display()))?;
        sync_directory(&output_parent)?;
        Ok(report)
    })();
    let _ = fs::remove_dir_all(&operation_dir);
    result
}

fn collect_source_entries(source: &Path) -> Result<Vec<SourceEntry>> {
    let mut entries = Vec::new();
    for entry in WalkDir::new(source).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(source)?;
        if is_source_only_file(relative, false) {
            continue;
        }
        let archive_name = relative
            .to_str()
            .ok_or_else(|| anyhow!("plugin source paths must be valid UTF-8"))?
            .replace(std::path::MAIN_SEPARATOR, "/");
        entries.push(SourceEntry {
            archive_name,
            path: entry.path().to_path_buf(),
            size: entry.metadata()?.len(),
        });
    }
    Ok(entries)
}

fn is_source_only_file(path: &Path, is_dir: bool) -> bool {
    !is_dir
        && path.components().count() == 1
        && matches!(
            path.file_name().and_then(|value| value.to_str()),
            Some("README.md" | "LICENSE" | "LICENSE.txt")
        )
}

fn validate_shipped_schema() -> Result<()> {
    let schema: serde_json::Value = serde_json::from_str(MANIFEST_V1_SCHEMA)
        .context("the shipped manifest schema is invalid")?;
    if schema.get("$schema").and_then(serde_json::Value::as_str)
        != Some("https://json-schema.org/draft/2020-12/schema")
        || schema
            .get("properties")
            .and_then(|properties| properties.get("schema_version"))
            .and_then(|version| version.get("const"))
            .and_then(serde_json::Value::as_u64)
            != Some(1)
    {
        bail!("the shipped manifest schema is not the schema-version-1 contract");
    }
    Ok(())
}

fn validate_manifest_against_shipped_schema(path: &Path) -> Result<()> {
    let instance: serde_json::Value =
        serde_json::from_slice(&fs::read(path)?).context("manifest.json is not valid JSON")?;
    let schema: serde_json::Value = serde_json::from_str(MANIFEST_V1_SCHEMA)
        .context("the shipped manifest schema is invalid")?;
    validate_schema_value(&instance, &schema, &schema, "manifest")
        .context("manifest.json does not match the shipped version-1 schema")
}

/// Minimal Draft 2020-12 evaluator for the keywords used by Crescendo's
/// shipped schema. The evaluator is deliberately closed: an unsupported
/// keyword reached while validating a manifest is an error, not a silent
/// omission.
fn validate_schema_value(
    instance: &serde_json::Value,
    schema: &serde_json::Value,
    root: &serde_json::Value,
    path: &str,
) -> Result<()> {
    let schema = schema
        .as_object()
        .ok_or_else(|| anyhow!("schema at {path} is not an object"))?;
    const SUPPORTED: &[&str] = &[
        "$schema",
        "$id",
        "$ref",
        "$defs",
        "title",
        "description",
        "type",
        "const",
        "enum",
        "properties",
        "required",
        "additionalProperties",
        "items",
        "oneOf",
        "minItems",
        "maxItems",
        "minLength",
        "maxLength",
        "pattern",
        "minimum",
        "maximum",
        "exclusiveMinimum",
    ];
    for keyword in schema.keys() {
        if !SUPPORTED.contains(&keyword.as_str()) {
            bail!("unsupported shipped-schema keyword {keyword:?} at {path}");
        }
    }

    if let Some(reference) = schema.get("$ref").and_then(serde_json::Value::as_str) {
        let pointer = reference
            .strip_prefix('#')
            .ok_or_else(|| anyhow!("only local schema references are supported"))?;
        let target = root
            .pointer(pointer)
            .ok_or_else(|| anyhow!("unresolved schema reference {reference:?}"))?;
        return validate_schema_value(instance, target, root, path);
    }
    if let Some(expected) = schema.get("const") {
        if instance != expected {
            bail!("{path} must equal {expected}");
        }
    }
    if let Some(options) = schema.get("enum").and_then(serde_json::Value::as_array) {
        if !options.contains(instance) {
            bail!("{path} is not one of the supported values");
        }
    }
    if let Some(branches) = schema.get("oneOf").and_then(serde_json::Value::as_array) {
        let matches = branches
            .iter()
            .filter(|branch| validate_schema_value(instance, branch, root, path).is_ok())
            .count();
        if matches != 1 {
            bail!("{path} must match exactly one supported declaration shape");
        }
    }

    if let Some(expected_type) = schema.get("type").and_then(serde_json::Value::as_str) {
        let matches = match expected_type {
            "object" => instance.is_object(),
            "array" => instance.is_array(),
            "string" => instance.is_string(),
            "boolean" => instance.is_boolean(),
            "number" => instance.is_number(),
            "integer" => instance.as_i64().is_some() || instance.as_u64().is_some(),
            other => bail!("unsupported schema type {other:?} at {path}"),
        };
        if !matches {
            bail!("{path} must be a {expected_type}");
        }
    }

    if let Some(object) = instance.as_object() {
        let properties = schema
            .get("properties")
            .and_then(serde_json::Value::as_object);
        if let Some(required) = schema.get("required").and_then(serde_json::Value::as_array) {
            for key in required.iter().filter_map(serde_json::Value::as_str) {
                if !object.contains_key(key) {
                    bail!("{path}.{key} is required");
                }
            }
        }
        if schema.get("additionalProperties") == Some(&serde_json::Value::Bool(false)) {
            for key in object.keys() {
                if !properties.is_some_and(|properties| properties.contains_key(key)) {
                    bail!("{path}.{key} is not a supported field");
                }
            }
        }
        if let Some(properties) = properties {
            for (key, child_schema) in properties {
                if let Some(child) = object.get(key) {
                    validate_schema_value(child, child_schema, root, &format!("{path}.{key}"))?;
                }
            }
        }
    }

    if let Some(array) = instance.as_array() {
        if let Some(minimum) = schema.get("minItems").and_then(serde_json::Value::as_u64) {
            if array.len() < minimum as usize {
                bail!("{path} contains too few items");
            }
        }
        if let Some(maximum) = schema.get("maxItems").and_then(serde_json::Value::as_u64) {
            if array.len() > maximum as usize {
                bail!("{path} contains too many items");
            }
        }
        if let Some(item_schema) = schema.get("items") {
            for (index, item) in array.iter().enumerate() {
                validate_schema_value(item, item_schema, root, &format!("{path}[{index}]"))?;
            }
        }
    }

    if let Some(value) = instance.as_str() {
        let length = value.chars().count();
        if schema
            .get("minLength")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|minimum| length < minimum as usize)
        {
            bail!("{path} is too short");
        }
        if schema
            .get("maxLength")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|maximum| length > maximum as usize)
        {
            bail!("{path} is too long");
        }
        if let Some(pattern) = schema.get("pattern").and_then(serde_json::Value::as_str) {
            let expression = regex::Regex::new(pattern)
                .with_context(|| format!("invalid shipped-schema pattern at {path}"))?;
            if !expression.is_match(value) {
                bail!("{path} does not match the required pattern");
            }
        }
    }

    if let Some(value) = instance.as_f64() {
        if schema
            .get("minimum")
            .and_then(serde_json::Value::as_f64)
            .is_some_and(|minimum| value < minimum)
        {
            bail!("{path} is below its minimum");
        }
        if schema
            .get("maximum")
            .and_then(serde_json::Value::as_f64)
            .is_some_and(|maximum| value > maximum)
        {
            bail!("{path} is above its maximum");
        }
        if schema
            .get("exclusiveMinimum")
            .and_then(serde_json::Value::as_f64)
            .is_some_and(|minimum| value <= minimum)
        {
            bail!("{path} must be greater than its minimum");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_source(root: &Path) {
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::write(
            root.join("manifest.json"),
            include_bytes!("../../../../plugins/dev.crescendo.primary-grouping/manifest.json"),
        )
        .unwrap();
        fs::write(root.join("assets/readme.txt"), b"passive asset\n").unwrap();
        fs::write(root.join("README.md"), b"author documentation\n").unwrap();
        fs::write(root.join("LICENSE"), b"test license\n").unwrap();
    }

    #[test]
    fn packs_reproducibly_and_validates_with_the_runtime_path() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        write_source(&source);
        let first = dir.path().join("first.crescendo-plugin");
        let second = dir.path().join("second.crescendo-plugin");

        let first_report = pack_plugin_source(&source, &first).unwrap();
        let second_report = pack_plugin_source(&source, &second).unwrap();
        assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
        assert_eq!(first_report.sha256, second_report.sha256);
        assert_eq!(first_report.entry_count, 2);
        assert_eq!(
            validate_plugin_package(&first).unwrap().manifest.id,
            "dev.crescendo.primary-grouping"
        );
    }

    #[test]
    fn source_and_archive_validation_reject_active_declarative_assets() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        write_source(&source);
        fs::write(source.join("assets/panel.html"), b"<script></script>").unwrap();
        assert!(validate_plugin_source(&source).is_err());

        let archive = dir.path().join("active.crescendo-plugin");
        let file = File::create(&archive).unwrap();
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        writer.start_file("manifest.json", options).unwrap();
        writer
            .write_all(&fs::read(source.join("manifest.json")).unwrap())
            .unwrap();
        writer.start_file("assets/panel.js", options).unwrap();
        writer.write_all(b"alert(1)").unwrap();
        writer.finish().unwrap();
        assert!(validate_plugin_package(&archive).is_err());
    }

    #[test]
    fn shipped_schema_is_valid_json_and_identifies_version_one() {
        validate_shipped_schema().unwrap();
        let schema: serde_json::Value = serde_json::from_str(MANIFEST_V1_SCHEMA).unwrap();
        assert_eq!(schema["properties"]["schema_version"]["const"], 1);

        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("manifest.json");
        fs::write(
            &manifest,
            include_bytes!("../../../../plugins/dev.crescendo.primary-grouping/manifest.json"),
        )
        .unwrap();
        validate_manifest_against_shipped_schema(&manifest).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        value["contributes"]["settings"][0]["default"] = serde_json::json!("yes");
        fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(validate_manifest_against_shipped_schema(&manifest).is_err());

        value["contributes"]["settings"][0]["default"] = serde_json::json!(true);
        value["contributes"]["facet_transforms"][0]["type"] = serde_json::json!("script.per_track");
        fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(validate_manifest_against_shipped_schema(&manifest).is_err());
    }
}
