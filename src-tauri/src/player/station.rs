//! Playing an internet radio station.
//!
//! A station is live: it has no length, cannot seek, never hands over to a
//! next uri, and may drop. `Playback` holds it as its own phase, so nothing in
//! the track path (handoffs, reloads, the broken-file skip) applies, and the
//! queue keeps its place for when the library plays again.
//!
//! What a station adds: buffering pauses the pipeline until it has refilled,
//! as GStreamer asks of network streams; the stream's `title` tag is the song
//! it announces, split into artist and title and scrobbled by the rule in
//! `RadioSong`; a dropped or failed stream is reconnected a few times before
//! the player gives up; and pausing disconnects, so Play rejoins it live
//! instead of resuming stale audio.

use super::{Engine, Status};
use crescendo_core::lastfm::scrobble::{now, Scrobbler};
use crescendo_core::radio::{check_stream_url, neighbour, song_from_tags, RadioSong, Station};
use gstreamer::prelude::*;
use serde::Serialize;
use std::time::{Duration, Instant};
use tauri::Emitter;

/// Failed connections in a row before the player stops trying.
const MAX_RECONNECTS: u32 = 3;
/// Playing this long without a failure forgives the earlier ones.
const STABLE_MS: u64 = 15_000;

/// The station as the frontend, MPRIS and the tray see it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StationNow {
    /// The saved station's id; None for one played from the directory.
    pub id: Option<i64>,
    pub uuid: Option<String>,
    pub name: String,
    /// Key for `crescendo-art://…/logo/{key}`.
    pub logo: Option<String>,
    pub homepage: Option<String>,
    /// The song it announces, split at the first " - ".
    pub artist: Option<String>,
    pub title: Option<String>,
    pub codec: Option<String>,
    /// kbit/s.
    pub bitrate: Option<i64>,
    /// Percent while it refills its buffer.
    pub buffering: Option<u8>,
    /// Connecting or reconnecting, not yet heard.
    pub connecting: bool,
}

impl StationNow {
    fn from_station(station: &Station) -> Self {
        Self {
            id: (station.id > 0).then_some(station.id),
            uuid: station.uuid.clone(),
            name: station.name.clone(),
            logo: station.logo.clone(),
            homepage: station.homepage.clone(),
            artist: None,
            title: None,
            codec: station.codec.clone(),
            bitrate: station.bitrate,
            buffering: None,
            connecting: false,
        }
    }
}

pub(super) struct StationSession {
    now: StationNow,
    stream_url: String,
    /// The last title tag, as sent.
    raw_title: Option<String>,
    /// A title has arrived since this connection began, so the next one is a
    /// song heard from its start.
    titled: bool,
    song: Option<RadioSong>,
    song_generation: Option<u64>,
    /// The pipeline is held paused while the buffer refills.
    buffering: bool,
    reconnects: u32,
    reconnect_at: Option<Instant>,
    stable_ms: u64,
    last_sample: Instant,
}

impl StationSession {
    fn new(station: &Station) -> Self {
        let mut now = StationNow::from_station(station);
        now.connecting = true;
        Self {
            now,
            stream_url: station.stream_url.clone(),
            raw_title: None,
            titled: false,
            song: None,
            song_generation: None,
            buffering: false,
            reconnects: 0,
            reconnect_at: None,
            stable_ms: 0,
            last_sample: Instant::now(),
        }
    }

    /// The announced song is over, or no longer heard. Scrobble it if the
    /// rule allows, and forget it.
    fn finish_song(&mut self, scrobbler: &Scrobbler, ended: bool) {
        let Some(song) = self.song.take() else {
            return;
        };
        if song.scrobble_due(ended) {
            if let Err(error) =
                scrobbler.enqueue(&song.as_track(), song.started_at, self.song_generation)
            {
                log::warn!("Cannot queue Last.fm scrobble: {error}");
            }
        }
    }

    /// A new connection: what was announced on the old one no longer holds.
    fn forget_titles(&mut self, scrobbler: &Scrobbler) {
        self.finish_song(scrobbler, false);
        self.raw_title = None;
        self.titled = false;
        self.now.artist = None;
        self.now.title = None;
    }
}

/// Pause or resume for buffering the way every pause goes: through `settle`,
/// which never lets a stuck pipeline stall the player thread.
fn buffer_hold(playbin: &gstreamer::Element, target: gstreamer::State) {
    if !super::settle::change_state(playbin, target, super::settle::PATIENCE) {
        log::warn!("GStreamer did not change state in time; the next stop replaces it");
    }
}

impl Engine {
    pub(super) fn start_station(&mut self, station: Station) {
        if let Err(error) = check_stream_url(&station.stream_url) {
            let _ = self
                .app
                .emit("radio:error", format!("{}: {error}", station.name));
            return;
        }
        self.leave_station();
        self.pending_seek = None;
        self.reload_retry.forget();
        self.playback().begin_station();
        // Credit the outgoing track while its position can still be read.
        self.commit_play();
        self.stop_streams(gstreamer::State::Ready);
        self.flush_bus();
        self.playbin
            .set_property("uri", station.stream_url.as_str());
        self.playback().station_loaded();
        let _ = self.playbin.set_state(gstreamer::State::Playing);
        // A station plays at normal speed whatever the track rate is.
        self.rate_pending = false;
        self.current_duration_ms = 0;
        self.station = Some(StationSession::new(&station));
        self.last_station = Some(station);
        self.sync_queue_shared();
        self.publish_state();
        self.emit_position();
    }

    /// End the station session, if one is loaded. The caller replaces or
    /// stops the stream.
    pub(super) fn leave_station(&mut self) {
        if let Some(mut session) = self.station.take() {
            session.finish_song(&self.scrobbler, false);
        }
    }

    /// Pause disconnects; Play reconnects and joins the stream live.
    pub(super) fn toggle_station(&mut self) {
        let status = self.playback().status();
        match status {
            Status::Playing => {
                if let Some(session) = self.station.as_mut() {
                    session.forget_titles(&self.scrobbler);
                    session.buffering = false;
                    session.reconnect_at = None;
                    session.now.buffering = None;
                    session.now.connecting = false;
                }
                self.stop_streams(gstreamer::State::Ready);
                self.flush_bus();
                self.playback().set_paused(true);
            }
            Status::Paused => {
                let Some(session) = self.station.as_mut() else {
                    return;
                };
                session.now.connecting = true;
                session.last_sample = Instant::now();
                // The pipeline may have been replaced while stopped.
                self.playbin
                    .set_property("uri", session.stream_url.as_str());
                self.playback().set_paused(false);
                let _ = self.playbin.set_state(gstreamer::State::Playing);
            }
            Status::Stopped => {}
        }
        self.publish_state();
    }

    /// Next and Previous move through the saved stations, wrapping around.
    pub(super) fn step_station(&mut self, forward: bool) {
        let current = self.station.as_ref().and_then(|session| session.now.id);
        let ids = self.db.station_ids().unwrap_or_default();
        let Some(id) = neighbour(&ids, current, forward) else {
            return;
        };
        let Ok(Some(mut station)) = self.db.station(id) else {
            return;
        };
        station.logo = station
            .logo_url
            .as_deref()
            .map(|url| self.logos.register(url));
        self.start_station(station);
    }

    /// Handle a bus message while a station is loaded. False leaves it to the
    /// track path (spectrum frames, state changes).
    pub(super) fn station_bus(&mut self, msg: &gstreamer::Message) -> bool {
        use gstreamer::MessageView;
        match msg.view() {
            MessageView::Error(err) => {
                log::warn!(
                    "radio stream error from {:?}: {} ({:?})",
                    err.src().map(|s| s.path_string()),
                    err.error(),
                    err.debug()
                );
                self.station_failed(&err.error().to_string());
                true
            }
            MessageView::Eos(_) => {
                self.station_failed("the station ended its stream");
                true
            }
            MessageView::Buffering(buffering) => {
                self.station_buffering(buffering.percent());
                true
            }
            MessageView::Tag(tag) => {
                self.station_tags(&tag.tags());
                true
            }
            MessageView::StreamStart(_) => {
                if let Some(session) = self.station.as_mut() {
                    session.now.connecting = false;
                }
                self.publish_state();
                true
            }
            _ => false,
        }
    }

    /// GStreamer's rule for network streams: hold the pipeline paused below
    /// 100 % and resume once full, unless the user paused meanwhile.
    fn station_buffering(&mut self, percent: i32) {
        let playing = self.playback().status() == Status::Playing;
        let Some(session) = self.station.as_mut() else {
            return;
        };
        let percent = percent.clamp(0, 100) as u8;
        let shown = session.now.buffering;
        if percent < 100 {
            if playing && !session.buffering {
                session.buffering = true;
                buffer_hold(&self.playbin, gstreamer::State::Paused);
            }
            session.now.buffering = Some(percent);
        } else {
            session.now.buffering = None;
            session.now.connecting = false;
            if session.buffering {
                session.buffering = false;
                if playing {
                    buffer_hold(&self.playbin, gstreamer::State::Playing);
                }
            }
        }
        // Publish in steps of 10 %, not on every message.
        let step = |value: Option<u8>| value.map(|p| p / 10);
        if step(shown) != step(session.now.buffering) {
            self.publish_state();
        }
    }

    fn station_tags(&mut self, tags: &gstreamer::TagList) {
        let Some(session) = self.station.as_mut() else {
            return;
        };
        let mut changed = false;
        if let Some(title) = tags.get::<gstreamer::tags::Title>() {
            let title = title.get().trim().to_string();
            let artist_tag = tags
                .get::<gstreamer::tags::Artist>()
                .map(|artist| artist.get().to_string());
            // Artist and title together identify the song.
            let raw = match &artist_tag {
                Some(artist) => format!("{artist}\u{1f}{title}"),
                None => title.clone(),
            };
            if !title.is_empty() && session.raw_title.as_deref() != Some(raw.as_str()) {
                session.finish_song(&self.scrobbler, true);
                let (artist, title) = song_from_tags(&title, artist_tag.as_deref());
                session.song = title.clone().map(|title| RadioSong {
                    artist: artist.clone(),
                    title,
                    started_at: now(),
                    heard_ms: 0,
                    from_start: session.titled,
                    scrobbled: false,
                });
                session.song_generation = self.scrobbler.generation();
                session.titled = true;
                session.raw_title = Some(raw);
                session.now.artist = artist;
                session.now.title = title;
                changed = true;
            }
        }
        // The saved details win; tags fill in what the station left out.
        if session.now.codec.is_none() {
            if let Some(codec) = tags.get::<gstreamer::tags::AudioCodec>() {
                session.now.codec = Some(codec.get().to_string());
                changed = true;
            }
        }
        if session.now.bitrate.is_none() {
            let bits = tags
                .get::<gstreamer::tags::NominalBitrate>()
                .map(|b| b.get())
                .or_else(|| tags.get::<gstreamer::tags::Bitrate>().map(|b| b.get()));
            if let Some(bits) = bits.filter(|&b| b > 0) {
                session.now.bitrate = Some(i64::from(bits.div_ceil(1000)));
                changed = true;
            }
        }
        if changed {
            self.publish_state();
        }
    }

    /// A dropped or refused stream. Try again after a growing pause, and stop
    /// with a message after MAX_RECONNECTS failures in a row.
    fn station_failed(&mut self, reason: &str) {
        let Some(session) = self.station.as_mut() else {
            return;
        };
        // One failure usually posts two errors; a reconnect is already due.
        if session.reconnect_at.is_some() {
            return;
        }
        session.forget_titles(&self.scrobbler);
        session.reconnects += 1;
        session.stable_ms = 0;
        session.buffering = false;
        session.now.buffering = None;
        if session.reconnects > MAX_RECONNECTS {
            let name = session.now.name.clone();
            let _ = self
                .app
                .emit("radio:error", format!("{name} stopped playing: {reason}"));
            self.stop();
            return;
        }
        let delay = Duration::from_secs(2 * u64::from(session.reconnects));
        session.reconnect_at = Some(Instant::now() + delay);
        session.now.connecting = true;
        log::warn!(
            "radio station {} failed ({reason}); reconnecting in {delay:?}",
            session.now.name
        );
        self.stop_streams(gstreamer::State::Ready);
        self.flush_bus();
        self.publish_state();
    }

    /// Called on the position tick: reconnect when it is time.
    pub(super) fn station_tick(&mut self) {
        let due = self
            .station
            .as_ref()
            .and_then(|session| session.reconnect_at)
            .is_some_and(|at| Instant::now() >= at);
        if !due || self.playback().status() != Status::Playing {
            return;
        }
        let Some(session) = self.station.as_mut() else {
            return;
        };
        session.reconnect_at = None;
        session.last_sample = Instant::now();
        self.playbin
            .set_property("uri", session.stream_url.as_str());
        let _ = self.playbin.set_state(gstreamer::State::Playing);
    }

    /// Count the time the announced song is heard, and scrobble it once it has
    /// played four minutes.
    pub(super) fn sample_station(&mut self) {
        let playing = self.playback().status() == Status::Playing
            && self.playbin.current_state() == gstreamer::State::Playing;
        let Some(session) = self.station.as_mut() else {
            return;
        };
        // A suspended machine must not credit hours of listening.
        let elapsed = (session.last_sample.elapsed().as_millis() as u64).min(1000);
        session.last_sample = Instant::now();
        if !playing || session.buffering || session.reconnect_at.is_some() {
            return;
        }
        session.stable_ms += elapsed;
        if session.stable_ms >= STABLE_MS {
            session.reconnects = 0;
        }
        let generation = session.song_generation;
        if let Some(song) = session.song.as_mut() {
            song.heard_ms += elapsed;
            if song.scrobble_due(false) {
                song.scrobbled = true;
                if let Err(error) =
                    self.scrobbler
                        .enqueue(&song.as_track(), song.started_at, generation)
                {
                    log::warn!("Cannot queue Last.fm scrobble: {error}");
                }
            }
        }
    }

    /// The station to publish: the loaded one, else, when stopped, the one
    /// Play would resume.
    pub(super) fn station_now(&self, status: Status) -> Option<StationNow> {
        if let Some(session) = &self.station {
            return Some(session.now.clone());
        }
        if status == Status::Stopped {
            return self.last_station.as_ref().map(StationNow::from_station);
        }
        None
    }

    /// Drop what the old stream posted, as `start_track` does.
    pub(super) fn flush_bus(&self) {
        if let Some(bus) = self.playbin.bus() {
            bus.set_flushing(true);
            bus.set_flushing(false);
        }
    }
}
