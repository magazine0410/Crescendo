//! Headless waveform decode test: decodes a file to its loudness peak envelope
//! and prints an ASCII sparkline, so the GStreamer decode path can be validated
//! without launching the windowed app.
//!
//! cargo run -p crescendo --example waveform_test -- <audio-file>

use lofty::file::AudioFile;
use lofty::probe::Probe;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: waveform_test <audio-file>");

    // Duration is what production passes in (it comes from the DB there).
    let duration_ms = Probe::open(&path)?.read()?.properties().duration().as_millis() as i64;

    let peaks = crescendo_lib::waveform::decode_peaks(&path, duration_ms)?;

    let bars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let step = peaks.len() / 80 + 1;
    let sparkline: String = peaks
        .iter()
        .step_by(step)
        .map(|&v| bars[(v as usize * 7 / 255).min(7)])
        .collect();
    let nonzero = peaks.iter().filter(|&&v| v > 0).count();

    println!("duration_ms={duration_ms} buckets={}", peaks.len());
    println!("{sparkline}");
    println!("nonzero={nonzero}/{} max={}", peaks.len(), peaks.iter().max().unwrap_or(&0));
    Ok(())
}
