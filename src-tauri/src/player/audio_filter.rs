//! Shared by the player and its headless playback/fidelity checks.

#[path = "replaygain.rs"]
mod replaygain;

use gstreamer::prelude::*;
use std::sync::{atomic::AtomicBool, Arc};

pub fn build(
    replaygain_enabled: Arc<AtomicBool>,
) -> Result<gstreamer::Bin, gstreamer::glib::Error> {
    // The audible branch must accept the decoder's format unchanged. In
    // particular, spectrum cannot accept flacdec's S24_32LE: putting it inline
    // lets negotiation choose S16LE and discard eight bits of a 24-bit FLAC.
    // Both analyzers therefore convert only their own copy after the tee.
    // Spectrum keeps the source rate; only projectM requires 44.1 kHz S16.
    //
    // Leaky visualizer queues cover the normal decode-ahead window but cannot
    // block playback if analysis falls behind. Their sinks follow the clock
    // and use async=false so an inactive projectM gate cannot block preroll.
    let bin = gstreamer::parse::bin_from_description(
        "tee name=visualizer_tee \
         visualizer_tee. ! queue max-size-buffers=3 max-size-bytes=0 max-size-time=0 \
           ! identity name=audio_passthrough \
           ! audioconvert ! audio/x-raw,format=F64LE \
           ! volume name=replaygain \
         visualizer_tee. ! queue name=spectrum_queue leaky=downstream \
             max-size-buffers=0 max-size-bytes=0 max-size-time=3000000000 \
           ! audioconvert \
           ! spectrum name=spectrum bands=512 interval=33000000 threshold=-80 \
             post-messages=false multi-channel=false \
           ! fakesink sync=true async=false \
         visualizer_tee. ! identity name=projectm_gate \
           ! queue name=projectm_queue leaky=downstream max-size-buffers=0 \
             max-size-bytes=0 max-size-time=3000000000 \
           ! audioconvert ! audioresample \
           ! audio/x-raw,format=S16LE,layout=interleaved,rate=44100,channels=2 \
           ! appsink name=projectm_pcm sync=true async=false max-buffers=2 drop=true",
        true,
    )?;
    // Float64 represents every 24/32-bit source sample exactly and gives
    // gain processing ample precision. Never negotiate a 16-bit RG path.
    // Unity gain (disabled or untagged) preserves the original sample values.
    replaygain::attach(
        &bin.by_name("replaygain").expect("ReplayGain volume"),
        replaygain_enabled,
    );
    Ok(bin)
}
