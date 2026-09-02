//! Outbound HTTP for `network.http` plugins.
//!
//! Every request is checked before a socket is opened, and again for each
//! redirect hop, because a redirect is an attacker-controlled URL: an approved
//! host can bounce a plugin at `http://169.254.169.254/` otherwise. The rules
//! are HTTPS only, an approved host, a public destination address, a bounded
//! body, and a hard timeout.
//!
//! Validation and connection share **one** DNS lookup. Checking a hostname and
//! then letting the HTTP client resolve it again is a rebinding hole: the same
//! name can answer with a public address for the check and a private one for
//! the connection. A custom resolver therefore does the lookup, rejects any
//! non-public answer, and hands the connector exactly the addresses it cleared.
//! The URI is untouched, so TLS still verifies the original hostname.
//!
//! Requests are made on a throwaway agent, so no cookie jar, redirect history,
//! or credential from Crescendo's own HTTP use is ever attached.

use anyhow::{anyhow, bail, Result};
use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::time::{Duration, Instant};
use ureq::unversioned::resolver::{ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::NextTimeout;

/// Largest response body handed to a plugin.
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
/// Largest request body a plugin may send.
pub const MAX_REQUEST_BYTES: usize = 256 * 1024;
/// Wall-clock ceiling for one plugin request, redirects included.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Redirect hops followed before giving up. Each is revalidated.
pub const MAX_REDIRECTS: usize = 3;

/// Why a request was refused before it left the process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetRejection {
    /// Not https, unsupported method, or an unusable URL.
    InvalidRequest(String),
    /// Host is outside the approved list, or resolves somewhere private.
    BlockedHost(String),
}

impl std::fmt::Display for NetRejection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(reason) | Self::BlockedHost(reason) => {
                formatter.write_str(reason)
            }
        }
    }
}

/// Split a URL into (host, port), rejecting anything that is not plain HTTPS.
/// Deliberately strict: no userinfo (it carries credentials and confuses host
/// parsing), no empty host, no non-ASCII (a homograph of an approved domain
/// must not pass an ASCII comparison).
fn https_host(url: &str) -> std::result::Result<(String, u16), NetRejection> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| NetRejection::InvalidRequest("plugin requests must use https".into()))?;
    if !url.is_ascii() {
        return Err(NetRejection::InvalidRequest(
            "plugin request URLs must be ASCII".into(),
        ));
    }
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    if authority.is_empty() {
        return Err(NetRejection::InvalidRequest("the URL has no host".into()));
    }
    if authority.contains('@') {
        return Err(NetRejection::InvalidRequest(
            "credentials in the URL are not allowed".into(),
        ));
    }
    let (host, port) = match authority.rsplit_once(':') {
        // An IPv6 literal keeps its brackets; only a trailing :port splits.
        Some((head, tail)) if !head.ends_with(']') && tail.chars().all(|c| c.is_ascii_digit()) => {
            let port = tail
                .parse::<u16>()
                .map_err(|_| NetRejection::InvalidRequest("invalid port".into()))?;
            (head.to_string(), port)
        }
        _ => (authority.to_string(), 443),
    };
    if host.is_empty() {
        return Err(NetRejection::InvalidRequest("the URL has no host".into()));
    }
    Ok((host.to_ascii_lowercase(), port))
}

/// Whether `host` is covered by an approved domain. A domain approves itself
/// and its subdomains, never a suffix match: "evil-example.com" must not be
/// approved by "example.com".
pub fn host_is_approved(host: &str, approved: &[String]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    approved.iter().any(|domain| {
        let domain = domain.trim_end_matches('.').to_ascii_lowercase();
        host == domain || host.ends_with(&format!(".{domain}"))
    })
}

/// Addresses a plugin may never reach: loopback, link-local (cloud metadata
/// lives at 169.254.169.254), private ranges, and the unspecified address.
pub fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                // 100.64.0.0/10 carrier-grade NAT.
                || (v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]))
                // 0.0.0.0/8 "this network".
                || v4.octets()[0] == 0)
        }
        IpAddr::V6(v6) => {
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // fc00::/7 unique-local.
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // fe80::/10 link-local.
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                // IPv4-mapped addresses inherit the IPv4 rules.
                || v6.to_ipv4_mapped().is_some_and(|v4| !is_public_address(IpAddr::V4(v4))))
        }
    }
}

/// Policy checks for one URL — this request or a redirect target. Everything
/// here is decidable without touching the network. Where the host actually
/// resolves is checked by [`GuardedResolver`], which is the only lookup and
/// therefore the only answer the connector can act on.
pub fn check_url(url: &str, approved: &[String]) -> std::result::Result<(), NetRejection> {
    let (host, _port) = https_host(url)?;
    if !host_is_approved(&host, approved) {
        return Err(NetRejection::BlockedHost(format!(
            "{host} is not an approved domain for this plugin"
        )));
    }
    Ok(())
}

/// How many lookups may be outstanding at once.
///
/// A timed-out lookup cannot be cancelled — `getaddrinfo` is a blocking libc
/// call with no interruption — so its thread lives until the resolver gives
/// up on its own. That is fine for one stalled lookup and not fine without a
/// ceiling, so refuse to start a new one while this many are still hanging.
const MAX_OUTSTANDING_LOOKUPS: usize = 4;
static OUTSTANDING_LOOKUPS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Decrements the outstanding-lookup count however the resolver thread ends.
struct LookupSlot;

impl Drop for LookupSlot {
    fn drop(&mut self) {
        OUTSTANDING_LOOKUPS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Resolve with a deadline. `getaddrinfo` has no timeout of its own, and this
/// runs on the shared Wasm runtime thread, so a hanging lookup would stall
/// every plugin rather than just this request.
fn resolve_within(
    host: &str,
    port: u16,
    budget: Duration,
) -> std::result::Result<Vec<std::net::SocketAddr>, String> {
    // Claim a slot before spawning, and release it from the thread itself so
    // an abandoned lookup still frees its slot when the resolver returns.
    if OUTSTANDING_LOOKUPS
        .fetch_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |count| (count < MAX_OUTSTANDING_LOOKUPS).then_some(count + 1),
        )
        .is_err()
    {
        return Err("too many name lookups are already outstanding".into());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let lookup_host = host.to_string();
    let spawned = std::thread::Builder::new()
        .name("crescendo-plugin-dns".into())
        .spawn(move || {
            // Held by the lookup itself, so the slot is returned when the
            // resolver finally answers — even long after the caller gave up.
            let _slot = LookupSlot;
            let result = (lookup_host.as_str(), port)
                .to_socket_addrs()
                .map(|addresses| addresses.collect::<Vec<_>>())
                .map_err(|error| error.to_string());
            let _ = tx.send(result);
        });
    if let Err(error) = spawned {
        OUTSTANDING_LOOKUPS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        return Err(format!("could not start the resolver: {error}"));
    }
    match rx.recv_timeout(budget) {
        Ok(Ok(addresses)) => Ok(addresses),
        Ok(Err(error)) => Err(error),
        Err(_) => Err("the lookup timed out".into()),
    }
}

/// The only DNS lookup in a plugin request. Returning just the cleared
/// addresses means the connector cannot reach anything this did not approve,
/// so there is no window between the check and the connect.
#[derive(Debug)]
struct GuardedResolver;

impl Resolver for GuardedResolver {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        _config: &ureq::config::Config,
        timeout: NextTimeout,
    ) -> std::result::Result<ResolvedSocketAddrs, ureq::Error> {
        let host = uri.host().ok_or(ureq::Error::HostNotFound)?;
        let port = uri.port_u16().unwrap_or(443);
        // `not_zero` is None when ureq has no deadline for this step; the
        // module's own ceiling still applies.
        let budget = timeout
            .not_zero()
            .map(|value| (*value).into())
            .unwrap_or(REQUEST_TIMEOUT);
        let resolved =
            resolve_within(host, port, budget).map_err(|_| ureq::Error::HostNotFound)?;
        // ArrayVec has no `new`; seed it the way ureq's own resolver does.
        let mut addresses =
            ResolvedSocketAddrs::from_fn(|_| "0.0.0.0:1".parse().expect("valid placeholder"));
        for address in resolved {
            if !is_public_address(address.ip()) {
                // One private answer poisons the name: a record mixing public
                // and private addresses must not be usable at all.
                return Err(ureq::Error::HostNotFound);
            }
            // ArrayVec is fixed-capacity; stop adding once it is full but keep
            // scanning, so a private address later in the record still refuses.
            if addresses.len() < 16 {
                addresses.push(address);
            }
        }
        if addresses.is_empty() {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(addresses)
    }
}

/// One plugin HTTP response.
#[derive(Debug, Clone)]
pub struct PluginResponse {
    pub status: u16,
    pub body: String,
}

/// Perform a checked request. `method` is GET or POST; redirects are followed
/// manually so each hop passes `check_url` before it is fetched.
pub fn fetch(
    method: &str,
    url: &str,
    body: Option<&str>,
    approved: &[String],
) -> Result<PluginResponse> {
    let method = method.to_ascii_uppercase();
    if method != "GET" && method != "POST" {
        bail!("plugin requests support GET and POST only");
    }
    if let Some(body) = body {
        if body.len() > MAX_REQUEST_BYTES {
            bail!("plugin request body exceeds 256 KiB");
        }
    }

    // A fresh agent per call: an empty cookie jar, none of Crescendo's own
    // HTTP configuration, and the guarded resolver, so the connector can only
    // reach addresses this module cleared.
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .max_redirects(0)
        .max_redirects_will_error(false)
        .build();
    let agent = ureq::Agent::with_parts(
        config,
        ureq::unversioned::transport::DefaultConnector::default(),
        GuardedResolver,
    );

    // One deadline for the whole request. `timeout_global` covers a single
    // call, and every manually followed redirect is a new call, so the agent's
    // own limit would restart on each hop — three redirects could take four
    // times the stated budget. Each hop is given only what is left of it.
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let mut target = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()).filter(|left| !left.is_zero())
        else {
            bail!("the request exceeded its {REQUEST_TIMEOUT:?} budget");
        };
        check_url(&target, approved).map_err(|rejection| anyhow!("{rejection}"))?;
        let response = if method == "GET" {
            agent
                .get(&target)
                .config()
                .timeout_global(Some(remaining))
                .build()
                .call()
        } else {
            agent
                .post(&target)
                .config()
                .timeout_global(Some(remaining))
                .build()
                .send(body.unwrap_or_default())
        };
        let mut response = response.map_err(|error| {
            // The guarded resolver reports a private or unresolvable answer as
            // HostNotFound. Both are refusals of the destination, not network
            // trouble, and they are deliberately not told apart: "this name
            // does not resolve publicly" is all a plugin needs to know.
            if matches!(error, ureq::Error::HostNotFound) {
                anyhow!("{} did not resolve to a public address (non-public or unknown host)",
                    https_host(&target).map(|(host, _)| host).unwrap_or_default())
            } else {
                anyhow!("request failed: {error}")
            }
        })?;
        let status = response.status().as_u16();

        if (300..400).contains(&status) {
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
                .ok_or_else(|| anyhow!("redirect without a Location header"))?;
            // Relative redirects stay on a host that already passed, but an
            // absolute one is revalidated on the next turn of the loop.
            target = if location.starts_with("https://") {
                location
            } else if location.starts_with("http://") {
                bail!("redirect to plain http is refused");
            } else {
                let (host, port) = https_host(&target).map_err(|e| anyhow!("{e}"))?;
                let authority = if port == 443 {
                    host
                } else {
                    format!("{host}:{port}")
                };
                if let Some(path) = location.strip_prefix('/') {
                    format!("https://{authority}/{path}")
                } else {
                    format!("https://{authority}/{location}")
                }
            };
            continue;
        }

        let mut text = String::new();
        response
            .body_mut()
            .as_reader()
            .take(MAX_RESPONSE_BYTES as u64 + 1)
            .read_to_string(&mut text)
            .map_err(|error| anyhow!("could not read the response: {error}"))?;
        if text.len() > MAX_RESPONSE_BYTES {
            bail!("the response exceeded the 1 MiB limit");
        }
        return Ok(PluginResponse { status, body: text });
    }
    bail!("too many redirects")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn only_plain_https_urls_are_accepted() {
        assert!(https_host("http://example.com").is_err());
        assert!(https_host("ftp://example.com").is_err());
        assert!(https_host("https://").is_err());
        assert!(https_host("https://user:pass@example.com").is_err());
        // A Unicode homograph must not slip past an ASCII allowlist compare.
        assert!(https_host("https://exampⅼe.com").is_err());
        assert_eq!(
            https_host("https://Example.com/path?q=1").unwrap(),
            ("example.com".into(), 443)
        );
        assert_eq!(
            https_host("https://example.com:8443/x").unwrap(),
            ("example.com".into(), 8443)
        );
    }

    #[test]
    fn approval_covers_subdomains_but_never_a_bare_suffix() {
        let approved = vec!["example.com".to_string()];
        assert!(host_is_approved("example.com", &approved));
        assert!(host_is_approved("api.example.com", &approved));
        assert!(host_is_approved("EXAMPLE.COM.", &approved));
        assert!(!host_is_approved("evil-example.com", &approved));
        assert!(!host_is_approved("example.com.evil.test", &approved));
        assert!(!host_is_approved("example.org", &approved));
        assert!(!host_is_approved("example.com", &[]));
    }

    #[test]
    fn private_loopback_and_metadata_addresses_are_never_public() {
        for blocked in [
            "127.0.0.1",
            "10.0.0.4",
            "192.168.1.10",
            "172.16.0.1",
            // Cloud metadata.
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
        ] {
            let address: Ipv4Addr = blocked.parse().unwrap();
            assert!(
                !is_public_address(IpAddr::V4(address)),
                "{blocked} must be refused"
            );
        }
        assert!(is_public_address(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));

        for blocked in ["::1", "fc00::1", "fe80::1", "::ffff:127.0.0.1"] {
            let address: Ipv6Addr = blocked.parse().unwrap();
            assert!(
                !is_public_address(IpAddr::V6(address)),
                "{blocked} must be refused"
            );
        }
        assert!(is_public_address(IpAddr::V6(
            "2606:2800:220:1:248:1893:25c8:1946".parse().unwrap()
        )));
    }

    #[test]
    fn an_unapproved_host_is_refused_before_any_dns_lookup() {
        let error = check_url("https://example.com/data", &["other.test".into()]).unwrap_err();
        assert!(matches!(error, NetRejection::BlockedHost(_)));
        let error = check_url("http://example.com", &["example.com".into()]).unwrap_err();
        assert!(matches!(error, NetRejection::InvalidRequest(_)));
    }

    #[test]
    fn the_resolver_refuses_an_approved_name_that_points_at_localhost() {
        // Policy alone cannot stop this: "localhost" *is* approved here, so the
        // address gate has to be the thing that refuses it — and it has to live
        // in the resolver, because that is the lookup the connector uses.
        let uri: ureq::http::Uri = "https://localhost/data".parse().unwrap();
        let outcome = GuardedResolver.resolve(
            &uri,
            &ureq::Agent::config_builder().build(),
            NextTimeout {
                after: ureq::unversioned::transport::time::Duration::from(
                    Duration::from_secs(5),
                ),
                reason: ureq::Timeout::Global,
            },
        );
        assert!(
            outcome.is_err(),
            "an approved name resolving to loopback must not produce an address"
        );
        // The policy layer, by contrast, is happy with it.
        assert!(check_url("https://localhost/data", &["localhost".into()]).is_ok());
    }
}
