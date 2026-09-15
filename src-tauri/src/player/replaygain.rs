use crescendo_core::replaygain::ReplayGain;
use gstreamer::{prelude::*, EventView, PadProbeReturn, PadProbeType};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

/// Apply gain at the processing pad, in stream order. Bus tag messages and
/// about-to-finish arrive ahead of audible playback and would let the next
/// track's gain affect the previous track's queued samples.
pub fn attach(volume: &gstreamer::Element, enabled: Arc<AtomicBool>) {
    let volume_weak = volume.downgrade();
    // Full-range volume was added in GStreamer 1.24. Older installations
    // support up to +20 dB through the original volume property.
    let full_range = volume.find_property("volume-full-range").is_some();
    let state = Mutex::new((ReplayGain::default(), 1.0));
    volume
        .static_pad("sink")
        .expect("volume sink pad")
        .add_probe(
            PadProbeType::EVENT_DOWNSTREAM | PadProbeType::BUFFER | PadProbeType::BUFFER_LIST,
            move |_, info| {
                let mut state = state.lock().expect("ReplayGain state poisoned");
                if let Some(event) = info.event() {
                    match event.view() {
                        EventView::StreamStart(_) => state.0 = ReplayGain::default(),
                        EventView::Tag(event) => {
                            let tags = event.tag();
                            // Tag events can be incremental; keep fields not
                            // present in this event until the next StreamStart.
                            if let Some(v) = tags.get::<gstreamer::tags::TrackGain>() {
                                state.0.track_gain = Some(v.get());
                            }
                            if let Some(v) = tags.get::<gstreamer::tags::TrackPeak>() {
                                state.0.track_peak = Some(v.get());
                            }
                            if let Some(v) = tags.get::<gstreamer::tags::AlbumGain>() {
                                state.0.album_gain = Some(v.get());
                            }
                            if let Some(v) = tags.get::<gstreamer::tags::AlbumPeak>() {
                                state.0.album_peak = Some(v.get());
                            }
                        }
                        _ => {}
                    }
                }
                let gain = if enabled.load(Ordering::Relaxed) {
                    state.0.multiplier()
                } else {
                    1.0
                };
                let gain = if full_range { gain } else { gain.min(10.0) };
                if gain != state.1 {
                    if let Some(volume) = volume_weak.upgrade() {
                        volume.set_property(
                            if full_range {
                                "volume-full-range"
                            } else {
                                "volume"
                            },
                            gain,
                        );
                        state.1 = gain;
                    }
                }
                PadProbeReturn::Ok
            },
        );
}
