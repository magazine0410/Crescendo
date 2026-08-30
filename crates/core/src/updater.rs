//! GitHub Release updater for Crescendo's native Linux packages.
//!
//! The app deliberately ships `.deb` and `.rpm` packages, so Tauri's
//! AppImage updater is not applicable. Downloads are kept in Crescendo's
//! cache and verified against GitHub's SHA-256 release-asset digest before
//! they are passed to the system package manager through PolicyKit.

use anyhow::{anyhow, bail, Context, Result};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/magazine0410/Crescendo/releases/latest";
const GITHUB_REPOSITORY: &str = "magazine0410/Crescendo";

/// Identify the running app. This crate carries its own (unrelated) version
/// number, so the app's is passed in by the caller rather than read from
/// `CARGO_PKG_VERSION` here.
fn user_agent(version: &Version) -> String {
    format!("Crescendo/{version}")
}

#[derive(Clone, Debug, Serialize)]
pub struct UpdateCheck {
    pub current_version: String,
    pub available: bool,
    pub latest_version: Option<String>,
    pub release_notes: Option<String>,
    pub package_name: Option<String>,
    pub downloaded: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct InstallResult {
    pub version: String,
    pub package_removed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PackageKind {
    Deb,
    Rpm,
}

/// Why an anonymous release lookup failed, so the fallback can explain itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AnonymousFailure {
    /// No release visible without credentials — a private repository, or none published.
    NotFound,
    /// GitHub's unauthenticated request budget for this address is spent.
    RateLimited,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ReleaseAccess {
    #[default]
    Anonymous,
    GithubCli,
}

impl PackageKind {
    fn extension(self) -> &'static str {
        match self {
            Self::Deb => ".deb",
            Self::Rpm => ".rpm",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Deb => "Debian",
            Self::Rpm => "RPM",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GithubAsset>,
    #[serde(skip)]
    access: ReleaseAccess,
}

#[derive(Clone, Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

struct SelectedUpdate {
    release: GithubRelease,
    version: Version,
    kind: PackageKind,
    asset: GithubAsset,
    path: PathBuf,
}

/// Query the latest published (non-draft, non-prerelease) GitHub Release and
/// compare it with the running app's version.
pub fn check(current_version: &str, cache_dir: &Path) -> Result<UpdateCheck> {
    let current = parse_version(current_version)
        .with_context(|| format!("the installed version {current_version:?} is invalid"))?;
    let release = fetch_latest_release(&user_agent(&current))?;
    let latest = version_from_tag(&release.tag_name)?;

    if latest <= current {
        return Ok(UpdateCheck {
            current_version: current.to_string(),
            available: false,
            latest_version: Some(latest.to_string()),
            release_notes: None,
            package_name: None,
            downloaded: false,
        });
    }

    let selected = select_update(release, latest, cache_dir)?;
    let downloaded = cached_package_is_valid(&selected.path, &selected.asset)?;
    Ok(UpdateCheck {
        current_version: current.to_string(),
        available: true,
        latest_version: Some(selected.version.to_string()),
        release_notes: selected
            .release
            .body
            .clone()
            .filter(|s| !s.trim().is_empty()),
        package_name: Some(selected.asset.name.clone()),
        downloaded,
    })
}

/// Download (or reuse) the current release package, authenticate it with the
/// digest GitHub calculated at upload time, then ask PolicyKit to run the
/// distro's package manager. A cancelled/failed install deliberately leaves
/// the verified package in the cache for a later retry.
pub fn install(
    current_version: &str,
    requested_version: &str,
    cache_dir: &Path,
) -> Result<InstallResult> {
    let current = parse_version(current_version)
        .with_context(|| format!("the installed version {current_version:?} is invalid"))?;
    let requested = parse_version(requested_version)
        .with_context(|| format!("the requested version {requested_version:?} is invalid"))?;
    let agent = user_agent(&current);
    let release = fetch_latest_release(&agent)?;
    let latest = version_from_tag(&release.tag_name)?;

    if latest != requested {
        bail!(
            "GitHub's latest release changed from {requested} to {latest}. Check for updates again."
        );
    }
    if latest <= current {
        bail!("Crescendo {current} is already up to date");
    }

    let selected = select_update(release, latest, cache_dir)?;
    if !cached_package_is_valid(&selected.path, &selected.asset)? {
        download_package(
            &selected.asset,
            &selected.path,
            selected.release.access,
            &selected.release.tag_name,
            &agent,
        )?;
    }

    // A cheap local check first, so a corrupt cache fails before the password
    // prompt instead of after it. The check that *decides* is the one the
    // elevated script makes against its own root-owned copy: this one runs as
    // the user, over a file the user can still rewrite afterwards.
    if !cached_package_is_valid(&selected.path, &selected.asset)? {
        bail!("The downloaded package failed its GitHub SHA-256 integrity check");
    }

    run_installer(selected.kind, &selected.path, &selected.asset)?;
    let package_removed = match fs::remove_file(&selected.path) {
        Ok(()) => true,
        Err(error) => {
            log::warn!(
                "Crescendo {} installed but cached package {} could not be removed: {error}",
                selected.version,
                selected.path.display()
            );
            false
        }
    };

    Ok(InstallResult {
        version: selected.version.to_string(),
        package_removed,
    })
}

fn fetch_latest_release(user_agent: &str) -> Result<GithubRelease> {
    let response = ureq::get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", user_agent)
        .config()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .call();

    let mut response = match response {
        Ok(response) => response,
        // 404 is what a private repository answers anonymously; 403 is the
        // unauthenticated rate limit. Both are worth retrying through the CLI,
        // which is authenticated, but they need different advice when it is
        // missing.
        Err(ureq::Error::StatusCode(404)) => {
            return fetch_latest_release_with_gh(AnonymousFailure::NotFound)
        }
        Err(ureq::Error::StatusCode(403)) => {
            return fetch_latest_release_with_gh(AnonymousFailure::RateLimited)
        }
        Err(error) => return Err(anyhow!(error)).context("could not check GitHub for updates"),
    };

    response
        .body_mut()
        .read_json::<GithubRelease>()
        .context("GitHub returned invalid release information")
}

fn fetch_latest_release_with_gh(reason: AnonymousFailure) -> Result<GithubRelease> {
    let gh = find_program(&["gh"]).ok_or_else(|| match reason {
        AnonymousFailure::NotFound => anyhow!(
            "No public Crescendo release was found, and GitHub CLI is unavailable. For a private repository, install GitHub CLI and run `gh auth login`."
        ),
        AnonymousFailure::RateLimited => anyhow!(
            "GitHub is rate-limiting update checks that are not signed in, for this whole network. Wait an hour and try again, or install GitHub CLI and run `gh auth login` to check as yourself."
        ),
    })?;
    let output = Command::new(gh)
        .args([
            "api",
            "repos/magazine0410/Crescendo/releases/latest",
            "-H",
            "Accept: application/vnd.github+json",
        ])
        .output()
        .context("could not run GitHub CLI for the private release check")?;
    if !output.status.success() {
        let detail = command_error_detail(&output);
        if detail.is_empty() {
            bail!(
                "GitHub CLI could not access a latest Crescendo release. Make sure `gh auth status` succeeds and a published release exists."
            );
        }
        bail!(
            "GitHub CLI could not access a latest Crescendo release: {detail}\nMake sure `gh auth status` succeeds and a published release exists."
        );
    }

    let mut release: GithubRelease = serde_json::from_slice(&output.stdout)
        .context("GitHub CLI returned invalid release information")?;
    release.access = ReleaseAccess::GithubCli;
    Ok(release)
}

fn select_update(
    release: GithubRelease,
    version: Version,
    cache_dir: &Path,
) -> Result<SelectedUpdate> {
    let kind = detect_package_kind()?;
    let asset = choose_asset(&release.assets, kind, env::consts::ARCH)
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "Crescendo {version} has no {} package for this computer ({})",
                kind.display_name(),
                env::consts::ARCH
            )
        })?;
    validate_asset(&asset)?;

    let file_name = Path::new(&asset.name)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| *name == asset.name)
        .ok_or_else(|| anyhow!("GitHub returned an unsafe package filename"))?;
    let path = cache_dir.join("updates").join(file_name);

    Ok(SelectedUpdate {
        release,
        version,
        kind,
        asset,
        path,
    })
}

fn version_from_tag(tag: &str) -> Result<Version> {
    let trimmed = tag.trim();
    let without_v = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    parse_version(without_v)
        .with_context(|| format!("GitHub's release tag {tag:?} is not a version"))
}

fn parse_version(value: &str) -> Result<Version> {
    Version::parse(value.trim()).map_err(Into::into)
}

fn choose_asset<'a>(
    assets: &'a [GithubAsset],
    kind: PackageKind,
    arch: &str,
) -> Option<&'a GithubAsset> {
    let expected_arches: Vec<&str> = match arch {
        "x86_64" => vec!["amd64", "x86_64", "x64"],
        "aarch64" => vec!["arm64", "aarch64"],
        "x86" | "i686" => vec!["i386", "i686", "x86"],
        "arm" | "armv7" => vec!["armhf", "armv7", "armv7hl"],
        other => vec![other],
    };
    let all_arches = [
        "amd64", "x86_64", "x64", "arm64", "aarch64", "i386", "i686", "armhf", "armv7", "armv7hl",
    ];

    assets
        .iter()
        .filter_map(|asset| {
            let name = asset.name.to_ascii_lowercase();
            if !name.starts_with("crescendo") || !name.ends_with(kind.extension()) {
                return None;
            }
            let matching = expected_arches.iter().any(|token| name.contains(token));
            let wrong = all_arches
                .iter()
                .any(|token| !expected_arches.contains(token) && name.contains(token));
            if !matching || wrong {
                return None;
            }

            // Prefer Tauri's exact native names if a release happens to carry
            // more than one compatible-looking package.
            let exact = match kind {
                PackageKind::Deb => name.contains("_amd64.deb") || name.contains("_arm64.deb"),
                PackageKind::Rpm => name.contains(".x86_64.rpm") || name.contains(".aarch64.rpm"),
            };
            Some((exact, asset))
        })
        .max_by_key(|(exact, _)| *exact)
        .map(|(_, asset)| asset)
}

fn validate_asset(asset: &GithubAsset) -> Result<()> {
    if !asset
        .name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
    {
        bail!("GitHub returned an unsafe package filename");
    }
    if asset.size == 0 {
        bail!("GitHub reports that {} is empty", asset.name);
    }
    if asset.size > 512 * 1024 * 1024 {
        bail!("GitHub reports an unexpectedly large update package");
    }
    if !asset
        .browser_download_url
        .starts_with("https://github.com/")
    {
        bail!("GitHub returned an unexpected package download address");
    }
    expected_sha256(asset)?;
    Ok(())
}

fn expected_sha256(asset: &GithubAsset) -> Result<&str> {
    let digest = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .filter(|digest| digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| {
            anyhow!(
                "GitHub did not provide a usable SHA-256 digest for {}. The update was not installed.",
                asset.name
            )
        })?;
    Ok(digest)
}

fn cached_package_is_valid(path: &Path, asset: &GithubAsset) -> Result<bool> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("could not inspect the cached update package"),
    };
    if !metadata.is_file() || metadata.len() != asset.size {
        return Ok(false);
    }
    Ok(file_sha256(path)? == expected_sha256(asset)?.to_ascii_lowercase())
}

fn file_sha256(path: &Path) -> Result<String> {
    let file = File::open(path).context("could not open the cached update package")?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .context("could not read the cached update package")?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Create a directory that only this user may enter, and tighten it if it
/// already exists with looser permissions.
fn ensure_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).context("could not create the update cache")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .context("could not restrict the update cache")?;
    Ok(())
}

fn download_package(
    asset: &GithubAsset,
    destination: &Path,
    access: ReleaseAccess,
    release_tag: &str,
    user_agent: &str,
) -> Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("the update cache path has no parent directory"))?;
    ensure_private_dir(parent)?;
    let partial = destination.with_extension(format!(
        "{}.part",
        destination
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("package")
    ));
    // A process interruption can leave this updater-owned temporary file
    // behind. `gh release download --output` refuses to overwrite it.
    let _ = fs::remove_file(&partial);

    let download_result = (|| -> Result<()> {
        let bytes = match access {
            ReleaseAccess::Anonymous => download_package_anonymously(asset, &partial, user_agent)?,
            ReleaseAccess::GithubCli => download_package_with_gh(asset, &partial, release_tag)?,
        };
        if bytes != asset.size {
            bail!(
                "The update download was incomplete (expected {} bytes, received {bytes})",
                asset.size
            );
        }
        if file_sha256(&partial)? != expected_sha256(asset)?.to_ascii_lowercase() {
            bail!("The downloaded package failed its GitHub SHA-256 integrity check");
        }
        fs::rename(&partial, destination).context("could not finish caching the update package")?;
        Ok(())
    })();

    if download_result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    download_result
}

fn download_package_anonymously(
    asset: &GithubAsset,
    partial: &Path,
    user_agent: &str,
) -> Result<u64> {
    let mut response = ureq::get(&asset.browser_download_url)
        .header("Accept", "application/octet-stream")
        .header("User-Agent", user_agent)
        .config()
        .timeout_global(Some(Duration::from_secs(10 * 60)))
        .build()
        .call()
        .with_context(|| format!("could not download {}", asset.name))?;
    let file = File::create(partial).context("could not create the cached update package")?;
    let mut writer = std::io::BufWriter::new(file);
    let mut reader = response
        .body_mut()
        .as_reader()
        .take(asset.size.saturating_add(1));
    let bytes = std::io::copy(&mut reader, &mut writer).context("the update download failed")?;
    writer
        .flush()
        .context("could not finish writing the update package")?;
    Ok(bytes)
}

fn download_package_with_gh(asset: &GithubAsset, partial: &Path, release_tag: &str) -> Result<u64> {
    let gh = find_program(&["gh"]).ok_or_else(|| {
        anyhow!("GitHub CLI is required to download an update from the private repository")
    })?;
    let output = Command::new(gh)
        .args([
            "release",
            "download",
            release_tag,
            "--repo",
            GITHUB_REPOSITORY,
        ])
        .arg("--pattern")
        .arg(&asset.name)
        .arg("--output")
        .arg(partial)
        .output()
        .context("could not run GitHub CLI to download the private update")?;
    if !output.status.success() {
        let detail = command_error_detail(&output);
        if detail.is_empty() {
            bail!("GitHub CLI could not download {}", asset.name);
        }
        bail!("GitHub CLI could not download {}: {detail}", asset.name);
    }
    fs::metadata(partial)
        .map(|metadata| metadata.len())
        .context("GitHub CLI did not create the downloaded update package")
}

fn detect_package_kind() -> Result<PackageKind> {
    let dpkg_query = find_program(&["dpkg-query"]);
    let rpm = find_program(&["rpm"]);

    if dpkg_query.as_deref().is_some_and(|program| {
        Command::new(program)
            .args(["-W", "crescendo"])
            .output()
            .is_ok_and(|out| out.status.success())
    }) {
        return Ok(PackageKind::Deb);
    }
    if rpm.as_deref().is_some_and(|program| {
        ["Crescendo", "crescendo"].iter().any(|name| {
            Command::new(program)
                .args(["-q", name])
                .output()
                .is_ok_and(|out| out.status.success())
        })
    }) {
        return Ok(PackageKind::Rpm);
    }

    let os_release = fs::read_to_string("/etc/os-release")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ["debian", "ubuntu", "mint", "pop", "zorin"]
        .iter()
        .any(|id| os_release.contains(id))
    {
        return Ok(PackageKind::Deb);
    }
    if [
        "fedora", "rhel", "centos", "suse", "opensuse", "rocky", "alma",
    ]
    .iter()
    .any(|id| os_release.contains(id))
    {
        return Ok(PackageKind::Rpm);
    }

    if find_program(&["apt-get", "dpkg"]).is_some() {
        Ok(PackageKind::Deb)
    } else if find_program(&["dnf5", "dnf", "zypper", "rpm"]).is_some() {
        Ok(PackageKind::Rpm)
    } else {
        bail!("This Linux system does not support Crescendo's .deb or .rpm updater")
    }
}

/// Exit codes the elevated script uses for its own failures, kept clear of the
/// ranges apt/dpkg/dnf/zypper/rpm and pkexec use so the two cannot be confused.
const EXIT_DIGEST_FAILED: i32 = 91;
const EXIT_COPY_FAILED: i32 = 92;

/// Run as root, through `sh -c`, by `run_installer`.
///
/// Root copies the package into a directory only root can enter, checks the
/// digest of *that* copy, and installs the copy. This is what closes the race:
/// the cache entry stays writable by this user, but nothing after the copy
/// reads it again, and a swap made during the copy fails the digest check.
/// Verifying as the user before elevating cannot do this — the checked path
/// stays writable by the user for as long as the installer takes to open it.
///
/// The script is a constant. Every value reaches it as a positional argument,
/// so nothing is ever spliced into a command line, whatever a future change
/// lets through the asset validators. Bare command names are safe because
/// pkexec replaces the environment, PATH included, with a root-controlled one.
const INSTALL_SCRIPT: &str = r#"set -eu
umask 077
src=$1
digest=$2
name=$3
shift 3
dir=$(mktemp -d /var/tmp/crescendo-update.XXXXXXXX)
trap 'rm -rf -- "$dir"' EXIT HUP INT TERM
cp -- "$src" "$dir/$name" || exit 92
chmod 0644 -- "$dir/$name"
printf '%s  %s\n' "$digest" "$dir/$name" > "$dir/sha256"
sha256sum -c --status -- "$dir/sha256" || exit 91
"$@" "$dir/$name"
"#;

/// The argument vector handed to the shell: `-c`, the constant script, `$0`,
/// then the script's own positional arguments. Split out so a test can prove
/// the values stay separate arguments.
fn install_script_args(
    package: &Path,
    digest: &str,
    name: &str,
    installer: &Path,
    args: &[&str],
) -> Vec<OsString> {
    let mut argv: Vec<OsString> = vec![
        OsString::from("-c"),
        OsString::from(INSTALL_SCRIPT),
        OsString::from("crescendo-update"),
        package.into(),
        OsString::from(digest),
        OsString::from(name),
        installer.into(),
    ];
    argv.extend(args.iter().map(OsString::from));
    argv
}

fn installer_for(kind: PackageKind) -> Result<(PathBuf, Vec<&'static str>)> {
    Ok(match kind {
        PackageKind::Deb => {
            if let Some(apt_get) = find_program(&["apt-get"]) {
                (apt_get, vec!["install", "-y"])
            } else if let Some(dpkg) = find_program(&["dpkg"]) {
                (dpkg, vec!["-i"])
            } else {
                bail!("No Debian package installer was found")
            }
        }
        PackageKind::Rpm => {
            if let Some(dnf) = find_program(&["dnf5", "dnf"]) {
                (dnf, vec!["install", "-y"])
            } else if let Some(zypper) = find_program(&["zypper"]) {
                (
                    zypper,
                    vec!["--non-interactive", "--no-gpg-checks", "install"],
                )
            } else if let Some(rpm) = find_program(&["rpm"]) {
                (rpm, vec!["-Uvh"])
            } else {
                bail!("No RPM package installer was found")
            }
        }
    })
}

fn run_installer(kind: PackageKind, package: &Path, asset: &GithubAsset) -> Result<()> {
    let pkexec = find_program(&["pkexec"]).ok_or_else(|| {
        anyhow!("PolicyKit (pkexec) is required to install the update with administrator access")
    })?;
    let shell = find_program(&["sh"])
        .ok_or_else(|| anyhow!("A POSIX shell is required to install the update"))?;
    let (installer, args) = installer_for(kind)?;
    let digest = expected_sha256(asset)?;
    let name = package
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("the update package has no usable filename"))?;

    let output = Command::new(pkexec)
        .arg(&shell)
        .args(install_script_args(
            package, digest, name, &installer, &args,
        ))
        .output()
        .context("could not start the system package installer")?;
    interpret_installer_output(output)
}

fn interpret_installer_output(output: Output) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    match output.status.code() {
        Some(EXIT_DIGEST_FAILED) => bail!(
            "The update package failed its SHA-256 integrity check when it was re-checked with administrator access. Nothing was installed."
        ),
        Some(EXIT_COPY_FAILED) => bail!(
            "The update package could not be copied for installation. Nothing was installed."
        ),
        Some(126 | 127) => bail!(
            "Installation was cancelled. The downloaded package was kept for another attempt."
        ),
        _ => {
            let detail = command_error_detail(&output);
            if detail.is_empty() {
                bail!(
                    "The system package installer failed. The downloaded package was kept for another attempt."
                );
            }
            bail!(
                "The system package installer failed: {detail}\nThe downloaded package was kept for another attempt."
            )
        }
    }
}

fn command_error_detail(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if !stderr.trim().is_empty() {
        stderr.trim()
    } else {
        stdout.trim()
    };
    detail
        .chars()
        .rev()
        .take(1200)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

fn find_program(names: &[&str]) -> Option<PathBuf> {
    for directory in [
        "/usr/local/bin",
        "/usr/local/sbin",
        "/usr/bin",
        "/usr/sbin",
        "/bin",
        "/sbin",
    ] {
        for name in names {
            let candidate = Path::new(directory).join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let path = env::var_os("PATH")?;
    for directory in env::split_paths(&path) {
        for name in names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> GithubAsset {
        GithubAsset {
            name: name.into(),
            browser_download_url: format!("https://github.com/example/releases/download/v1/{name}"),
            size: 123,
            digest: Some(format!("sha256:{}", "a".repeat(64))),
        }
    }

    #[test]
    fn release_tags_accept_the_usual_optional_v() {
        assert_eq!(version_from_tag("v0.8.0").unwrap(), Version::new(0, 8, 0));
        assert_eq!(version_from_tag("0.7.3").unwrap(), Version::new(0, 7, 3));
        assert!(version_from_tag("release-eight").is_err());
    }

    #[test]
    fn chooses_tauri_package_for_format_and_architecture() {
        let assets = vec![
            asset("Crescendo_0.8.0_arm64.deb"),
            asset("Crescendo_0.8.0_amd64.deb"),
            asset("Crescendo-0.8.0-1.x86_64.rpm"),
            asset("source.tar.gz"),
        ];
        assert_eq!(
            choose_asset(&assets, PackageKind::Deb, "x86_64")
                .unwrap()
                .name,
            "Crescendo_0.8.0_amd64.deb"
        );
        assert_eq!(
            choose_asset(&assets, PackageKind::Rpm, "x86_64")
                .unwrap()
                .name,
            "Crescendo-0.8.0-1.x86_64.rpm"
        );
        assert_eq!(
            choose_asset(&assets, PackageKind::Deb, "aarch64")
                .unwrap()
                .name,
            "Crescendo_0.8.0_arm64.deb"
        );
    }

    #[test]
    fn rejects_assets_without_githubs_digest() {
        let mut unsigned = asset("Crescendo_0.8.0_amd64.deb");
        unsigned.digest = None;
        assert!(validate_asset(&unsigned).is_err());
    }

    /// Run INSTALL_SCRIPT exactly as pkexec would, minus the elevation, with a
    /// stub standing in for the package manager. The stub copies the package it
    /// was handed to `delivered`, so a test can check what the installer would
    /// actually have received.
    fn run_install_script(source: &Path, digest: &str, delivered: &Path) -> Output {
        let stub = source.parent().unwrap().join("stub-installer.sh");
        // "$@" "$dir/$name" puts the package last: $1 is the destination the
        // stub was configured with, $2 the package root staged for it.
        fs::write(&stub, "cp -- \"$2\" \"$1\"\n").unwrap();
        let name = source.file_name().unwrap().to_str().unwrap();
        let argv = install_script_args(
            source,
            digest,
            name,
            Path::new("/bin/sh"),
            &[stub.to_str().unwrap(), delivered.to_str().unwrap()],
        );
        Command::new("/bin/sh").args(&argv).output().unwrap()
    }

    #[test]
    fn the_elevated_script_installs_only_what_matches_the_digest() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Crescendo_0.8.0_amd64.deb");
        let delivered = dir.path().join("delivered.deb");
        fs::write(&source, b"real package").unwrap();

        let good = format!("{:x}", Sha256::digest(b"real package"));
        let output = run_install_script(&source, &good, &delivered);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            fs::read(&delivered).unwrap(),
            b"real package",
            "the installer must receive the checked bytes"
        );

        // The same package after someone else rewrote the cache entry.
        fs::remove_file(&delivered).unwrap();
        fs::write(&source, b"swapped package").unwrap();
        let output = run_install_script(&source, &good, &delivered);
        assert_eq!(output.status.code(), Some(EXIT_DIGEST_FAILED));
        assert!(
            !delivered.exists(),
            "a mismatching package reached the installer"
        );
    }

    #[test]
    fn elevated_values_stay_separate_arguments() {
        let argv = install_script_args(
            Path::new("/home/u/.cache/crescendo/updates/Crescendo_0.8.0_amd64.deb"),
            &"a".repeat(64),
            "Crescendo_0.8.0_amd64.deb",
            Path::new("/usr/bin/apt-get"),
            &["install", "-y"],
        );
        assert_eq!(argv[0], "-c");
        assert_eq!(
            argv[1],
            OsString::from(INSTALL_SCRIPT),
            "the script root runs must stay a constant"
        );
        assert_eq!(
            argv[3],
            OsString::from("/home/u/.cache/crescendo/updates/Crescendo_0.8.0_amd64.deb")
        );
        assert_eq!(argv[4], OsString::from("a".repeat(64)));
        assert_eq!(argv[6], OsString::from("/usr/bin/apt-get"));
        assert_eq!(argv[7], OsString::from("install"));
    }

    #[test]
    fn cached_package_is_reused_only_while_its_digest_matches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Crescendo_0.8.0_amd64.deb");
        fs::write(&path, b"real package").unwrap();
        let mut package = asset("Crescendo_0.8.0_amd64.deb");
        package.size = 12;
        package.digest = Some(format!("sha256:{:x}", Sha256::digest(b"real package")));

        assert!(cached_package_is_valid(&path, &package).unwrap());
        fs::write(&path, b"fake package").unwrap();
        assert!(!cached_package_is_valid(&path, &package).unwrap());
    }
}
