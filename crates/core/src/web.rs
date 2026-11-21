//! What the online lookups share: one way of asking, and one cache.
//!
//! ListenBrainz and the artist-profile sources are all asked the same way: a
//! JSON GET at a modest rate per site, where a transport failure says nothing
//! about the name that was asked for and a site that has just failed is left
//! alone for a while. Answers are kept in the `web_cache` table.
use crate::db::Db;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long an answer is kept.
pub(crate) const MAX_AGE_SECS: i64 = 30 * 24 * 3600;
/// How long "nothing under that name" is kept. Shorter, so an entry created
/// later still turns up.
pub(crate) const MISSING_AGE_SECS: i64 = 7 * 24 * 3600;
/// How long a site that failed is not asked again. Without this, a session
/// without a connection waits out one timeout per site on every track change.
const DOWN_FOR: Duration = Duration::from_secs(10 * 60);
const TIMEOUT: Duration = Duration::from_secs(12);
pub(crate) const USER_AGENT: &str = concat!("Crescendo/", env!("CARGO_PKG_VERSION"));

/// What one lookup came to. Only `Found` and `Missing` are answers; `Failed`
/// (no connection, a server error, a rate limit, a rejected token) means
/// nothing can be concluded, so it is never cached.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Fetch<T> {
    Found(T),
    Missing,
    Failed,
}

impl<T> Fetch<T> {
    pub(crate) fn found(self) -> Option<T> {
        match self {
            Fetch::Found(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Site {
    last_request: Option<Instant>,
    down_until: Option<Instant>,
}

/// Per site, keyed by `host:port`.
static SITES: Mutex<Option<HashMap<String, Site>>> = Mutex::new(None);

fn with_site<T>(origin: &str, f: impl FnOnce(&mut Site) -> T) -> T {
    let mut sites = SITES.lock().unwrap_or_else(|e| e.into_inner());
    f(sites
        .get_or_insert_with(HashMap::new)
        .entry(origin.to_string())
        .or_default())
}

fn origin(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| {
            let host = u.host_str()?.to_string();
            Some(match u.port() {
                Some(port) => format!("{host}:{port}"),
                None => host,
            })
        })
        .unwrap_or_default()
}

/// Waits until `every` has passed since the last request to this site. Runs
/// only on blocking workers, never the audio or UI thread.
fn pace(origin: &str, every: Duration) {
    let wait = with_site(origin, |site| {
        let now = Instant::now();
        let at = match site.last_request {
            Some(last) => (last + every).max(now),
            None => now,
        };
        site.last_request = Some(at);
        at - now
    });
    if !cfg!(test) {
        std::thread::sleep(wait);
    }
}

fn is_down(origin: &str) -> bool {
    with_site(origin, |site| {
        site.down_until.is_some_and(|until| Instant::now() < until)
    })
}

fn answer(
    origin: &str,
    result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Fetch<Value> {
    let down = || {
        with_site(origin, |site| {
            site.down_until = Some(Instant::now() + DOWN_FOR)
        });
        Fetch::Failed
    };
    let mut response = match result {
        Ok(response) => response,
        Err(error) => {
            log::warn!("{origin}: request failed: {error}");
            return down();
        }
    };
    match response.status().as_u16() {
        200..=299 => match crate::http::read_json(response.body_mut()) {
            Ok(value) => Fetch::Found(value),
            Err(error) => {
                log::warn!("{origin}: unreadable answer: {error}");
                Fetch::Failed
            }
        },
        404 => Fetch::Missing,
        status @ (429 | 500..=599) => {
            log::warn!("{origin}: HTTP {status}");
            down()
        }
        // A rejected token or a malformed request: asking again is pointless,
        // but the site itself is fine.
        status => {
            log::warn!("{origin}: HTTP {status}");
            Fetch::Failed
        }
    }
}

/// One JSON GET. `every` is the least time between two requests to the site.
pub(crate) fn get_json(
    url: &str,
    query: &[(&str, &str)],
    headers: &[(&str, &str)],
    every: Duration,
) -> Fetch<Value> {
    let origin = origin(url);
    if is_down(&origin) {
        return Fetch::Failed;
    }
    pace(&origin, every);
    let mut request = ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .config()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .build();
    for (key, value) in headers {
        request = request.header(*key, *value);
    }
    for (key, value) in query {
        request = request.query(key, value);
    }
    answer(&origin, request.call())
}

/// One POST of a JSON body, answered in JSON.
pub(crate) fn post_json(
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
    every: Duration,
) -> Fetch<Value> {
    let origin = origin(url);
    if is_down(&origin) {
        return Fetch::Failed;
    }
    pace(&origin, every);
    let mut request = ureq::post(url)
        .header("User-Agent", USER_AGENT)
        .config()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .build();
    for (key, value) in headers {
        request = request.header(*key, *value);
    }
    answer(&origin, request.send_json(body))
}

/// The cached answer for `key`; `Failed` when there is none that is fresh.
pub(crate) fn peek<T: DeserializeOwned>(db: &Db, key: &str) -> Fetch<T> {
    let read = |max_age| -> Option<Option<T>> {
        let raw = db.web_cache_get(key, max_age).ok()??;
        serde_json::from_str(&raw).ok()
    };
    match read(MISSING_AGE_SECS) {
        Some(Some(value)) => Fetch::Found(value),
        Some(None) => Fetch::Missing,
        None => match read(MAX_AGE_SECS) {
            Some(Some(value)) => Fetch::Found(value),
            _ => Fetch::Failed,
        },
    }
}

/// Caches an answer for `key`. A failure is not one, and is not stored.
pub(crate) fn store<T: Serialize>(db: &Db, key: &str, answer: &Fetch<T>) {
    let json = match answer {
        Fetch::Found(value) => serde_json::to_string(&Some(value)).ok(),
        Fetch::Missing => Some("null".to_string()),
        Fetch::Failed => None,
    };
    if let Some(json) = json {
        if let Err(error) = db.web_cache_put(key, &json) {
            log::warn!("cannot cache an online answer: {error}");
        }
    }
}

/// `fetch`'s answer for `key`, from the cache while it is fresh.
pub(crate) fn cached<T: Serialize + DeserializeOwned>(
    db: &Db,
    key: &str,
    fetch: impl FnOnce() -> Fetch<T>,
) -> Fetch<T> {
    match peek(db, key) {
        Fetch::Failed => {}
        answer => return answer,
    }
    let answer = fetch();
    store(db, key, &answer);
    answer
}

/// Like [`cached`], for a source whose answers and misses alike may be kept
/// only for `max_age_secs`.
pub(crate) fn cached_within<T: Serialize + DeserializeOwned>(
    db: &Db,
    key: &str,
    max_age_secs: i64,
    fetch: impl FnOnce() -> Fetch<T>,
) -> Fetch<T> {
    let kept = db
        .web_cache_get(key, max_age_secs)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<Option<T>>(&raw).ok());
    match kept {
        Some(Some(value)) => return Fetch::Found(value),
        Some(None) => return Fetch::Missing,
        None => {}
    }
    let answer = fetch();
    store(db, key, &answer);
    answer
}

/// A cache key from its parts, which may hold any text.
pub(crate) fn key(parts: &[&str]) -> String {
    parts.join("\u{1f}")
}

#[cfg(test)]
pub(crate) mod testing {
    use serde_json::Value;
    use std::io::{Read, Write};
    use std::time::Duration;

    /// Answers one request per response, in order, on one local address, and
    /// returns what each request said (request line, headers and body).
    pub(crate) fn server(
        responses: Vec<(u16, Value)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, value) in responses {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                }
                let length = String::from_utf8_lossy(&bytes)
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|s| s.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                let mut body = vec![0; length];
                socket.read_exact(&mut body).unwrap();
                bytes.extend(body);
                let body = value.to_string();
                write!(
                    socket,
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                requests.push(String::from_utf8(bytes).unwrap());
            }
            requests
        });
        (url, thread)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("cache.db")).unwrap();
        (dir, db)
    }

    #[test]
    fn answers_and_misses_are_cached_but_failures_are_not() {
        let (_dir, db) = db();
        assert_eq!(cached::<i64>(&db, "a", || Fetch::Failed), Fetch::Failed);
        assert_eq!(cached(&db, "a", || Fetch::Found(7)), Fetch::Found(7));
        assert_eq!(
            cached::<i64>(&db, "a", || panic!("asked again")),
            Fetch::Found(7)
        );
        assert_eq!(cached::<i64>(&db, "b", || Fetch::Missing), Fetch::Missing);
        assert_eq!(
            cached::<i64>(&db, "b", || panic!("asked again")),
            Fetch::Missing
        );
    }

    #[test]
    fn a_source_with_its_own_age_limit_is_asked_again_after_it() {
        let (_dir, db) = db();
        let six_hours = 6 * 3600;
        assert_eq!(
            cached_within(&db, "k", six_hours, || Fetch::Found(1)),
            Fetch::Found(1)
        );
        assert_eq!(
            cached_within::<i64>(&db, "k", six_hours, || panic!("asked again")),
            Fetch::Found(1)
        );
        db.with(|conn| {
            conn.execute(
                "UPDATE web_cache SET fetched_at = fetched_at - ?1",
                [six_hours + 60],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            cached_within(&db, "k", six_hours, || Fetch::Found(2)),
            Fetch::Found(2)
        );
        // And what it kept is removed at startup once it is past that age.
        db.with(|conn| {
            conn.execute(
                "UPDATE web_cache SET fetched_at = fetched_at - ?1",
                [six_hours + 60],
            )?;
            Ok(())
        })
        .unwrap();
        db.web_cache_put("other", "1").unwrap();
        db.web_cache_prune_prefix("k", six_hours).unwrap();
        assert!(db.web_cache_get("k", i64::MAX / 2).unwrap().is_none());
        assert!(db.web_cache_get("other", six_hours).unwrap().is_some());
    }

    #[test]
    fn a_miss_expires_sooner_than_an_answer() {
        let (_dir, db) = db();
        cached(&db, "answer", || Fetch::Found(1)).found().unwrap();
        let _ = cached::<i64>(&db, "miss", || Fetch::Missing);
        db.with(|conn| {
            conn.execute(
                "UPDATE web_cache SET fetched_at = fetched_at - ?1",
                [MISSING_AGE_SECS + 60],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            cached::<i64>(&db, "answer", || panic!("asked again")),
            Fetch::Found(1)
        );
        assert_eq!(cached(&db, "miss", || Fetch::Found(2)), Fetch::Found(2));
    }

    #[test]
    fn a_site_that_failed_is_left_alone_and_a_missing_page_is_an_answer() {
        let (url, requests) = testing::server(vec![(404, json!({})), (503, json!({}))]);
        let every = Duration::ZERO;
        assert_eq!(get_json(&url, &[("q", "夜")], &[], every), Fetch::Missing);
        assert_eq!(get_json(&url, &[], &[], every), Fetch::Failed);
        // No third request reaches the server, which has stopped listening.
        assert_eq!(get_json(&url, &[], &[], every), Fetch::Failed);
        let requests = requests.join().unwrap();
        assert!(
            requests[0].starts_with("GET /?q=%E5%A4%9C "),
            "{requests:?}"
        );
        assert!(requests[0]
            .to_lowercase()
            .contains(&format!("user-agent: {USER_AGENT}").to_lowercase()));
    }
}
