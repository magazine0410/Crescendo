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

/// Whether plays are being sent for the connected account.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Scrobbling {
    #[default]
    Off,
    On,
    /// Last.fm rejected the session. Unlike Off, this was not the user's choice,
    /// so waiting plays are kept for a reconnect of the same account.
    Suspended,
}

impl Scrobbling {
    fn from_pref(value: Option<&str>) -> Self {
        match value {
            Some("true") => Scrobbling::On,
            Some("suspended") => Scrobbling::Suspended,
            _ => Scrobbling::Off,
        }
    }

    fn as_pref(self) -> &'static str {
        match self {
            Scrobbling::Off => "false",
            Scrobbling::On => "true",
            Scrobbling::Suspended => "suspended",
        }
    }
}

const SESSION_REJECTED: &str =
    "Last.fm rejected this session. Reconnect to resume scrobbling; waiting plays are kept.";

/// Where one queued play stands. Pending is the only state a row can leave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Settled {
    Submitted,
    Ignored,
    Skipped,
    Failed,
}

impl Settled {
    fn as_str(self) -> &'static str {
        match self {
            Settled::Submitted => "submitted",
            Settled::Ignored => "ignored",
            Settled::Skipped => "skipped",
            Settled::Failed => "failed",
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Credentials {
    api_key: String,
    secret: String,
    session: String,
    username: String,
    // Persisted in SQLite, never in the credential file.
    #[serde(skip)]
    scrobbling: Scrobbling,
}

/// A connection the user has started in the browser. It stays beside the current
/// account until Last.fm issues a session, so abandoning it loses nothing, and it
/// is never saved: a restart simply means starting the authorization again.
struct Authorization {
    api_key: String,
    secret: String,
    token: String,
    issued: i64,
}

impl Authorization {
    fn expired(&self) -> bool {
        now() - self.issued >= 3600
    }
}

#[derive(Default)]
struct Config {
    credentials: Credentials,
    authorization: Option<Authorization>,
    generation: u64,
    error: Option<String>,
}

/// Queued plays as read: id, track JSON, start time, failed attempts.
type OutboxBatch = Vec<(i64, String, i64, u32)>;

/// Deliveries of one scrobble that may fail for reasons of its own before it
/// is given up on.
const MAX_ITEM_ATTEMPTS: u32 = 5;

#[derive(Serialize)]
pub struct ScrobbleStatus {
    pub username: Option<String>,
    pub enabled: bool,
    /// Last.fm rejected the session; reconnecting resumes sending.
    pub suspended: bool,
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
        if !credentials.session.is_empty() {
            credentials.scrobbling =
                Scrobbling::from_pref(db.prefs_get("lastfm_scrobbling_enabled")?.as_deref());
        }
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
        // The pref first: if the file then fails, this errs towards not sending.
        self.db.prefs_set(
            "lastfm_scrobbling_enabled",
            credentials.scrobbling.as_pref(),
        )?;
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
        let suspended = config.credentials.scrobbling == Scrobbling::Suspended;
        Ok(ScrobbleStatus {
            username: (!config.credentials.username.is_empty())
                .then(|| config.credentials.username.clone()),
            enabled: config.credentials.scrobbling == Scrobbling::On,
            suspended,
            pending,
            error: config
                .error
                .clone()
                .or_else(|| suspended.then(|| SESSION_REJECTED.to_string())),
            authorizing: config
                .authorization
                .as_ref()
                .is_some_and(|authorization| !authorization.expired()),
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
        let mut url = url::Url::parse("https://www.last.fm/api/auth/")?;
        url.query_pairs_mut()
            .append_pair("api_key", &credentials.api_key)
            .append_pair("token", &token);
        let mut config = self.config.lock().unwrap();
        config.authorization = Some(Authorization {
            api_key: credentials.api_key,
            secret: credentials.secret,
            token,
            issued: now(),
        });
        config.error = None;
        Ok(url.into())
    }

    /// Give up on a started connection. The current account is untouched.
    pub fn cancel_connect(&self) {
        let _operation = self.operation.lock().unwrap();
        let mut config = self.config.lock().unwrap();
        config.authorization = None;
        config.error = None;
    }

    pub fn finish_connect(&self) -> Result<()> {
        let _operation = self.operation.lock().unwrap();
        let (mut credentials, token) = {
            let mut config = self.config.lock().unwrap();
            let authorization = config
                .authorization
                .as_ref()
                .context("Start a new Last.fm connection first.")?;
            if authorization.expired() {
                config.authorization = None;
                bail!("Authorization timed out. Connect Last.fm again.")
            }
            (
                Credentials {
                    api_key: authorization.api_key.clone(),
                    secret: authorization.secret.clone(),
                    ..Default::default()
                },
                authorization.token.clone(),
            )
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
        let account = credentials.username.clone();
        {
            let mut config = self.config.lock().unwrap();
            config.credentials = credentials;
            config.authorization = None;
            config.generation += 1;
            config.error = None;
        }
        // Preserve pending plays only when reconnecting the same account.
        self.db.with(|conn| {
            conn.execute(
                "DELETE FROM lastfm_outbox WHERE state = 'pending' AND account != ?1",
                [&account],
            )?;
            Ok(())
        })
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        let _operation = self.operation.lock().unwrap();
        {
            let mut config = self.config.lock().unwrap();
            if enabled && config.credentials.session.is_empty() {
                bail!("Connect Last.fm first.")
            }
            let mut credentials = config.credentials.clone();
            credentials.scrobbling = if enabled {
                Scrobbling::On
            } else {
                Scrobbling::Off
            };
            self.save(&credentials)?;
            config.credentials = credentials;
            config.generation += 1;
            config.error = None;
        }
        if !enabled {
            self.clear_pending()?;
        }
        let _ = self.wake.try_send(());
        Ok(())
    }

    /// Memory follows the file before the queue is cleared, so a failure there
    /// cannot leave a disconnected account still sending until the next start.
    pub fn disconnect(&self) -> Result<()> {
        let _operation = self.operation.lock().unwrap();
        self.save(&Credentials::default())?;
        {
            let mut config = self.config.lock().unwrap();
            let generation = config.generation + 1;
            *config = Config {
                generation,
                ..Default::default()
            };
        }
        self.clear_pending()
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
        (c.credentials.scrobbling == Scrobbling::On).then_some(c.generation)
    }

    pub fn enqueue(&self, track: &Track, started_at: i64, generation: Option<u64>) -> Result<()> {
        let config = self.config.lock().unwrap();
        if config.credentials.scrobbling != Scrobbling::On || generation != Some(config.generation)
        {
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

    /// Settle a pending row. A row already settled or deleted stays as it is.
    fn mark(&self, id: i64, state: Settled, message: Option<&str>) -> Result<()> {
        self.db.with(|conn| {
            conn.execute(
                "UPDATE lastfm_outbox SET state = ?2, message = ?3 WHERE id = ?1 AND state = 'pending'",
                params![id, state.as_str(), message],
            )?;
            // Retain only recent delivery diagnostics; song history is separate.
            conn.execute("DELETE FROM lastfm_outbox WHERE state != 'pending' AND id < (SELECT COALESCE(MAX(id),0)-1000 FROM lastfm_outbox)", [])?;
            Ok(())
        })
    }

    fn note_attempt(&self, id: i64) -> Result<()> {
        self.db.with(|conn| {
            conn.execute(
                "UPDATE lastfm_outbox SET attempts = attempts + 1 WHERE id = ?1 AND state = 'pending'",
                [id],
            )?;
            Ok(())
        })
    }

    fn deliver(&self) -> Result<()> {
        let (generation, batch) = self.read_batch()?;
        self.deliver_batch(generation, batch)
    }

    /// One batch is bounded; wakeups coalesce, and outages retry once a minute.
    fn read_batch(&self) -> Result<(u64, OutboxBatch)> {
        let (account, generation) = {
            let config = self.config.lock().unwrap();
            (config.credentials.username.clone(), config.generation)
        };
        let batch = self.db.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, track_json, started_at, attempts FROM lastfm_outbox \
                 WHERE state = 'pending' AND account = ?1 ORDER BY id LIMIT 50",
            )?;
            let rows = stmt
                .query_map([&account], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })?;
        Ok((generation, batch))
    }

    /// A scrobble that fails on its own account does not stop the batch, so it
    /// cannot hold back the ones queued after it.
    fn deliver_batch(&self, generation: u64, batch: OutboxBatch) -> Result<()> {
        for (id, raw, started_at, attempts) in batch {
            let _operation = self.operation.lock().unwrap();
            let credentials = {
                let config = self.config.lock().unwrap();
                // Any account change since the batch was read may have deleted
                // or rebound its rows, even one the user has since undone.
                if config.generation != generation {
                    return Ok(());
                }
                config.credentials.clone()
            };
            if credentials.scrobbling != Scrobbling::On {
                return Ok(());
            }
            if now() - started_at > 14 * 86400 {
                self.mark(id, Settled::Skipped, Some("Too old for Last.fm"))?;
                continue;
            }
            let Ok(track) = serde_json::from_str::<Track>(&raw) else {
                self.mark(
                    id,
                    Settled::Failed,
                    Some("The queued play could not be read."),
                )?;
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
                self.mark(id, Settled::Skipped, Some(message))?;
                self.config.lock().unwrap().error = Some(message.into());
                continue;
            }
            let result = self.submit(&credentials, &track, started_at);
            match result {
                Ok((state, message)) => {
                    self.mark(id, state, message.as_deref())?;
                    self.config.lock().unwrap().error = message;
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
                            c.scrobbling = Scrobbling::Suspended;
                            self.save(&c)?;
                            config.credentials = c;
                            config.generation += 1;
                            return Err(e);
                        }
                        self.mark(id, Settled::Failed, Some(&e.to_string()))?;
                        self.config.lock().unwrap().error = Some(e.to_string());
                        continue;
                    }
                    // Not an outage and not a verdict from the API: retry on
                    // later wakes, then give up on this one scrobble.
                    self.config.lock().unwrap().error = Some(e.to_string());
                    if attempts + 1 >= MAX_ITEM_ATTEMPTS {
                        self.mark(id, Settled::Failed, Some(&e.to_string()))?;
                    } else {
                        self.note_attempt(id)?;
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
    ) -> Result<(Settled, Option<String>)> {
        if track_url_at(&self.endpoint, &self.db, &credentials.api_key, track)?.is_none() {
            return Ok((
                Settled::Skipped,
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

fn submission_result(result: &Value) -> Result<(Settled, Option<String>)> {
    let attr = &result["scrobbles"]["@attr"];
    if super::int_of(&attr["accepted"]) == Some(1) {
        Ok((Settled::Submitted, None))
    } else if super::int_of(&attr["ignored"]) == Some(1) {
        Ok((
            Settled::Ignored,
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
                    scrobbling: Scrobbling::On,
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
            Settled::Submitted
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
        assert_eq!(
            service.submit(&c, &track, now()).unwrap().0,
            Settled::Skipped
        );
        assert!(request.join().unwrap().starts_with("GET /?"));
        assert_eq!(
            service.submit(&c, &track, now()).unwrap().0,
            Settled::Skipped
        );
    }

    #[test]
    fn http_200_ignored_is_not_reported_as_submitted() {
        assert_eq!(
            submission_result(
                &serde_json::json!({"scrobbles":{"@attr":{"accepted":"0","ignored":"1"}}})
            )
            .unwrap()
            .0,
            Settled::Ignored
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
            // Until a session exists, the current account carries on as it was.
            let status = service.status().unwrap();
            assert!(status.authorizing);
            assert!(status.enabled);
            assert_eq!(status.username.as_deref(), Some("alice"));
            assert_eq!(status.pending, 1);
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
        let status = service.status().unwrap();
        assert_eq!(status.pending, 1);
        assert!(!status.enabled);
        assert!(status.suspended);
        assert_eq!(
            service
                .db
                .prefs_get("lastfm_scrobbling_enabled")
                .unwrap()
                .as_deref(),
            Some("suspended")
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
    fn abandoning_a_reconnect_leaves_the_account_and_its_plays_alone() {
        let (_dir, mut service, track) = fixture();
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();
        let generation = service.generation();
        let (endpoint, request) = server(200, serde_json::json!({"token":"auth-token"}));
        service.endpoint = endpoint;
        service.connect("c".repeat(32), "d".repeat(32)).unwrap();
        request.join().unwrap();
        assert_eq!(
            service.generation(),
            generation,
            "the stream keeps scrobbling"
        );

        service.cancel_connect();
        let status = service.status().unwrap();
        assert!(!status.authorizing);
        assert!(status.enabled);
        assert_eq!(status.username.as_deref(), Some("alice"));
        assert_eq!(status.pending, 1);
        assert_eq!(
            service.config.lock().unwrap().credentials.session,
            "test-session"
        );
        assert!(service.finish_connect().is_err(), "nothing left to finish");
    }

    #[test]
    fn a_reconnect_in_progress_is_not_saved() {
        let (dir, mut service, _track) = fixture();
        let (endpoint, request) = server(200, serde_json::json!({"token":"auth-token"}));
        service.endpoint = endpoint;
        service.connect("c".repeat(32), "d".repeat(32)).unwrap();
        request.join().unwrap();
        assert!(!dir.path().join("credentials.json").exists());
    }

    #[test]
    fn an_expired_authorization_is_dropped_when_it_is_used() {
        let (_dir, service, _track) = fixture();
        service.config.lock().unwrap().authorization = Some(Authorization {
            api_key: "c".repeat(32),
            secret: "d".repeat(32),
            token: "old".into(),
            issued: now() - 3600,
        });
        assert!(!service.status().unwrap().authorizing);
        let refused = service.finish_connect().unwrap_err();
        assert!(refused.to_string().contains("timed out"));
        assert!(service.config.lock().unwrap().authorization.is_none());
    }

    #[test]
    fn a_rejected_session_stays_apart_from_turning_scrobbling_off() {
        let (dir, service, track) = fixture();
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();
        let mut suspended = service.config.lock().unwrap().credentials.clone();
        suspended.scrobbling = Scrobbling::Suspended;
        service.save(&suspended).unwrap();

        let restarted =
            Scrobbler::start(service.db.clone(), dir.path().join("credentials.json")).unwrap();
        let status = restarted.status().unwrap();
        assert!(status.suspended && !status.enabled);
        assert_eq!(status.pending, 1, "kept, unlike turning it off");
        assert!(status.error.unwrap().contains("rejected"));
        assert!(restarted.generation().is_none(), "nothing new is queued");

        restarted.set_enabled(false).unwrap();
        let status = restarted.status().unwrap();
        assert!(!status.suspended && !status.enabled);
        assert_eq!(status.pending, 0);
    }

    #[test]
    fn a_settled_row_stays_settled() {
        let (_dir, service, track) = fixture();
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();
        service.mark(1, Settled::Submitted, None).unwrap();
        service.mark(1, Settled::Failed, Some("too late")).unwrap();
        assert_eq!(outbox_states(&service), ["submitted"]);
    }

    /// Row ids can be reused once the highest row is deleted.
    #[test]
    fn failed_attempts_are_stored_with_their_row() {
        let (_dir, service, track) = fixture();
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();
        for _ in 0..MAX_ITEM_ATTEMPTS - 1 {
            service.note_attempt(1).unwrap();
        }
        let attempts = |service: &Scrobbler| -> i64 {
            service
                .db
                .with(|c| Ok(c.query_row("SELECT attempts FROM lastfm_outbox", [], |r| r.get(0))?))
                .unwrap()
        };
        assert_eq!(attempts(&service), i64::from(MAX_ITEM_ATTEMPTS) - 1);

        service.set_enabled(false).unwrap();
        service.set_enabled(true).unwrap();
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();
        assert_eq!(
            attempts(&service),
            0,
            "a new row starts clean, whatever its id"
        );
    }

    /// Off then on after a batch was read: its rows are gone, and a POST would
    /// submit a play the user discarded. The endpoint here is unreachable, so an
    /// attempted POST comes back as an error.
    #[test]
    fn a_batch_stops_when_the_account_changed_since_it_was_read() {
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
        service.endpoint = "http://127.0.0.1:1/".into();
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();
        let (generation, batch) = service.read_batch().unwrap();
        assert_eq!(batch.len(), 1);
        assert!(
            service.deliver_batch(generation, batch.clone()).is_err(),
            "unchanged, it does try to send"
        );

        service.set_enabled(false).unwrap();
        service.set_enabled(true).unwrap();
        service.deliver_batch(generation, batch).unwrap();
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
