//! Optional plugins published in the latest release of
//! [`CATALOG_REPOSITORY`], listed and installed from Settings.
//!
//! A release carries every current plugin package, an `index.json` that
//! describes them (with each package's size and SHA-256), and
//! `index.json.sig`, a minisign signature made with Crescendo's offline
//! release key, the same key installed copies already trust for app updates.
//! The index is used only once its signature checks out, and a package is
//! installed only when its bytes match the hash in that index, so neither a
//! changed release asset nor a changed index can install code nobody signed.
//! Installing then goes through [`PluginManager::install`], exactly like a
//! package picked from disk: it arrives disabled, and a Wasm plugin's
//! capabilities still need the user's approval.
//!
//! Nothing about the user is sent: the requests are GitHub's release API and
//! asset downloads with a `Crescendo/<version>` User-Agent. When the
//! repository is private, or GitHub is rate-limiting anonymous requests, the
//! signed-in GitHub CLI is used instead, as the updater does.

use super::manager::{compatibility_problem, PluginManager, PluginMutationResult};
use super::manifest::PluginKind;
use crate::updater::{
    command_error_detail, find_program, minisign_text, output_within, UPDATE_PUBLIC_KEY,
};
use anyhow::{anyhow, bail, Context, Result};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub const CATALOG_REPOSITORY: &str = "magazine0410/crescendo-plugins";
const INDEX_ASSET: &str = "index.json";
const INDEX_SIGNATURE_ASSET: &str = "index.json.sig";
const MAX_INDEX_BYTES: u64 = 1024 * 1024;
const MAX_SIGNATURE_BYTES: u64 = 4096;
const INDEX_SCHEMA_VERSION: u32 = 1;
const GH_TIMEOUT: Duration = Duration::from_secs(120);

/// One plugin as the signed index describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogPlugin {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub kind: PluginKind,
    pub api_version: String,
    pub min_crescendo_version: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    /// The release asset holding the package.
    pub asset: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    schema_version: u32,
    plugins: Vec<CatalogPlugin>,
}

/// What installing a listed plugin would do here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CatalogStatus {
    NotInstalled,
    UpToDate,
    /// The listed version is newer than the installed one.
    UpdateAvailable,
    /// The installed copy is newer than the listed one.
    InstalledNewer,
    /// This build cannot load the listed version.
    Incompatible {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntry {
    #[serde(flatten)]
    pub plugin: CatalogPlugin,
    pub installed_version: Option<String>,
    pub status: CatalogStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct Catalog {
    /// The release tag the list came from.
    pub release: String,
    pub plugins: Vec<CatalogEntry>,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
    #[serde(skip)]
    through_cli: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}

fn user_agent(app_version: &Version) -> String {
    format!("Crescendo/{app_version}")
}

fn latest_release(app_version: &Version) -> Result<Release> {
    let url = format!("https://api.github.com/repos/{CATALOG_REPOSITORY}/releases/latest");
    let response = ureq::get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", &user_agent(app_version))
        .config()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .call();
    match response {
        Ok(mut response) => crate::http::read_json::<Release>(response.body_mut())
            .context("GitHub returned invalid release information"),
        // 404 is a private repository (or no release yet) to an anonymous
        // request, 403 the anonymous rate limit. The CLI is signed in.
        Err(ureq::Error::StatusCode(404 | 403)) => latest_release_with_cli(),
        Err(error) => Err(anyhow!(error)).context("could not reach GitHub for the plugin list"),
    }
}

fn gh() -> Result<PathBuf> {
    find_program(&["gh"]).ok_or_else(|| {
        anyhow!(
            "No public plugin release was found, and GitHub CLI is unavailable. For a private repository, install GitHub CLI and run `gh auth login`."
        )
    })
}

fn latest_release_with_cli() -> Result<Release> {
    let mut command = Command::new(gh()?);
    command.args([
        "api",
        &format!("repos/{CATALOG_REPOSITORY}/releases/latest"),
        "-H",
        "Accept: application/vnd.github+json",
    ]);
    let output = output_within(&mut command, GH_TIMEOUT)
        .context("could not run GitHub CLI for the plugin list")?;
    if !output.status.success() {
        bail!(
            "GitHub CLI could not read the latest plugin release: {}",
            command_error_detail(&output)
        );
    }
    let mut release: Release = serde_json::from_slice(&output.stdout)
        .context("GitHub CLI returned invalid release information")?;
    release.through_cli = true;
    Ok(release)
}

/// Download one release asset into `dir` (private to this user), refusing
/// anything larger than `limit` or than GitHub says it is.
fn download(
    release: &Release,
    name: &str,
    dir: &Path,
    limit: u64,
    app_version: &Version,
) -> Result<Vec<u8>> {
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .ok_or_else(|| anyhow!("the plugin release {} has no {name}", release.tag_name))?;
    if asset.size > limit {
        bail!("{name} is larger than Crescendo accepts");
    }
    if release.through_cli {
        fs::create_dir_all(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        let partial = dir.join(format!("{name}.part"));
        let _ = fs::remove_file(&partial);
        let mut command = Command::new(gh()?);
        command
            .args([
                "release",
                "download",
                &release.tag_name,
                "--repo",
                CATALOG_REPOSITORY,
            ])
            .arg("--pattern")
            .arg(name)
            .arg("--output")
            .arg(&partial);
        let output = output_within(&mut command, GH_TIMEOUT)
            .with_context(|| format!("could not run GitHub CLI to download {name}"))?;
        let bytes = if output.status.success() {
            fs::read(&partial).with_context(|| format!("GitHub CLI did not save {name}"))
        } else {
            Err(anyhow!(
                "GitHub CLI could not download {name}: {}",
                command_error_detail(&output)
            ))
        };
        let _ = fs::remove_file(&partial);
        let bytes = bytes?;
        if bytes.len() as u64 != asset.size {
            bail!("the download of {name} was incomplete");
        }
        return Ok(bytes);
    }
    let mut response = ureq::get(&asset.browser_download_url)
        .header("Accept", "application/octet-stream")
        .header("User-Agent", &user_agent(app_version))
        .config()
        .timeout_global(Some(Duration::from_secs(5 * 60)))
        .build()
        .call()
        .with_context(|| format!("could not download {name}"))?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("the download of {name} failed"))?;
    if bytes.len() as u64 != asset.size {
        bail!("the download of {name} was incomplete");
    }
    Ok(bytes)
}

/// Check that `signature` was made over `index` with the release key, for a
/// file named `index.json` (the trusted comment, which the signature covers,
/// says so).
fn verify_index_signature(public_key: &str, index: &[u8], signature: &[u8]) -> Result<()> {
    let key = minisign_verify::PublicKey::decode(&minisign_text(public_key)?)
        .map_err(|error| anyhow!("the release public key is invalid: {error}"))?;
    let signature = std::str::from_utf8(signature).context("the index signature is not text")?;
    let signature = minisign_verify::Signature::decode(&minisign_text(signature)?)
        .map_err(|error| anyhow!("the index signature is malformed: {error}"))?;
    let expected = format!("file:{INDEX_ASSET}");
    if !signature
        .trusted_comment()
        .split('\t')
        .any(|field| field == expected)
    {
        bail!("the index signature was made for a different file");
    }
    key.verify(index, &signature, false)
        .map_err(|_| anyhow!("the plugin list is not signed by Crescendo's release key"))
}

/// The plugins a verified index lists. Each must name its own package file
/// and carry a real SHA-256.
fn parse_index(index: &[u8]) -> Result<Vec<CatalogPlugin>> {
    let index: Index = serde_json::from_slice(index).context("the plugin list is not valid")?;
    if index.schema_version != INDEX_SCHEMA_VERSION {
        bail!(
            "the plugin list uses format {}, which this build does not read",
            index.schema_version
        );
    }
    for plugin in &index.plugins {
        let expected_asset = format!("{}-{}.crescendo-plugin", plugin.id, plugin.version);
        if plugin.asset != expected_asset {
            bail!(
                "the plugin list names an unexpected package {:?}",
                plugin.asset
            );
        }
        if plugin.sha256.len() != 64 || !plugin.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("the plugin list has an invalid hash for {}", plugin.id);
        }
    }
    Ok(index.plugins)
}

fn status(
    manager: &PluginManager,
    plugin: &CatalogPlugin,
    installed: Option<&str>,
) -> CatalogStatus {
    if let Some(reason) = compatibility_problem(
        plugin.kind,
        &plugin.api_version,
        &plugin.min_crescendo_version,
        &plugin.permissions,
        manager.app_version(),
    ) {
        return CatalogStatus::Incompatible { reason };
    }
    let Some(installed) = installed else {
        return CatalogStatus::NotInstalled;
    };
    match (Version::parse(&plugin.version), Version::parse(installed)) {
        (Ok(listed), Ok(installed)) => match listed.cmp_precedence(&installed) {
            std::cmp::Ordering::Greater => CatalogStatus::UpdateAvailable,
            std::cmp::Ordering::Equal => CatalogStatus::UpToDate,
            std::cmp::Ordering::Less => CatalogStatus::InstalledNewer,
        },
        _ => CatalogStatus::UpToDate,
    }
}

fn fetch_verified(
    app_version: &Version,
    cache_dir: &Path,
) -> Result<(Release, Vec<CatalogPlugin>)> {
    let release = latest_release(app_version)?;
    let index = download(
        &release,
        INDEX_ASSET,
        cache_dir,
        MAX_INDEX_BYTES,
        app_version,
    )?;
    let signature = download(
        &release,
        INDEX_SIGNATURE_ASSET,
        cache_dir,
        MAX_SIGNATURE_BYTES,
        app_version,
    )?;
    verify_index_signature(UPDATE_PUBLIC_KEY, &index, &signature)?;
    Ok((release, parse_index(&index)?))
}

/// The plugins in the latest release, each with what installing it would do.
pub fn fetch(manager: &PluginManager, cache_dir: &Path) -> Result<Catalog> {
    let (release, plugins) = fetch_verified(manager.app_version(), cache_dir)?;
    let snapshot = manager.snapshot()?;
    let entries = plugins
        .into_iter()
        .map(|plugin| {
            let installed = snapshot
                .plugins
                .iter()
                .find(|info| info.id == plugin.id)
                .and_then(|info| info.version.clone());
            CatalogEntry {
                status: status(manager, &plugin, installed.as_deref()),
                installed_version: installed,
                plugin,
            }
        })
        .collect();
    Ok(Catalog {
        release: release.tag_name,
        plugins: entries,
    })
}

/// Download `plugin_id` from the latest release, check it against the
/// signed index, and install it (or stage it as an update).
pub fn install(
    manager: &PluginManager,
    cache_dir: &Path,
    plugin_id: &str,
) -> Result<PluginMutationResult> {
    let app_version = manager.app_version().clone();
    // The list is fetched and verified again rather than trusted from the
    // webview, so the hash below always comes from a signed index.
    let (release, plugins) = fetch_verified(&app_version, cache_dir)?;
    let plugin = plugins
        .into_iter()
        .find(|plugin| plugin.id == plugin_id)
        .ok_or_else(|| anyhow!("{plugin_id} is not in the plugin list"))?;
    if let CatalogStatus::Incompatible { reason } = status(manager, &plugin, None) {
        bail!("{} cannot be installed: {reason}", plugin.name);
    }
    if plugin.size > super::package::MAX_ARCHIVE_BYTES {
        bail!("{} is larger than Crescendo accepts", plugin.name);
    }
    let bytes = download(
        &release,
        &plugin.asset,
        cache_dir,
        super::package::MAX_ARCHIVE_BYTES,
        &app_version,
    )?;
    if bytes.len() as u64 != plugin.size
        || format!("{:x}", Sha256::digest(&bytes)) != plugin.sha256.to_ascii_lowercase()
    {
        bail!(
            "the downloaded {} does not match the signed plugin list",
            plugin.name
        );
    }
    fs::create_dir_all(cache_dir)?;
    fs::set_permissions(cache_dir, fs::Permissions::from_mode(0o700))?;
    let path = cache_dir.join(&plugin.asset);
    fs::write(&path, &bytes)?;
    let result = (|| {
        // The package must be the plugin and version the index describes.
        let report = super::tooling::validate_plugin_package(&path)?;
        if report.manifest.id != plugin.id || report.manifest.version != plugin.version {
            bail!(
                "the downloaded package is {} {}, not {} {}",
                report.manifest.id,
                report.manifest.version,
                plugin.id,
                plugin.version
            );
        }
        manager.install(&path)
    })();
    let _ = fs::remove_file(&path);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway key made with `tauri signer generate` for these tests only;
    /// its secret half was discarded. The signatures were made with
    /// `tauri signer sign` over [`INDEX`].
    const TEST_PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDZENTQ2M0JCRkJDMDEwRkUKUldUK0VNRDd1Mk5VYlI5VnNzaWo4bHhoeHF4SkRJRFhqeEhUTkFHeTE4TDYwcHUxb25MbGZPc0cK";
    const INDEX: &[u8] = b"{\"schema_version\":1,\"plugins\":[]}\n";
    const SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUK0VNRDd1Mk5VYmZmYzBlTFU4NlFnbGgwSjFnTG9adytKWlVnWDM2RStsKzFlUUNQS3VyY2xNVzU1UklrNFBrT0ZKTnY3QWNjY2cvSDlkY1hjcHE2WUxoK1hHMHZOcmcwPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0ODYzMzA5CWZpbGU6aW5kZXguanNvbgp6OUwyMXJJSDBYaU9nYTZRR1dNa0MwdE5OMndOZEYxV2NoQjFqQUNJbnpibnlwaGxoWDR6MlZ3cHU1YS9zL3pXL2JOTkxYaW0wUG52RmNlYXpFZ1dEUT09Cg==";
    /// The same bytes, signed by the test key as `other.json`.
    const SIGNED_AS_OTHER_FILE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUK0VNRDd1Mk5VYlVsa0ZsSkZNT0Q5SldveFRmWnBkWnl6YVRyN1VlQWswVEF2WkFiakpWMHZ3czdsdGk5VWNYZmJSdjRDaHkxc0RKSkd2THl6YWVIUXRlc0FsV0NsWmc4PQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0ODYzMzQxCWZpbGU6b3RoZXIuanNvbgowT2I3clhBOThxRHphT1NOVDZyRWNjL2Y4YkhZdWd3RjN5Sy90NEp2T0oyMjExYkk4V2ZWQ2tNZjBST2luSjBEeWxiSFZpYlpaYlFlN0Z1NGRpaHBCdz09Cg==";
    /// The same bytes and name, signed by a different throwaway key.
    const SIGNED_BY_OTHER_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTOUhtelRJSUc1Z2FCa2c2NHAyWlpWWkVLakxNTVMzODg0K3l2Q01TQld2Rk9wY0pNV29nWjdIN3BKaVVLeXVjRThrdkE5MUxWUjc3eTA1c21XbE9rY1N2T1F2T1BZR3dBPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0ODYzMzQxCWZpbGU6aW5kZXguanNvbgpweWQ4NXk1bk96bXprb1BMMS8zOGMzN1Y3RXdGTTZxaEVsNFRzdjhTL1U0RlRVZUZ2S3pwYWppWUljdmhwWEU3RGtkRjVXbWNsZkVLQUJvdllzdjJCQT09Cg==";

    #[test]
    fn only_an_index_signed_by_the_key_for_index_json_is_accepted() {
        verify_index_signature(TEST_PUBLIC_KEY, INDEX, SIGNATURE.as_bytes()).unwrap();
        let mut changed = INDEX.to_vec();
        changed[1] = b' ';
        assert!(verify_index_signature(TEST_PUBLIC_KEY, &changed, SIGNATURE.as_bytes()).is_err());
        assert!(
            verify_index_signature(TEST_PUBLIC_KEY, INDEX, SIGNED_AS_OTHER_FILE.as_bytes())
                .is_err(),
            "a signature for another file"
        );
        assert!(
            verify_index_signature(TEST_PUBLIC_KEY, INDEX, SIGNED_BY_OTHER_KEY.as_bytes()).is_err(),
            "another key"
        );
        assert!(
            verify_index_signature(UPDATE_PUBLIC_KEY, INDEX, SIGNATURE.as_bytes()).is_err(),
            "the real release key did not sign the test index"
        );
    }

    fn entry(id: &str, version: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id, "name": "Example", "version": version,
            "description": "An example", "kind": "declarative",
            "api_version": "1.0.0", "min_crescendo_version": "0.8.0",
            "asset": format!("{id}-{version}.crescendo-plugin"),
            "size": 10, "sha256": "ab".repeat(32),
        })
    }

    fn index_of(schema: u32, plugins: Vec<serde_json::Value>) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"schema_version": schema, "plugins": plugins}))
            .unwrap()
    }

    #[test]
    fn index_entries_must_name_their_own_package_and_a_real_hash() {
        let plugins =
            parse_index(&index_of(1, vec![entry("dev.crescendo.example", "1.0.0")])).unwrap();
        assert_eq!(
            plugins[0].asset,
            "dev.crescendo.example-1.0.0.crescendo-plugin"
        );

        let mut elsewhere = entry("dev.crescendo.example", "1.0.0");
        elsewhere["asset"] = "../evil.crescendo-plugin".into();
        assert!(parse_index(&index_of(1, vec![elsewhere])).is_err());
        let mut short_hash = entry("dev.crescendo.example", "1.0.0");
        short_hash["sha256"] = "zz".into();
        assert!(parse_index(&index_of(1, vec![short_hash])).is_err());
        let mut extra = entry("dev.crescendo.example", "1.0.0");
        extra["script"] = "run me".into();
        assert!(
            parse_index(&index_of(1, vec![extra])).is_err(),
            "unknown fields"
        );
        assert!(
            parse_index(&index_of(2, vec![])).is_err(),
            "a future format"
        );
    }
}
