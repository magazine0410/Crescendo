//! Manifest-declared, user-approved plugin capabilities.
//!
//! A declarative plugin never requests permissions. A Wasm plugin may request
//! any capability in [`PluginPermission::ALL`]; the grant is stored against the
//! plugin identity, package version, and package hash, so an update that
//! changes the package — including one that requests additional capabilities —
//! stays disabled until the user approves it again.

use anyhow::{anyhow, bail, Result};
use std::collections::BTreeSet;

/// Domains a `network.http` plugin may contact. Declared in the manifest and
/// approved together with the permission itself.
pub const MAX_NETWORK_DOMAINS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PluginPermission {
    LibraryRead,
    LibraryPaths,
    PlayerRead,
    PlayerControl,
    PlaylistsRead,
    PlaylistsWrite,
    NetworkHttp,
    Storage,
}

impl std::fmt::Display for PluginPermission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl PluginPermission {
    /// The complete v1 capability vocabulary. The manifest schema accepts
    /// exactly these identifiers.
    pub const ALL: &'static [PluginPermission] = &[
        PluginPermission::LibraryRead,
        PluginPermission::LibraryPaths,
        PluginPermission::PlayerRead,
        PluginPermission::PlayerControl,
        PluginPermission::PlaylistsRead,
        PluginPermission::PlaylistsWrite,
        PluginPermission::NetworkHttp,
        PluginPermission::Storage,
    ];

    /// Capabilities whose host interfaces exist in this build. A plugin
    /// requesting another capability stays installed but cannot be enabled
    /// until a later runtime provides it. Runtime API 0.2.0 provides the whole
    /// v1 vocabulary, so this is currently every permission — the list stays
    /// because it is the mechanism that keeps a package naming a future
    /// capability from being enabled against a host that would ignore it.
    pub const RUNTIME_SUPPORTED: &'static [PluginPermission] = PluginPermission::ALL;

    pub fn as_str(self) -> &'static str {
        match self {
            PluginPermission::LibraryRead => "library.read",
            PluginPermission::LibraryPaths => "library.paths",
            PluginPermission::PlayerRead => "player.read",
            PluginPermission::PlayerControl => "player.control",
            PluginPermission::PlaylistsRead => "playlists.read",
            PluginPermission::PlaylistsWrite => "playlists.write",
            PluginPermission::NetworkHttp => "network.http",
            PluginPermission::Storage => "storage",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        PluginPermission::ALL
            .iter()
            .copied()
            .find(|permission| permission.as_str() == value)
    }

    pub fn runtime_supported(self) -> bool {
        PluginPermission::RUNTIME_SUPPORTED.contains(&self)
    }
}

/// A user-approved capability grant, pinned to one exact installed package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginGrant {
    pub plugin_id: String,
    pub permissions: BTreeSet<PluginPermission>,
    pub network_domains: Vec<String>,
    pub version: String,
    pub package_hash: String,
    pub granted_at: i64,
}

impl PluginGrant {
    /// True when this grant approves exactly the permissions and domains the
    /// installed package requests, and was recorded for that package's own
    /// version and hash.
    pub fn approves(
        &self,
        plugin_id: &str,
        permissions: &BTreeSet<PluginPermission>,
        network_domains: &[String],
        version: &str,
        package_hash: &str,
    ) -> bool {
        self.plugin_id == plugin_id
            && self.version == version
            && self.package_hash == package_hash
            && self.permissions == *permissions
            && self.network_domains == network_domains
    }
}

/// Parse and validate a manifest permission list. Rejects unknown names,
/// duplicates, and inconsistent combinations.
pub fn parse_permissions(values: &[String]) -> Result<BTreeSet<PluginPermission>> {
    let mut permissions = BTreeSet::new();
    for value in values {
        let permission = PluginPermission::parse(value)
            .ok_or_else(|| anyhow!("unknown plugin permission {value:?}"))?;
        if !permissions.insert(permission) {
            bail!("plugin permission {value:?} is declared more than once");
        }
    }
    if permissions.contains(&PluginPermission::LibraryPaths)
        && !permissions.contains(&PluginPermission::LibraryRead)
    {
        bail!("the library.paths permission requires library.read");
    }
    Ok(permissions)
}

/// Parse and validate the domains declared for a `network.http` permission.
pub fn parse_network_domains(values: &[String]) -> Result<Vec<String>> {
    if values.is_empty() {
        bail!("network.http plugins must declare at least one approved domain");
    }
    if values.len() > MAX_NETWORK_DOMAINS {
        bail!("plugins cannot declare more than {MAX_NETWORK_DOMAINS} domains");
    }
    parse_network_domain_list(values)
}

/// Validate a stored grant's domain list: the same hostname rules, but an
/// empty list is valid because only `network.http` plugins declare domains.
pub fn parse_stored_network_domains(values: &[String]) -> Result<Vec<String>> {
    if values.len() > MAX_NETWORK_DOMAINS {
        bail!("a stored grant cannot hold more than {MAX_NETWORK_DOMAINS} domains");
    }
    parse_network_domain_list(values)
}

fn parse_network_domain_list(values: &[String]) -> Result<Vec<String>> {
    let mut domains = Vec::with_capacity(values.len());
    for value in values {
        validate_network_domain(value)?;
        if domains.contains(&value.to_owned()) {
            bail!("network domain {value:?} is declared more than once");
        }
        domains.push(value.clone());
    }
    Ok(domains)
}

/// Hostnames only: lowercase labels of ASCII letters, digits, and inner
/// hyphens. No scheme, userinfo, port, or path — HTTPS and port rules are
/// enforced by the future `network.http` host implementation, not the package.
fn validate_network_domain(domain: &str) -> Result<()> {
    if domain.is_empty() || domain.len() > 253 || domain != domain.to_ascii_lowercase() {
        bail!("network domains must be lowercase hostnames of at most 253 bytes");
    }
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        bail!("network domains must be multi-label hostnames, not bare TLDs");
    }
    for label in labels {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            bail!("network domain {domain:?} is not a valid hostname");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permission_strings(values: &[PluginPermission]) -> Vec<String> {
        values.iter().map(|p| p.as_str().to_owned()).collect()
    }

    #[test]
    fn parses_the_full_vocabulary_and_rejects_unknown_names() {
        let all = permission_strings(PluginPermission::ALL);
        let parsed = parse_permissions(&all).unwrap();
        assert_eq!(parsed.len(), PluginPermission::ALL.len());

        assert!(parse_permissions(&["library.read".into()]).is_ok());
        assert!(parse_permissions(&["fs.read".into()]).is_err());
        assert!(parse_permissions(&["library.read".into(), "library.read".into()]).is_err());
    }

    #[test]
    fn library_paths_requires_library_read() {
        assert!(parse_permissions(&["library.paths".into()]).is_err());
        assert!(parse_permissions(&["library.read".into(), "library.paths".into()]).is_ok());
    }

    #[test]
    fn network_domains_are_validated_hostnames() {
        assert!(parse_network_domains(&["example.com".into()]).is_ok());
        assert!(parse_network_domains(&["api.example.com".into()]).is_ok());
        assert!(parse_network_domains(&[]).is_err());
        for bad in [
            "Example.com",
            "https://example.com",
            "example.com:443",
            "user@example.com",
            "example.com/path",
            "com",
            "-example.com",
            "example..com",
            &"a".repeat(254),
        ] {
            assert!(
                parse_network_domains(&[bad.into()]).is_err(),
                "{bad} must be rejected"
            );
        }
    }

    #[test]
    fn grants_match_only_the_exact_package_and_request() {
        let grant = PluginGrant {
            plugin_id: "dev.crescendo.example".into(),
            permissions: parse_permissions(&["storage".into()]).unwrap(),
            network_domains: vec![],
            version: "1.0.0".into(),
            package_hash: "aa".repeat(32),
            granted_at: 0,
        };
        let permissions = parse_permissions(&["storage".into()]).unwrap();
        assert!(grant.approves(
            "dev.crescendo.example",
            &permissions,
            &[],
            "1.0.0",
            &"aa".repeat(32)
        ));
        // A different version, hash, or permission set voids the grant.
        assert!(!grant.approves(
            "dev.crescendo.example",
            &permissions,
            &[],
            "1.1.0",
            &"aa".repeat(32)
        ));
        assert!(!grant.approves(
            "dev.crescendo.example",
            &permissions,
            &[],
            "1.0.0",
            &"bb".repeat(32)
        ));
        assert!(!grant.approves(
            "dev.crescendo.example",
            &parse_permissions(&["storage".into(), "library.read".into()]).unwrap(),
            &[],
            "1.0.0",
            &"aa".repeat(32)
        ));
    }
}
