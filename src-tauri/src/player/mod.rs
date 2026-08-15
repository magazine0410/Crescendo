//! GStreamer playback engine. One dedicated thread owns a `playbin3`;
//! commands arrive over an mpsc channel, state changes and a 250 ms position
//! tick are emitted as Tauri events. Gapless playback comes from setting the
//! next uri inside `about-to-finish`.

use crescendo_core::db::Db;
use crescendo_core::model::{PlayContext, Track};
use crescendo_core::playqueue::{PlayQueue, RepeatMode};
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const MAX_PROJECTM_PCM_BYTES: usize = 128 * 1024;

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
    /// Toggle PCM frames for projectM. The tap is a side branch, resampled to
    /// projectM's expected 44.1 kHz without changing the audible stream.
    SetProjectMEnabled(bool),
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

/// Coordinates the streaming thread's gapless handoff with explicit track
/// changes on the player thread.
#[derive(Default)]
struct HandoffCoordinator {
    generation: u64,
    manual_transition: bool,
}

impl HandoffCoordinator {
    fn snapshot(&self) -> Option<u64> {
        (!self.manual_transition).then_some(self.generation)
    }

    fn accepts(&self, generation: u64) -> bool {
        !self.manual_transition && self.generation == generation
    }

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    fn begin_manual(&mut self) {
        self.invalidate();
        self.manual_transition = true;
    }

    fn finish_manual(&mut self) {
        self.manual_transition = false;
    }
}

fn advance_for_transition(queue: &mut PlayQueue, gapless_pending: bool) -> Option<i64> {
    if gapless_pending {
        queue.current()
    } else {
        queue.advance()
    }
}

#[derive(Clone)]
pub struct PlayerHandle {
    tx: Sender<Cmd>,
    shared: Arc<Mutex<PlayerState>>,
    queue_shared: Arc<Mutex<Vec<i64>>>,
    projectm_pcm: Arc<Mutex<VecDeque<Vec<u8>>>>,
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
    /// Drain the small bounded PCM handoff queue. The Tauri command returns
    /// this as an ArrayBuffer, avoiding JSON number-array serialization.
    pub fn take_projectm_pcm(&self) -> Vec<u8> {
        let mut chunks = self
            .projectm_pcm
            .lock()
            .expect("projectM PCM queue poisoned");
        let len = chunks.iter().map(Vec::len).sum();
        let mut out = Vec::with_capacity(len);
        for chunk in chunks.drain(..) {
            out.extend_from_slice(&chunk);
        }
        out
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
    let projectm_pcm = Arc::new(Mutex::new(VecDeque::new()));

    let handle = PlayerHandle {
        tx,
        shared: shared.clone(),
        queue_shared: queue_shared.clone(),
        projectm_pcm: projectm_pcm.clone(),
    };
    std::thread::Builder::new()
        .name("player".into())
        .spawn(move || {
            if let Err(e) = run_player(app, db, rx, shared, queue_shared, projectm_pcm) {
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
    /// The appsink callback reads this without touching the player thread.
    projectm_enabled: Arc<AtomicBool>,
    projectm_pcm: Arc<Mutex<VecDeque<Vec<u8>>>>,
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
    /// Serializes queue/URI gapless handoffs with explicit track changes.
    handoff: Arc<Mutex<HandoffCoordinator>>,
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
    projectm_pcm: Arc<Mutex<VecDeque<Vec<u8>>>>,
) -> anyhow::Result<()> {
    let playbin = gstreamer::ElementFactory::make("playbin3").build()?;

    // One audio-filter bin serves both visualizers. The audible branch is a
    // passthrough spectrum element and therefore preserves the decoded format.
    // A leaky side branch converts/resamples only a copy for projectM, so its
    // 44.1 kHz S16 input requirement never reduces playback fidelity. Its
    // queue is bounded by three seconds of timestamps (about 517 KiB of stereo
    // S16) and drops visualizer data if it ever fills, so this branch can never
    // stall the audible one. The appsink must remain clock-synchronized:
    // without `sync=true`, it consumes playbin's decode-ahead PCM roughly
    // 1.5 seconds before the audio sink actually plays the same samples. It
    // must also use `async=false`: the gate starts closed, so an asynchronous
    // appsink would wait forever for a preroll buffer and hold playbin in Ready.
    // The gate is a buffer-only pad probe rather than a valve: flush, segment,
    // and EOS events must continue downstream so seeks clear the leaky queue.
    let audio_filter = gstreamer::parse::bin_from_description(
        "tee name=visualizer_tee \
         visualizer_tee. ! queue max-size-buffers=3 max-size-bytes=0 max-size-time=0 \
           ! spectrum name=spectrum bands=512 interval=33000000 threshold=-80 \
             post-messages=false multi-channel=false \
         visualizer_tee. ! identity name=projectm_gate \
           ! queue leaky=downstream max-size-buffers=0 \
             max-size-bytes=0 max-size-time=3000000000 \
           ! audioconvert ! audioresample \
           ! audio/x-raw,format=S16LE,layout=interleaved,rate=44100,channels=2 \
           ! appsink name=projectm_pcm sync=true async=false max-buffers=2 drop=true",
        true,
    )?;
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

    let queue = Arc::new(Mutex::new(PlayQueue::default()));
    let gapless_pending = Arc::new(Mutex::new(false));
    let handoff = Arc::new(Mutex::new(HandoffCoordinator::default()));
    let stop_after_current = Arc::new(AtomicBool::new(false));

    // Gapless: when the current track is about to end, hand playbin the next
    // uri synchronously from the streaming thread.
    {
        let queue = queue.clone();
        let db = db.clone();
        let gapless_pending = gapless_pending.clone();
        let handoff = handoff.clone();
        let stop_after_current = stop_after_current.clone();
        playbin.connect("about-to-finish", false, move |values| {
            // "Stop after current": skip the handoff so the stream ends and
            // EOS fires; the EOS handler does the actual stop.
            if stop_after_current.load(Ordering::SeqCst) {
                return None;
            }
            let playbin = values[0].get::<gstreamer::Element>().ok()?;
            // Resolve the path without holding either playback lock. The
            // generation check below rejects this candidate if a command moved
            // or changed the queue while the lookup was in flight.
            let (generation, next_id) = {
                let coordinator = handoff.lock().ok()?;
                let generation = coordinator.snapshot()?;
                let next_id = queue.lock().ok()?.peek_advance()?;
                (generation, next_id)
            };
            let track = db.track_by_id(next_id).ok().flatten()?;
            if let Ok(uri) = gstreamer::glib::filename_to_uri(&track.path, None) {
                let coordinator = handoff.lock().ok()?;
                if !coordinator.accepts(generation) || stop_after_current.load(Ordering::SeqCst) {
                    return None;
                }
                let mut live_queue = queue.lock().ok()?;
                let mut pending = gapless_pending.lock().ok()?;
                if live_queue.peek_advance() != Some(next_id) {
                    log::error!("gapless queue changed without advancing its generation");
                    return None;
                }
                if live_queue.advance() != Some(next_id) {
                    log::error!("gapless queue peek and advance disagreed");
                    return None;
                }
                playbin.set_property("uri", uri.as_str());
                *pending = true;
            }
            None
        });
    }

    let mut engine = Engine {
        playbin,
        spectrum,
        projectm_enabled,
        projectm_pcm,
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
        handoff,
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

#[cfg(test)]
mod tests {
    use super::{
        advance_for_transition, enqueue_projectm_pcm, HandoffCoordinator, MAX_PROJECTM_PCM_BYTES,
    };
    use crescendo_core::playqueue::PlayQueue;
    use std::collections::VecDeque;

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

    #[test]
    fn manual_transition_suppresses_gapless_handoffs() {
        let mut handoff = HandoffCoordinator::default();
        let stale = handoff.snapshot().unwrap();

        handoff.begin_manual();
        assert!(!handoff.accepts(stale));
        assert!(handoff.snapshot().is_none());

        handoff.finish_manual();
        let fresh = handoff.snapshot().unwrap();
        assert_ne!(fresh, stale);
        assert!(handoff.accepts(fresh));
    }

    #[test]
    fn next_does_not_skip_a_handoff_that_is_not_audible_yet() {
        let mut queue = PlayQueue::default();
        queue.set_context(vec![10, 20, 30], 0);
        assert_eq!(queue.advance(), Some(20)); // gapless callback reserved 20

        assert_eq!(advance_for_transition(&mut queue, true), Some(20));
        assert_eq!(queue.current(), Some(20));
        assert_eq!(advance_for_transition(&mut queue, false), Some(30));
    }
}

impl Engine {
    /// Bar gapless callbacks, then mutate the queue. The player thread is the
    /// only caller, so one manual transition cannot overlap another.
    fn begin_manual_transition<T>(
        &self,
        mutate: impl FnOnce(&mut PlayQueue, bool) -> T,
    ) -> T {
        let mut handoff = self.handoff.lock().expect("handoff coordinator poisoned");
        handoff.begin_manual();
        let was_gapless = {
            let mut pending = self.gapless_pending.lock().expect("gapless flag poisoned");
            std::mem::take(&mut *pending)
        };
        let mut queue = self.queue.lock().expect("queue poisoned");
        mutate(&mut queue, was_gapless)
    }

    /// Queue-only changes take the same lock as the callback's queue/URI pair.
    fn mutate_queue<T>(&self, mutate: impl FnOnce(&mut PlayQueue) -> T) -> T {
        let mut handoff = self.handoff.lock().expect("handoff coordinator poisoned");
        handoff.invalidate();
        let mut queue = self.queue.lock().expect("queue poisoned");
        mutate(&mut queue)
    }

    fn finish_manual_transition(&self) {
        self.handoff
            .lock()
            .expect("handoff coordinator poisoned")
            .finish_manual();
    }

    fn handle_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::PlayContext { ctx, start_index } => {
                match self.db.resolve_context(&ctx) {
                    Ok(ids) if !ids.is_empty() => {
                        let id = self.begin_manual_transition(|q, _| {
                            q.set_context(ids, start_index);
                            q.current()
                        });
                        if let Some(id) = id {
                            self.start_track(id);
                        } else {
                            self.finish_manual_transition();
                        }
                    }
                    Ok(_) => log::warn!("play_context resolved to an empty track list"),
                    Err(e) => log::error!("resolve_context failed: {e}"),
                }
            }
            Cmd::JumpTo(display_index) => {
                let id = self.begin_manual_transition(|q, _| q.jump_to(display_index));
                if let Some(id) = id {
                    self.start_track(id);
                } else {
                    self.finish_manual_transition();
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
                    let id = self.begin_manual_transition(|q, _| {
                        q.current().or_else(|| q.advance())
                    });
                    if let Some(id) = id {
                        self.start_track(id);
                    } else {
                        self.finish_manual_transition();
                    }
                }
            },
            Cmd::Stop => self.stop(),
            Cmd::Next => {
                // The callback may already have queued the next URI while the
                // outgoing track is still audible. In that case its queue entry
                // is the track Next should start, not one to skip over again.
                let id = self.begin_manual_transition(advance_for_transition);
                self.sync_queue_shared();
                match id {
                    Some(id) => self.start_track(id),
                    None => self.stop(),
                }
            }
            Cmd::Prev => {
                let id = self.begin_manual_transition(|q, _| q.previous());
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
                self.mutate_queue(|q| q.set_shuffle(on));
                self.publish_state();
            }
            Cmd::CycleRepeat => {
                self.mutate_queue(|q| {
                    q.cycle_repeat();
                });
                self.publish_state();
            }
            Cmd::SetStopAfterCurrent(on) => {
                let mut handoff = self.handoff.lock().expect("handoff coordinator poisoned");
                handoff.invalidate();
                self.stop_after_current.store(on, Ordering::SeqCst);
                drop(handoff);
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
                self.mutate_queue(|q| q.queue_add(&ids));
                self.sync_queue_shared();
            }
            Cmd::QueueRemove(positions) => {
                self.mutate_queue(|q| q.queue_remove(&positions));
                self.sync_queue_shared();
            }
            Cmd::QueueClear => {
                self.mutate_queue(PlayQueue::queue_clear);
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
                let id = self.begin_manual_transition(advance_for_transition);
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
                let id = self.begin_manual_transition(advance_for_transition);
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
            self.finish_manual_transition();
            return;
        };
        let Ok(uri) = gstreamer::glib::filename_to_uri(&track.path, None) else {
            log::error!("cannot build uri for {}", track.path);
            self.finish_manual_transition();
            return;
        };
        let _ = self.playbin.set_state(gstreamer::State::Ready);
        {
            // about-to-finish performs its queue decision and URI write under
            // this lock, so neither side can land a stale URI after the other.
            let mut handoff = self.handoff.lock().expect("handoff coordinator poisoned");
            self.playbin.set_property("uri", uri.as_str());
            handoff.finish_manual();
        }
        let _ = self.playbin.set_state(gstreamer::State::Playing);
        self.status = Status::Playing;
        self.current_duration_ms = track.duration_ms.max(0) as u64;
        self.current_track_id = Some(track_id);
        self.max_position_ms = 0;
        self.publish_state();
        self.emit_position();
    }

    fn stop(&mut self) {
        self.begin_manual_transition(|_, _| ());
        self.commit_play();
        let _ = self.playbin.set_state(gstreamer::State::Null);
        self.finish_manual_transition();
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
