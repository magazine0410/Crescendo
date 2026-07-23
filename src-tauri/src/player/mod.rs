//! GStreamer playback engine. One dedicated thread owns a `playbin3`;
//! commands arrive over an mpsc channel, state changes and a 250 ms position
//! tick are emitted as Tauri events. Gapless playback comes from setting the
//! next uri inside `about-to-finish`.

use crescendo_core::db::Db;
use crescendo_core::model::{PlayContext, Track};
use crescendo_core::playqueue::{PlayQueue, RepeatMode};
use gstreamer::prelude::*;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

#[derive(Debug)]
pub enum Cmd {
    PlayContext { ctx: PlayContext, start_index: usize },
    JumpTo(usize),
    TogglePause,
    Stop,
    Next,
    Prev,
    Seek(u64),
    SetVolume(f64),
    SetMuted(bool),
    SetShuffle(bool),
    CycleRepeat,
    SetStopAfterCurrent(bool),
    /// Toggle spectrum-analysis messages on the audio-filter; the frontend
    /// enables this only while the Visualizer tab is visible.
    SetSpectrumEnabled(bool),
    QueueAdd(Vec<i64>),
    QueueRemove(Vec<usize>),
    QueueClear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlayerState {
    pub status: Status,
    pub track: Option<Track>,
    pub context_index: Option<usize>,
    pub context_len: usize,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub volume: f64,
    pub muted: bool,
    /// One-shot: stop playback when the current track finishes, then reset.
    pub stop_after_current: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PositionEvent {
    pub ms: u64,
    pub duration_ms: u64,
}

/// Spectrum-analyser frame. `bands` are per-band magnitudes (dB), low frequency
/// first; `rate` is the audio sample rate the FFT ran at, so the frontend can
/// label bars in Hz correctly regardless of the file's rate.
#[derive(Debug, Clone, Serialize)]
pub struct SpectrumEvent {
    pub bands: Vec<f32>,
    pub rate: u32,
}

/// Emitted when a track crosses the "played" threshold and its counter is bumped.
#[derive(Debug, Clone, Serialize)]
pub struct PlayRecorded {
    pub id: i64,
    pub plays: i64,
}

#[derive(Clone)]
pub struct PlayerHandle {
    tx: Sender<Cmd>,
    shared: Arc<Mutex<PlayerState>>,
    queue_shared: Arc<Mutex<Vec<i64>>>,
}

impl PlayerHandle {
    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
    pub fn state(&self) -> PlayerState {
        self.shared.lock().expect("player state poisoned").clone()
    }
    pub fn queue_ids(&self) -> Vec<i64> {
        self.queue_shared.lock().expect("queue state poisoned").clone()
    }
}

pub fn spawn(app: AppHandle, db: Arc<Db>) -> anyhow::Result<PlayerHandle> {
    gstreamer::init()?;
    let (tx, rx) = channel::<Cmd>();
    let shared = Arc::new(Mutex::new(PlayerState {
        status: Status::Stopped,
        track: None,
        context_index: None,
        context_len: 0,
        shuffle: false,
        repeat: RepeatMode::Off,
        volume: 0.8,
        muted: false,
        stop_after_current: false,
    }));
    let queue_shared = Arc::new(Mutex::new(Vec::new()));

    let handle = PlayerHandle { tx, shared: shared.clone(), queue_shared: queue_shared.clone() };
    std::thread::Builder::new()
        .name("player".into())
        .spawn(move || {
            if let Err(e) = run_player(app, db, rx, shared, queue_shared) {
                log::error!("player thread died: {e}");
            }
        })?;
    Ok(handle)
}

struct Engine {
    playbin: gstreamer::Element,
    /// `spectrum` element spliced into playbin's audio path as its audio-filter;
    /// posts per-band magnitudes on the bus when `post-messages` is on.
    spectrum: gstreamer::Element,
    app: AppHandle,
    db: Arc<Db>,
    queue: Arc<Mutex<PlayQueue>>,
    shared: Arc<Mutex<PlayerState>>,
    queue_shared: Arc<Mutex<Vec<i64>>>,
    /// UI volume 0..1 (perceptual); playbin gets volume^3.
    volume: f64,
    muted: bool,
    status: Status,
    current_duration_ms: u64,
    /// The track currently loaded in playbin, tracked so its play can be
    /// credited when it is replaced, skipped, or ends.
    current_track_id: Option<i64>,
    /// Highest position (ms) reached in the current track. Sampled on the tick
    /// so a gapless handoff (which resets the pipeline position) still knows how
    /// far the outgoing track got.
    max_position_ms: u64,
    /// Set inside about-to-finish; consumed on the next stream-start.
    gapless_pending: Arc<Mutex<bool>>,
    /// One-shot "stop after this song": suppresses the gapless handoff so EOS
    /// fires, then the EOS handler stops and clears it. Shared with the
    /// about-to-finish callback (streaming thread), hence atomic.
    stop_after_current: Arc<AtomicBool>,
}

fn run_player(
    app: AppHandle,
    db: Arc<Db>,
    rx: Receiver<Cmd>,
    shared: Arc<Mutex<PlayerState>>,
    queue_shared: Arc<Mutex<Vec<i64>>>,
) -> anyhow::Result<()> {
    let playbin = gstreamer::ElementFactory::make("playbin3").build()?;

    // Spectrum analyser spliced into the audio path as a passthrough
    // audio-filter. It starts silent (post-messages off) and the frontend
    // turns it on only while the Visualizer tab is visible, so the FFT costs
    // nothing the rest of the time. `bands` is the FFT resolution (log-binned
    // into display bars on the frontend); `interval` sets the message rate.
    let spectrum = gstreamer::ElementFactory::make("spectrum")
        .property("bands", 512u32)
        .property("interval", 33_000_000u64) // ns -> ~30 messages/sec
        .property("threshold", -80i32) // dB noise floor
        .property("post-messages", false)
        .property("multi-channel", false) // single magnitude array (channels averaged)
        .build()?;
    playbin.set_property("audio-filter", &spectrum);

    let queue = Arc::new(Mutex::new(PlayQueue::default()));
    let gapless_pending = Arc::new(Mutex::new(false));
    let stop_after_current = Arc::new(AtomicBool::new(false));

    // Gapless: when the current track is about to end, hand playbin the next
    // uri synchronously from the streaming thread.
    {
        let queue = queue.clone();
        let db = db.clone();
        let gapless_pending = gapless_pending.clone();
        let stop_after_current = stop_after_current.clone();
        playbin.connect("about-to-finish", false, move |values| {
            // "Stop after current": skip the handoff so the stream ends and
            // EOS fires; the EOS handler does the actual stop.
            if stop_after_current.load(Ordering::SeqCst) {
                return None;
            }
            let playbin = values[0].get::<gstreamer::Element>().ok()?;
            let next_id = queue.lock().ok()?.advance()?;
            let track = db.track_by_id(next_id).ok().flatten()?;
            if let Ok(uri) = gstreamer::glib::filename_to_uri(&track.path, None) {
                playbin.set_property("uri", uri.as_str());
                if let Ok(mut pending) = gapless_pending.lock() {
                    *pending = true;
                }
            }
            None
        });
    }

    let mut engine = Engine {
        playbin,
        spectrum,
        app,
        db,
        queue,
        shared,
        queue_shared,
        volume: 0.8,
        muted: false,
        status: Status::Stopped,
        current_duration_ms: 0,
        current_track_id: None,
        max_position_ms: 0,
        gapless_pending,
        stop_after_current,
    };
    engine.apply_volume();

    let bus = engine.playbin.bus().expect("playbin has no bus");
    let mut last_tick = Instant::now();

    loop {
        // Commands (non-blocking)
        loop {
            match rx.try_recv() {
                Ok(cmd) => engine.handle_cmd(cmd),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
        // Bus messages (blocks up to 50 ms — this is the loop's pacing).
        // `timed_pop` returns as soon as a message is available, so after the
        // first one drain the rest immediately: the spectrum element posts
        // ~30/s and we don't want its messages to back up behind the timeout.
        if let Some(msg) = bus.timed_pop(gstreamer::ClockTime::from_mseconds(50)) {
            engine.handle_bus(&msg);
            while let Some(msg) = bus.pop() {
                engine.handle_bus(&msg);
            }
        }
        // Position tick
        if engine.status == Status::Playing && last_tick.elapsed() >= Duration::from_millis(250) {
            last_tick = Instant::now();
            engine.max_position_ms = engine.max_position_ms.max(engine.position_ms());
            engine.emit_position();
        }
    }
}

impl Engine {
    fn handle_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::PlayContext { ctx, start_index } => {
                match self.db.resolve_context(&ctx) {
                    Ok(ids) if !ids.is_empty() => {
                        let mut q = self.queue.lock().expect("queue poisoned");
                        q.set_context(ids, start_index);
                        let id = q.current();
                        drop(q);
                        if let Some(id) = id {
                            self.start_track(id);
                        }
                    }
                    Ok(_) => log::warn!("play_context resolved to an empty track list"),
                    Err(e) => log::error!("resolve_context failed: {e}"),
                }
            }
            Cmd::JumpTo(display_index) => {
                let id = self.queue.lock().expect("queue poisoned").jump_to(display_index);
                if let Some(id) = id {
                    self.start_track(id);
                }
            }
            Cmd::TogglePause => match self.status {
                Status::Playing => {
                    let _ = self.playbin.set_state(gstreamer::State::Paused);
                    self.status = Status::Paused;
                    self.publish_state();
                }
                Status::Paused => {
                    let _ = self.playbin.set_state(gstreamer::State::Playing);
                    self.status = Status::Playing;
                    self.publish_state();
                }
                Status::Stopped => {
                    // Play from the top of the context if there is one.
                    let id = {
                        let mut q = self.queue.lock().expect("queue poisoned");
                        q.current().or_else(|| q.advance())
                    };
                    if let Some(id) = id {
                        self.start_track(id);
                    }
                }
            },
            Cmd::Stop => self.stop(),
            Cmd::Next => {
                let id = self.queue.lock().expect("queue poisoned").advance();
                self.sync_queue_shared();
                match id {
                    Some(id) => self.start_track(id),
                    None => self.stop(),
                }
            }
            Cmd::Prev => {
                let id = self.queue.lock().expect("queue poisoned").previous();
                match id {
                    Some(id) => self.start_track(id),
                    None => self.stop(),
                }
            }
            Cmd::Seek(ms) => {
                let _ = self.playbin.seek_simple(
                    gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::ACCURATE,
                    gstreamer::ClockTime::from_mseconds(ms),
                );
                self.emit_position();
            }
            Cmd::SetVolume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                if self.volume > 0.0 {
                    self.muted = false;
                }
                self.apply_volume();
                self.publish_state();
            }
            Cmd::SetMuted(m) => {
                self.muted = m;
                self.apply_volume();
                self.publish_state();
            }
            Cmd::SetShuffle(on) => {
                self.queue.lock().expect("queue poisoned").set_shuffle(on);
                self.publish_state();
            }
            Cmd::CycleRepeat => {
                self.queue.lock().expect("queue poisoned").cycle_repeat();
                self.publish_state();
            }
            Cmd::SetStopAfterCurrent(on) => {
                self.stop_after_current.store(on, Ordering::SeqCst);
                self.publish_state();
            }
            Cmd::SetSpectrumEnabled(on) => {
                self.spectrum.set_property("post-messages", on);
            }
            Cmd::QueueAdd(ids) => {
                self.queue.lock().expect("queue poisoned").queue_add(&ids);
                self.sync_queue_shared();
            }
            Cmd::QueueRemove(positions) => {
                self.queue.lock().expect("queue poisoned").queue_remove(&positions);
                self.sync_queue_shared();
            }
            Cmd::QueueClear => {
                self.queue.lock().expect("queue poisoned").queue_clear();
                self.sync_queue_shared();
            }
        }
    }

    fn handle_bus(&mut self, msg: &gstreamer::Message) {
        use gstreamer::MessageView;
        match msg.view() {
            MessageView::Eos(_) => {
                // "Stop after current" armed: consume the one-shot flag and
                // stop instead of advancing. publish_state() (via stop) then
                // reports the flag cleared.
                if self.stop_after_current.swap(false, Ordering::SeqCst) {
                    self.stop();
                    return;
                }
                // Normal end without gapless handoff (e.g. repeat-off at the
                // end of the context).
                let id = self.queue.lock().expect("queue poisoned").advance();
                self.sync_queue_shared();
                match id {
                    Some(id) => self.start_track(id),
                    None => self.stop(),
                }
            }
            MessageView::Error(err) => {
                log::error!(
                    "playback error from {:?}: {} ({:?})",
                    err.src().map(|s| s.path_string()),
                    err.error(),
                    err.debug()
                );
                let _ = self.app.emit("player:error", err.error().to_string());
                // Skip to the next track rather than going silent.
                let id = self.queue.lock().expect("queue poisoned").advance();
                self.sync_queue_shared();
                match id {
                    Some(id) => self.start_track(id),
                    None => self.stop(),
                }
            }
            MessageView::Element(el) => {
                // Spectrum analyser output: one magnitude value (dB) per band,
                // low frequency first. Forward the raw array; the frontend
                // log-bins, normalizes and quantizes it into display bars.
                if let Some(s) = el.structure() {
                    if s.has_name("spectrum") {
                        if let Ok(list) = s.get::<gstreamer::List>("magnitude") {
                            let bands: Vec<f32> =
                                list.iter().filter_map(|v| v.get::<f32>().ok()).collect();
                            // Sample rate of the audio the FFT actually ran at,
                            // read from the analyser's negotiated input caps.
                            let rate = self
                                .spectrum
                                .static_pad("sink")
                                .and_then(|p| p.current_caps())
                                .and_then(|c| {
                                    c.structure(0).and_then(|st| st.get::<i32>("rate").ok())
                                })
                                .unwrap_or(44_100)
                                .max(1) as u32;
                            let _ = self.app.emit("player:spectrum", SpectrumEvent { bands, rate });
                        }
                    }
                }
            }
            MessageView::StreamStart(_) => {
                // Fires on every new stream; after a gapless handoff this is
                // the moment the queue's current track is actually audible.
                let was_gapless = {
                    let mut pending =
                        self.gapless_pending.lock().expect("gapless flag poisoned");
                    std::mem::take(&mut *pending)
                };
                if was_gapless {
                    // about-to-finish only fires near the natural end, so the
                    // outgoing track is credited here (its max position, sampled
                    // by the tick, still holds the near-end value). Commit before
                    // refreshing the duration to the new track.
                    self.commit_play();
                    self.sync_queue_shared();
                    self.refresh_current_duration();
                    self.current_track_id = self.queue.lock().expect("queue poisoned").current();
                    self.max_position_ms = 0;
                    self.publish_state();
                }
            }
            _ => {}
        }
    }

    fn start_track(&mut self, track_id: i64) {
        // Credit the outgoing track (still live, so its position is queryable)
        // before we tear its stream down.
        self.commit_play();
        let Ok(Some(track)) = self.db.track_by_id(track_id) else {
            log::error!("track {track_id} vanished from the library");
            return;
        };
        let Ok(uri) = gstreamer::glib::filename_to_uri(&track.path, None) else {
            log::error!("cannot build uri for {}", track.path);
            return;
        };
        let _ = self.playbin.set_state(gstreamer::State::Ready);
        self.playbin.set_property("uri", uri.as_str());
        let _ = self.playbin.set_state(gstreamer::State::Playing);
        self.status = Status::Playing;
        self.current_duration_ms = track.duration_ms.max(0) as u64;
        self.current_track_id = Some(track_id);
        self.max_position_ms = 0;
        self.publish_state();
        self.emit_position();
    }

    fn stop(&mut self) {
        self.commit_play();
        let _ = self.playbin.set_state(gstreamer::State::Null);
        self.status = Status::Stopped;
        self.current_duration_ms = 0;
        self.publish_state();
        let _ = self.app.emit("player:position", PositionEvent { ms: 0, duration_ms: 0 });
    }

    /// Count a play for the current track if more than half of it was heard,
    /// then clear the per-track progress. Idempotent: a second call before a new
    /// track starts is a no-op (the id has been taken).
    fn commit_play(&mut self) {
        let Some(id) = self.current_track_id.take() else { return };
        let pos = self.max_position_ms.max(self.position_ms());
        let dur = self.current_duration_ms;
        self.max_position_ms = 0;
        if dur > 0 && pos * 2 > dur {
            match self.db.record_play(id) {
                Ok(plays) => {
                    let _ = self.app.emit("play:recorded", PlayRecorded { id, plays });
                }
                Err(e) => log::warn!("record_play failed for track {id}: {e}"),
            }
        }
    }

    fn apply_volume(&self) {
        let effective = if self.muted { 0.0 } else { self.volume.powi(3) };
        self.playbin.set_property("volume", effective);
    }

    fn position_ms(&self) -> u64 {
        self.playbin
            .query_position::<gstreamer::ClockTime>()
            .map(|p| p.mseconds())
            .unwrap_or(0)
    }

    fn refresh_current_duration(&mut self) {
        let current = self.queue.lock().expect("queue poisoned").current();
        if let Some(id) = current {
            if let Ok(Some(t)) = self.db.track_by_id(id) {
                self.current_duration_ms = t.duration_ms.max(0) as u64;
            }
        }
    }

    fn emit_position(&self) {
        let _ = self.app.emit(
            "player:position",
            PositionEvent { ms: self.position_ms(), duration_ms: self.current_duration_ms },
        );
    }

    fn sync_queue_shared(&self) {
        let ids = self.queue.lock().expect("queue poisoned").queue_ids().to_vec();
        *self.queue_shared.lock().expect("queue state poisoned") = ids;
        let _ = self.app.emit("queue:changed", ());
    }

    fn publish_state(&self) {
        let q = self.queue.lock().expect("queue poisoned");
        let track = q.current().and_then(|id| self.db.track_by_id(id).ok().flatten());
        let state = PlayerState {
            status: self.status,
            context_index: q.context_index(),
            context_len: q.context_len(),
            shuffle: q.shuffle,
            repeat: q.repeat,
            volume: self.volume,
            muted: self.muted,
            stop_after_current: self.stop_after_current.load(Ordering::SeqCst),
            track,
        };
        drop(q);
        *self.shared.lock().expect("player state poisoned") = state.clone();
        let _ = self.app.emit("player:state", state);
    }
}
