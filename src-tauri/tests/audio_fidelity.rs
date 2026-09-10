//! Silent integration checks using real FLAC encode/decode and the player filter.
//! Run: cargo test -p crescendo --test audio_fidelity

#[path = "../src/player/audio_filter.rs"]
mod audio_filter;

use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app::{AppSink, AppSinkCallbacks, AppSrc};
use std::sync::{Arc, Mutex};

struct StopOnDrop(gst::Element);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        let _ = self.0.set_state(gst::State::Null);
    }
}

struct TestFile(std::path::PathBuf);
impl Drop for TestFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn wait_for_eos(pipeline: &gst::Element) -> anyhow::Result<usize> {
    let bus = pipeline.bus().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut spectra = 0;
    while std::time::Instant::now() < deadline {
        if let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) {
            match message.view() {
                gst::MessageView::Error(e) => {
                    anyhow::bail!(
                        "{}: {} ({:?})",
                        e.src().unwrap().name(),
                        e.error(),
                        e.debug()
                    )
                }
                gst::MessageView::Eos(_) => return Ok(spectra),
                gst::MessageView::Element(e) => {
                    if e.structure().is_some_and(|s| s.has_name("spectrum")) {
                        spectra += 1;
                    }
                }
                _ => {}
            }
        }
    }
    anyhow::bail!("pipeline did not reach EOS")
}

fn encode_flac(bits: u32, rate: i32) -> anyhow::Result<(TestFile, Vec<i32>)> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let path = TestFile(std::env::temp_dir().join(format!(
        "crescendo-fidelity-{}-{nonce}.flac",
        std::process::id()
    )));
    // Distinct channels and nonzero low bits expose precision loss, channel
    // mixing, missing samples and reordering. A quarter second includes FFTs.
    let samples: Vec<i32> = (0..rate / 2)
        .map(|i| ((i as i64 * 104729 + 17) % (1 << (bits - 1))) as i32 - (1 << (bits - 2)))
        .collect();
    let bytes: Vec<u8> = samples
        .iter()
        .flat_map(|s| s.to_le_bytes()[..if bits == 16 { 2 } else { 4 }].to_vec())
        .collect();
    let pipeline = gst::parse::launch("appsrc name=source ! flacenc ! filesink name=file")?;
    let _stop = StopOnDrop(pipeline.clone());
    let bin = pipeline.downcast_ref::<gst::Bin>().unwrap();
    bin.by_name("file")
        .unwrap()
        .set_property("location", path.0.to_str().unwrap());
    let source = bin.by_name("source").unwrap().downcast::<AppSrc>().unwrap();
    source.set_caps(Some(
        &gst::Caps::builder("audio/x-raw")
            .field("format", if bits == 16 { "S16LE" } else { "S24_32LE" })
            .field("layout", "interleaved")
            .field("channels", 2i32)
            .field("channel-mask", gst::Bitmask::new(3))
            .field("rate", rate)
            .build(),
    ));
    source.set_format(gst::Format::Time);
    pipeline.set_state(gst::State::Playing)?;
    let mut buffer = gst::Buffer::from_mut_slice(bytes);
    buffer.get_mut().unwrap().set_pts(gst::ClockTime::ZERO);
    buffer
        .get_mut()
        .unwrap()
        .set_duration(gst::ClockTime::from_mseconds(250));
    source.push_buffer(buffer)?;
    source.end_of_stream()?;
    wait_for_eos(&pipeline)?;
    pipeline.set_state(gst::State::Null)?;
    Ok((path, samples))
}

#[test]
fn flac_samples_and_rates_survive_both_visualizers() -> anyhow::Result<()> {
    gst::init()?;
    for (bits, rate) in [
        (16, 44100),
        (24, 44100),
        (24, 48000),
        (24, 96000),
        (24, 192000),
    ] {
        let (file, expected) = encode_flac(bits, rate)?;
        for spectrum_on in [false, true] {
            for projectm_on in [false, true] {
                let playbin = gst::ElementFactory::make("playbin3").build()?;
                let _stop = StopOnDrop(playbin.clone());
                playbin.set_property("uri", gst::glib::filename_to_uri(&file.0, None)?.as_str());
                playbin.set_property("volume", 1.0f64);
                let filter = audio_filter::build()?;
                filter
                    .by_name("spectrum")
                    .unwrap()
                    .set_property("post-messages", spectrum_on);
                if !projectm_on {
                    filter
                        .by_name("projectm_gate")
                        .unwrap()
                        .static_pad("src")
                        .unwrap()
                        .add_probe(
                            gst::PadProbeType::BUFFER | gst::PadProbeType::BUFFER_LIST,
                            |_, _| gst::PadProbeReturn::Drop,
                        );
                }
                let tap = filter
                    .by_name("projectm_pcm")
                    .unwrap()
                    .downcast::<AppSink>()
                    .unwrap();
                let tapped = Arc::new(Mutex::new(0usize));
                tap.set_callbacks(
                    AppSinkCallbacks::builder()
                        .new_sample({
                            let tapped = tapped.clone();
                            move |sink| {
                                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                                let caps = sample.caps().unwrap().structure(0).unwrap();
                                assert_eq!(caps.get::<&str>("format").unwrap(), "S16LE");
                                assert_eq!(caps.get::<i32>("rate").unwrap(), 44100);
                                *tapped.lock().unwrap() += 1;
                                Ok(gst::FlowSuccess::Ok)
                            }
                        })
                        .build(),
                );
                playbin.set_property("audio-filter", &filter);

                // Do not force output caps: that could hide the original
                // negotiation bug by making the sink demand a better format.
                let output = gst::ElementFactory::make("appsink")
                    .property("sync", true)
                    .build()?
                    .downcast::<AppSink>()
                    .unwrap();
                let captured = Arc::new(Mutex::new(Vec::new()));
                output.set_callbacks(
                    AppSinkCallbacks::builder()
                        .new_sample({
                            let captured = captured.clone();
                            move |sink| {
                                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                                captured.lock().unwrap().push(sample);
                                Ok(gst::FlowSuccess::Ok)
                            }
                        })
                        .build(),
                );
                playbin.set_property("audio-sink", &output);
                playbin.set_state(gst::State::Playing)?;
                let spectra = wait_for_eos(&playbin)?;
                let spectrum_caps = filter
                    .by_name("spectrum")
                    .unwrap()
                    .static_pad("sink")
                    .unwrap()
                    .current_caps()
                    .unwrap();
                assert_eq!(
                    spectrum_caps.structure(0).unwrap().get::<i32>("rate")?,
                    rate
                );
                playbin.set_state(gst::State::Null)?;

                let mut actual = Vec::new();
                for sample in captured.lock().unwrap().iter() {
                    let caps = sample.caps().unwrap().structure(0).unwrap();
                    assert_eq!(caps.get::<i32>("rate")?, rate);
                    assert_eq!(caps.get::<i32>("channels")?, 2);
                    let format = caps.get::<&str>("format")?;
                    let map = sample.buffer().unwrap().map_readable()?;
                    match format {
                        "S16LE" if bits == 16 => actual.extend(
                            map.as_slice()
                                .chunks_exact(2)
                                .map(|b| i16::from_le_bytes(b.try_into().unwrap()) as i32),
                        ),
                        "S24LE" if bits == 24 => actual.extend(
                            map.as_slice()
                                .chunks_exact(3)
                                .map(|b| i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8),
                        ),
                        "S24_32LE" if bits == 24 => actual.extend(
                            map.as_slice()
                                .chunks_exact(4)
                                .map(|b| i32::from_le_bytes(b.try_into().unwrap())),
                        ),
                        "S32LE" => actual.extend(
                            map.as_slice()
                                .chunks_exact(4)
                                .map(|b| i32::from_le_bytes(b.try_into().unwrap()) >> (32 - bits)),
                        ),
                        other => {
                            anyhow::bail!("{bits}-bit FLAC negotiated unexpected output {other}")
                        }
                    }
                }
                assert!(actual == expected,
                    "samples changed: {bits}-bit/{rate} spectrum={spectrum_on} projectM={projectm_on}");
                assert_eq!(spectra > 0, spectrum_on);
                assert_eq!(*tapped.lock().unwrap() > 0, projectm_on);
            }
        }
    }
    Ok(())
}
