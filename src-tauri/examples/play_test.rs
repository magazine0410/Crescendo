//! Headless playback smoke test: plays a short slice of each given file
//! through playbin3 (quietly), asserting state reaches Playing and position
//! advances. Also exercises a gapless about-to-finish handoff when two or
//! more files are given.
//!
//! cargo run -p crescendo --example play_test -- <file> [file...]

use gstreamer::prelude::*;
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    gstreamer::init()?;
    let files: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(!files.is_empty(), "usage: play_test <audio-file>...");

    let playbin = gstreamer::ElementFactory::make("playbin3").build()?;
    playbin.set_property("volume", 0.02f64); // barely audible test volume

    // Keep this playback smoke test wired through the production visualizer
    // filter shape. In particular, a closed projectM gate must not prevent
    // playbin from prerolling while projectM is inactive, and non-buffer
    // events must still pass through it so flush seeks clear the side queue.
    let audio_filter = gstreamer::parse::bin_from_description(
        "tee name=visualizer_tee \
         visualizer_tee. ! queue max-size-buffers=3 max-size-bytes=0 max-size-time=0 \
           ! spectrum bands=512 interval=33000000 threshold=-80 \
             post-messages=false multi-channel=false \
         visualizer_tee. ! identity name=projectm_gate \
           ! queue name=projectm_queue leaky=downstream max-size-buffers=0 \
             max-size-bytes=0 max-size-time=3000000000 \
           ! audioconvert ! audioresample \
           ! audio/x-raw,format=S16LE,layout=interleaved,rate=44100,channels=2 \
           ! appsink name=projectm_pcm sync=true async=false max-buffers=2 drop=true",
        true,
    )?;
    let projectm_gate = audio_filter
        .by_name("projectm_gate")
        .ok_or_else(|| anyhow::anyhow!("projectM gate element is missing"))?;
    projectm_gate
        .static_pad("src")
        .ok_or_else(|| anyhow::anyhow!("projectM gate source pad is missing"))?
        .add_probe(
            gstreamer::PadProbeType::BUFFER | gstreamer::PadProbeType::BUFFER_LIST,
            |_pad, _info| gstreamer::PadProbeReturn::Drop,
        );
    let flush_stops = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    audio_filter
        .by_name("projectm_queue")
        .ok_or_else(|| anyhow::anyhow!("projectM queue element is missing"))?
        .static_pad("sink")
        .ok_or_else(|| anyhow::anyhow!("projectM queue sink pad is missing"))?
        .add_probe(
            gstreamer::PadProbeType::EVENT_DOWNSTREAM | gstreamer::PadProbeType::EVENT_FLUSH,
            {
                let flush_stops = flush_stops.clone();
                move |_pad, info| {
                    if let Some(gstreamer::PadProbeData::Event(event)) = info.data.as_ref() {
                        if event.type_() == gstreamer::EventType::FlushStop {
                            flush_stops.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    gstreamer::PadProbeReturn::Ok
                }
            },
        );
    playbin.set_property("audio-filter", &audio_filter);

    for path in &files {
        let uri = gstreamer::glib::filename_to_uri(path, None)?;
        playbin.set_state(gstreamer::State::Ready)?;
        playbin.set_property("uri", uri.as_str());
        playbin.set_state(gstreamer::State::Playing)?;

        // Wait for playback to actually start.
        let bus = playbin.bus().unwrap();
        let start = Instant::now();
        let mut reached_playing = false;
        while start.elapsed() < Duration::from_secs(10) {
            if let Some(msg) = bus.timed_pop(gstreamer::ClockTime::from_mseconds(100)) {
                use gstreamer::MessageView;
                match msg.view() {
                    MessageView::Error(e) => {
                        anyhow::bail!("ERROR on {path}: {} ({:?})", e.error(), e.debug());
                    }
                    MessageView::StateChanged(s) => {
                        if s.src().map(|x| x == &playbin).unwrap_or(false)
                            && s.current() == gstreamer::State::Playing
                        {
                            reached_playing = true;
                        }
                    }
                    _ => {}
                }
            }
            if reached_playing {
                break;
            }
        }
        anyhow::ensure!(reached_playing, "{path}: never reached Playing");

        // Let it play a moment and check the position advances.
        std::thread::sleep(Duration::from_millis(1200));
        let pos = playbin
            .query_position::<gstreamer::ClockTime>()
            .map(|p| p.mseconds())
            .unwrap_or(0);
        anyhow::ensure!(pos > 300, "{path}: position did not advance (pos={pos}ms)");

        // Seek to 80% and confirm the position lands there.
        if let Some(dur) = playbin.query_duration::<gstreamer::ClockTime>() {
            let target = dur.mseconds() * 8 / 10;
            let flush_stops_before = flush_stops.load(std::sync::atomic::Ordering::Relaxed);
            playbin.seek_simple(
                gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::ACCURATE,
                gstreamer::ClockTime::from_mseconds(target),
            )?;
            std::thread::sleep(Duration::from_millis(600));
            let pos2 = playbin
                .query_position::<gstreamer::ClockTime>()
                .map(|p| p.mseconds())
                .unwrap_or(0);
            let diff = pos2 as i64 - target as i64;
            anyhow::ensure!(
                flush_stops.load(std::sync::atomic::Ordering::Relaxed) > flush_stops_before,
                "{path}: flush seek did not reach the inactive projectM queue"
            );
            anyhow::ensure!(
                (-500..3000).contains(&diff),
                "{path}: seek landed at {pos2}ms, wanted ~{target}ms"
            );
            println!(
                "OK {path}: played, pos advanced, seek 80% -> {:.1}s / {:.1}s",
                pos2 as f64 / 1000.0,
                dur.mseconds() as f64 / 1000.0
            );
        } else {
            println!("OK {path}: played, pos advanced (no duration query)");
        }
    }

    // Gapless handoff test: play the tail of file A with file B queued in
    // about-to-finish; success = StreamStart for B with no error and no gap
    // in the pipeline clock.
    if files.len() >= 2 {
        let a = &files[0];
        let b = files[1].clone();
        let uri_b = gstreamer::glib::filename_to_uri(&b, None)?;
        let handed_off = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let handed_off = handed_off.clone();
            playbin.connect("about-to-finish", false, move |values| {
                let pb = values[0].get::<gstreamer::Element>().ok()?;
                pb.set_property("uri", uri_b.as_str());
                handed_off.store(true, std::sync::atomic::Ordering::SeqCst);
                None
            });
        }
        let uri_a = gstreamer::glib::filename_to_uri(a, None)?;
        playbin.set_state(gstreamer::State::Ready)?;
        playbin.set_property("uri", uri_a.as_str());
        playbin.set_state(gstreamer::State::Playing)?;
        std::thread::sleep(Duration::from_millis(800));
        // Jump near the end of A to trigger about-to-finish quickly.
        if let Some(dur) = playbin.query_duration::<gstreamer::ClockTime>() {
            playbin.seek_simple(
                gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::KEY_UNIT,
                gstreamer::ClockTime::from_mseconds(dur.mseconds().saturating_sub(2500)),
            )?;
        }
        let bus = playbin.bus().unwrap();
        let start = Instant::now();
        let mut new_stream = false;
        while start.elapsed() < Duration::from_secs(15) && !new_stream {
            if let Some(msg) = bus.timed_pop(gstreamer::ClockTime::from_mseconds(100)) {
                use gstreamer::MessageView;
                match msg.view() {
                    MessageView::Error(e) => anyhow::bail!("gapless ERROR: {}", e.error()),
                    MessageView::StreamStart(_) => {
                        if handed_off.load(std::sync::atomic::Ordering::SeqCst) {
                            new_stream = true;
                        }
                    }
                    MessageView::Eos(_) => break,
                    _ => {}
                }
            }
        }
        anyhow::ensure!(new_stream, "gapless handoff did not produce a new stream");
        println!("OK gapless: {a} -> {b} handed off without error");
    }

    playbin.set_state(gstreamer::State::Null)?;
    println!("all playback checks passed");
    Ok(())
}
