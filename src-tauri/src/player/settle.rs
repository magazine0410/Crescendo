//! Changing a pipeline's state without trusting GStreamer to come back.
//!
//! On GStreamer 1.28.8 a state change can deadlock against a streaming thread
//! around a gapless handoff. Going down, uridecodebin3's input switch holds its
//! play-items lock and waits for the state lock the change holds. Going up, with a
//! file short enough to be read before the pipeline is playing, urisourcebin's
//! typefind thread waits for a parsebin state lock while the change waits for that
//! thread. Nothing in the public API orders those locks, so these calls run on a
//! thread of their own and the caller gives up on them after a while.

use gstreamer as gst;
use gstreamer::prelude::*;
use std::sync::mpsc::sync_channel;
use std::time::Duration;

/// A healthy stop measured under 8 ms with 16 pipelines competing for the CPU.
/// A false alarm only replaces a pipeline that was being stopped anyway.
pub const PATIENCE: Duration = Duration::from_secs(2);

/// Move `element` to READY or NULL. True once it is there with its streaming
/// threads stopped, including a change that returned ASYNC and continued on a
/// GStreamer thread. False if that did not happen in time: the element must then
/// be abandoned, since any further call on it can block too. The thread keeps its
/// reference, so an element that was only slow is released when it finishes.
pub fn stop_streams(element: &gst::Element, target: gst::State, patience: Duration) -> bool {
    let (done, finished) = sync_channel(1);
    let element = element.clone();
    let spawned = std::thread::Builder::new()
        .name("gst-stop".into())
        .spawn(move || {
            let reached =
                element.set_state(target).is_ok() && element.state(gst::ClockTime::NONE).0.is_ok();
            if done.send(reached).is_err() {
                // Abandoned, and it turned out to be slow rather than stuck.
                let _ = element.set_state(gst::State::Null);
            }
        });
    spawned.is_ok() && finished.recv_timeout(patience).unwrap_or(false)
}

/// Pause or resume. True if the call returned. On false the pipeline is stuck, but
/// the caller stays free to answer commands, and the next stop replaces it.
pub fn change_state(element: &gst::Element, target: gst::State, patience: Duration) -> bool {
    let (done, finished) = sync_channel(1);
    let element = element.clone();
    let spawned = std::thread::Builder::new()
        .name("gst-state".into())
        .spawn(move || {
            let _ = element.set_state(target);
            let _ = done.send(());
        });
    spawned.is_ok() && finished.recv_timeout(patience).is_ok()
}
