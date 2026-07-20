use anyhow::Result;
use lofty::file::{FileType, TaggedFileExt};
use lofty::prelude::{Accessor, AudioFile, ItemKey};
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct TrackMeta {
    pub format: String,
    pub duration_ms: i64,
    pub bitrate: Option<i64>,
    pub sample_rate: Option<i64>,
    pub bit_depth: Option<i64>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub albumartist: Option<String>,
    pub titlesort: Option<String>,
    pub artistsort: Option<String>,
    pub albumartistsort: Option<String>,
    pub genre: Option<String>,
    pub grouping: Option<String>,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    pub year: Option<i64>,
    pub date: Option<String>,
    pub has_embedded_art: bool,
}

pub fn format_name(ft: FileType) -> &'static str {
    match ft {
        FileType::Flac => "FLAC",
        FileType::Mpeg => "MP3",
        FileType::Opus => "Opus",
        FileType::Vorbis => "Vorbis",
        FileType::Wav => "WAV",
        FileType::Mp4 => "AAC",
        FileType::Aiff => "AIFF",
        FileType::Ape => "APE",
        FileType::WavPack => "WavPack",
        _ => "Other",
    }
}

pub fn read_meta(path: &Path) -> Result<TrackMeta> {
    // Relaxed parsing: skip malformed tag items (broken ID3 timestamps and
    // the like are common in the wild) instead of rejecting the whole file.
    let file = lofty::probe::Probe::open(path)?
        .options(lofty::config::ParseOptions::new().parsing_mode(lofty::config::ParsingMode::Relaxed))
        .read()?;
    let props = file.properties();
    let mut meta = TrackMeta {
        format: format_name(file.file_type()).to_string(),
        duration_ms: props.duration().as_millis() as i64,
        bitrate: props.audio_bitrate().map(|b| b as i64),
        sample_rate: props.sample_rate().map(|s| s as i64),
        bit_depth: props.bit_depth().map(|b| b as i64),
        ..Default::default()
    };

    let tag = file.primary_tag().or_else(|| file.first_tag());
    if let Some(tag) = tag {
        meta.title = tag.title().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        meta.artist = tag.artist().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        meta.album = tag.album().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        meta.genre = tag.genre().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        meta.albumartist = get_str(tag, &ItemKey::AlbumArtist);
        meta.titlesort = get_str(tag, &ItemKey::TrackTitleSortOrder);
        meta.artistsort = get_str(tag, &ItemKey::TrackArtistSortOrder);
        meta.albumartistsort = get_str(tag, &ItemKey::AlbumArtistSortOrder);
        meta.grouping = get_str(tag, &ItemKey::ContentGroup);
        meta.track_no = tag.track().map(|n| n as i64).or_else(|| {
            get_str(tag, &ItemKey::TrackNumber).and_then(|s| leading_int(&s))
        });
        meta.disc_no = tag.disk().map(|n| n as i64).or_else(|| {
            get_str(tag, &ItemKey::DiscNumber).and_then(|s| leading_int(&s))
        });
        meta.date = get_str(tag, &ItemKey::RecordingDate)
            .or_else(|| get_str(tag, &ItemKey::Year))
            .or_else(|| get_str(tag, &ItemKey::OriginalReleaseDate));
        meta.year = tag.year().map(|y| y as i64).or_else(|| {
            meta.date.as_deref().and_then(year_from_date)
        });
        meta.has_embedded_art = !tag.pictures().is_empty();
    }

    // Untagged files still get a display title from the filename.
    if meta.title.is_none() {
        meta.title = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty());
    }
    Ok(meta)
}

fn get_str(tag: &lofty::tag::Tag, key: &ItemKey) -> Option<String> {
    tag.get_string(key).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// "5/12" -> 5, "05" -> 5
fn leading_int(s: &str) -> Option<i64> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Minimal metadata for files whose tags cannot be read at all: they must
/// still appear in the library (GStreamer can usually play them anyway).
pub fn fallback_meta(path: &Path) -> TrackMeta {
    let format = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("flac") => "FLAC",
        Some("mp3") => "MP3",
        Some("opus") => "Opus",
        Some("ogg") => "Vorbis",
        Some("wav") => "WAV",
        Some("m4a") | Some("aac") => "AAC",
        Some("aiff") => "AIFF",
        _ => "Other",
    };
    TrackMeta {
        format: format.to_string(),
        title: path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty()),
        ..Default::default()
    }
}

/// "2008-05-01" / "2008" -> 2008
fn year_from_date(s: &str) -> Option<i64> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 4 {
        digits[..4].parse().ok()
    } else {
        None
    }
}

/// Lowercased blob free-text search runs against (kept in sync with
/// `Node::FreeText` compilation).
pub fn search_blob(meta: &TrackMeta) -> String {
    let mut blob = String::new();
    for part in [
        &meta.title,
        &meta.artist,
        &meta.album,
        &meta.albumartist,
        &meta.titlesort,
        &meta.artistsort,
        &meta.albumartistsort,
        &meta.genre,
    ] {
        if let Some(s) = part {
            blob.push_str(&s.to_lowercase());
            blob.push('\n');
        }
    }
    blob
}
