//! Silent checks of `Playback` against the message order of a real playbin3.
//! The model counts on one stream-start per file, in order, including for files
//! short enough to chain handoffs before any of them is heard.
//! Run: cargo test -p crescendo --test gapless_handoff

use crescendo_core::playback::{Playback, StreamStarted};
use crescendo_core::playqueue::RepeatMode;
use gst::prelude::*;
use gstreamer as gst;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Files {
    dir: PathBuf,
    uris: HashMap<i64, String>,
}

impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// One FLAC per `(id, milliseconds)`. Noise, because it stays as large on disk
/// as music does, and how far playbin reads ahead depends on that. The fake sink
/// keeps it silent.
fn flacs(name: &str, tracks: &[(i64, u64)]) -> anyhow::Result<Files> {
    gst::init()?;
    let dir = std::env::temp_dir().join(format!("crescendo-gapless-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let mut uris = HashMap::new();
    for &(id, ms) in tracks {
        let path = dir.join(format!("{id}.flac"));
        let pipeline = gst::parse::launch(&format!(
            "audiotestsrc wave=white-noise samplesperbuffer=441 num-buffers={} \
             ! audio/x-raw,format=S16LE,rate=44100,channels=2 \
             ! flacenc ! filesink location={}",
            ms / 10,
            path.display()
        ))?;
        pipeline.set_state(gst::State::Playing)?;
        let done = pipeline
            .bus()
            .unwrap()
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(10),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .is_some_and(|message| matches!(message.view(), gst::MessageView::Eos(_)));
        pipeline.set_state(gst::State::Null)?;
        anyhow::ensure!(done, "could not encode {}", path.display());
        uris.insert(id, gst::glib::filename_to_uri(&path, None)?.to_string());
    }
    Ok(Files { dir, uris })
}

/// The player's callback, with the database lookup replaced by a map.
fn player(uris: &HashMap<i64, String>) -> anyhow::Result<(gst::Element, Arc<Mutex<Playback>>)> {
    let playbin = gst::ElementFactory::make("playbin3").build()?;
    let sink = gst::ElementFactory::make("fakesink")
        .property("sync", true)
        .build()?;
    playbin.set_property("audio-sink", &sink);
    let playback = Arc::new(Mutex::new(Playback::default()));
    let callback_playback = playback.clone();
    let uris = uris.clone();
    playbin.connect("about-to-finish", false, move |values| {
        let playbin = values[0].get::<gst::Element>().ok()?;
        let (generation, next_id) = callback_playback.lock().ok()?.handoff_candidate()?;
        let uri = uris.get(&next_id)?;
        let mut playback = callback_playback.lock().ok()?;
        if playback.reserve_handoff(generation) {
            playbin.set_property("uri", uri.as_str());
        }
        None
    });
    Ok((playbin, playback))
}

/// Start the first track through the model, as `start_track` does.
fn start(playbin: &gst::Element, playback: &Mutex<Playback>, uri: &str, id: i64) {
    let mut guard = playback.lock().unwrap();
    playbin.set_property("uri", uri);
    guard.loaded(id);
    drop(guard);
    playbin.set_state(gst::State::Playing).unwrap();
}

#[test]
fn the_queue_moves_when_each_stream_is_heard_even_for_short_files() -> anyhow::Result<()> {
    // Short enough to be read whole, and handed over, right after starting.
    let durations = [(1, 300), (2, 300), (3, 1500), (4, 300), (5, 300)];
    let files = flacs("walk", &durations)?;
    let (playbin, playback) = player(&files.uris)?;
    let id = playback
        .lock()
        .unwrap()
        .play_context(vec![1, 2, 3, 4, 5], 0)
        .unwrap();
    let started_at = Instant::now();
    start(&playbin, &playback, &files.uris[&id], id);

    let bus = playbin.bus().unwrap();
    let mut heard = vec![id];
    let mut opened = 0;
    let mut reached_eos = false;
    while started_at.elapsed() < Duration::from_secs(15) && !reached_eos {
        let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(50)) else {
            continue;
        };
        match message.view() {
            gst::MessageView::StreamStart(_) => {
                let started = playback.lock().unwrap().stream_started();
                match started {
                    StreamStarted::Opened => opened += 1,
                    StreamStarted::HandedOver => {
                        heard.push(playback.lock().unwrap().queue().current().unwrap());
                        let due: u64 = durations[..heard.len() - 1].iter().map(|d| d.1).sum();
                        let at = started_at.elapsed().as_millis() as u64;
                        assert!(
                            at.abs_diff(due) < 200,
                            "track {} taken at {at} ms but heard from {due} ms",
                            heard.last().unwrap()
                        );
                    }
                    StreamStarted::Stop => anyhow::bail!("nothing armed a stop"),
                }
            }
            gst::MessageView::Eos(_) => {
                assert_eq!(playback.lock().unwrap().ended(), None);
                reached_eos = true;
            }
            gst::MessageView::Error(error) => anyhow::bail!("{}", error.error()),
            _ => {}
        }
    }
    let elapsed = started_at.elapsed();
    playbin.set_state(gst::State::Null)?;

    assert!(reached_eos, "the walk did not finish; heard {heard:?}");
    assert_eq!(heard, [1, 2, 3, 4, 5]);
    assert_eq!(opened, 1, "only the first stream opens outside a handoff");
    let total: u64 = durations.iter().map(|&(_, ms)| ms).sum();
    assert!(
        elapsed >= Duration::from_millis(total) - Duration::from_millis(200),
        "the sink is clocked, so {total} ms of audio cannot finish in {elapsed:?}"
    );
    Ok(())
}

#[test]
fn repeat_one_hands_a_track_over_to_itself_until_next() -> anyhow::Result<()> {
    let files = flacs("repeat", &[(1, 300), (2, 300)])?;
    let (playbin, playback) = player(&files.uris)?;
    let id = {
        let mut guard = playback.lock().unwrap();
        guard.edit_queue(|q| q.repeat = RepeatMode::One);
        guard.play_context(vec![1, 2], 0).unwrap()
    };
    start(&playbin, &playback, &files.uris[&id], id);

    let bus = playbin.bus().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut repeats = 0;
    while repeats < 3 && Instant::now() < deadline {
        let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(50)) else {
            continue;
        };
        match message.view() {
            gst::MessageView::StreamStart(_) => {
                if playback.lock().unwrap().stream_started() == StreamStarted::HandedOver {
                    repeats += 1;
                    assert_eq!(playback.lock().unwrap().queue().current(), Some(1));
                }
            }
            gst::MessageView::Error(error) => anyhow::bail!("{}", error.error()),
            _ => {}
        }
    }
    let next = playback.lock().unwrap().next();
    playbin.set_state(gst::State::Null)?;
    assert_eq!(repeats, 3);
    assert_eq!(next, Some(2));
    Ok(())
}

#[test]
fn stop_after_current_armed_during_a_handoff_stops_when_the_next_stream_starts(
) -> anyhow::Result<()> {
    let files = flacs("stop", &[(1, 600), (2, 600)])?;
    let (playbin, playback) = player(&files.uris)?;
    let id = playback
        .lock()
        .unwrap()
        .play_context(vec![1, 2], 0)
        .unwrap();
    start(&playbin, &playback, &files.uris[&id], id);

    let bus = playbin.bus().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut outcome = None;
    while outcome.is_none() && Instant::now() < deadline {
        let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(10)) else {
            let mut guard = playback.lock().unwrap();
            // Armed only once the handoff is reserved, so it cannot be declined.
            if guard.handoff_reserved() {
                guard.set_stop_after_current(true);
            }
            continue;
        };
        match message.view() {
            gst::MessageView::StreamStart(_) => {
                let started = playback.lock().unwrap().stream_started();
                if started != StreamStarted::Opened {
                    outcome = Some(started);
                }
            }
            gst::MessageView::Error(error) => anyhow::bail!("{}", error.error()),
            _ => {}
        }
    }
    playbin.set_state(gst::State::Null)?;
    assert_eq!(outcome, Some(StreamStarted::Stop));
    assert_eq!(playback.lock().unwrap().queue().current(), Some(1));
    Ok(())
}

#[test]
fn a_seek_with_a_handoff_reserved_stays_on_the_audible_track() -> anyhow::Result<()> {
    let files = flacs("seek", &[(1, 5000), (2, 500)])?;
    let (playbin, playback) = player(&files.uris)?;
    let id = playback
        .lock()
        .unwrap()
        .play_context(vec![1, 2], 0)
        .unwrap();
    start(&playbin, &playback, &files.uris[&id], id);

    let bus = playbin.bus().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut seek_pending = false;
    let mut reloaded_at = None;
    let mut handed_over_at = None;
    while handed_over_at.is_none() && Instant::now() < deadline {
        if reloaded_at.is_none() && !seek_pending && playback.lock().unwrap().handoff_reserved() {
            // What the player's reload_at does, about 2 s before the track ends.
            assert_eq!(playback.lock().unwrap().begin_reload(), Some(1));
            playbin.set_state(gst::State::Ready)?;
            let bus = playbin.bus().unwrap();
            bus.set_flushing(true);
            bus.set_flushing(false);
            playbin.set_property("uri", files.uris[&1].as_str());
            playbin.set_state(gst::State::Paused)?;
            seek_pending = true;
        }
        let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(10)) else {
            continue;
        };
        match message.view() {
            gst::MessageView::AsyncDone(_) if seek_pending => {
                seek_pending = false;
                playbin.seek(
                    1.0,
                    gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                    gst::SeekType::Set,
                    gst::ClockTime::from_mseconds(2000),
                    gst::SeekType::None,
                    gst::ClockTime::NONE,
                )?;
                assert!(playback.lock().unwrap().reloaded());
                playbin.set_state(gst::State::Playing)?;
                reloaded_at = Some(Instant::now());
            }
            gst::MessageView::StreamStart(_) => {
                let started = playback.lock().unwrap().stream_started();
                if started == StreamStarted::HandedOver {
                    handed_over_at = Some(Instant::now());
                }
            }
            gst::MessageView::Eos(_) => anyhow::bail!("the handoff should come again after EOS"),
            gst::MessageView::Error(error) => anyhow::bail!("{}", error.error()),
            _ => {}
        }
    }
    playbin.set_state(gst::State::Null)?;

    let heard = handed_over_at.expect("the next track never started")
        - reloaded_at.expect("the seek never happened");
    assert!(
        heard.as_millis().abs_diff(3000) < 300,
        "3 s remained after seeking to 2 s, but the next track came after {heard:?}"
    );
    assert_eq!(playback.lock().unwrap().queue().current(), Some(2));
    Ok(())
}
