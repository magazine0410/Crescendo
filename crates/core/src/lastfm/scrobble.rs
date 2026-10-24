//! Authenticated Last.fm writes and durable, account-bound background delivery.
//! Secrets never enter prefs_get_all, plugin storage, or the webview response.
use crate::{db::Db, model::Track};
use anyhow::{bail, Context, Result};
use md5::{Digest, Md5};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::Write,
    path::PathBuf,
    sync::{
        mpsc::{sync_channel, SyncSender},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Credentials {
    api_key: String,
    secret: String,
    session: String,
    username: String,
    // Persisted in SQLite, never in the credential file.
    #[serde(skip)]
    enabled: bool,
}

#[derive(Default)]
struct Config {
    credentials: Credentials,
    token: Option<(String, i64)>,
    generation: u64,
    error: Option<String>,
    /// Failed deliveries per outbox row in this run that were not outages.
    /// Kept in memory: a row reaching `MAX_ITEM_ATTEMPTS` is marked failed,
    /// and a restart only grants the rest another few tries.
    attempts: std::collections::HashMap<i64, u32>,
}

/// Deliveries of one scrobble that may fail for reasons of its own before it
/// is given up on.
const MAX_ITEM_ATTEMPTS: u32 = 5;

#[derive(Serialize)]
pub struct ScrobbleStatus {
    pub username: Option<String>,
    pub enabled: bool,
    pub pending: i64,
    pub error: Option<String>,
    pub authorizing: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
struct ApiError {
    code: i64,
    message: String,
}

/// Last.fm as a whole could not take the request: unreachable, a server
/// error, or rate limiting. Delivery stops and retries later, without holding
/// it against the scrobble that happened to be first.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct Outage(&'static str);

fn is_outage_status(status: u16) -> bool {
    status == 429 || status >= 500
}

fn is_outage(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Outage>().is_some()
        || error
            .downcast_ref::<ApiError>()
            .is_some_and(|e| matches!(e.code, 11 | 16 | 29))
}

/// A response body that is not JSON: an outage when the status says so,
/// otherwise a rejection of this one request (a proxy's 4xx page, say).
fn unreadable(status: u16) -> anyhow::Error {
    if is_outage_status(status) {
        Outage("Last.fm returned an unreadable response. Will retry.").into()
    } else {
        anyhow::anyhow!("Last.fm returned an unreadable response (HTTP {status}).")
    }
}

fn api_result(status: u16, value: Value) -> Result<Value> {
    if let Some(code) = super::int_of(&value["error"]) {
        // Do not echo arbitrary server text: it can contain submitted secrets.
        let message = match code {
            9 => "Last.fm authorization expired. Reconnect your account.",
            10 | 26 => "Last.fm rejected the API key.",
            13 => "Last.fm rejected the signature. Check the API shared secret.",
            14 | 15 => "Authorize Crescendo in your browser, then try again.",
            11 | 16 | 29 => "Last.fm is temporarily unavailable or rate limited. Will retry.",
            6 | 7 => "Last.fm could not find the requested track or resource.",
            _ => "Last.fm rejected the request.",
        };
        return Err(ApiError {
            code,
            message: message.into(),
        }
        .into());
    }
    if is_outage_status(status) {
        return Err(Outage("Last.fm is unavailable. Will retry.").into());
    }
    if !(200..300).contains(&status) {
        bail!("Last.fm returned HTTP {status}.");
    }
    Ok(value)
}

fn signature(params: &BTreeMap<String, String>, secret: &str) -> String {
    let mut digest = Md5::new();
    for (key, value) in params {
        if key != "format" && key != "callback" && key != "api_sig" {
            digest.update(key.as_bytes());
            digest.update(value.as_bytes());
        }
    }
    digest.update(secret.as_bytes());
    format!("{:x}", digest.finalize())
}

fn signed_call(
    endpoint: &str,
    c: &Credentials,
    method: &str,
    mut params: BTreeMap<String, String>,
) -> Result<Value> {
    super::pace_requests();
    params.insert("api_key".into(), c.api_key.clone());
    params.insert("method".into(), method.into());
    if !c.session.is_empty() {
        params.insert("sk".into(), c.session.clone());
    }
    params.insert("api_sig".into(), signature(&params, &c.secret));
    params.insert("format".into(), "json".into());
    let mut response = ureq::post(endpoint)
        .config()
        .timeout_global(Some(Duration::from_secs(15)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .send_form(&params)
        .map_err(|_| Outage("Cannot reach Last.fm. Will retry."))?;
    let status = response.status().as_u16();
    let value: Value =
        crate::http::read_json(response.body_mut()).map_err(|_| unreadable(status))?;
    api_result(status, value)
}

pub fn valid_track_url(raw: &str) -> Option<String> {
    let mut url = url::Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("www.last.fm" | "last.fm"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !url.path().starts_with("/music/")
    {
        return None;
    }
    url.set_scheme("https").ok()?;
    Some(url.into())
}

/// Catalogue recognition uses original tags with autocorrection disabled.
/// Network/authentication failures are never cached as a missing track.
pub fn track_url(db: &Db, api_key: &str, track: &Track) -> Result<Option<String>> {
    track_url_at(super::API_ROOT, db, api_key, track)
}

fn track_url_at(endpoint: &str, db: &Db, api_key: &str, track: &Track) -> Result<Option<String>> {
    let (Some(artist), Some(title)) = (track.artist.as_deref(), track.title.as_deref()) else {
        return Ok(None);
    };
    if artist.trim().is_empty() || title.trim().is_empty() || api_key.is_empty() {
        return Ok(None);
    }
    let cache_key = format!("track:{}", serde_json::to_string(&(artist, title))?);
    if let Some(raw) = db.lastfm_cache_get(&cache_key, super::CACHE_TTL_SECS)? {
        let url: Option<String> = serde_json::from_str(&raw)?;
        return Ok(url.and_then(|u| valid_track_url(&u)));
    }
    super::pace_requests();
    let mut response = ureq::get(endpoint)
        .query("method", "track.getInfo")
        .query("api_key", api_key)
        .query("artist", artist)
        .query("track", title)
        .query("autocorrect", "0")
        .query("format", "json")
        .config()
        .timeout_global(Some(Duration::from_secs(10)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|_| Outage("Cannot reach Last.fm. Will retry."))?;
    let status = response.status().as_u16();
    let value: Value =
        crate::http::read_json(response.body_mut()).map_err(|_| unreadable(status))?;
    let found = match api_result(status, value) {
        Ok(value) => {
            let t = &value["track"];
            if t["name"].as_str().is_none() {
                bail!("Last.fm returned no track data.")
            }
            t["url"].as_str().and_then(valid_track_url)
        }
        Err(e)
            if e.downcast_ref::<ApiError>()
                .is_some_and(|e| matches!(e.code, 6 | 7)) =>
        {
            None
        }
        Err(e) => return Err(e),
    };
    db.lastfm_cache_put(&cache_key, &serde_json::to_string(&found)?)?;
    Ok(found)
}

pub struct Scrobbler {
    db: Arc<Db>,
    endpoint: String,
    path: PathBuf,
    config: Mutex<Config>,
    /// Serializes account changes with submissions, without blocking the player.
    operation: Mutex<()>,
    wake: SyncSender<()>,
}

impl Scrobbler {
    pub fn start(db: Arc<Db>, path: PathBuf) -> Result<Arc<Self>> {
        let loaded = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Credentials>(&bytes).map_err(anyhow::Error::from),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Credentials::default()),
            Err(e) => Err(e.into()),
        };
        let error = loaded
            .as_ref()
            .err()
            .map(|_| "Could not read Last.fm credentials. Reconnect your account.".to_string());
        let mut credentials = loaded.unwrap_or_default();
        credentials.enabled = !credentials.session.is_empty()
            && db.prefs_get("lastfm_scrobbling_enabled")?.as_deref() == Some("true");
        let (wake, rx) = sync_channel(1);
        let service = Arc::new(Self {
            db,
            path,
            endpoint: super::API_ROOT.into(),
            config: Mutex::new(Config {
                credentials,
                error,
                ..Default::default()
            }),
            operation: Mutex::new(()),
            wake,
        });
        let weak = Arc::downgrade(&service);
        std::thread::Builder::new()
            .name("lastfm-scrobbler".into())
            .spawn(move || loop {
                let Some(service) = weak.upgrade() else { break };
                if let Err(e) = service.deliver() {
                    service.config.lock().unwrap().error = Some(e.to_string());
                }
                drop(service);
                if matches!(
                    rx.recv_timeout(Duration::from_secs(60)),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
                ) {
                    break;
                }
            })?;
        Ok(service)
    }

    fn save(&self, credentials: &Credentials) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        // A previous interrupted write may have left this private temp file.
        match std::fs::remove_file(&tmp) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let mut file = options.open(&tmp)?;
        file.write_all(&serde_json::to_vec(credentials)?)?;
        file.sync_all()?;
        std::fs::rename(tmp, &self.path)?;
        self.db.prefs_set(
            "lastfm_scrobbling_enabled",
            if credentials.enabled { "true" } else { "false" },
        )?;
        Ok(())
    }

    pub fn status(&self) -> Result<ScrobbleStatus> {
        let config = self.config.lock().unwrap();
        let pending = self.db.with(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM lastfm_outbox WHERE state = 'pending' AND account = ?1",
                [&config.credentials.username],
                |r| r.get(0),
            )?)
        })?;
        Ok(ScrobbleStatus {
            username: (!config.credentials.username.is_empty())
                .then(|| config.credentials.username.clone()),
            enabled: config.credentials.enabled,
            pending,
            error: config.error.clone(),
            authorizing: config
                .token
                .as_ref()
                .is_some_and(|(_, issued)| now() - issued < 3600),
        })
    }

    pub fn connect(&self, api_key: String, secret: String) -> Result<String> {
        if [&api_key, &secret]
            .iter()
            .any(|s| s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            bail!("Enter the 32-character Last.fm API key and shared secret.");
        }
        let _operation = self.operation.lock().unwrap();
        let credentials = Credentials {
            api_key,
            secret,
            ..Default::default()
        };
        let result = signed_call(
            &self.endpoint,
            &credentials,
            "auth.getToken",
            BTreeMap::new(),
        )?;
        let token = result["token"]
            .as_str()
            .context("Last.fm returned no authorization token")?
            .to_string();
        self.save(&credentials)?;
        let mut url = url::Url::parse("https://www.last.fm/api/auth/")?;
        url.query_pairs_mut()
            .append_pair("api_key", &credentials.api_key)
            .append_pair("token", &token);
        let mut config = self.config.lock().unwrap();
        config.credentials = credentials;
        config.token = Some((token, now()));
        config.generation += 1;
        config.error = None;
        Ok(url.into())
    }

    pub fn finish_connect(&self) -> Result<()> {
        let _operation = self.operation.lock().unwrap();
        let (mut credentials, token) = {
            let config = self.config.lock().unwrap();
            let (token, issued) = config
                .token
                .clone()
                .context("Start a new Last.fm connection first.")?;
            if now() - issued >= 3600 {
                bail!("Authorization timed out. Connect Last.fm again.")
            }
            (config.credentials.clone(), token)
        };
        let result = signed_call(
            &self.endpoint,
            &credentials,
            "auth.getSession",
            BTreeMap::from([("token".into(), token)]),
        )?;
        credentials.session = result["session"]["key"]
            .as_str()
            .filter(|s| !s.is_empty())
            .context("Last.fm returned no session")?
            .into();
        credentials.username = result["session"]["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .context("Last.fm returned no account")?
            .into();
        // Connecting does not silently opt in; the checkbox is explicit.
        self.save(&credentials)?;
        // Preserve pending plays only when reconnecting the same account.
        self.db.with(|conn| {
            conn.execute(
                "DELETE FROM lastfm_outbox WHERE state = 'pending' AND account != ?1",
                [&credentials.username],
            )?;
            Ok(())
        })?;
        let mut config = self.config.lock().unwrap();
        config.credentials = credentials;
        config.token = None;
        config.generation += 1;
        config.error = None;
        Ok(())
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        let _operation = self.operation.lock().unwrap();
        let mut config = self.config.lock().unwrap();
        if enabled && config.credentials.session.is_empty() {
            bail!("Connect Last.fm first.")
        }
        let mut credentials = config.credentials.clone();
        credentials.enabled = enabled;
        self.save(&credentials)?;
        if !enabled {
            self.clear_pending()?;
        }
        config.credentials = credentials;
        config.generation += 1;
        config.error = None;
        let _ = self.wake.try_send(());
        Ok(())
    }

    pub fn disconnect(&self) -> Result<()> {
        let _operation = self.operation.lock().unwrap();
        self.save(&Credentials::default())?;
        self.clear_pending()?;
        let mut config = self.config.lock().unwrap();
        let generation = config.generation + 1;
        *config = Config {
            generation,
            ..Default::default()
        };
        Ok(())
    }

    fn clear_pending(&self) -> Result<()> {
        self.db.with(|conn| {
            conn.execute("DELETE FROM lastfm_outbox WHERE state = 'pending'", [])?;
            Ok(())
        })
    }

    /// Captured when a stream starts; enabling midway cannot upload earlier listening.
    pub fn generation(&self) -> Option<u64> {
        let c = self.config.lock().unwrap();
        (c.credentials.enabled && !c.credentials.session.is_empty()).then_some(c.generation)
    }

    pub fn enqueue(&self, track: &Track, started_at: i64, generation: Option<u64>) -> Result<()> {
        let config = self.config.lock().unwrap();
        if !config.credentials.enabled || generation != Some(config.generation) {
            return Ok(());
        }
        if track.artist.as_deref().is_none_or(|s| s.trim().is_empty())
            || track.title.as_deref().is_none_or(|s| s.trim().is_empty())
        {
            return Ok(());
        }
        self.db.with(|conn| {
            conn.execute("INSERT OR IGNORE INTO lastfm_outbox(account, track_json, started_at) VALUES (?1, ?2, ?3)",
                params![config.credentials.username, serde_json::to_string(track)?, started_at])?;
            Ok(())
        })?;
        let _ = self.wake.try_send(());
        Ok(())
    }

    fn mark(&self, id: i64, state: &str, message: Option<&str>) -> Result<()> {
        self.db.with(|conn| {
            conn.execute("UPDATE lastfm_outbox SET state = ?2, message = ?3 WHERE id = ?1", params![id, state, message])?;
            // Retain only recent delivery diagnostics; song history is separate.
            conn.execute("DELETE FROM lastfm_outbox WHERE state != 'pending' AND id < (SELECT COALESCE(MAX(id),0)-1000 FROM lastfm_outbox)", [])?;
            Ok(())
        })
    }

    fn deliver(&self) -> Result<()> {
        // One batch is bounded; wakeups coalesce, and outages retry once a
        // minute. A scrobble that fails on its own account does not stop the
        // batch, so it cannot hold back the ones queued after it.
        let account = {
            let config = self.config.lock().unwrap();
            config.credentials.username.clone()
        };
        let batch: Vec<(i64, String, i64)> = self.db.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, track_json, started_at FROM lastfm_outbox \
                 WHERE state = 'pending' AND account = ?1 ORDER BY id LIMIT 50",
            )?;
            let rows = stmt
                .query_map([&account], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })?;
        for (id, raw, started_at) in batch {
            let _operation = self.operation.lock().unwrap();
            let credentials = self.config.lock().unwrap().credentials.clone();
            // An account change while the batch ran clears or rebinds the
            // rows fetched above.
            if !credentials.enabled
                || credentials.session.is_empty()
                || credentials.username != account
            {
                return Ok(());
            }
            if now() - started_at > 14 * 86400 {
                self.mark(id, "skipped", Some("Too old for Last.fm"))?;
                continue;
            }
            let Ok(track) = serde_json::from_str::<Track>(&raw) else {
                self.mark(id, "failed", Some("The queued play could not be read."))?;
                continue;
            };
            // The library deliberately fills absent titles from filenames. Verify
            // tagged metadata on the worker before making any write request.
            let tagged =
                crate::scanner::tags::read_scrobble_meta(std::path::Path::new(&track.path));
            if !tagged.is_ok_and(|meta| {
                meta.title.is_some()
                    && meta.artist.is_some()
                    && meta.title == track.title
                    && meta.artist == track.artist
            }) {
                let message = "Missing, changed, or unreadable artist/title tags; not submitted.";
                self.mark(id, "skipped", Some(message))?;
                self.config.lock().unwrap().error = Some(message.into());
                continue;
            }
            let result = self.submit(&credentials, &track, started_at);
            match result {
                Ok((state, message)) => {
                    self.mark(id, state, message.as_deref())?;
                    let mut config = self.config.lock().unwrap();
                    config.attempts.remove(&id);
                    config.error = message;
                }
                // A local database error is no fault of the scrobble either.
                Err(e) if is_outage(&e) || e.downcast_ref::<rusqlite::Error>().is_some() => {
                    return Err(e)
                }
                Err(e) => {
                    if let Some(api) = e.downcast_ref::<ApiError>() {
                        if matches!(api.code, 9 | 10 | 13 | 26) {
                            let mut config = self.config.lock().unwrap();
                            let mut c = config.credentials.clone();
                            c.enabled = false;
                            self.save(&c)?;
                            config.credentials = c;
                            config.generation += 1;
                            return Err(e);
                        }
                        self.mark(id, "failed", Some(&e.to_string()))?;
                        self.config.lock().unwrap().error = Some(e.to_string());
                        continue;
                    }
                    // Not an outage and not a verdict from the API: retry on
                    // later wakes, then give up on this one scrobble.
                    let give_up = {
                        let mut config = self.config.lock().unwrap();
                        config.error = Some(e.to_string());
                        let attempts = config.attempts.entry(id).or_insert(0);
                        *attempts += 1;
                        let give_up = *attempts >= MAX_ITEM_ATTEMPTS;
                        if give_up {
                            config.attempts.remove(&id);
                        }
                        give_up
                    };
                    if give_up {
                        self.mark(id, "failed", Some(&e.to_string()))?;
                    }
                }
            }
        }
        Ok(())
    }

    fn submit(
        &self,
        credentials: &Credentials,
        track: &Track,
        started_at: i64,
    ) -> Result<(&'static str, Option<String>)> {
        if track_url_at(&self.endpoint, &self.db, &credentials.api_key, track)?.is_none() {
            return Ok((
                "skipped",
                Some("Track not found on Last.fm; not submitted.".into()),
            ));
        }
        let mut params = BTreeMap::from([
            ("artist".into(), track.artist.clone().unwrap_or_default()),
            ("track".into(), track.title.clone().unwrap_or_default()),
            ("timestamp".into(), started_at.to_string()),
            ("duration".into(), (track.duration_ms / 1000).to_string()),
        ]);
        if let Some(album) = &track.album {
            params.insert("album".into(), album.clone());
        }
        if let Some(artist) = &track.albumartist {
            params.insert("albumArtist".into(), artist.clone());
        }
        let result = signed_call(&self.endpoint, credentials, "track.scrobble", params)?;
        submission_result(&result)
    }
}

fn submission_result(result: &Value) -> Result<(&'static str, Option<String>)> {
    let attr = &result["scrobbles"]["@attr"];
    if super::int_of(&attr["accepted"]) == Some(1) {
        Ok(("submitted", None))
    } else if super::int_of(&attr["ignored"]) == Some(1) {
        Ok((
            "ignored",
            Some("Last.fm ignored this scrobble (metadata, timestamp, or daily limit).".into()),
        ))
    } else {
        bail!("Last.fm did not confirm the scrobble. Will retry.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn fixture() -> (tempfile::TempDir, Scrobbler, Track) {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(&dir.path().join("library.db")).unwrap());
        db.with(|c| {
            c.execute("INSERT INTO tracks(id,path,dir,filename,mtime,size,format,title,artist,albumartist,duration_ms) VALUES (1,'/music/song.flac','/music','song.flac',0,0,'FLAC','夜の歌','歌手','アルバム歌手',300000)", [])?;
            Ok(())
        }).unwrap();
        let track = db.track_by_id(1).unwrap().unwrap();
        let (wake, _rx) = sync_channel(1);
        let service = Scrobbler {
            db,
            path: dir.path().join("credentials.json"),
            endpoint: super::super::API_ROOT.into(),
            config: Mutex::new(Config {
                credentials: Credentials {
                    api_key: "a".repeat(32),
                    secret: "b".repeat(32),
                    session: "test-session".into(),
                    username: "alice".into(),
                    enabled: true,
                },
                ..Default::default()
            }),
            operation: Mutex::new(()),
            wake,
        };
        (dir, service, track)
    }

    /// No real account is contacted by these tests. Capture a single request.
    fn server(status: u16, value: Value) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let header_end;
            loop {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
                if bytes.ends_with(b"\r\n\r\n") {
                    header_end = bytes.len();
                    break;
                }
            }
            let headers = String::from_utf8(bytes.clone()).unwrap();
            let length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|s| s.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            bytes.resize(header_end + length, 0);
            socket.read_exact(&mut bytes[header_end..]).unwrap();
            let body = value.to_string();
            write!(socket, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            String::from_utf8(bytes).unwrap()
        });
        (url, thread)
    }

    #[test]
    fn signed_posts_preserve_unicode_and_start_time() {
        let (_dir, mut service, track) = fixture();
        let cache = format!(
            "track:{}",
            serde_json::to_string(&("歌手", "夜の歌")).unwrap()
        );
        service
            .db
            .lastfm_cache_put(&cache, r#""https://www.last.fm/music/Artist/_/Song""#)
            .unwrap();
        let (endpoint, request) = server(
            200,
            serde_json::json!({"scrobbles":{"@attr":{"accepted":"1","ignored":"0"}}}),
        );
        service.endpoint = endpoint;
        let c = service.config.lock().unwrap().credentials.clone();
        assert_eq!(
            service.submit(&c, &track, 1700000000).unwrap().0,
            "submitted"
        );
        let request = request.join().unwrap();
        assert!(request.starts_with("POST / HTTP/1.1"));
        let body = request.split("\r\n\r\n").nth(1).unwrap();
        let mut p: BTreeMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect();
        assert_eq!(p["artist"], "歌手");
        assert_eq!(p["track"], "夜の歌");
        assert_eq!(p["albumArtist"], "アルバム歌手");
        assert_eq!(p["timestamp"], "1700000000");
        assert_eq!(p["sk"], "test-session");
        assert_eq!(p.remove("api_sig").unwrap(), signature(&p, &c.secret));
        assert!(!body.contains(&c.secret));
    }

    #[test]
    fn auth_signature_has_known_md5_and_ignores_format() {
        let mut p = BTreeMap::from([
            ("method".into(), "auth.getSession".into()),
            ("api_key".into(), "key".into()),
            ("token".into(), "token".into()),
        ]);
        assert_eq!(signature(&p, "secret"), "9ac306496295a8866c4a8673395540eb");
        p.insert("format".into(), "json".into());
        assert_eq!(signature(&p, "secret"), "9ac306496295a8866c4a8673395540eb");
    }

    #[test]
    fn lookup_does_not_cache_temporary_errors_as_absence() {
        let (_dir, service, track) = fixture();
        let (endpoint, request) = server(503, serde_json::json!({"error":11}));
        assert!(track_url_at(&endpoint, &service.db, "key", &track).is_err());
        request.join().unwrap();
        let (endpoint, request) = server(
            200,
            serde_json::json!({"track":{"name":"夜の歌","url":"http://www.last.fm/music/Artist/_/Song"}}),
        );
        assert_eq!(
            track_url_at(&endpoint, &service.db, "key", &track)
                .unwrap()
                .as_deref(),
            Some("https://www.last.fm/music/Artist/_/Song")
        );
        request.join().unwrap();
        // A third call uses the cache even with an unreachable endpoint.
        assert!(
            track_url_at("http://127.0.0.1:1/", &service.db, "key", &track)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn missing_track_is_cached_and_never_submitted() {
        let (_dir, mut service, track) = fixture();
        let (endpoint, request) = server(200, serde_json::json!({"error":6}));
        service.endpoint = endpoint;
        let c = service.config.lock().unwrap().credentials.clone();
        assert_eq!(service.submit(&c, &track, now()).unwrap().0, "skipped");
        assert!(request.join().unwrap().starts_with("GET /?"));
        assert_eq!(service.submit(&c, &track, now()).unwrap().0, "skipped");
    }

    #[test]
    fn http_200_ignored_is_not_reported_as_submitted() {
        assert_eq!(
            submission_result(
                &serde_json::json!({"scrobbles":{"@attr":{"accepted":"0","ignored":"1"}}})
            )
            .unwrap()
            .0,
            "ignored"
        );
        assert!(submission_result(&serde_json::json!({})).is_err());
        assert_eq!(
            api_result(400, serde_json::json!({"error":9}))
                .unwrap_err()
                .downcast_ref::<ApiError>()
                .unwrap()
                .code,
            9
        );
        assert!(api_result(500, serde_json::json!({})).is_err());
    }

    #[test]
    fn disabling_and_changing_accounts_cannot_send_old_listening() {
        let (_dir, service, track) = fixture();
        let generation = service.generation();
        service.enqueue(&track, now(), generation).unwrap();
        service.enqueue(&track, now(), generation).unwrap();
        assert_eq!(service.status().unwrap().pending, 1);
        service.set_enabled(false).unwrap();
        assert_eq!(service.status().unwrap().pending, 0);
        service.set_enabled(true).unwrap();
        service.enqueue(&track, now(), generation).unwrap();
        assert_eq!(service.status().unwrap().pending, 0);
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();
        service.disconnect().unwrap();
        assert!(service.status().unwrap().username.is_none());
        assert_eq!(service.status().unwrap().pending, 0);
        assert!(service.set_enabled(true).is_err());
    }

    #[test]
    fn pending_queue_survives_reopening_database_and_is_account_bound() {
        let (dir, service, track) = fixture();
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();
        let db = Db::open(&dir.path().join("library.db")).unwrap();
        let account: String = db
            .with(|c| {
                Ok(c.query_row(
                    "SELECT account FROM lastfm_outbox WHERE state='pending'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(account, "alice");
        service.config.lock().unwrap().credentials.username = "bob".into();
        service.deliver().unwrap(); // no network: Alice's play is not Bob's
        assert_eq!(service.status().unwrap().pending, 0);
    }

    #[test]
    fn secrets_are_private_and_absent_from_status_and_preferences() {
        let (_dir, service, _track) = fixture();
        let c = service.config.lock().unwrap().credentials.clone();
        service.save(&c).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&service.path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let status = serde_json::to_string(&service.status().unwrap()).unwrap();
        assert!(!status.contains(&c.session));
        assert!(!status.contains(&c.secret));
        let prefs = serde_json::to_string(&service.db.prefs_get_all().unwrap()).unwrap();
        assert!(!prefs.contains(&c.secret));
        assert!(!prefs.contains(&c.session));
    }

    #[test]
    fn reconnect_preserves_only_the_same_accounts_pending_plays() {
        for username in ["alice", "bob"] {
            let (_dir, mut service, track) = fixture();
            service
                .enqueue(&track, now(), service.generation())
                .unwrap();
            let (endpoint, request) = server(200, serde_json::json!({"token":"auth-token"}));
            service.endpoint = endpoint;
            let url = service.connect("a".repeat(32), "b".repeat(32)).unwrap();
            assert!(url.starts_with("https://www.last.fm/api/auth/"));
            assert!(url.contains("token=auth-token"));
            request.join().unwrap();
            assert!(!service.status().unwrap().enabled);
            assert!(service.status().unwrap().authorizing);
            let (endpoint, request) = server(
                200,
                serde_json::json!({"session":{"name":username,"key":"new-session"}}),
            );
            service.endpoint = endpoint;
            service.finish_connect().unwrap();
            let request = request.join().unwrap();
            assert!(request.contains("token=auth-token"));
            assert!(!request.contains("sk=test-session"));
            let status = service.status().unwrap();
            assert_eq!(status.username.as_deref(), Some(username));
            assert!(!status.enabled);
            assert!(!status.authorizing);
            assert_eq!(status.pending, if username == "alice" { 1 } else { 0 });
        }
    }

    fn tagged_wav(dir: &std::path::Path, track: &mut Track) {
        use lofty::{
            prelude::{Accessor, TagExt},
            tag::{Tag, TagType},
        };
        let path = dir.join("tagged.wav");
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&8036u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.resize(8044, 128);
        std::fs::write(&path, wav).unwrap();
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title(track.title.clone().unwrap());
        tag.set_artist(track.artist.clone().unwrap());
        tag.save_to_path(&path, lofty::config::WriteOptions::default())
            .unwrap();
        track.path = path.to_string_lossy().into_owned();
    }

    #[test]
    fn delivery_retries_outages_but_pauses_after_revoked_authorization() {
        let (dir, mut service, mut track) = fixture();
        tagged_wav(dir.path(), &mut track);
        let cache = format!(
            "track:{}",
            serde_json::to_string(&("歌手", "夜の歌")).unwrap()
        );
        service
            .db
            .lastfm_cache_put(&cache, r#""https://www.last.fm/music/Artist/_/Song""#)
            .unwrap();
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();
        let (endpoint, request) = server(503, serde_json::json!({"error":11}));
        service.endpoint = endpoint;
        assert!(service.deliver().is_err());
        request.join().unwrap();
        assert_eq!(service.status().unwrap().pending, 1);
        assert!(service.status().unwrap().enabled);
        let (endpoint, request) = server(
            200,
            serde_json::json!({"scrobbles":{"@attr":{"accepted":"1","ignored":"0"}}}),
        );
        service.endpoint = endpoint;
        service.deliver().unwrap();
        request.join().unwrap();
        assert_eq!(service.status().unwrap().pending, 0);
        service
            .enqueue(&track, now() + 1, service.generation())
            .unwrap();
        let (endpoint, request) = server(400, serde_json::json!({"error":9}));
        service.endpoint = endpoint;
        assert!(service.deliver().is_err());
        request.join().unwrap();
        assert_eq!(service.status().unwrap().pending, 1);
        assert!(!service.status().unwrap().enabled);
        assert_eq!(
            service
                .db
                .prefs_get("lastfm_scrobbling_enabled")
                .unwrap()
                .as_deref(),
            Some("false")
        );
    }

    /// Answer one request per response, in order, on the same address.
    fn servers(responses: Vec<(u16, Value)>) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
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
                let body = value.to_string();
                write!(socket, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        (url, thread)
    }

    fn outbox_states(service: &Scrobbler) -> Vec<String> {
        service
            .db
            .with(|c| {
                let mut stmt = c.prepare("SELECT state FROM lastfm_outbox ORDER BY id")?;
                let states = stmt
                    .query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(states)
            })
            .unwrap()
    }

    /// A scrobble Last.fm keeps refusing used to be retried first, forever,
    /// with every later play queued behind it until all were too old.
    #[test]
    fn a_failing_scrobble_does_not_hold_back_later_ones() {
        let (dir, mut service, mut track) = fixture();
        tagged_wav(dir.path(), &mut track);
        let cache = format!(
            "track:{}",
            serde_json::to_string(&("歌手", "夜の歌")).unwrap()
        );
        service
            .db
            .lastfm_cache_put(&cache, r#""https://www.last.fm/music/Artist/_/Song""#)
            .unwrap();
        service
            .enqueue(&track, now() - 10, service.generation())
            .unwrap();
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();

        let unconfirmed = serde_json::json!({"scrobbles":{"@attr":{"accepted":"0","ignored":"0"}}});
        let accepted = serde_json::json!({"scrobbles":{"@attr":{"accepted":"1","ignored":"0"}}});
        let (endpoint, requests) = servers(vec![(200, unconfirmed.clone()), (200, accepted)]);
        service.endpoint = endpoint;
        service.deliver().unwrap();
        requests.join().unwrap();
        assert_eq!(outbox_states(&service), ["pending", "submitted"]);

        // Each later wake retries it, until it is given up on.
        for attempt in 2..=MAX_ITEM_ATTEMPTS {
            let (endpoint, requests) = servers(vec![(200, unconfirmed.clone())]);
            service.endpoint = endpoint;
            service.deliver().unwrap();
            requests.join().unwrap();
            let expected = if attempt < MAX_ITEM_ATTEMPTS {
                "pending"
            } else {
                "failed"
            };
            assert_eq!(outbox_states(&service)[0], expected);
        }
        assert_eq!(service.status().unwrap().pending, 0);
    }

    /// An outage is no fault of the first scrobble and never counts against it.
    #[test]
    fn outages_stop_the_batch_without_counting_against_a_scrobble() {
        let (dir, mut service, mut track) = fixture();
        tagged_wav(dir.path(), &mut track);
        let cache = format!(
            "track:{}",
            serde_json::to_string(&("歌手", "夜の歌")).unwrap()
        );
        service
            .db
            .lastfm_cache_put(&cache, r#""https://www.last.fm/music/Artist/_/Song""#)
            .unwrap();
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();
        for _ in 0..MAX_ITEM_ATTEMPTS + 1 {
            let (endpoint, requests) = servers(vec![(502, serde_json::json!("bad gateway"))]);
            service.endpoint = endpoint;
            assert!(service.deliver().is_err());
            requests.join().unwrap();
        }
        assert_eq!(outbox_states(&service), ["pending"]);
    }

    #[test]
    fn rows_too_old_or_unreadable_are_settled_before_parsing() {
        let (_dir, service, _track) = fixture();
        service
            .db
            .with(|c| {
                c.execute(
                    "INSERT INTO lastfm_outbox(account, track_json, started_at) VALUES \
                     ('alice', '{\"older format\": true}', ?1), \
                     ('alice', '{\"older format\": true}', ?2)",
                    params![now() - 15 * 86400, now()],
                )?;
                Ok(())
            })
            .unwrap();
        service.deliver().unwrap();
        assert_eq!(outbox_states(&service), ["skipped", "failed"]);
    }

    #[test]
    fn unreadable_credential_file_does_not_break_playback_startup() {
        let (dir, service, _track) = fixture();
        let path = dir.path().join("broken.json");
        std::fs::write(&path, "broken").unwrap();
        let restarted = Scrobbler::start(service.db.clone(), path).unwrap();
        let status = restarted.status().unwrap();
        assert!(!status.enabled);
        assert!(status.error.is_some());
    }

    #[test]
    fn track_links_are_restricted_to_lastfm() {
        for url in [
            "https://evil.test/music/A",
            "https://www.last.fm.evil.test/music/A",
            "https://a@www.last.fm/music/A",
            "file:///music/A",
            "https://www.last.fm/login",
            "https://www.last.fm:444/music/A",
        ] {
            assert!(valid_track_url(url).is_none(), "{url}");
        }
        assert_eq!(
            valid_track_url("http://last.fm/music/A/_/B").as_deref(),
            Some("https://last.fm/music/A/_/B")
        );
    }
}
