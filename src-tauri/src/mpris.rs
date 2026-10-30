//! MPRIS2: publishes the player on the session bus as
//! `org.mpris.MediaPlayer2.crescendo`, so desktop media controls (the Plasma
//! media applet in the system tray, media keys, the lock screen, KDE Connect)
//! can show and drive playback without the main window.
//!
//! Property getters read the player's shared state snapshot; change signals
//! come from the same `player:state` / `player:position` Tauri events the
//! webview and plugins listen to, so the player thread keeps no D-Bus
//! coupling. Metadata follows the display rule the UI uses: a romanized sort
//! tag stands in for a CJK name when the preference is on, and the artist
//! line is the album artist (what the transport bar shows), falling back to
//! the track artist.

use crate::player::{Cmd, PlayerHandle, PlayerState, Status};
use crate::plugin_bridge::PositionMirror;
use crescendo_core::art::{ArtData, ArtStore};
use crescendo_core::db::{has_cjk, Db};
use crescendo_core::model::Track;
use crescendo_core::playqueue::RepeatMode;
use crescendo_core::plugins::PluginManager;
use mpris_server::zbus::{self, fdo};
use mpris_server::{
    LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, Property, RootInterface,
    Server, Signal, Time, TrackId, Volume,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Listener, Manager};
use tokio::sync::mpsc;

const BUS_NAME_SUFFIX: &str = "crescendo";
const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";
/// A position tick this far from where steady playback would have put it is
/// reported to clients as a seek.
const SEEK_JUMP_MS: u64 = 1_000;

enum Update {
    State,
    Position(u64),
}

struct Mpris {
    app: AppHandle,
    player: PlayerHandle,
    position: Arc<PositionMirror>,
    /// Built by the update loop from each published state, so a property read
    /// never resolves cover art or touches the database on the D-Bus task.
    metadata: Mutex<Metadata>,
}

impl Mpris {
    fn state(&self) -> Option<PlayerState> {
        self.player.state().ok()
    }

    fn has_track(&self) -> bool {
        self.state().is_some_and(|state| state.track.is_some())
    }

    fn send(&self, cmd: Cmd) -> fdo::Result<()> {
        self.player.send(cmd).map_err(fdo::Error::Failed)
    }

    fn progress(&self) -> (Option<i64>, u64, u64) {
        let track_id = self
            .state()
            .and_then(|state| state.track.map(|track| track.id));
        let (position_ms, duration_ms) = self.position.get(track_id);
        (track_id, position_ms, duration_ms)
    }
}

impl RootInterface for Mpris {
    async fn raise(&self) -> fdo::Result<()> {
        if let Some(window) = self.app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
        Ok(())
    }

    async fn quit(&self) -> fdo::Result<()> {
        self.app.exit(0);
        Ok(())
    }

    async fn can_quit(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn set_fullscreen(&self, _fullscreen: bool) -> zbus::Result<()> {
        Ok(())
    }

    async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn can_raise(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn has_track_list(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn identity(&self) -> fdo::Result<String> {
        Ok("Crescendo".into())
    }

    async fn desktop_entry(&self) -> fdo::Result<String> {
        Ok("crescendo".into())
    }

    async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }
}

impl PlayerInterface for Mpris {
    async fn next(&self) -> fdo::Result<()> {
        self.send(Cmd::Next)
    }

    async fn previous(&self) -> fdo::Result<()> {
        self.send(Cmd::Prev)
    }

    async fn pause(&self) -> fdo::Result<()> {
        match self.state().map(|state| state.status) {
            Some(Status::Playing) => self.send(Cmd::TogglePause),
            _ => Ok(()),
        }
    }

    async fn play_pause(&self) -> fdo::Result<()> {
        self.send(Cmd::TogglePause)
    }

    async fn stop(&self) -> fdo::Result<()> {
        self.send(Cmd::Stop)
    }

    async fn play(&self) -> fdo::Result<()> {
        match self.state().map(|state| state.status) {
            // From Stopped the player starts at the top of its context.
            Some(Status::Paused) | Some(Status::Stopped) => self.send(Cmd::TogglePause),
            _ => Ok(()),
        }
    }

    async fn seek(&self, offset: Time) -> fdo::Result<()> {
        let (track_id, position_ms, duration_ms) = self.progress();
        if track_id.is_none() {
            return Ok(());
        }
        let target = (position_ms as i64 + offset.as_millis()).max(0) as u64;
        if duration_ms > 0 && target >= duration_ms {
            // The spec asks for a skip when a seek runs past the end.
            return self.send(Cmd::Next);
        }
        self.send(Cmd::Seek(target))
    }

    async fn set_position(&self, track_id: TrackId, position: Time) -> fdo::Result<()> {
        let (current, _, duration_ms) = self.progress();
        // A stale track id means the client is talking about a song that has
        // since ended; the spec says to ignore it.
        if current.map(track_path) != Some(track_id) || position.is_negative() {
            return Ok(());
        }
        let target = position.as_millis() as u64;
        if duration_ms > 0 && target > duration_ms {
            return Ok(());
        }
        self.send(Cmd::Seek(target))
    }

    async fn open_uri(&self, _uri: String) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported(
            "Crescendo plays from its library only".into(),
        ))
    }

    async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
        Ok(self.state().map_or(PlaybackStatus::Stopped, |state| {
            playback_status(state.status)
        }))
    }

    async fn loop_status(&self) -> fdo::Result<LoopStatus> {
        Ok(self
            .state()
            .map_or(LoopStatus::None, |state| loop_status(state.repeat)))
    }

    async fn set_loop_status(&self, loop_status: LoopStatus) -> zbus::Result<()> {
        let mode = match loop_status {
            LoopStatus::None => RepeatMode::Off,
            LoopStatus::Track => RepeatMode::One,
            LoopStatus::Playlist => RepeatMode::All,
        };
        self.send(Cmd::SetRepeat(mode))?;
        settle(&self.player, |state| state.repeat == mode).await;
        Ok(())
    }

    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn set_rate(&self, _rate: PlaybackRate) -> zbus::Result<()> {
        Ok(())
    }

    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(self.state().is_some_and(|state| state.shuffle))
    }

    async fn set_shuffle(&self, shuffle: bool) -> zbus::Result<()> {
        self.send(Cmd::SetShuffle(shuffle))?;
        settle(&self.player, |state| state.shuffle == shuffle).await;
        Ok(())
    }

    async fn metadata(&self) -> fdo::Result<Metadata> {
        Ok(self
            .metadata
            .lock()
            .map(|metadata| metadata.clone())
            .unwrap_or_default())
    }

    async fn volume(&self) -> fdo::Result<Volume> {
        Ok(self.state().map_or(0.0, volume))
    }

    async fn set_volume(&self, volume: Volume) -> zbus::Result<()> {
        if !volume.is_finite() {
            return Ok(());
        }
        let volume = volume.clamp(0.0, 1.0);
        self.send(Cmd::SetVolume(volume))?;
        settle(&self.player, |state| state.volume == volume).await;
        Ok(())
    }

    async fn position(&self) -> fdo::Result<Time> {
        let (_, position_ms, _) = self.progress();
        Ok(Time::from_millis(position_ms as i64))
    }

    async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn can_go_next(&self) -> fdo::Result<bool> {
        Ok(self.has_track())
    }

    async fn can_go_previous(&self) -> fdo::Result<bool> {
        Ok(self.has_track())
    }

    async fn can_play(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_pause(&self) -> fdo::Result<bool> {
        Ok(self.has_track())
    }

    async fn can_seek(&self) -> fdo::Result<bool> {
        Ok(self.has_track())
    }

    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(true)
    }
}

/// Wait, briefly, until the player thread has applied a setting. zbus
/// answers a property write by re-reading the property and signalling it,
/// so without this clients would be told the old value and only learn the
/// new one from the state event a moment later.
async fn settle(player: &PlayerHandle, applied: impl Fn(&PlayerState) -> bool) {
    for _ in 0..40 {
        if player.state().ok().is_some_and(|state| applied(&state)) {
            return;
        }
        async_io::Timer::after(Duration::from_millis(5)).await;
    }
}

/// What MPRIS has no word for, served on the same object as the MPRIS
/// interfaces so a client already talking to the player finds it there.
/// The Plasma widget uses it for "Stop after this song".
struct CrescendoPlayer {
    player: PlayerHandle,
}

impl CrescendoPlayer {
    async fn arm(&self, on: bool) -> fdo::Result<()> {
        self.player
            .send(Cmd::SetStopAfterCurrent(on))
            .map_err(fdo::Error::Failed)?;
        settle(&self.player, |state| state.stop_after_current == on).await;
        Ok(())
    }
}

#[zbus::interface(name = "dev.crescendo.Player")]
impl CrescendoPlayer {
    /// One-shot: stop when the current song ends, then clear itself.
    #[zbus(property)]
    async fn stop_after_current(&self) -> bool {
        self.player
            .state()
            .is_ok_and(|state| state.stop_after_current)
    }

    #[zbus(property)]
    async fn set_stop_after_current(&self, on: bool) -> zbus::Result<()> {
        self.arm(on).await.map_err(Into::into)
    }

    /// The property write as a plain method, for clients whose bindings
    /// cannot build a variant: Plasma's QML D-Bus module sends a bare `b`.
    #[zbus(name = "SetStopAfterCurrent")]
    async fn set_stop_after_current_method(&self, on: bool) -> fdo::Result<()> {
        self.arm(on).await
    }
}

fn playback_status(status: Status) -> PlaybackStatus {
    match status {
        Status::Playing => PlaybackStatus::Playing,
        Status::Paused => PlaybackStatus::Paused,
        Status::Stopped => PlaybackStatus::Stopped,
    }
}

fn loop_status(repeat: RepeatMode) -> LoopStatus {
    match repeat {
        RepeatMode::Off => LoopStatus::None,
        RepeatMode::One => LoopStatus::Track,
        RepeatMode::All => LoopStatus::Playlist,
    }
}

fn volume(state: PlayerState) -> Volume {
    if state.muted {
        0.0
    } else {
        state.volume
    }
}

fn track_path(id: i64) -> TrackId {
    TrackId::try_from(format!("/dev/crescendo/track/{id}")).unwrap_or_default()
}

fn file_uri(path: &Path) -> Option<String> {
    tauri::Url::from_file_path(path)
        .ok()
        .map(|url| url.to_string())
}

/// The display rule of `src/lib/format.ts`: the sort tag replaces a value
/// that contains CJK text when the preference is on.
fn romanized(
    value: &Option<String>,
    sort: &Option<String>,
    prefer_romanized: bool,
) -> Option<String> {
    match (value, sort) {
        (Some(value), Some(sort)) if prefer_romanized && has_cjk(value) => Some(sort.clone()),
        (value, _) => value.clone(),
    }
}

fn file_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

/// A `file://` URL for the cover. The cached resolution is trusted as is:
/// the folder image's own path, or the cached thumbnail for embedded art,
/// which has no file of its own. Only an album nothing has resolved yet
/// pays for the full resolution, which decodes the image to validate it.
fn art_url(db: &Db, art: &ArtStore, track: &Track) -> Option<String> {
    let Some(album_id) = track.album_id else {
        return match art.track_art(db, track.id).ok()? {
            ArtData::File(path) => file_uri(&path),
            ArtData::Bytes(_) => None,
        };
    };
    let path = match art.cover_source(db, album_id).ok().flatten() {
        Some(source) if source.starts_with("folder:") => PathBuf::from(&source["folder:".len()..]),
        Some(_) => art.album_thumb(db, album_id).ok()?,
        None => match art.album_full(db, album_id).ok()? {
            ArtData::File(path) => path,
            ArtData::Bytes(_) => art.album_thumb(db, album_id).ok()?,
        },
    };
    file_uri(&path)
}

fn build_metadata(
    state: &PlayerState,
    db: &Db,
    art: &ArtStore,
    prefer_romanized: bool,
) -> Metadata {
    let Some(track) = &state.track else {
        return Metadata::builder()
            .trackid(TrackId::try_from(NO_TRACK).unwrap_or_default())
            .build();
    };
    let title = romanized(&track.title, &track.titlesort, prefer_romanized)
        .unwrap_or_else(|| file_stem(&track.path));
    let artist = romanized(&track.albumartist, &track.albumartistsort, prefer_romanized)
        .or_else(|| romanized(&track.artist, &track.artistsort, prefer_romanized));
    let mut builder = Metadata::builder()
        .trackid(track_path(track.id))
        .title(title)
        .length(Time::from_millis(track.duration_ms.max(0)));
    if let Some(artist) = artist {
        builder = builder.artist([artist.clone()]).album_artist([artist]);
    }
    if let Some(album) = &track.album {
        builder = builder.album(album.clone());
    }
    if let Some(genre) = &track.genre {
        builder = builder.genre(genre.split("; ").map(str::to_owned));
    }
    if let Some(n) = track.track_no.and_then(|n| i32::try_from(n).ok()) {
        builder = builder.track_number(n);
    }
    if let Some(n) = track.disc_no.and_then(|n| i32::try_from(n).ok()) {
        builder = builder.disc_number(n);
    }
    if let Some(url) = file_uri(Path::new(&track.path)) {
        builder = builder.url(url);
    }
    if let Some(url) = art_url(db, art, track) {
        builder = builder.art_url(url);
    }
    builder.build()
}

/// What clients were last told, so a state event only signals what changed.
struct Published {
    status: PlaybackStatus,
    loop_status: LoopStatus,
    shuffle: bool,
    volume: Volume,
    track_id: Option<i64>,
    /// The track as last published, so a state event that changes nothing
    /// about the song (pause, volume) does not rebuild its metadata.
    track: Option<Track>,
    stop_after_current: bool,
    metadata: Metadata,
}

impl Published {
    fn from_state(state: &PlayerState, metadata: Metadata) -> Self {
        Self {
            status: playback_status(state.status),
            loop_status: loop_status(state.repeat),
            shuffle: state.shuffle,
            volume: volume(state.clone()),
            track_id: state.track.as_ref().map(|track| track.id),
            track: state.track.clone(),
            stop_after_current: state.stop_after_current,
            metadata,
        }
    }

    /// The cheap properties that differ from what was last published.
    /// Metadata is announced separately, after it has been built.
    fn changes(&self, previous: Option<&Published>) -> Vec<Property> {
        let mut changed = Vec::new();
        let Some(previous) = previous else {
            let has_track = self.track_id.is_some();
            changed.extend([
                Property::PlaybackStatus(self.status),
                Property::LoopStatus(self.loop_status),
                Property::Shuffle(self.shuffle),
                Property::Volume(self.volume),
                Property::CanGoNext(has_track),
                Property::CanGoPrevious(has_track),
                Property::CanPause(has_track),
                Property::CanSeek(has_track),
            ]);
            return changed;
        };
        if self.status != previous.status {
            changed.push(Property::PlaybackStatus(self.status));
        }
        if self.loop_status != previous.loop_status {
            changed.push(Property::LoopStatus(self.loop_status));
        }
        if self.shuffle != previous.shuffle {
            changed.push(Property::Shuffle(self.shuffle));
        }
        if self.volume != previous.volume {
            changed.push(Property::Volume(self.volume));
        }
        let has_track = self.track_id.is_some();
        if has_track != previous.track_id.is_some() {
            changed.extend([
                Property::CanGoNext(has_track),
                Property::CanGoPrevious(has_track),
                Property::CanPause(has_track),
                Property::CanSeek(has_track),
            ]);
        }
        changed
    }
}

async fn announce_stop_after_current(server: &Server<Mpris>) {
    let Ok(iface) = server
        .connection()
        .object_server()
        .interface::<_, CrescendoPlayer>(OBJECT_PATH)
        .await
    else {
        return;
    };
    let extension = iface.get().await;
    let result = extension
        .stop_after_current_changed(iface.signal_emitter())
        .await;
    drop(extension);
    if let Err(error) = result {
        log::debug!("dev.crescendo.Player PropertiesChanged failed: {error}");
    }
}

struct Tick {
    track_id: Option<i64>,
    position_ms: u64,
    at: Instant,
}

/// Register on the session bus and keep clients informed. Failure to reach
/// the bus only logs: the player works the same without MPRIS.
pub fn start(
    app: AppHandle,
    player: PlayerHandle,
    position: Arc<PositionMirror>,
    plugins: Arc<PluginManager>,
    db: Arc<Db>,
    art: Arc<ArtStore>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    // Fill the metadata cache from whatever is already playing.
    let _ = tx.send(Update::State);
    {
        let tx = tx.clone();
        app.listen("player:state", move |_| {
            let _ = tx.send(Update::State);
        });
    }
    app.listen("player:position", move |event| {
        let ms = serde_json::from_str::<serde_json::Value>(event.payload())
            .ok()
            .and_then(|value| value.get("ms")?.as_u64());
        if let Some(ms) = ms {
            let _ = tx.send(Update::Position(ms));
        }
    });
    tauri::async_runtime::spawn(async move {
        let imp = Mpris {
            app,
            player,
            position,
            metadata: Mutex::new(Metadata::new()),
        };
        let server = match Server::new(BUS_NAME_SUFFIX, imp).await {
            Ok(server) => server,
            Err(error) => {
                log::warn!("MPRIS unavailable: {error}");
                return;
            }
        };
        let extension = CrescendoPlayer {
            player: server.imp().player.clone(),
        };
        if let Err(error) = server
            .connection()
            .object_server()
            .at(OBJECT_PATH, extension)
            .await
        {
            log::warn!("dev.crescendo.Player unavailable: {error}");
        }
        run(server, rx, plugins, db, art).await;
    });
}

async fn run(
    server: Server<Mpris>,
    mut rx: mpsc::UnboundedReceiver<Update>,
    plugins: Arc<PluginManager>,
    db: Arc<Db>,
    art: Arc<ArtStore>,
) {
    let imp = server.imp();
    let mut published: Option<Published> = None;
    let mut last_tick: Option<Tick> = None;
    while let Some(update) = rx.recv().await {
        match update {
            Update::State => {
                let Some(state) = imp.state() else {
                    continue;
                };
                // Pause, volume and the like must not wait behind cover-art
                // resolution: announce them before touching the metadata.
                let same_track = published
                    .as_ref()
                    .is_some_and(|previous| previous.track == state.track);
                let previous_metadata = published
                    .as_ref()
                    .map(|previous| previous.metadata.clone())
                    .unwrap_or_default();
                let mut next = Published::from_state(&state, previous_metadata);
                let changed = next.changes(published.as_ref());
                if published.as_ref().map(|p| p.track_id) != Some(next.track_id) {
                    // A new song starts near zero; that is not a seek.
                    last_tick = None;
                }
                if !changed.is_empty() {
                    if let Err(error) = server.properties_changed(changed).await {
                        log::debug!("MPRIS PropertiesChanged failed: {error}");
                    }
                }
                if published.as_ref().map(|p| p.stop_after_current) != Some(next.stop_after_current)
                {
                    announce_stop_after_current(&server).await;
                }
                if !same_track {
                    let prefer_romanized = plugins
                        .projection(None)
                        .map(|guard| guard.prefer_romanized())
                        .unwrap_or(true);
                    let metadata = {
                        let (state, db, art) = (state.clone(), db.clone(), art.clone());
                        tauri::async_runtime::spawn_blocking(move || {
                            build_metadata(&state, &db, &art, prefer_romanized)
                        })
                        .await
                    };
                    if let Ok(metadata) = metadata {
                        if metadata != next.metadata {
                            if let Ok(mut cached) = imp.metadata.lock() {
                                *cached = metadata.clone();
                            }
                            let announce = [Property::Metadata(metadata.clone())];
                            if let Err(error) = server.properties_changed(announce).await {
                                log::debug!("MPRIS PropertiesChanged failed: {error}");
                            }
                            next.metadata = metadata;
                        }
                    }
                }
                published = Some(next);
            }
            Update::Position(position_ms) => {
                let track_id = published.as_ref().and_then(|p| p.track_id);
                let playing = published
                    .as_ref()
                    .is_some_and(|p| p.status == PlaybackStatus::Playing);
                if let Some(tick) = last_tick.as_ref().filter(|tick| tick.track_id == track_id) {
                    let expected = if playing {
                        tick.position_ms + tick.at.elapsed().as_millis() as u64
                    } else {
                        tick.position_ms
                    };
                    if position_ms.abs_diff(expected) > SEEK_JUMP_MS {
                        let signal = Signal::Seeked {
                            position: Time::from_millis(position_ms as i64),
                        };
                        if let Err(error) = server.emit(signal).await {
                            log::debug!("MPRIS Seeked failed: {error}");
                        }
                    }
                }
                last_tick = Some(Tick {
                    track_id,
                    position_ms,
                    at: Instant::now(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, titlesort: Option<&str>, albumartist: &str, sort: Option<&str>) -> Track {
        Track {
            id: 7,
            path: "/music/album/07 song.flac".into(),
            title: Some(title.into()),
            artist: Some("track artist".into()),
            album: Some("Album".into()),
            albumartist: Some(albumartist.into()),
            titlesort: titlesort.map(Into::into),
            artistsort: None,
            albumartistsort: sort.map(Into::into),
            genre: Some("Rock; J-Pop".into()),
            grouping: None,
            composer: None,
            arranger: None,
            lyricist: None,
            vocalist: None,
            catalognumber: None,
            event: None,
            bpm: None,
            track_no: Some(7),
            disc_no: None,
            year: None,
            duration_ms: 123_000,
            format: "flac".into(),
            album_id: None,
            rating: None,
            play_count: 0,
            missing: false,
        }
    }

    fn state(track: Track) -> PlayerState {
        PlayerState {
            status: Status::Playing,
            track: Some(track),
            context_index: Some(0),
            context_len: 1,
            shuffle: false,
            repeat: RepeatMode::Off,
            volume: 0.5,
            muted: false,
            stop_after_current: false,
        }
    }

    #[test]
    fn metadata_shows_the_album_artist_romanized_when_preferred() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let art = ArtStore::new(dir.path()).unwrap();
        let state = state(track(
            "幽雅に咲かせ",
            Some("Yuuga ni Sakase"),
            "東方",
            Some("Touhou"),
        ));

        let romaji = build_metadata(&state, &db, &art, true);
        assert_eq!(romaji.title(), Some("Yuuga ni Sakase"));
        assert_eq!(romaji.artist(), Some(vec!["Touhou".to_string()]));
        assert_eq!(romaji.album_artist(), Some(vec!["Touhou".to_string()]));
        assert_eq!(romaji.length(), Some(Time::from_millis(123_000)));
        assert_eq!(romaji.track_number(), Some(7));
        assert_eq!(
            romaji.genre(),
            Some(vec!["Rock".to_string(), "J-Pop".to_string()])
        );
        assert_eq!(romaji.trackid(), Some(track_path(7)));
        assert_eq!(
            romaji.url().as_deref(),
            Some("file:///music/album/07%20song.flac")
        );

        let original = build_metadata(&state, &db, &art, false);
        assert_eq!(original.title(), Some("幽雅に咲かせ"));
        assert_eq!(original.artist(), Some(vec!["東方".to_string()]));
    }

    #[test]
    fn metadata_keeps_latin_names_and_falls_back_to_the_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let art = ArtStore::new(dir.path()).unwrap();
        let mut untitled = track("Latin", Some("Sort"), "Band", Some("Band, The"));
        untitled.title = None;
        untitled.albumartist = None;
        untitled.albumartistsort = None;
        untitled.artist = Some("Someone".into());

        let metadata = build_metadata(&state(untitled), &db, &art, true);
        assert_eq!(metadata.title(), Some("07 song"));
        assert_eq!(metadata.artist(), Some(vec!["Someone".to_string()]));
        assert!(metadata.art_url().is_none());

        let no_track = build_metadata(
            &PlayerState {
                track: None,
                ..state(track("x", None, "y", None))
            },
            &db,
            &art,
            true,
        );
        assert_eq!(no_track.trackid(), TrackId::try_from(NO_TRACK).ok());
    }
}
