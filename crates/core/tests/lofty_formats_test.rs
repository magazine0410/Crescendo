//! WavPack, Monkey's Audio, Musepack and Speex: the formats lofty reads that
//! the scanner did not index before. The fixtures are two seconds of silence
//! straight from each format's own encoder (`wavpack`, `mac`, ffmpeg's
//! libspeex, `mpcenc`); the tests write the tags themselves, with APE keys spelled the
//! way foobar2000 writes them.

use crescendo_core::art::{embedded_art, ArtData, ArtStore};
use crescendo_core::db::Db;
use crescendo_core::lyrics;
use crescendo_core::model::BrowseMode;
use crescendo_core::scanner::tags::read_meta;
use crescendo_core::scanner::{is_audio_file, scan, ScanOptions};
use lofty::ape::{ApeItem, ApeTag};
use lofty::config::WriteOptions;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::OggPictureStorage;
use lofty::picture::{MimeType, Picture, PictureInformation, PictureType};
use lofty::prelude::TagExt;
use lofty::tag::ItemValue;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

const LYRICS: &str = "[00:00.50]静かな夜";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats")
        .join(name)
}

fn cover() -> Picture {
    let mut png = Vec::new();
    image::RgbImage::from_pixel(2, 2, image::Rgb([200, 30, 30]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    Picture::unchecked(png)
        .pic_type(PictureType::CoverFront)
        .mime_type(MimeType::Png)
        .build()
}

fn text(key: &str, value: &str) -> ApeItem {
    ApeItem::new(key.into(), ItemValue::Text(value.into())).unwrap()
}

/// An APEv2 tag as foobar2000 writes it: mixed-case keys, a multi-value
/// genre separated by NUL, "3/12" track numbers.
fn tag_ape(path: &Path) {
    let mut tag = ApeTag::new();
    for (key, value) in [
        ("Title", "幽雅に咲かせ、墨染の桜"),
        ("Artist", "上海アリス幻樂団"),
        ("Album", "東方妖々夢"),
        ("Album Artist", "ZUN"),
        ("TITLESORT", "Yuuga ni Sakase, Sumizome no Sakura"),
        ("ARTISTSORT", "Shanghai Alice Gengakudan"),
        ("Genre", "Touhou\0Game"),
        ("Composer", "ZUN"),
        ("Track", "3/12"),
        ("Disc", "1/2"),
        ("Year", "2003-08-17"),
        ("VOCALIST", "なし"),
        ("event", "C64"),
        ("Lyrics", LYRICS),
    ] {
        tag.insert(text(key, value));
    }
    tag.insert(
        ApeItem::new(
            "Cover Art (Front)".into(),
            ItemValue::Binary(cover().as_ape_bytes()),
        )
        .unwrap(),
    );
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

fn tag_vorbis(path: &Path) {
    let mut vc = VorbisComments::default();
    for (key, value) in [
        ("TITLE", "幽雅に咲かせ、墨染の桜"),
        ("ARTIST", "上海アリス幻樂団"),
        ("ALBUM", "東方妖々夢"),
        ("ALBUMARTIST", "ZUN"),
        ("TITLESORT", "Yuuga ni Sakase, Sumizome no Sakura"),
        ("ARTISTSORT", "Shanghai Alice Gengakudan"),
        ("GENRE", "Touhou"),
        ("GENRE", "Game"),
        ("COMPOSER", "ZUN"),
        ("TRACKNUMBER", "3"),
        ("DISCNUMBER", "1"),
        ("DATE", "2003-08-17"),
        ("VOCALIST", "なし"),
        ("EVENT", "C64"),
        ("LYRICS", LYRICS),
    ] {
        vc.push(key.into(), value.into());
    }
    let picture = cover();
    let info = PictureInformation::from_picture(&picture).unwrap();
    vc.insert_picture(picture, Some(info)).unwrap();
    vc.save_to_path(path, WriteOptions::default()).unwrap();
}

/// Each fixture copied to `dir` under a non-ASCII name and tagged.
fn tagged_copies(dir: &Path) -> Vec<(PathBuf, &'static str)> {
    let mut out = Vec::new();
    for (fixture_name, file_name, format) in [
        ("silence.wv", "03 幽雅に咲かせ.wv", "WavPack"),
        ("silence.ape", "03 幽雅に咲かせ.ape", "APE"),
        ("silence.mpc", "03 幽雅に咲かせ.mpc", "Musepack"),
        ("silence.spx", "03 幽雅に咲かせ.spx", "Speex"),
    ] {
        let path = dir.join(file_name);
        std::fs::copy(fixture(fixture_name), &path).unwrap();
        if format == "Speex" {
            tag_vorbis(&path);
        } else {
            tag_ape(&path);
        }
        out.push((path, format));
    }
    out
}

#[test]
fn the_new_extensions_are_scanned_in_any_case() {
    for name in ["a.wv", "a.WV", "a.ape", "a.mpc", "a.MP+", "a.mpp", "a.spx"] {
        assert!(is_audio_file(Path::new(name)), "{name}");
    }
    // Correction files and cue sheets stay out.
    for name in ["a.wvc", "a.cue", "a.apl"] {
        assert!(!is_audio_file(Path::new(name)), "{name}");
    }
}

#[test]
fn every_tag_and_the_length_are_read() {
    let dir = tempfile::tempdir().unwrap();
    for (path, format) in tagged_copies(dir.path()) {
        let meta = read_meta(&path).unwrap();
        let what = format!("{format}: {meta:?}");
        assert_eq!(meta.format, format, "{what}");
        assert!((1990..=2010).contains(&meta.duration_ms), "{what}");
        assert!(meta.sample_rate.is_some_and(|r| r > 0), "{what}");
        assert_eq!(
            meta.title.as_deref(),
            Some("幽雅に咲かせ、墨染の桜"),
            "{what}"
        );
        assert_eq!(meta.artist.as_deref(), Some("上海アリス幻樂団"), "{what}");
        assert_eq!(meta.album.as_deref(), Some("東方妖々夢"), "{what}");
        assert_eq!(meta.albumartist.as_deref(), Some("ZUN"), "{what}");
        assert_eq!(
            meta.titlesort.as_deref(),
            Some("Yuuga ni Sakase, Sumizome no Sakura"),
            "{what}"
        );
        assert_eq!(
            meta.artistsort.as_deref(),
            Some("Shanghai Alice Gengakudan"),
            "{what}"
        );
        assert_eq!(meta.genre.as_deref(), Some("Touhou; Game"), "{what}");
        assert_eq!(meta.composer.as_deref(), Some("ZUN"), "{what}");
        assert_eq!(meta.track_no, Some(3), "{what}");
        assert_eq!(meta.disc_no, Some(1), "{what}");
        assert_eq!(meta.year, Some(2003), "{what}");
        assert_eq!(meta.vocalist.as_deref(), Some("なし"), "{what}");
        assert_eq!(meta.event.as_deref(), Some("C64"), "{what}");
        assert!(meta.has_embedded_art, "{what}");
        assert!(
            embedded_art(&path).is_some_and(|bytes| bytes.starts_with(b"\x89PNG")),
            "{format}: embedded cover"
        );
    }
}

#[test]
fn a_scan_indexes_them_with_their_covers_and_lyrics() {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("Music");
    std::fs::create_dir(&music).unwrap();
    tagged_copies(&music);
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    let opts = ScanOptions {
        root: music,
        threads: 1,
        full: false,
    };
    scan(&db, &opts, &|_| {}, &AtomicBool::new(false)).unwrap();

    let tracks = db
        .tracks_range_exact(None, BrowseMode::Album, &[], None, 0, 100)
        .unwrap();
    assert_eq!(tracks.len(), 4);
    let mut formats: Vec<&str> = tracks.iter().map(|t| t.format.as_str()).collect();
    formats.sort();
    assert_eq!(formats, ["APE", "Musepack", "Speex", "WavPack"]);
    for track in &tracks {
        assert!(track.duration_ms >= 1990, "{track:?}");
        let found = lyrics::for_track(&db, track)
            .unwrap()
            .expect("embedded lyrics");
        assert_eq!(found.source, "Embedded tag");
    }
    let album = tracks[0].album_id.expect("an album row");
    assert!(
        tracks.iter().all(|t| t.album_id == Some(album)),
        "one album across the four formats"
    );
    let art = ArtStore::new(&dir.path().join("cache")).unwrap();
    for track in &tracks {
        let data = art.track_art(&db, track.id).unwrap();
        assert!(
            matches!(data, ArtData::Bytes(bytes) if bytes.starts_with(b"\x89PNG")),
            "{}",
            track.format
        );
    }
    assert!(art.album_thumb(&db, album).unwrap().is_file());
    let source = art.cover_source(&db, album).unwrap();
    assert!(
        source
            .as_deref()
            .is_some_and(|s| s.starts_with("embedded:")),
        "{source:?}"
    );
}

/// An APEv2 tag written by hand, so it can hold what lofty refuses to write:
/// an item name outside ASCII. `(name, value, flags)`; flags 2 is binary.
fn raw_ape_tag(items: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value, flags) in items {
        body.extend((value.len() as u32).to_le_bytes());
        body.extend(flags.to_le_bytes());
        body.extend(name.as_bytes());
        body.push(0);
        body.extend(*value);
    }
    let size = (body.len() + 32) as u32;
    let frame = |flags: u32| {
        let mut out = b"APETAGEX".to_vec();
        out.extend(2000u32.to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend((items.len() as u32).to_le_bytes());
        out.extend(flags.to_le_bytes());
        out.extend([0u8; 8]);
        out
    };
    let mut tag = frame(0xA000_0000);
    tag.extend(body);
    tag.extend(frame(0x8000_0000));
    tag
}

#[test]
fn a_malformed_ape_item_name_costs_only_that_item() {
    // As found in real files: a translated "Cover Art (Front)" as the item
    // name. lofty refuses the whole tag, and before this the whole file.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad name.wv");
    let mut bytes = std::fs::read(fixture("silence.wv")).unwrap();
    bytes.extend(raw_ape_tag(&[
        ("Title", "幽雅に咲かせ、墨染の桜".as_bytes(), 0),
        ("Artist", "上海アリス幻樂団".as_bytes(), 0),
        ("Genre", b"Touhou\0Game", 0),
        ("VOCALIST", "なし".as_bytes(), 0),
        ("Lyrics", LYRICS.as_bytes(), 0),
        ("封面 (前面)", b"\0not really a picture", 2),
    ]));
    std::fs::write(&path, bytes).unwrap();

    let meta = read_meta(&path).unwrap();
    assert_eq!(meta.format, "WavPack");
    assert_eq!(meta.duration_ms, 2000, "the audio is still read");
    assert_eq!(meta.title.as_deref(), Some("幽雅に咲かせ、墨染の桜"));
    assert_eq!(meta.artist.as_deref(), Some("上海アリス幻樂団"));
    assert_eq!(meta.genre.as_deref(), Some("Touhou; Game"));
    assert_eq!(meta.vocalist.as_deref(), Some("なし"));
    assert!(!meta.has_embedded_art, "the malformed item is dropped");

    let db = Db::open(&dir.path().join("t.db")).unwrap();
    let opts = ScanOptions {
        root: dir.path().to_path_buf(),
        threads: 1,
        full: false,
    };
    let progress = std::sync::Mutex::new(None);
    scan(
        &db,
        &opts,
        &|p| *progress.lock().unwrap() = Some(p),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(progress.lock().unwrap().as_ref().unwrap().errors, 0);
    let track = &db
        .tracks_range_exact(None, BrowseMode::Album, &[], None, 0, 10)
        .unwrap()[0];
    let found = lyrics::for_track(&db, track)
        .unwrap()
        .expect("embedded lyrics");
    assert_eq!(found.source, "Embedded tag");
}
