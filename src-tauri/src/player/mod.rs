//! GStreamer playback engine. One dedicated thread owns a `playbin3`;
//! commands arrive over an mpsc channel, state changes and a 250 ms position
//! tick are emitted as Tauri events. Gapless playback comes from setting the
//! next uri inside `about-to-finish`. The transport state both threads share
//! is `crescendo_core::playback::Playback`; this module drives GStreamer from it.

mod audio_filter;

use crescendo_core::db::Db;
use crescendo_core::lastfm::scrobble::{now, Scrobbler};
use crescendo_core::listening::ListeningProgress;
use crescendo_core::model::{Track, MAX_SEEK_MS};
pub use crescendo_core::playback::Status;
use crescendo_core::playback::{Playback, StreamStarted};
use crescendo_core::playqueue::{PlayQueue, RepeatMode};
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks};
use serde::Serialize;
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const MAX_PROJECTM_PCM_BYTES: usize = 128 * 1024;

fn seek_clock_time(ms: u64) -> Option<gstreamer::ClockTime> {
    ms.checked_mul(1_000_000)
        .filter(|&ns| ns <= i64::MAX as u64)
        .map(gstreamer::ClockTime::from_nseconds)
}

fn enqueue_projectm_pcm(chunks: &mut VecDeque<Vec<u8>>, bytes: &[u8]) {
    // Keep complete stereo S16 frames and, defensively, cap even a single
    // unexpectedly large GStreamer buffer to the newest sub-second slice.
    let complete_len = bytes.len() & !3;
    let start = complete_len.saturating_sub(MAX_PROJECTM_PCM_BYTES);
    let newest = &bytes[start..complete_len];
    if newest.is_empty() {
        return;
    }
    let mut queued_bytes: usize = chunks.iter().map(Vec::len).sum();
    while queued_bytes.saturating_add(newest.len()) > MAX_PROJECTM_PCM_BYTES {
        let Some(oldest) = chunks.pop_front() else {
            break;
        };
        queued_bytes = queued_bytes.saturating_sub(oldest.len());
    }
    chunks.push_back(newest.to_vec());
}

#[derive(Debug)]
pub enum Cmd {
    /// Context descriptors are resolved at the command boundary while its
    /// projection guard is held. The player thread receives stable IDs only.
    PlayResolvedContext {
        ids: Vec<i64>,
        start_index: usize,
    },
    JumpTo(usize),
    TogglePause,
    Stop,
    Next,
    Prev,
    Seek(u64),
    SetVolume(f64),
    SetMuted(bool),
    /// Playback speed within RATE_MIN..=RATE_MAX; the pitch is kept.
    SetRate(f64),
    SetReplayGain(bool),
    /// Publish current database metadata after Forte saves without interrupting audio.
    RefreshMetadata,
    SetShuffle(bool),
    CycleRepeat,
    /// Set the repeat mode outright; MPRIS clients name a mode rather than cycling.
    SetRepeat(RepeatMode),
    SetStopAfterCurrent(bool),
    /// Toggle spectrum-analysis messages on the audio-filter; the frontend
    /// enables this only while the Visualizer tab is visible.
    SetSpectrumEnabled(bool),
    /// Toggle PCM frames for projectM. The tap is a side branch, resampled to
    /// projectM's expected 44.1 kHz without changing the audible stream.
    SetProjectMEnabled(bool),
    QueueAdd(Vec<i64>),
    /// `(position, id)` as the queue view showed them.
    QueueRemove(Vec<(usize, i64)>),
    QueueClear,
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
    /// Playback speed, 1.0 = normal (see RATE_MIN / RATE_MAX).
    pub rate: f64,
}

/// Bounds of the playback speed. scaletempo keeps the pitch across this range.
pub const RATE_MIN: f64 = 0.5;
pub const RATE_MAX: f64 = 2.0;

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
    failure: Arc<Mutex<Option<String>>>,
    queue_shared: Arc<Mutex<Vec<i64>>>,
    projectm_pcm: Arc<Mutex<VecDeque<Vec<u8>>>>,
}

impl PlayerHandle {
    pub fn send(&self, cmd: Cmd) -> Result<(), String> {
        if matches!(cmd, Cmd::Seek(ms) if ms > MAX_SEEK_MS) {
            return Err("seek position is out of range".into());
        }
        self.tx.send(cmd).map_err(|_| {
            self.failure
                .lock()
                .ok()
                .and_then(|failure| failure.clone())
                .unwrap_or_else(|| "the player thread is unavailable".to_string())
        })
    }
    pub fn state(&self) -> Result<PlayerState, String> {
        if let Some(failure) = self
            .failure
            .lock()
            .map_err(|_| "the player failure state is unavailable".to_string())?
            .clone()
        {
            return Err(failure);
        }
        self.shared
            .lock()
            .map(|state| state.clone())
            .map_err(|_| "the player state is unavailable".to_string())
    }
    pub fn queue_ids(&self) -> Result<Vec<i64>, String> {
        self.queue_shared
            .lock()
            .map(|ids| ids.clone())
            .map_err(|_| "the player queue is unavailable".to_string())
    }
    /// Drain the small bounded PCM handoff queue. The Tauri command returns
    /// this as an ArrayBuffer, avoiding JSON number-array serialization.
    pub fn take_projectm_pcm(&self) -> Result<Vec<u8>, String> {
        let mut chunks = self
            .projectm_pcm
            .lock()
            .map_err(|_| "the projectM PCM queue is unavailable".to_string())?;
        let len = chunks.iter().map(Vec::len).sum();
        let mut out = Vec::with_capacity(len);
        for chunk in chunks.drain(..) {
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }
}

fn report_player_failure(
    app: &AppHandle,
    shared: &Mutex<PlayerState>,
    failure: &Mutex<Option<String>>,
    message: String,
) {
    log::error!("{message}");
    *failure
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(message.clone());
    // Recover only the UI snapshot, whose fields can be replaced with a known
    // stopped state. Playback/queue locks are deliberately not recovered: a
    // panic may have broken their invariants and the player thread is gone.
    let final_state = {
        let mut state = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.status = Status::Stopped;
        state.track = None;
        state.context_index = None;
        state.context_len = 0;
        state.stop_after_current = false;
        state.clone()
    };
    let _ = app.emit("player:state", final_state);
    let _ = app.emit(
        "player:position",
        PositionEvent {
            ms: 0,
            duration_ms: 0,
        },
    );
    let _ = app.emit("player:fatal", message);
}

pub fn spawn(
    app: AppHandle,
    db: Arc<Db>,
    scrobbler: Arc<Scrobbler>,
) -> anyhow::Result<PlayerHandle> {
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
        rate: 1.0,
    }));
    let queue_shared = Arc::new(Mutex::new(Vec::new()));
    let projectm_pcm = Arc::new(Mutex::new(VecDeque::new()));
    let failure = Arc::new(Mutex::new(None));

    let handle = PlayerHandle {
        tx,
        shared: shared.clone(),
        failure: failure.clone(),
        queue_shared: queue_shared.clone(),
        projectm_pcm: projectm_pcm.clone(),
    };
    let failure_app = app.clone();
    let failure_shared = shared.clone();
    let failure_message = failure.clone();
    std::thread::Builder::new()
        .name("player".into())
        .spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                run_player(app, db, scrobbler, rx, shared, queue_shared, projectm_pcm)
            }));
            let failure = match outcome {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(format!("Player stopped unexpectedly: {error}")),
                Err(payload) => {
                    let detail = payload
                        .downcast_ref::<&str>()
                        .map(|s| (*s).to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".to_string());
                    Some(format!("Player stopped unexpectedly: {detail}"))
                }
            };
            if let Some(message) = failure {
                report_player_failure(&failure_app, &failure_shared, &failure_message, message);
            }
        })?;
    Ok(handle)
}

struct ListenSession {
    track: Track,
    started_at: i64,
    generation: Option<u64>,
    progress: ListeningProgress,
    last_sample: Instant,
    history_recorded: bool,
    scrobble_recorded: bool,
}

struct Engine {
    playbin: gstreamer::Element,
    replaygain_enabled: Arc<AtomicBool>,
    /// `spectrum` element on a separate analysis branch of the audio-filter;
    /// posts per-band magnitudes on the bus when `post-messages` is on.
    spectrum: gstreamer::Element,
    /// The appsink callback reads this without touching the player thread.
    projectm_enabled: Arc<AtomicBool>,
    projectm_pcm: Arc<Mutex<VecDeque<Vec<u8>>>>,
    app: AppHandle,
    db: Arc<Db>,
    scrobbler: Arc<Scrobbler>,
    listening: Option<ListenSession>,
    /// Shared with the about-to-finish callback (streaming thread). Never hold
    /// it across `set_state`, which can wait for that thread.
    playback: Arc<Mutex<Playback>>,
    shared: Arc<Mutex<PlayerState>>,
    queue_shared: Arc<Mutex<Vec<i64>>>,
    /// UI volume 0..1 (perceptual); playbin gets volume^3.
    volume: f64,
    muted: bool,
    /// Playback speed. Every new stream starts at 1.0, so it is re-applied
    /// once a started track has prerolled (`rate_pending`) and at the stream
    /// start of a gapless handoff.
    rate: f64,
    rate_pending: bool,
    /// Where to move a reloaded stream once it has prerolled; set exactly while
    /// `Playback` is reloading. See `seek_audible`.
    pending_seek: Option<gstreamer::ClockTime>,
    current_duration_ms: u64,
    /// Playback errors since a track last played for a second or the user
    /// last picked a track; see `Playback::failed`.
    consecutive_failures: usize,
}

fn run_player(
    app: AppHandle,
    db: Arc<Db>,
    scrobbler: Arc<Scrobbler>,
    rx: Receiver<Cmd>,
    shared: Arc<Mutex<PlayerState>>,
    queue_shared: Arc<Mutex<Vec<i64>>>,
    projectm_pcm: Arc<Mutex<VecDeque<Vec<u8>>>>,
) -> anyhow::Result<()> {
    let playbin = gstreamer::ElementFactory::make("playbin3").build()?;

    let replaygain_enabled = Arc::new(AtomicBool::new(db.replaygain_enabled()));
    let audio_filter = audio_filter::build(replaygain_enabled.clone())?;
    let spectrum = audio_filter
        .by_name("spectrum")
        .ok_or_else(|| anyhow::anyhow!("visualizer spectrum element is missing"))?;
    let projectm_gate = audio_filter
        .by_name("projectm_gate")
        .ok_or_else(|| anyhow::anyhow!("projectM gate element is missing"))?;
    let pcm_sink = audio_filter
        .by_name("projectm_pcm")
        .ok_or_else(|| anyhow::anyhow!("projectM PCM appsink is missing"))?
        .downcast::<AppSink>()
        .map_err(|_| anyhow::anyhow!("projectM PCM element is not an appsink"))?;
    let projectm_enabled = Arc::new(AtomicBool::new(false));
    // Drop only buffers: flush, segment and EOS must still reach the side
    // branch so inactive projectM cannot hold up seeks or track transitions.
    projectm_gate
        .static_pad("src")
        .ok_or_else(|| anyhow::anyhow!("projectM gate source pad is missing"))?
        .add_probe(
            gstreamer::PadProbeType::BUFFER | gstreamer::PadProbeType::BUFFER_LIST,
            {
                let enabled = projectm_enabled.clone();
                move |_pad, _info| {
                    if enabled.load(Ordering::Relaxed) {
                        gstreamer::PadProbeReturn::Ok
                    } else {
                        gstreamer::PadProbeReturn::Drop
                    }
                }
            },
        );
    pcm_sink.set_callbacks(
        AppSinkCallbacks::builder()
            .new_sample({
                let enabled = projectm_enabled.clone();
                let pcm = projectm_pcm.clone();
                move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                    if !enabled.load(Ordering::Relaxed) {
                        return Ok(gstreamer::FlowSuccess::Ok);
                    }
                    let buffer = sample.buffer().ok_or(gstreamer::FlowError::Error)?;
                    let map = buffer
                        .map_readable()
                        .map_err(|_| gstreamer::FlowError::Error)?;
                    // If the webview misses frames, discard the oldest visual
                    // data rather than building latency or memory.
                    let mut chunks = pcm.lock().map_err(|_| gstreamer::FlowError::Error)?;
                    enqueue_projectm_pcm(&mut chunks, map.as_slice());
                    Ok(gstreamer::FlowSuccess::Ok)
                }
            })
            .build(),
    );
    playbin.set_property("audio-filter", &audio_filter);

    let playback = Arc::new(Mutex::new(Playback::default()));

    // Gapless: when the current track is about to end, hand playbin the next
    // uri synchronously from the streaming thread.
    {
        let playback = playback.clone();
        let db = db.clone();
        playbin.connect("about-to-finish", false, move |values| {
            let playbin = values[0].get::<gstreamer::Element>().ok()?;
            // Resolve the path without holding the lock. The generation check
            // below refuses this candidate if a command moved or changed the
            // queue while the lookup was in flight.
            let (generation, next_id) = playback.lock().ok()?.handoff_candidate()?;
            let track = db.track_by_id(next_id).ok().flatten()?;
            let uri = gstreamer::glib::filename_to_uri(&track.path, None).ok()?;
            let mut playback = playback.lock().ok()?;
            if playback.reserve_handoff(generation) {
                playbin.set_property("uri", uri.as_str());
            }
            None
        });
    }

    let mut engine = Engine {
        playbin,
        replaygain_enabled,
        spectrum,
        projectm_enabled,
        projectm_pcm,
        app,
        db,
        scrobbler,
        listening: None,
        playback,
        shared,
        queue_shared,
        volume: 0.8,
        muted: false,
        rate: 1.0,
        rate_pending: false,
        pending_seek: None,
        current_duration_ms: 0,
        consecutive_failures: 0,
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
        if last_tick.elapsed() >= Duration::from_millis(250) {
            last_tick = Instant::now();
            engine.sample_listening();
            engine.emit_position();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        enqueue_projectm_pcm, seek_clock_time, Cmd, PlayerHandle, PlayerState, Status,
        MAX_PROJECTM_PCM_BYTES, MAX_SEEK_MS,
    };
    use crescendo_core::playqueue::RepeatMode;
    use std::collections::VecDeque;
    use std::sync::{mpsc::channel, Arc, Mutex};

    #[test]
    fn invalid_seeks_never_reach_the_player_and_conversion_cannot_panic() {
        let (tx, rx) = channel();
        let handle = PlayerHandle {
            tx,
            shared: Arc::new(Mutex::new(PlayerState {
                status: Status::Stopped,
                track: None,
                context_index: None,
                context_len: 0,
                shuffle: false,
                repeat: RepeatMode::Off,
                volume: 0.8,
                muted: false,
                stop_after_current: false,
                rate: 1.0,
            })),
            failure: Arc::new(Mutex::new(None)),
            queue_shared: Arc::new(Mutex::new(Vec::new())),
            projectm_pcm: Arc::new(Mutex::new(VecDeque::new())),
        };
        for ms in [MAX_SEEK_MS + 1, u64::MAX] {
            assert!(handle.send(Cmd::Seek(ms)).is_err());
            assert!(rx.try_recv().is_err());
            assert!(seek_clock_time(ms).is_none());
        }
        for ms in [0, 180_000, MAX_SEEK_MS] {
            handle.send(Cmd::Seek(ms)).unwrap();
            assert!(matches!(rx.try_recv(), Ok(Cmd::Seek(actual)) if actual == ms));
            assert_eq!(seek_clock_time(ms).unwrap().mseconds(), ms);
        }
    }

    #[test]
    fn disconnected_player_commands_return_an_error() {
        let (tx, rx) = channel();
        drop(rx);
        let handle = PlayerHandle {
            tx,
            shared: Arc::new(Mutex::new(PlayerState {
                status: Status::Stopped,
                track: None,
                context_index: None,
                context_len: 0,
                shuffle: false,
                repeat: RepeatMode::Off,
                volume: 0.8,
                muted: false,
                stop_after_current: false,
                rate: 1.0,
            })),
            failure: Arc::new(Mutex::new(None)),
            queue_shared: Arc::new(Mutex::new(Vec::new())),
            projectm_pcm: Arc::new(Mutex::new(VecDeque::new())),
        };
        assert!(handle.send(Cmd::Stop).is_err());
        *handle.failure.lock().unwrap() = Some("player initialization failed".into());
        assert_eq!(handle.state().unwrap_err(), "player initialization failed");
        assert_eq!(
            handle.send(Cmd::Stop).unwrap_err(),
            "player initialization failed"
        );
    }

    #[test]
    fn pcm_handoff_is_byte_bounded_and_keeps_newest_data() {
        let mut chunks = VecDeque::new();
        enqueue_projectm_pcm(&mut chunks, &vec![1; 48 * 1024]);
        enqueue_projectm_pcm(&mut chunks, &vec![2; 48 * 1024]);
        enqueue_projectm_pcm(&mut chunks, &vec![3; 48 * 1024]);
        assert_eq!(chunks.iter().map(Vec::len).sum::<usize>(), 96 * 1024);
        assert_eq!(chunks.front().unwrap()[0], 2);

        let oversized: Vec<u8> = (0..MAX_PROJECTM_PCM_BYTES + 8)
            .map(|index| (index % 251) as u8)
            .collect();
        enqueue_projectm_pcm(&mut chunks, &oversized);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), MAX_PROJECTM_PCM_BYTES);
        assert_eq!(chunks[0], oversized[8..]);
    }
}

impl Engine {
    fn playback(&self) -> MutexGuard<'_, Playback> {
        self.playback.lock().expect("playback poisoned")
    }

    /// Finish an explicit change: start the track it chose, or stop.
    fn go_to(&mut self, id: Option<i64>) {
        match id {
            Some(id) => self.start_track(id),
            None => self.stop(),
        }
    }

    fn handle_cmd(&mut self, cmd: Cmd) {
        self.sample_listening();
        if matches!(
            cmd,
            Cmd::PlayResolvedContext { .. }
                | Cmd::JumpTo(_)
                | Cmd::TogglePause
                | Cmd::Next
                | Cmd::Prev
        ) {
            // A track the user picks gets tried even after a run of failures.
            self.consecutive_failures = 0;
        }
        match cmd {
            Cmd::RefreshMetadata => self.publish_state(),
            Cmd::PlayResolvedContext { ids, start_index } => {
                let id = self.playback().play_context(ids, start_index);
                let Some(id) = id else {
                    log::warn!("play_context resolved to an empty track list");
                    return;
                };
                self.start_track(id);
            }
            Cmd::JumpTo(display_index) => {
                let id = self.playback().jump_to(display_index);
                let Some(id) = id else {
                    log::warn!("ignoring jump to a row outside the context");
                    return;
                };
                self.start_track(id);
            }
            Cmd::TogglePause => {
                let status = self.playback().status();
                match status {
                    // A reloading stream stays paused until it has moved, and
                    // the pause flag then decides whether it resumes.
                    Status::Playing => {
                        if self.pending_seek.is_none() {
                            let _ = self.playbin.set_state(gstreamer::State::Paused);
                        }
                        self.playback().set_paused(true);
                        self.publish_state();
                    }
                    Status::Paused => {
                        if self.pending_seek.is_none() {
                            let _ = self.playbin.set_state(gstreamer::State::Playing);
                        }
                        self.playback().set_paused(false);
                        self.publish_state();
                    }
                    Status::Stopped => {
                        let id = self.playback().play_from_stopped();
                        if let Some(id) = id {
                            self.start_track(id);
                        }
                    }
                }
            }
            Cmd::Stop => self.stop(),
            Cmd::Next => {
                let id = self.playback().next();
                self.go_to(id);
            }
            Cmd::Prev => {
                let id = self.playback().previous();
                self.go_to(id);
            }
            Cmd::Seek(ms) => {
                let Some(position) = seek_clock_time(ms) else {
                    log::warn!("ignoring out-of-range seek position");
                    return;
                };
                self.seek_audible(position);
                self.emit_position();
            }
            Cmd::SetRate(rate) => {
                if !rate.is_finite() {
                    log::warn!("ignoring a non-finite playback rate");
                    return;
                }
                self.rate = rate.clamp(RATE_MIN, RATE_MAX);
                let stopped = self.playback().status() == Status::Stopped;
                if !stopped {
                    self.apply_rate();
                    self.emit_position();
                }
                self.publish_state();
            }
            Cmd::SetReplayGain(enabled) => {
                self.replaygain_enabled.store(enabled, Ordering::Relaxed);
            }
            Cmd::SetVolume(v) => {
                // NaN survives clamp, and glib panics on it as a property.
                if !v.is_finite() {
                    log::warn!("ignoring a non-finite volume");
                    return;
                }
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
                self.playback().edit_queue(|q| q.set_shuffle(on));
                self.publish_state();
            }
            Cmd::CycleRepeat => {
                self.playback().edit_queue(|q| {
                    q.cycle_repeat();
                });
                self.publish_state();
            }
            Cmd::SetRepeat(mode) => {
                self.playback().edit_queue(|q| q.repeat = mode);
                self.publish_state();
            }
            Cmd::SetStopAfterCurrent(on) => {
                self.playback().set_stop_after_current(on);
                self.publish_state();
            }
            Cmd::SetSpectrumEnabled(on) => {
                self.spectrum.set_property("post-messages", on);
            }
            Cmd::SetProjectMEnabled(on) => {
                self.projectm_enabled.store(on, Ordering::Relaxed);
                if !on {
                    self.projectm_pcm
                        .lock()
                        .expect("projectM PCM queue poisoned")
                        .clear();
                }
            }
            Cmd::QueueAdd(ids) => {
                self.playback().edit_queue(|q| q.queue_add(&ids));
                self.sync_queue_shared();
            }
            Cmd::QueueRemove(items) => {
                self.playback().edit_queue(|q| q.queue_remove(&items));
                self.sync_queue_shared();
            }
            Cmd::QueueClear => {
                self.playback().edit_queue(PlayQueue::queue_clear);
                self.sync_queue_shared();
            }
        }
    }

    fn handle_bus(&mut self, msg: &gstreamer::Message) {
        use gstreamer::MessageView;
        match msg.view() {
            MessageView::Eos(_) => {
                // Normal end without gapless handoff (e.g. repeat-off at the
                // end of the context), or "stop after current" declining one.
                let id = self.playback().ended();
                self.go_to(id);
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
                self.consecutive_failures += 1;
                let failures = self.consecutive_failures;
                let id = self.playback().failed(failures);
                match id {
                    Some(id) => self.start_track(id),
                    None => {
                        log::warn!("stopping after {failures} playback errors in a row");
                        self.stop();
                    }
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
                            let _ = self
                                .app
                                .emit("player:spectrum", SpectrumEvent { bands, rate });
                        }
                    }
                }
            }
            MessageView::AsyncDone(_) => {
                if let Some(position) = self.pending_seek.take() {
                    self.seek_at_rate(position);
                    let resume = self.playback().reloaded();
                    if resume {
                        let _ = self.playbin.set_state(gstreamer::State::Playing);
                    }
                } else if self.rate_pending {
                    self.rate_pending = false;
                    self.apply_rate();
                }
            }
            MessageView::StreamStart(_) => {
                // Fires on every new stream; after a gapless handoff this is
                // the moment the reserved track is actually audible.
                let started = self.playback().stream_started();
                match started {
                    StreamStarted::Stop => self.stop(),
                    StreamStarted::HandedOver => {
                        // Finish the outgoing session before starting the next;
                        // queued scrobbles already contain a snapshot of its tags.
                        self.commit_play();
                        self.sync_queue_shared();
                        self.refresh_current_duration();
                        self.publish_state();
                        // The handed-over stream plays at normal speed on its own.
                        // The position still reads the outgoing stream's here, so
                        // this one is told to start from zero. A flushing seek
                        // with a further handoff reserved would jump into that
                        // one, so a file too short to have none plays at normal
                        // speed instead of being skipped.
                        let chained = self.playback().handoff_reserved();
                        if self.rate != 1.0 && !chained {
                            self.seek_at_rate(gstreamer::ClockTime::ZERO);
                        }
                        self.begin_listening();
                    }
                    StreamStarted::Opened => {
                        if self.listening.is_none() {
                            self.begin_listening();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn start_track(&mut self, track_id: i64) {
        self.pending_seek = None;
        // Credit the outgoing track (still live, so its position is queryable)
        // before we tear its stream down.
        self.commit_play();
        let Ok(Some(track)) = self.db.track_by_id(track_id) else {
            log::error!("track {track_id} vanished from the library");
            self.stop();
            return;
        };
        let Ok(uri) = gstreamer::glib::filename_to_uri(&track.path, None) else {
            log::error!("cannot build uri for {}", track.path);
            self.stop();
            return;
        };
        let _ = self.playbin.set_state(gstreamer::State::Ready);
        // READY stops the old stream's threads but, unlike NULL, keeps what
        // they already posted. A decoder usually posts two errors for one bad
        // file, and an EOS can be waiting behind a user command; read after
        // this point, either would be taken as the new track's and skip it.
        if let Some(bus) = self.playbin.bus() {
            bus.set_flushing(true);
            bus.set_flushing(false);
        }
        {
            // about-to-finish performs its queue decision and URI write under
            // this lock, so neither side can land a stale URI after the other.
            let mut playback = self.playback();
            self.playbin.set_property("uri", uri.as_str());
            playback.loaded(track_id);
        }
        let _ = self.playbin.set_state(gstreamer::State::Playing);
        // A rate seek needs a prerolled pipeline: wait for its async-done.
        self.rate_pending = self.rate != 1.0;
        self.current_duration_ms = track.duration_ms.max(0) as u64;
        // Starting a track may have popped the ad-hoc queue (Play, an error,
        // an undone handoff); the view's positions must match it again.
        self.sync_queue_shared();
        self.publish_state();
        self.emit_position();
    }

    fn stop(&mut self) {
        self.pending_seek = None;
        self.playback().begin_stop();
        self.commit_play();
        let _ = self.playbin.set_state(gstreamer::State::Null);
        self.playback().stopped();
        self.current_duration_ms = 0;
        self.sync_queue_shared();
        self.publish_state();
        let _ = self.app.emit(
            "player:position",
            PositionEvent {
                ms: 0,
                duration_ms: 0,
            },
        );
    }

    fn begin_listening(&mut self) {
        let current = self.playback().track();
        self.listening = current
            .and_then(|id| self.db.track_by_id(id).ok().flatten())
            .map(|track| ListenSession {
                track,
                started_at: now(),
                generation: self.scrobbler.generation(),
                progress: ListeningProgress::default(),
                last_sample: Instant::now(),
                history_recorded: false,
                scrobble_recorded: false,
            });
    }

    fn sample_listening(&mut self) {
        let position = self.position_ms();
        let playing = self.playback().status() == Status::Playing
            && self.playbin.current_state() == gstreamer::State::Playing;
        if playing && position >= 1000 {
            self.consecutive_failures = 0;
        }
        let Some(session) = &mut self.listening else {
            return;
        };
        let elapsed = session.last_sample.elapsed().as_millis() as u64;
        session.last_sample = Instant::now();
        session.progress.sample(position, elapsed, playing);
        let duration = session.track.duration_ms.max(0) as u64;
        if !session.history_recorded && session.progress.history_due(duration) {
            match self.db.record_play_at(session.track.id, session.started_at) {
                Ok(plays) => {
                    session.history_recorded = true;
                    let _ = self.app.emit(
                        "play:recorded",
                        PlayRecorded {
                            id: session.track.id,
                            plays,
                        },
                    );
                }
                Err(e) => log::warn!("record_play failed: {e}"),
            }
        }
        if !session.scrobble_recorded && session.progress.scrobble_due(duration) {
            match self
                .scrobbler
                .enqueue(&session.track, session.started_at, session.generation)
            {
                Ok(()) => session.scrobble_recorded = true,
                Err(e) => log::warn!("Cannot queue Last.fm scrobble: {e}"),
            }
        }
    }

    fn commit_play(&mut self) {
        self.sample_listening();
        self.listening = None;
    }

    /// Seek to where playback is, carrying the playback rate. Speed is a
    /// property of the seek segment, so this is how it is set and how a
    /// fresh stream, which starts at 1.0, is brought back to it.
    fn apply_rate(&mut self) {
        // A reloading stream takes the current rate when it is moved.
        if self.pending_seek.is_some() {
            return;
        }
        let position = self
            .playbin
            .query_position::<gstreamer::ClockTime>()
            .unwrap_or(gstreamer::ClockTime::ZERO);
        self.seek_audible(position);
    }

    /// Move within the audible track. With a handoff reserved, a flushing seek
    /// would jump into the next file at this offset instead, so reload this one
    /// first, paused so nothing is heard from its start.
    fn seek_audible(&mut self, position: gstreamer::ClockTime) {
        if self.pending_seek.is_some() {
            self.pending_seek = Some(position);
        } else if self.playback().handoff_reserved() {
            self.reload_at(position);
        } else {
            self.seek_at_rate(position);
        }
    }

    fn reload_at(&mut self, position: gstreamer::ClockTime) {
        let track = self.playback().track();
        let uri = track
            .and_then(|id| self.db.track_by_id(id).ok().flatten())
            .and_then(|track| gstreamer::glib::filename_to_uri(&track.path, None).ok());
        let Some(uri) = uri else {
            log::error!("cannot reload the playing track to seek in it");
            return;
        };
        if self.playback().begin_reload().is_none() {
            return;
        }
        let _ = self.playbin.set_state(gstreamer::State::Ready);
        // As in start_track: nothing the old stream posted applies to this one.
        if let Some(bus) = self.playbin.bus() {
            bus.set_flushing(true);
            bus.set_flushing(false);
        }
        self.playbin.set_property("uri", uri.as_str());
        let _ = self.playbin.set_state(gstreamer::State::Paused);
        // The move below carries the rate, and a stale position must not credit.
        self.rate_pending = false;
        self.pending_seek = Some(position);
        if let Some(session) = &mut self.listening {
            session.progress.reset_position();
        }
    }

    fn seek_at_rate(&mut self, position: gstreamer::ClockTime) {
        if let Some(session) = &mut self.listening {
            session.progress.reset_position();
        }
        if let Err(error) = self.playbin.seek(
            self.rate,
            gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::ACCURATE,
            gstreamer::SeekType::Set,
            position,
            gstreamer::SeekType::None,
            gstreamer::ClockTime::NONE,
        ) {
            log::warn!("cannot set the playback rate to {}: {error}", self.rate);
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
        let current = self.playback().queue().current();
        if let Some(id) = current {
            if let Ok(Some(t)) = self.db.track_by_id(id) {
                self.current_duration_ms = t.duration_ms.max(0) as u64;
            }
        }
    }

    fn emit_position(&self) {
        let _ = self.app.emit(
            "player:position",
            PositionEvent {
                // A reloading stream reads zero until it has moved.
                ms: self
                    .pending_seek
                    .map_or_else(|| self.position_ms(), |position| position.mseconds()),
                duration_ms: self.current_duration_ms,
            },
        );
    }

    fn sync_queue_shared(&self) {
        let ids = self.playback().queue().queue_ids().to_vec();
        *self.queue_shared.lock().expect("queue state poisoned") = ids;
        let _ = self.app.emit("queue:changed", ());
    }

    fn publish_state(&self) {
        // Snapshot the cheap queue fields first. A DB lookup may wait behind an
        // import, and holding the queue during that wait can make the streaming
        // thread miss its gapless handoff deadline.
        let (status, stop_after_current, track_id, context_index, context_len, shuffle, repeat) = {
            let playback = self.playback();
            let q = playback.queue();
            (
                playback.status(),
                playback.stop_after_current(),
                q.current(),
                q.context_index(),
                q.context_len(),
                q.shuffle,
                q.repeat,
            )
        };
        let track = track_id.and_then(|id| self.db.track_by_id(id).ok().flatten());
        let state = PlayerState {
            status,
            context_index,
            context_len,
            shuffle,
            repeat,
            volume: self.volume,
            muted: self.muted,
            stop_after_current,
            rate: self.rate,
            track,
        };
        *self.shared.lock().expect("player state poisoned") = state.clone();
        let _ = self.app.emit("player:state", state);
    }
}
