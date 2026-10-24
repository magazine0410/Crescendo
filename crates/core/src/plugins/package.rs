use super::manifest::PluginManifest;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use walkdir::WalkDir;
use zip::ZipArchive;

pub(crate) const MAX_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const MAX_EXTRACTED_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
pub(crate) const MAX_ENTRIES: usize = 512;
const MAX_COMPRESSION_RATIO: u64 = 200;
const INSTALL_METADATA: &str = ".crescendo-install.json";

static OPERATION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct StagedPackage {
    pub operation_id: String,
    pub operation_dir: PathBuf,
    pub content_dir: PathBuf,
    pub manifest: PluginManifest,
    pub package_hash: String,
}

#[derive(Debug, Clone)]
pub(crate) struct InstalledPackage {
    pub manifest: PluginManifest,
    pub package_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallMetadata {
    package_hash: String,
}

#[derive(Debug)]
struct EntryPlan {
    index: usize,
    relative: PathBuf,
    is_dir: bool,
    uncompressed_size: u64,
}

pub(crate) fn stage_archive(archive_path: &Path, staging_root: &Path) -> Result<StagedPackage> {
    let archive_meta = fs::metadata(archive_path)
        .with_context(|| format!("could not inspect {}", archive_path.display()))?;
    if !archive_meta.is_file() {
        bail!("the selected plugin package is not a regular file");
    }
    if archive_meta.len() == 0 || archive_meta.len() > MAX_ARCHIVE_BYTES {
        bail!("plugin packages must be between 1 byte and 32 MiB");
    }
    let extension = archive_path.extension().and_then(|value| value.to_str());
    if extension != Some("crescendo-plugin") {
        bail!("select a .crescendo-plugin package");
    }

    fs::create_dir_all(staging_root).context("could not create the plugin staging directory")?;
    let (operation_id, operation_dir) = create_operation_dir(staging_root)?;
    let content_dir = operation_dir.join("new");
    fs::create_dir(&content_dir).context("could not create the package extraction directory")?;

    let result = (|| {
        // Work only from a private, immutable-for-this-operation copy. The
        // selected file can live in a user-writable download directory and may
        // otherwise change between hashing and extraction.
        let archive_copy = operation_dir.join("package.crescendo-plugin");
        let package_hash = copy_and_hash(archive_path, &archive_copy, archive_meta.len())?;
        let mut file =
            File::open(&archive_copy).context("could not open the staged plugin package")?;
        let declared = declared_entry_count(&mut file)?;
        let mut archive = ZipArchive::new(file).context("the plugin package is not a valid ZIP")?;
        // The zip crate keys entries by name, so a second `module.wasm` or
        // `manifest.json` silently replaces the first and is counted once. A
        // reviewer's tool that takes the first copy would then inspect other
        // bytes than the ones installed.
        if declared != archive.len() {
            bail!("plugin package contains duplicate entry names");
        }
        let plans = validate_entries(&mut archive)?;
        extract_entries(&mut archive, &plans, &content_dir)?;

        let manifest_path = content_dir.join("manifest.json");
        let manifest = read_manifest(&manifest_path)?;
        let has_module = content_dir.join("module.wasm").is_file();
        manifest.validate(has_module)?;
        validate_declarative_assets(&content_dir, &manifest)?;

        let metadata = InstallMetadata {
            package_hash: package_hash.clone(),
        };
        let metadata_bytes = serde_json::to_vec(&metadata)?;
        let mut metadata_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(content_dir.join(INSTALL_METADATA))
            .context("could not write plugin installation metadata")?;
        metadata_file.write_all(&metadata_bytes)?;
        metadata_file.sync_all()?;
        sync_directory(&content_dir)?;
        sync_directory(&operation_dir)?;

        Ok(StagedPackage {
            operation_id,
            operation_dir: operation_dir.clone(),
            content_dir,
            manifest,
            package_hash,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&operation_dir);
    }
    result
}

/// The entry count the end-of-central-directory record declares. Packages are
/// far below the sizes that need ZIP64, so a ZIP64 marker is refused.
fn declared_entry_count(file: &mut File) -> Result<usize> {
    use std::io::{Read, Seek, SeekFrom};
    const EOCD_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    const EOCD_MIN_LEN: u64 = 22;
    let len = file.seek(SeekFrom::End(0))?;
    // The record sits at the end, followed by a comment of at most 64 KiB.
    let tail_len = len.min(EOCD_MIN_LEN + u64::from(u16::MAX));
    file.seek(SeekFrom::Start(len - tail_len))?;
    let mut tail = vec![0; tail_len as usize];
    file.read_exact(&mut tail)?;
    file.seek(SeekFrom::Start(0))?;
    let start = tail
        .windows(4)
        .rposition(|window| window == EOCD_SIGNATURE)
        .filter(|&start| tail.len() - start >= EOCD_MIN_LEN as usize)
        .ok_or_else(|| anyhow!("the plugin package is not a valid ZIP"))?;
    let total = u16::from_le_bytes([tail[start + 10], tail[start + 11]]);
    if total == u16::MAX {
        bail!("plugin packages cannot use ZIP64");
    }
    Ok(usize::from(total))
}

pub(crate) fn load_installed(path: &Path, expected_id: &str) -> Result<InstalledPackage> {
    let root_meta = fs::symlink_metadata(path)?;
    if !root_meta.file_type().is_dir() || root_meta.file_type().is_symlink() {
        bail!("installed plugin path is not a regular directory");
    }

    // Files are counted against the package limit, as the archive's entries
    // were: extraction creates the parent directories a package's paths
    // imply, and counting those let a package that validated and installed
    // read as invalid afterwards. Directories get a loose bound of their own.
    let mut files = 0usize;
    let mut directories = 0usize;
    let mut total = 0u64;
    let mut has_module = false;
    for entry in WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        if entry.path() == path {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        let counter = if metadata.is_dir() {
            &mut directories
        } else {
            &mut files
        };
        *counter += 1;
        // `+ 1` is the installation metadata file the host adds.
        if files > MAX_ENTRIES + 1 || directories > 4 * MAX_ENTRIES {
            bail!("installed plugin contains too many entries");
        }
        if metadata.file_type().is_symlink()
            || (!metadata.file_type().is_file() && !metadata.file_type().is_dir())
        {
            bail!("installed plugin contains a link or special file");
        }
        let relative = entry.path().strip_prefix(path)?;
        validate_installed_path(relative, metadata.is_dir())?;
        if metadata.is_file() {
            total = total
                .checked_add(metadata.len())
                .ok_or_else(|| anyhow!("installed plugin is too large"))?;
            if total > MAX_EXTRACTED_BYTES + MAX_MANIFEST_BYTES {
                bail!("installed plugin exceeds the extracted-size limit");
            }
            has_module |= relative == Path::new("module.wasm");
        }
    }

    let manifest = read_manifest(&path.join("manifest.json"))?;
    manifest.validate(has_module)?;
    validate_declarative_assets(path, &manifest)?;
    if manifest.id != expected_id {
        bail!("manifest id does not match its installed directory");
    }
    let metadata: InstallMetadata = serde_json::from_slice(&read_bounded(
        &path.join(INSTALL_METADATA),
        4096,
        "installation metadata",
    )?)?;
    if metadata.package_hash.len() != 64
        || !metadata
            .package_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        bail!("installed plugin has invalid package metadata");
    }
    Ok(InstalledPackage {
        manifest,
        package_hash: metadata.package_hash.to_lowercase(),
    })
}

fn validate_entries(archive: &mut ZipArchive<File>) -> Result<Vec<EntryPlan>> {
    if archive.len() == 0 || archive.len() > MAX_ENTRIES {
        bail!("plugin packages must contain between 1 and {MAX_ENTRIES} entries");
    }
    let mut plans = Vec::with_capacity(archive.len());
    let mut paths = HashMap::<PathBuf, bool>::new();
    let mut total_uncompressed = 0u64;
    let mut has_manifest = false;

    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let raw_name = std::str::from_utf8(entry.name_raw())
            .map_err(|_| anyhow!("plugin package paths must be valid UTF-8"))?;
        let (relative, trailing_slash) = normalize_archive_path(raw_name)?;
        if entry.is_symlink() || (!entry.is_dir() && !entry.is_file()) {
            bail!("plugin packages cannot contain links or special files");
        }
        let unix_type = entry.unix_mode().map(|mode| mode & 0o170000).unwrap_or(0);
        let is_dir = entry.is_dir() || trailing_slash;
        let allowed_type = if is_dir {
            unix_type == 0 || unix_type == 0o040000
        } else {
            unix_type == 0 || unix_type == 0o100000
        };
        if !allowed_type {
            bail!("plugin packages cannot contain links or special files");
        }
        if paths.insert(relative.clone(), is_dir).is_some() {
            bail!("plugin package contains duplicate normalized paths");
        }
        validate_package_layout(&relative, is_dir)?;
        has_manifest |= relative == Path::new("manifest.json") && !is_dir;

        let size = entry.size();
        if is_dir && size != 0 {
            bail!("directory entries must not contain data");
        }
        total_uncompressed = total_uncompressed
            .checked_add(size)
            .ok_or_else(|| anyhow!("plugin package is too large"))?;
        if total_uncompressed > MAX_EXTRACTED_BYTES {
            bail!("plugin package exceeds the 64 MiB extracted-size limit");
        }
        if relative == Path::new("manifest.json") && size > MAX_MANIFEST_BYTES {
            bail!("manifest.json exceeds 256 KiB");
        }
        if size > 1024 * 1024
            && entry
                .compressed_size()
                .saturating_mul(MAX_COMPRESSION_RATIO)
                < size
        {
            bail!("plugin package has an excessive decompression ratio");
        }
        plans.push(EntryPlan {
            index,
            relative,
            is_dir,
            uncompressed_size: size,
        });
    }
    if !has_manifest {
        bail!("plugin package must contain manifest.json at its root");
    }

    let files: HashSet<&Path> = paths
        .iter()
        .filter_map(|(path, is_dir)| (!is_dir).then_some(path.as_path()))
        .collect();
    for path in paths.keys() {
        let mut parent = path.parent();
        while let Some(candidate) = parent {
            if candidate.as_os_str().is_empty() {
                break;
            }
            if files.contains(candidate) {
                bail!("plugin package contains conflicting file and directory paths");
            }
            parent = candidate.parent();
        }
    }
    Ok(plans)
}

fn extract_entries(
    archive: &mut ZipArchive<File>,
    plans: &[EntryPlan],
    destination: &Path,
) -> Result<()> {
    let mut total_written = 0u64;
    for plan in plans {
        let output = destination.join(&plan.relative);
        if plan.is_dir {
            fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut entry = archive.by_index(plan.index)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)?;
        let written = copy_bounded(&mut entry, &mut file, plan.uncompressed_size)?;
        total_written = total_written
            .checked_add(written)
            .ok_or_else(|| anyhow!("plugin package is too large"))?;
        if total_written > MAX_EXTRACTED_BYTES {
            bail!("plugin package exceeds the 64 MiB extracted-size limit");
        }
        file.sync_all()?;
    }
    Ok(())
}

pub(super) fn normalize_archive_path(name: &str) -> Result<(PathBuf, bool)> {
    if name.is_empty()
        || name.starts_with('/')
        || name.starts_with('\\')
        || name.contains('\\')
        || name.contains('\0')
    {
        bail!("plugin package contains an unsafe path");
    }
    let trailing_slash = name.ends_with('/');
    let mut normalized = PathBuf::new();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(value) => {
                let value = value
                    .to_str()
                    .ok_or_else(|| anyhow!("plugin package paths must be valid UTF-8"))?;
                if value.contains(':') {
                    bail!("plugin package contains an unsafe path");
                }
                normalized.push(value);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("plugin package contains an unsafe path");
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        bail!("plugin package contains an empty path");
    }
    Ok((normalized, trailing_slash))
}

pub(super) fn validate_package_layout(path: &Path, is_dir: bool) -> Result<()> {
    let first = path
        .components()
        .next()
        .and_then(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        });
    match first {
        Some("assets") if path.components().count() > 1 || is_dir => Ok(()),
        Some("manifest.json") if !is_dir && path.components().count() == 1 => Ok(()),
        Some("module.wasm") if !is_dir && path.components().count() == 1 => Ok(()),
        _ => {
            bail!("plugin package contains an entry outside manifest.json, assets/, or module.wasm")
        }
    }
}

fn validate_installed_path(path: &Path, is_dir: bool) -> Result<()> {
    if path == Path::new(INSTALL_METADATA) {
        return Ok(());
    }
    validate_package_layout(path, is_dir)
}

pub(super) fn read_manifest(path: &Path) -> Result<PluginManifest> {
    let bytes = read_bounded(path, MAX_MANIFEST_BYTES, "manifest.json")?;
    serde_json::from_slice(&bytes).context("manifest.json is not valid schema-version-1 JSON")
}

/// Packages may carry only passive assets, Wasm or declarative alike. The
/// v1 host never serves these files to the webview; future host contributions
/// must resolve them by plugin/asset id and receive bounded bytes. Keeping
/// the package allow-list here prevents an apparently declarative package
/// from smuggling executable or remotely-loading content into a later
/// consumer. `module.wasm` is the single executable entry, and only for
/// `kind: "wasm"` packages.
pub(super) fn validate_declarative_assets(
    package_root: &Path,
    _manifest: &PluginManifest,
) -> Result<()> {
    let assets = package_root.join("assets");
    if !assets.try_exists()? {
        return Ok(());
    }
    for entry in WalkDir::new(&assets).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let extension = entry
            .path()
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .ok_or_else(|| anyhow!("plugin assets must have a supported extension"))?;
        if !matches!(
            extension.as_str(),
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "txt" | "md" | "json"
        ) {
            bail!(
                "plugin asset {} has an unsupported or active-content extension",
                entry.path().strip_prefix(package_root)?.display()
            );
        }
    }
    Ok(())
}

fn read_bounded(path: &Path, limit: u64, label: &str) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).with_context(|| format!("missing {label}"))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!("{label} is not a regular file");
    }
    if metadata.len() > limit {
        bail!("{label} is too large");
    }
    let file = File::open(path)?;
    let mut reader = BufReader::new(file).take(limit + 1);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("{label} is too large");
    }
    Ok(bytes)
}

fn copy_bounded(reader: &mut impl Read, writer: &mut impl Write, expected: u64) -> Result<u64> {
    let limit = expected
        .checked_add(1)
        .ok_or_else(|| anyhow!("plugin package entry is too large"))?;
    let mut limited = reader.take(limit);
    let written = std::io::copy(&mut limited, writer)?;
    if written != expected {
        bail!("plugin package entry size does not match its ZIP metadata");
    }
    Ok(written)
}

pub(crate) fn create_operation_dir(staging_root: &Path) -> Result<(String, PathBuf)> {
    for _ in 0..32 {
        let counter = OPERATION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let id = format!("op-{nanos:032x}-{:08x}-{counter:016x}", std::process::id());
        let path = staging_root.join(&id);
        match fs::create_dir(&path) {
            Ok(()) => {
                if let Err(error) = sync_directory(staging_root) {
                    let _ = fs::remove_dir(&path);
                    return Err(error).context("could not persist the plugin staging operation");
                }
                return Ok((id, path));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("could not create a plugin staging operation"),
        }
    }
    bail!("could not allocate a unique plugin staging operation")
}

fn copy_and_hash(source: &Path, destination: &Path, expected_size: u64) -> Result<String> {
    let file = File::open(source)?;
    let mut reader = BufReader::new(file).take(MAX_ARCHIVE_BYTES + 1);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > MAX_ARCHIVE_BYTES {
            bail!("plugin package exceeds the 32 MiB compressed-size limit");
        }
        hasher.update(&buffer[..read]);
        output.write_all(&buffer[..read])?;
    }
    if total != expected_size {
        bail!("the selected plugin package changed while it was being staged");
    }
    output.sync_all()?;
    Ok(format!("{:x}", hasher.finalize()))
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn write_test_package(
    path: &Path,
    manifest: &PluginManifest,
    extra_entries: &[(&str, &[u8], Option<u32>)],
) {
    use zip::write::SimpleFileOptions;
    let file = File::create(path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut special_modes = Vec::new();
    writer.start_file("manifest.json", options).unwrap();
    writer
        .write_all(&serde_json::to_vec(manifest).unwrap())
        .unwrap();
    for (name, bytes, mode) in extra_entries {
        if mode.is_some_and(|mode| mode & 0o170000 == 0o120000) {
            writer
                .add_symlink(*name, std::str::from_utf8(bytes).unwrap(), options)
                .unwrap();
            continue;
        }
        if mode.is_some_and(|mode| {
            let kind = mode & 0o170000;
            kind != 0 && kind != 0o100000
        }) {
            special_modes.push(((*name).to_owned(), mode.unwrap()));
        }
        let options = mode.map_or(options, |mode| options.unix_permissions(mode));
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();

    if !special_modes.is_empty() {
        let mut bytes = fs::read(path).unwrap();
        for (name, mode) in special_modes {
            patch_test_zip_unix_mode(&mut bytes, &name, mode);
        }
        fs::write(path, bytes).unwrap();
    }
}

#[cfg(test)]
fn patch_test_zip_unix_mode(bytes: &mut [u8], target: &str, mode: u32) {
    const CENTRAL_HEADER: &[u8] = b"PK\x01\x02";
    let mut cursor = 0;
    while cursor + 46 <= bytes.len() {
        let Some(offset) = bytes[cursor..]
            .windows(CENTRAL_HEADER.len())
            .position(|window| window == CENTRAL_HEADER)
            .map(|offset| cursor + offset)
        else {
            break;
        };
        if offset + 46 > bytes.len() {
            break;
        }
        let name_len = u16::from_le_bytes([bytes[offset + 28], bytes[offset + 29]]) as usize;
        let name_start = offset + 46;
        let name_end = name_start + name_len;
        if name_end <= bytes.len() && &bytes[name_start..name_end] == target.as_bytes() {
            bytes[offset + 5] = 3; // Unix creator system.
            bytes[offset + 38..offset + 42].copy_from_slice(&(mode << 16).to_le_bytes());
            return;
        }
        cursor = offset + CENTRAL_HEADER.len();
    }
    panic!("ZIP entry {target:?} was not found");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::{PluginContributions, PluginKind};

    fn manifest() -> PluginManifest {
        PluginManifest {
            schema_version: 1,
            id: "dev.crescendo.archive-test".into(),
            name: "Archive Test".into(),
            version: "1.0.0".into(),
            description: String::new(),
            authors: vec!["Crescendo".into()],
            license: "MIT".into(),
            homepage: None,
            kind: PluginKind::Declarative,
            api_version: "1.0.0".into(),
            min_crescendo_version: "0.8.0".into(),
            permissions: vec![],
            network_domains: vec![],
            contributes: PluginContributions::default(),
        }
    }

    #[test]
    fn extracts_a_valid_declarative_package_and_records_its_hash() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("valid.crescendo-plugin");
        write_test_package(
            &archive,
            &manifest(),
            &[("assets/readme.txt", b"safe", None)],
        );
        let staged = stage_archive(&archive, &dir.path().join("staging")).unwrap();
        assert_eq!(staged.manifest.id, manifest().id);
        assert_eq!(staged.package_hash.len(), 64);
        assert_eq!(
            fs::read(staged.content_dir.join("assets/readme.txt")).unwrap(),
            b"safe"
        );
    }

    #[test]
    fn rejects_traversal_symlinks_and_declarative_modules() {
        for (name, mode) in [
            ("../escape", None),
            ("/absolute", None),
            ("assets/link", Some(0o120777)),
            ("assets/device", Some(0o020666)),
            ("module.wasm", None),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let archive = dir.path().join("bad.crescendo-plugin");
            write_test_package(&archive, &manifest(), &[(name, b"payload", mode)]);
            assert!(
                stage_archive(&archive, &dir.path().join("staging")).is_err(),
                "{name}"
            );
            assert!(!dir.path().join("escape").exists());
        }
    }

    #[test]
    fn rejects_normalized_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("duplicate.crescendo-plugin");
        write_test_package(
            &archive,
            &manifest(),
            &[("assets/a/./x", b"one", None), ("assets/a/x", b"two", None)],
        );
        assert!(stage_archive(&archive, &dir.path().join("staging")).is_err());
    }

    #[test]
    fn rejects_exact_duplicate_entry_names() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("duplicate.crescendo-plugin");
        // The writer refuses a repeated name, so write a same-length stand-in
        // and rename it in both the local header and the central directory.
        write_test_package(&archive, &manifest(), &[("manifest.jsoX", b"{}", None)]);
        let mut bytes = fs::read(&archive).unwrap();
        let stand_in = b"manifest.jsoX";
        let mut at = 0;
        while let Some(found) = bytes[at..]
            .windows(stand_in.len())
            .position(|window| window == stand_in)
        {
            bytes[at + found + stand_in.len() - 1] = b'n';
            at += found + stand_in.len();
        }
        fs::write(&archive, bytes).unwrap();
        let error = stage_archive(&archive, &dir.path().join("staging"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("duplicate entry names"), "{error}");
    }

    /// Extraction creates the directories a package's paths imply. Counting
    /// them made a package that validated read as invalid once installed.
    #[test]
    fn implied_directories_do_not_count_against_the_entry_limit() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("nested.crescendo-plugin");
        let names: Vec<String> = (0..300).map(|i| format!("assets/d{i}/note.txt")).collect();
        let entries: Vec<(&str, &[u8], Option<u32>)> = names
            .iter()
            .map(|name| (name.as_str(), b"x".as_slice(), None))
            .collect();
        write_test_package(&archive, &manifest(), &entries);
        let staged = stage_archive(&archive, &dir.path().join("staging")).unwrap();
        load_installed(&staged.content_dir, &manifest().id).unwrap();
    }

    #[test]
    fn enforces_archive_entry_and_decompression_limits() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("ratio.crescendo-plugin");
        let repeated = vec![0u8; 2 * 1024 * 1024];
        write_test_package(
            &archive,
            &manifest(),
            &[("assets/repeated.bin", &repeated, None)],
        );
        assert!(stage_archive(&archive, &dir.path().join("staging")).is_err());

        let archive = dir.path().join("entries.crescendo-plugin");
        let file = File::create(&archive).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("manifest.json", options).unwrap();
        writer
            .write_all(&serde_json::to_vec(&manifest()).unwrap())
            .unwrap();
        for index in 0..MAX_ENTRIES {
            writer
                .start_file(format!("assets/{index}"), options)
                .unwrap();
        }
        writer.finish().unwrap();
        assert!(stage_archive(&archive, &dir.path().join("staging")).is_err());
    }

    #[test]
    fn rejects_an_oversized_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("manifest.crescendo-plugin");
        let file = File::create(&archive).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("manifest.json", options).unwrap();
        let mut bytes = serde_json::to_vec(&manifest()).unwrap();
        bytes.resize(MAX_MANIFEST_BYTES as usize + 1, b' ');
        writer.write_all(&bytes).unwrap();
        writer.finish().unwrap();

        assert!(stage_archive(&archive, &dir.path().join("staging")).is_err());
    }

    #[test]
    fn rejects_packages_over_the_compressed_size_limit_before_opening_them() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("large.crescendo-plugin");
        File::create(&archive)
            .unwrap()
            .set_len(MAX_ARCHIVE_BYTES + 1)
            .unwrap();
        assert!(stage_archive(&archive, &dir.path().join("staging")).is_err());
    }
}
