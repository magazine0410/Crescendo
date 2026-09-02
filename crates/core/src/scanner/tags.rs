use anyhow::Result;
use lofty::config::ParseOptions;
use lofty::file::{FileType, TaggedFile, TaggedFileExt};
use lofty::flac::FlacFile;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::{OpusFile, SpeexFile, VorbisFile};
use lofty::prelude::{Accessor, AudioFile, ItemKey};
use lofty::probe::Probe;
use std::io::{Read, Seek};
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
    pub composer: Option<String>,
    pub arranger: Option<String>,
    pub lyricist: Option<String>,
    pub vocalist: Option<String>,
    pub catalognumber: Option<String>,
    pub event: Option<String>,
    pub bpm: Option<i64>,
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

/// Tag values lofty's format-agnostic `Tag` cannot carry. Converting a
/// format-specific tag to `Tag` drops every field without an `ItemKey`
/// mapping, and neither `VOCALIST` nor `EVENT` — both conventions of the
/// doujin releases this library is full of — has one. They are read straight
/// off the concrete tag instead, which is why [`read_meta`] dispatches on the
/// file type rather than going through `Probe::read`.
#[derive(Debug, Default, Clone)]
struct RawExtras {
    vocalist: Option<String>,
    event: Option<String>,
}

fn vorbis_extras(vc: &VorbisComments) -> RawExtras {
    // VorbisComments::get matches the field name case-insensitively, as the
    // spec requires — taggers write both `VOCALIST` and `vocalist`.
    let get = |key: &str| {
        vc.get(key)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    RawExtras {
        vocalist: get("VOCALIST"),
        event: get("EVENT"),
    }
}

/// Read the file, keeping the format-specific tag long enough to pull the
/// fields `Tag` would discard. The Vorbis-comment formats are the ones that
/// carry `VOCALIST`/`EVENT` in practice; everything else parses exactly as
/// `Probe::read` would and simply has no extras.
///
/// The file type comes from the extension alone, as before — `Probe::read`
/// never sniffed content either, so detection is unchanged.
fn read_tagged<R: Read + Seek>(
    reader: &mut R,
    file_type: Option<FileType>,
    opts: ParseOptions,
) -> Result<(TaggedFile, RawExtras)> {
    Ok(match file_type {
        Some(FileType::Flac) => {
            let f = FlacFile::read_from(reader, opts)?;
            let extras = f.vorbis_comments().map(vorbis_extras).unwrap_or_default();
            (f.into(), extras)
        }
        Some(FileType::Vorbis) => {
            let f = VorbisFile::read_from(reader, opts)?;
            let extras = vorbis_extras(f.vorbis_comments());
            (f.into(), extras)
        }
        Some(FileType::Opus) => {
            let f = OpusFile::read_from(reader, opts)?;
            let extras = vorbis_extras(f.vorbis_comments());
            (f.into(), extras)
        }
        Some(FileType::Speex) => {
            let f = SpeexFile::read_from(reader, opts)?;
            let extras = vorbis_extras(f.vorbis_comments());
            (f.into(), extras)
        }
        Some(ft) => (
            Probe::with_file_type(reader, ft).options(opts).read()?,
            RawExtras::default(),
        ),
        None => (
            Probe::new(reader).options(opts).read()?,
            RawExtras::default(),
        ),
    })
}

pub fn read_meta(path: &Path) -> Result<TrackMeta> {
    // Relaxed parsing: skip malformed tag items (broken ID3 timestamps and
    // the like are common in the wild) instead of rejecting the whole file.
    let opts = ParseOptions::new().parsing_mode(lofty::config::ParsingMode::Relaxed);
    let probe = Probe::open(path)?.options(opts);
    let file_type = probe.file_type();
    let (file, extras) = read_tagged(&mut probe.into_inner(), file_type, opts)?;
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
        meta.title = tag
            .title()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        meta.artist = tag
            .artist()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        meta.album = tag
            .album()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        meta.genre = tag
            .genre()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        meta.albumartist = get_str(tag, ItemKey::AlbumArtist);
        meta.titlesort = get_str(tag, ItemKey::TrackTitleSortOrder);
        meta.artistsort = get_str(tag, ItemKey::TrackArtistSortOrder);
        meta.albumartistsort = get_str(tag, ItemKey::AlbumArtistSortOrder);
        meta.grouping = get_str(tag, ItemKey::ContentGroup);
        meta.composer = get_str(tag, ItemKey::Composer);
        meta.arranger = get_str(tag, ItemKey::Arranger);
        meta.lyricist = get_str(tag, ItemKey::Lyricist);
        meta.catalognumber = get_str(tag, ItemKey::CatalogNumber);
        // Vorbis writes BPM as free text, ID3v2's TBPM is the integer variant.
        meta.bpm = get_str(tag, ItemKey::Bpm)
            .or_else(|| get_str(tag, ItemKey::IntegerBpm))
            .and_then(|s| leading_int(&s))
            .filter(|n| *n > 0);
        meta.track_no = tag
            .track()
            .map(|n| n as i64)
            .or_else(|| get_str(tag, ItemKey::TrackNumber).and_then(|s| leading_int(&s)));
        meta.disc_no = tag
            .disk()
            .map(|n| n as i64)
            .or_else(|| get_str(tag, ItemKey::DiscNumber).and_then(|s| leading_int(&s)));
        meta.date = get_str(tag, ItemKey::RecordingDate)
            .or_else(|| get_str(tag, ItemKey::Year))
            .or_else(|| get_str(tag, ItemKey::OriginalReleaseDate));
        // lofty 0.25 removed `Accessor::year`. Its replacement, `date`, reads
        // RecordingDate *before* Year — the reverse of the old `year` — so a
        // file carrying both (a reissue `DATE=2020-09-11` alongside
        // `YEAR=2017`) would silently change year. Keep the original
        // precedence rather than inherit the new one.
        meta.year = get_str(tag, ItemKey::Year)
            .or_else(|| get_str(tag, ItemKey::RecordingDate))
            .and_then(|s| year_from_date(&s))
            .or_else(|| meta.date.as_deref().and_then(year_from_date));
        meta.has_embedded_art = !tag.pictures().is_empty();
    }
    meta.vocalist = extras.vocalist;
    meta.event = extras.event;

    // Untagged files still get a display title from the filename.
    if meta.title.is_none() {
        meta.title = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty());
    }
    Ok(meta)
}

fn get_str(tag: &lofty::tag::Tag, key: ItemKey) -> Option<String> {
    tag.get_string(key)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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
        &meta.composer,
        &meta.arranger,
        &meta.lyricist,
        &meta.vocalist,
        &meta.catalognumber,
        &meta.event,
    ] {
        // Credits repeat the artist constantly on doujin releases (arranger,
        // vocalist and composer are often all the circle name), and the blob is
        // scanned in full for every free-text query — so store each value once.
        if let Some(s) = part {
            let lower = s.to_lowercase();
            if !blob.split('\n').any(|seen| seen == lower) {
                blob.push_str(&lower);
                blob.push('\n');
            }
        }
    }
    blob
}
