//! Loudness-waveform generation and cache.
//!
//! Mirrors the cover-art cache (`crescendo_core::art`): per track we compute a
//! small array of peak values (one byte each, `0..=255`) by decoding the audio
//! with a throwaway GStreamer pipeline, and cache it on disk keyed by track id.
//! Only the now-playing track is ever decoded — never the whole library.

use anyhow::{anyhow, Result};
use crescendo_core::db::Db;
use gstreamer::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Number of peak buckets in a waveform (one drawn bar each).
const BUCKETS: usize = 500;
/// Decode/downsample rate. A waveform needs only a coarse loudness envelope, so
/// we resample hard to keep the sample count (and CPU) small.
const RATE: i32 = 8000;
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub struct WaveformStore {
    dir: PathBuf,
}

impl WaveformStore {
    pub fn new(cache_dir: &Path) -> Result<Self> {
        let dir = cache_dir.join("waveforms");
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn path(&self, track_id: i64) -> PathBuf {
        self.dir.join(format!("{track_id}.bin"))
    }

    /// Peaks for a track; decodes and caches on first request, else reads cache.
    pub fn get_or_generate(&self, db: &Db, track_id: i64) -> Result<Vec<u8>> {
        let cached = self.path(track_id);
        if let Ok(bytes) = std::fs::read(&cached) {
            if bytes.len() == BUCKETS {
                return Ok(bytes);
            }
        }
        let track = db
            .track_by_id(track_id)?
            .ok_or_else(|| anyhow!("track {track_id} not found"))?;
        let peaks = decode_peaks(&track.path, track.duration_ms)?;
        let tmp = unique_temp_path(&cached);
        if let Err(e) = std::fs::write(&tmp, &peaks).and_then(|_| std::fs::rename(&tmp, &cached)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(peaks)
    }
}

fn unique_temp_path(destination: &Path) -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let name = destination
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("waveform");
    destination.with_file_name(format!("{name}.{}.{}.tmp", std::process::id(), id))
}

/// Decode an audio file to a `BUCKETS`-long peak envelope normalized to `0..=255`.
pub fn decode_peaks(path: &str, duration_ms: i64) -> Result<Vec<u8>> {
    gstreamer::init()?; // idempotent — the player also inits on its own thread.

    let pipeline = gstreamer::Pipeline::new();
    let src = gstreamer::ElementFactory::make("filesrc")
        .property("location", path)
        .build()?;
    let dec = gstreamer::ElementFactory::make("decodebin").build()?;
    let convert = gstreamer::ElementFactory::make("audioconvert").build()?;
    let resample = gstreamer::ElementFactory::make("audioresample").build()?;
    let caps = gstreamer::Caps::builder("audio/x-raw")
        .field("format", "S16LE")
        .field("channels", 1i32)
        .field("rate", RATE)
        .field("layout", "interleaved")
        .build();
    let appsink = gstreamer_app::AppSink::builder()
        .caps(&caps)
        .sync(false) // decode as fast as possible, not in realtime.
        .build();
    let appsink_el: &gstreamer::Element = appsink.upcast_ref();

    pipeline.add_many([&src, &dec, &convert, &resample, appsink_el])?;
    gstreamer::Element::link_many([&src, &dec])?;
    gstreamer::Element::link_many([&convert, &resample, appsink_el])?;

    // decodebin exposes its output pad only once it has sniffed the stream.
    let convert_weak = convert.downgrade();
    dec.connect_pad_added(move |_, pad| {
        let Some(convert) = convert_weak.upgrade() else {
            return;
        };
        let Some(sink) = convert.static_pad("sink") else {
            return;
        };
        if sink.is_linked() {
            return;
        }
        let is_audio = pad
            .current_caps()
            .and_then(|c| c.structure(0).map(|s| s.name().starts_with("audio/")))
            .unwrap_or(true);
        if is_audio {
            let _ = pad.link(&sink);
        }
    });

    pipeline.set_state(gstreamer::State::Playing)?;

    let total_samples = ((duration_ms.max(0) * RATE as i64) / 1000).max(1) as u64;
    let mut buckets = vec![0f32; BUCKETS];
    let mut sample_index: u64 = 0;
    let mut idle = 0;
    loop {
        match appsink.try_pull_sample(gstreamer::ClockTime::from_seconds(5)) {
            Some(sample) => {
                idle = 0;
                if let Some(buffer) = sample.buffer() {
                    if let Ok(map) = buffer.map_readable() {
                        for chunk in map.as_slice().chunks_exact(2) {
                            let amp = (i16::from_le_bytes([chunk[0], chunk[1]]) as f32).abs();
                            let b = ((sample_index * BUCKETS as u64) / total_samples)
                                .min(BUCKETS as u64 - 1)
                                as usize;
                            if amp > buckets[b] {
                                buckets[b] = amp;
                            }
                            sample_index += 1;
                        }
                    }
                }
            }
            None => {
                // Timeout: EOS is the normal end; a few empty waits means the
                // pipeline stalled or errored — bail rather than hang forever.
                if appsink.is_eos() {
                    break;
                }
                idle += 1;
                if idle >= 3 {
                    break;
                }
            }
        }
    }
    let _ = pipeline.set_state(gstreamer::State::Null);

    let max = buckets.iter().copied().fold(0f32, f32::max);
    let out = if max > 0.0 {
        buckets
            .iter()
            .map(|&v| ((v / max) * 255.0).round().clamp(0.0, 255.0) as u8)
            .collect()
    } else {
        vec![0u8; BUCKETS]
    };
    Ok(out)
}
