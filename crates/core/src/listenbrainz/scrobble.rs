//! Sending listens to ListenBrainz: durable, account-bound background delivery.
//! The token never enters prefs_get_all, plugin storage, or the webview response.
use crate::{db::Db, model::Track, private_file};
use anyhow::{bail, Context, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
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

/// Whether listens are being sent for the connected account.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Scrobbling {
    #[default]
    Off,
    On,
    /// ListenBrainz rejected the token. Unlike Off, this was not the user's
    /// choice, so waiting listens are kept for a reconnect of the same account.
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

const ENABLED_PREF: &str = "listenbrainz_scrobbling_enabled";
const TOKEN_REJECTED: &str =
    "ListenBrainz rejected this token. Connect again to resume; waiting listens are kept.";

/// Where one queued listen stands. Pending is the only state a row can leave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Settled {
    Submitted,
    Skipped,
    Failed,
}

impl Settled {
    fn as_str(self) -> &'static str {
        match self {
            Settled::Submitted => "submitted",
            Settled::Skipped => "skipped",
            Settled::Failed => "failed",
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Credentials {
    token: String,
    username: String,
    // Persisted in SQLite, never in the credential file.
    #[serde(skip)]
    scrobbling: Scrobbling,
}

#[derive(Default)]
struct Config {
    credentials: Credentials,
    generation: u64,
    error: Option<String>,
}

/// Queued listens as read: id, track JSON, start time, failed attempts.
type OutboxBatch = Vec<(i64, String, i64, u32)>;

/// Deliveries of one listen that may fail for reasons of its own before it
/// is given up on.
const MAX_ITEM_ATTEMPTS: u32 = 5;

#[derive(Serialize)]
pub struct ScrobbleStatus {
    pub username: Option<String>,
    pub enabled: bool,
    /// ListenBrainz rejected the token; connecting again resumes sending.
    pub suspended: bool,
    pub pending: i64,
    pub error: Option<String>,
}

/// ListenBrainz does not accept the token.
#[derive(Debug, thiserror::Error)]
#[error("ListenBrainz rejected the token.")]
struct Unauthorized;

/// ListenBrainz refused this one request as malformed. Trying it again
/// cannot help.
#[derive(Debug, thiserror::Error)]
#[error("ListenBrainz did not accept this listen.")]
struct Refused;

/// ListenBrainz as a whole could not take the request: unreachable, a server
/// error, or rate limiting. Delivery stops and retries later, without holding
/// it against the listen that happened to be first.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct Outage(&'static str);

fn is_outage(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Outage>().is_some()
}

/// One authenticated request. Server text is never passed on: it can repeat
/// what was submitted.
fn call(endpoint: &str, token: &str, path: &str, body: Option<&Value>) -> Result<Value> {
    let url = format!("{endpoint}{path}");
    let authorization = format!("Token {token}");
    let unreachable = |_| Outage("Cannot reach ListenBrainz. Will retry.");
    let mut response = match body {
        Some(body) => ureq::post(&url)
            .header("User-Agent", crate::web::USER_AGENT)
            .header("Authorization", &authorization)
            .config()
            .timeout_global(Some(Duration::from_secs(15)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .send_json(body)
            .map_err(unreachable)?,
        None => ureq::get(&url)
            .header("User-Agent", crate::web::USER_AGENT)
            .header("Authorization", &authorization)
            .config()
            .timeout_global(Some(Duration::from_secs(15)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .call()
            .map_err(unreachable)?,
    };
    match response.status().as_u16() {
        200..=299 => crate::http::read_json(response.body_mut())
            .map_err(|_| anyhow::anyhow!("ListenBrainz returned an unreadable response.")),
        401 | 403 => Err(Unauthorized.into()),
        429 | 500..=599 => {
            Err(Outage("ListenBrainz is unavailable or rate limited. Will retry.").into())
        }
        400 => Err(Refused.into()),
        status => bail!("ListenBrainz returned HTTP {status}."),
    }
}

/// The account a token belongs to, or None when ListenBrainz does not know it.
fn account_of(endpoint: &str, token: &str) -> Result<Option<String>> {
    let answer = call(endpoint, token, "1/validate-token", None)?;
    if answer["valid"].as_bool() != Some(true) {
        return Ok(None);
    }
    Ok(Some(
        answer["user_name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .context("ListenBrainz returned no account name")?
            .to_string(),
    ))
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
        let loaded = private_file::read::<Credentials>(&path);
        let error = loaded.as_ref().err().map(|_| {
            "Could not read the ListenBrainz token. Connect your account again.".to_string()
        });
        let mut credentials = loaded.unwrap_or_default();
        if !credentials.token.is_empty() {
            credentials.scrobbling = Scrobbling::from_pref(db.prefs_get(ENABLED_PREF)?.as_deref());
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
            .name("listenbrainz-scrobbler".into())
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
        self.db
            .prefs_set(ENABLED_PREF, credentials.scrobbling.as_pref())?;
        private_file::write(&self.path, credentials)
    }

    pub fn status(&self) -> Result<ScrobbleStatus> {
        let config = self.config.lock().unwrap();
        let pending = self.db.with(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM listenbrainz_outbox WHERE state = 'pending' AND account = ?1",
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
                .or_else(|| suspended.then(|| TOKEN_REJECTED.to_string())),
        })
    }

    /// The token for lookups, while ListenBrainz accepts it.
    pub fn token(&self) -> Option<String> {
        let config = self.config.lock().unwrap();
        let credentials = &config.credentials;
        (!credentials.token.is_empty() && credentials.scrobbling != Scrobbling::Suspended)
            .then(|| credentials.token.clone())
    }

    /// Connects the account a user token belongs to. The current account stays
    /// as it is until ListenBrainz has confirmed the new token.
    pub fn connect(&self, token: String) -> Result<()> {
        let token = token.trim().to_string();
        if token.is_empty() || token.len() > 200 || !token.bytes().all(|b| b.is_ascii_graphic()) {
            bail!("Enter the user token from your ListenBrainz settings page.");
        }
        let _operation = self.operation.lock().unwrap();
        let username = match account_of(&self.endpoint, &token) {
            Ok(Some(username)) => username,
            Ok(None) => bail!("ListenBrainz does not know this token."),
            Err(e) if e.downcast_ref::<Unauthorized>().is_some() => {
                bail!("ListenBrainz does not know this token.")
            }
            Err(e) => return Err(e),
        };
        // Connecting does not silently opt in; the checkbox is explicit.
        let credentials = Credentials {
            token,
            username,
            scrobbling: Scrobbling::Off,
        };
        self.save(&credentials)?;
        let account = credentials.username.clone();
        {
            let mut config = self.config.lock().unwrap();
            config.credentials = credentials;
            config.generation += 1;
            config.error = None;
        }
        // Preserve pending listens only when reconnecting the same account.
        self.db.with(|conn| {
            conn.execute(
                "DELETE FROM listenbrainz_outbox WHERE state = 'pending' AND account != ?1",
                [&account],
            )?;
            Ok(())
        })
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        let _operation = self.operation.lock().unwrap();
        {
            let mut config = self.config.lock().unwrap();
            if enabled && config.credentials.token.is_empty() {
                bail!("Connect ListenBrainz first.")
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
            conn.execute(
                "DELETE FROM listenbrainz_outbox WHERE state = 'pending'",
                [],
            )?;
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
            conn.execute("INSERT OR IGNORE INTO listenbrainz_outbox(account, track_json, started_at) VALUES (?1, ?2, ?3)",
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
                "UPDATE listenbrainz_outbox SET state = ?2, message = ?3 WHERE id = ?1 AND state = 'pending'",
                params![id, state.as_str(), message],
            )?;
            // Retain only recent delivery diagnostics; song history is separate.
            conn.execute("DELETE FROM listenbrainz_outbox WHERE state != 'pending' AND id < (SELECT COALESCE(MAX(id),0)-1000 FROM listenbrainz_outbox)", [])?;
            Ok(())
        })
    }

    fn note_attempt(&self, id: i64) -> Result<()> {
        self.db.with(|conn| {
            conn.execute(
                "UPDATE listenbrainz_outbox SET attempts = attempts + 1 WHERE id = ?1 AND state = 'pending'",
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
                "SELECT id, track_json, started_at, attempts FROM listenbrainz_outbox \
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

    /// A listen that fails on its own account does not stop the batch, so it
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
            let Ok(track) = serde_json::from_str::<Track>(&raw) else {
                self.mark(
                    id,
                    Settled::Failed,
                    Some("The queued listen could not be read."),
                )?;
                continue;
            };
            // The library deliberately fills absent titles from filenames, and
            // those must not be submitted as a song's name. Verify the tagged
            // metadata on the worker before making any write request. A song
            // a radio station announced has no file to check.
            if !track.path.is_empty() {
                let tagged =
                    crate::scanner::tags::read_scrobble_meta(std::path::Path::new(&track.path));
                if !tagged.is_ok_and(|meta| {
                    meta.title.is_some()
                        && meta.artist.is_some()
                        && meta.title == track.title
                        && meta.artist == track.artist
                }) {
                    let message =
                        "Missing, changed, or unreadable artist/title tags; not submitted.";
                    self.mark(id, Settled::Skipped, Some(message))?;
                    self.config.lock().unwrap().error = Some(message.into());
                    continue;
                }
            }
            match self.submit(&credentials, &track, started_at) {
                Ok(()) => {
                    self.mark(id, Settled::Submitted, None)?;
                    self.config.lock().unwrap().error = None;
                }
                // A local database error is no fault of the listen either.
                Err(e) if is_outage(&e) || e.downcast_ref::<rusqlite::Error>().is_some() => {
                    return Err(e)
                }
                Err(e) if e.downcast_ref::<Unauthorized>().is_some() => {
                    let mut config = self.config.lock().unwrap();
                    let mut c = config.credentials.clone();
                    c.scrobbling = Scrobbling::Suspended;
                    self.save(&c)?;
                    config.credentials = c;
                    config.generation += 1;
                    return Err(e);
                }
                Err(e) => {
                    self.config.lock().unwrap().error = Some(e.to_string());
                    // A refusal is final. Anything else is retried on later
                    // wakes, then given up on for this one listen.
                    if e.downcast_ref::<Refused>().is_some() || attempts + 1 >= MAX_ITEM_ATTEMPTS {
                        self.mark(id, Settled::Failed, Some(&e.to_string()))?;
                    } else {
                        self.note_attempt(id)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Sends one listen under the tags as written. ListenBrainz matches it to
    /// a recording itself, and keeps one it cannot match in the account's
    /// history all the same.
    fn submit(&self, credentials: &Credentials, track: &Track, started_at: i64) -> Result<()> {
        let mut info = json!({
            "media_player": "Crescendo",
            "submission_client": "Crescendo",
        });
        // A radio song's length is unknown.
        if track.duration_ms > 0 {
            info["duration_ms"] = track.duration_ms.into();
        }
        let mut metadata = json!({
            "artist_name": track.artist.clone().unwrap_or_default(),
            "track_name": track.title.clone().unwrap_or_default(),
            "additional_info": info,
        });
        if let Some(album) = track.album.as_deref().filter(|a| !a.trim().is_empty()) {
            metadata["release_name"] = album.into();
        }
        let body = json!({
            "listen_type": "single",
            "payload": [{ "listened_at": started_at, "track_metadata": metadata }],
        });
        let result = call(
            &self.endpoint,
            &credentials.token,
            "1/submit-listens",
            Some(&body),
        )?;
        if result["status"].as_str() != Some("ok") {
            bail!("ListenBrainz did not confirm the listen. Will retry.")
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::testing::server;

    fn fixture() -> (tempfile::TempDir, Scrobbler, Track) {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(&dir.path().join("library.db")).unwrap());
        db.with(|c| {
            c.execute("INSERT INTO tracks(id,path,dir,filename,mtime,size,format,title,artist,album,albumartist,duration_ms) VALUES (1,'/music/song.flac','/music','song.flac',0,0,'FLAC','夜の歌','歌手','アルバム','アルバム歌手',300000)", [])?;
            Ok(())
        }).unwrap();
        let track = db.track_by_id(1).unwrap().unwrap();
        let (wake, _rx) = sync_channel(1);
        let service = Scrobbler {
            db,
            path: dir.path().join("credentials.json"),
            endpoint: "http://127.0.0.1:1/".into(),
            config: Mutex::new(Config {
                credentials: Credentials {
                    token: "test-token".into(),
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

    fn ok() -> (u16, Value) {
        (200, json!({"status": "ok"}))
    }

    fn body(request: &str) -> Value {
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap()
    }

    #[test]
    fn a_listen_carries_the_tags_as_written_and_its_start_time() {
        let (_dir, mut service, track) = fixture();
        let (endpoint, requests) = server(vec![ok()]);
        service.endpoint = endpoint;
        let c = service.config.lock().unwrap().credentials.clone();
        service.submit(&c, &track, 1700000000).unwrap();
        let request = requests.join().unwrap().remove(0);
        assert!(request.starts_with("POST /1/submit-listens HTTP/1.1"));
        assert!(request
            .to_lowercase()
            .contains("authorization: token test-token"));
        let sent = body(&request);
        assert_eq!(sent["listen_type"], "single");
        let listen = &sent["payload"][0];
        assert_eq!(listen["listened_at"], 1700000000);
        let metadata = &listen["track_metadata"];
        assert_eq!(metadata["artist_name"], "歌手");
        assert_eq!(metadata["track_name"], "夜の歌");
        assert_eq!(metadata["release_name"], "アルバム");
        assert_eq!(metadata["additional_info"]["duration_ms"], 300000);
        assert_eq!(metadata["additional_info"]["media_player"], "Crescendo");
        // The token goes in the header only.
        assert!(!sent.to_string().contains("test-token"));
    }

    #[test]
    fn an_unconfirmed_answer_is_not_a_submission() {
        let (_dir, mut service, track) = fixture();
        let (endpoint, requests) = server(vec![(200, json!({}))]);
        service.endpoint = endpoint;
        let c = service.config.lock().unwrap().credentials.clone();
        assert!(service.submit(&c, &track, now()).is_err());
        requests.join().unwrap();
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
        assert!(service.token().is_none());
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
                    "SELECT account FROM listenbrainz_outbox WHERE state='pending'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(account, "alice");
        service.config.lock().unwrap().credentials.username = "bob".into();
        service.deliver().unwrap(); // no network: Alice's listen is not Bob's
        assert_eq!(service.status().unwrap().pending, 0);
    }

    #[test]
    fn the_token_is_private_and_absent_from_status_and_preferences() {
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
        assert!(!status.contains(&c.token));
        let prefs = serde_json::to_string(&service.db.prefs_get_all().unwrap()).unwrap();
        assert!(!prefs.contains(&c.token));
        assert_eq!(service.token().as_deref(), Some("test-token"));
    }

    #[test]
    fn connecting_keeps_only_the_same_accounts_pending_listens() {
        for username in ["alice", "bob"] {
            let (_dir, mut service, track) = fixture();
            service
                .enqueue(&track, now(), service.generation())
                .unwrap();
            let (endpoint, requests) = server(vec![(
                200,
                json!({"code": 200, "message": "Token valid.", "valid": true, "user_name": username}),
            )]);
            service.endpoint = endpoint;
            service.connect("  new-token\n".into()).unwrap();
            let request = requests.join().unwrap().remove(0);
            assert!(request.starts_with("GET /1/validate-token HTTP/1.1"));
            assert!(request
                .to_lowercase()
                .contains("authorization: token new-token\r\n"));
            let status = service.status().unwrap();
            assert_eq!(status.username.as_deref(), Some(username));
            assert!(!status.enabled, "connecting does not opt in");
            assert_eq!(status.pending, if username == "alice" { 1 } else { 0 });
            assert_eq!(service.token().as_deref(), Some("new-token"));
        }
    }

    #[test]
    fn a_token_listenbrainz_does_not_know_leaves_the_account_alone() {
        let (dir, mut service, track) = fixture();
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();
        let generation = service.generation();
        let (endpoint, requests) = server(vec![(
            200,
            json!({"code": 200, "message": "Token invalid.", "valid": false}),
        )]);
        service.endpoint = endpoint;
        let refused = service.connect("wrong".into()).unwrap_err();
        requests.join().unwrap();
        assert!(refused.to_string().contains("does not know"));
        assert!(service.connect("has a space".into()).is_err());
        assert!(service.connect(String::new()).is_err());
        let status = service.status().unwrap();
        assert!(status.enabled);
        assert_eq!(status.username.as_deref(), Some("alice"));
        assert_eq!(status.pending, 1);
        assert_eq!(service.generation(), generation, "the stream keeps going");
        assert_eq!(service.token().as_deref(), Some("test-token"));
        assert!(!dir.path().join("credentials.json").exists());
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
    fn delivery_retries_outages_but_pauses_after_a_rejected_token() {
        let (dir, mut service, mut track) = fixture();
        tagged_wav(dir.path(), &mut track);
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();
        let (endpoint, requests) = server(vec![(503, json!({"code": 503}))]);
        service.endpoint = endpoint;
        assert!(service.deliver().is_err());
        requests.join().unwrap();
        assert_eq!(service.status().unwrap().pending, 1);
        assert!(service.status().unwrap().enabled);
        let (endpoint, requests) = server(vec![ok()]);
        service.endpoint = endpoint;
        service.deliver().unwrap();
        requests.join().unwrap();
        assert_eq!(service.status().unwrap().pending, 0);
        service
            .enqueue(&track, now() + 1, service.generation())
            .unwrap();
        let (endpoint, requests) = server(vec![(401, json!({"code": 401}))]);
        service.endpoint = endpoint;
        assert!(service.deliver().is_err());
        requests.join().unwrap();
        let status = service.status().unwrap();
        assert_eq!(status.pending, 1);
        assert!(!status.enabled);
        assert!(status.suspended);
        assert!(
            service.token().is_none(),
            "no lookups with a rejected token"
        );
        assert_eq!(
            service.db.prefs_get(ENABLED_PREF).unwrap().as_deref(),
            Some("suspended")
        );
    }

    fn outbox_states(service: &Scrobbler) -> Vec<String> {
        service
            .db
            .with(|c| {
                let mut stmt = c.prepare("SELECT state FROM listenbrainz_outbox ORDER BY id")?;
                let states = stmt
                    .query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(states)
            })
            .unwrap()
    }

    /// A listen that keeps failing used to be retried first, forever, with
    /// every later one queued behind it.
    #[test]
    fn a_failing_listen_does_not_hold_back_later_ones() {
        let (dir, mut service, mut track) = fixture();
        tagged_wav(dir.path(), &mut track);
        service
            .enqueue(&track, now() - 10, service.generation())
            .unwrap();
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();

        let unconfirmed = (200, json!({}));
        let (endpoint, requests) = server(vec![unconfirmed.clone(), ok()]);
        service.endpoint = endpoint;
        service.deliver().unwrap();
        requests.join().unwrap();
        assert_eq!(outbox_states(&service), ["pending", "submitted"]);

        // Each later wake retries it, until it is given up on.
        for attempt in 2..=MAX_ITEM_ATTEMPTS {
            let (endpoint, requests) = server(vec![unconfirmed.clone()]);
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

    /// A listen ListenBrainz calls malformed will be malformed next time too.
    #[test]
    fn a_refused_listen_is_given_up_on_at_once() {
        let (dir, mut service, mut track) = fixture();
        tagged_wav(dir.path(), &mut track);
        service
            .enqueue(&track, now() - 10, service.generation())
            .unwrap();
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();
        let (endpoint, requests) = server(vec![(400, json!({"code": 400})), ok()]);
        service.endpoint = endpoint;
        service.deliver().unwrap();
        requests.join().unwrap();
        assert_eq!(outbox_states(&service), ["failed", "submitted"]);
    }

    /// An outage is no fault of the first listen and never counts against it.
    #[test]
    fn outages_stop_the_batch_without_counting_against_a_listen() {
        let (dir, mut service, mut track) = fixture();
        tagged_wav(dir.path(), &mut track);
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();
        for _ in 0..MAX_ITEM_ATTEMPTS + 1 {
            let (endpoint, requests) = server(vec![(502, json!("bad gateway"))]);
            service.endpoint = endpoint;
            assert!(service.deliver().is_err());
            requests.join().unwrap();
        }
        assert_eq!(outbox_states(&service), ["pending"]);
    }

    #[test]
    fn untagged_and_unreadable_rows_are_settled_without_a_request() {
        let (_dir, service, track) = fixture();
        // The file of the fixture's track does not exist, so its tags cannot
        // be confirmed; the second row is not a track at all.
        service
            .enqueue(&track, now(), service.generation())
            .unwrap();
        service
            .db
            .with(|c| {
                c.execute(
                    "INSERT INTO listenbrainz_outbox(account, track_json, started_at) \
                     VALUES ('alice', '{\"older format\": true}', ?1)",
                    params![now()],
                )?;
                Ok(())
            })
            .unwrap();
        service.deliver().unwrap();
        assert_eq!(outbox_states(&service), ["skipped", "failed"]);
    }

    /// A song a radio station announced has no file whose tags could be read.
    #[test]
    fn a_radio_song_is_sent_without_a_file_check() {
        let (_dir, mut service, _track) = fixture();
        let song = crate::radio::RadioSong {
            artist: Some("歌手".into()),
            title: "ラジオの歌".into(),
            started_at: 1700000000,
            heard_ms: 240_000,
            from_start: true,
            scrobbled: false,
        };
        service
            .enqueue(&song.as_track(), song.started_at, service.generation())
            .unwrap();
        let (endpoint, requests) = server(vec![ok()]);
        service.endpoint = endpoint;
        service.deliver().unwrap();
        let sent = body(&requests.join().unwrap().remove(0));
        let metadata = &sent["payload"][0]["track_metadata"];
        assert_eq!(metadata["track_name"], "ラジオの歌");
        assert!(metadata.get("release_name").is_none());
        assert!(metadata["additional_info"].get("duration_ms").is_none());
        assert_eq!(outbox_states(&service), ["submitted"]);
    }

    #[test]
    fn a_rejected_token_stays_apart_from_turning_scrobbling_off() {
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
                .with(|c| {
                    Ok(c.query_row("SELECT attempts FROM listenbrainz_outbox", [], |r| r.get(0))?)
                })
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
    /// submit a listen the user discarded. The endpoint here is unreachable,
    /// so an attempted POST comes back as an error.
    #[test]
    fn a_batch_stops_when_the_account_changed_since_it_was_read() {
        let (dir, service, mut track) = fixture();
        tagged_wav(dir.path(), &mut track);
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
}
