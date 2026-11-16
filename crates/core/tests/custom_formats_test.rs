//! TrueAudio, WMA and DSF, which lofty has no reader for and Crescendo reads
//! itself (`scanner::formats`). The fixtures were tagged by mutagen, Quod
//! Libet's tag library; `fixtures/formats/make_tagged.py` rebuilds them.

use crescendo_core::art::{embedded_art, ArtData, ArtStore};
use crescendo_core::db::Db;
use crescendo_core::lyrics;
use crescendo_core::model::BrowseMode;
use crescendo_core::scanner::tags::read_meta;
use crescendo_core::scanner::{is_audio_file, scan, ScanOptions};
use lofty::ape::{ApeItem, ApeTag};
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::TagExt;
use lofty::tag::ItemValue;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats")
        .join(name)
}

/// (fixture, format, length in ms)
const FILES: [(&str, &str, i64); 3] = [
    ("tagged.tta", "TTA", 2000),
    ("tagged.wma", "WMA", 1999),
    ("tagged.dsf", "DSF", 46),
];

#[test]
fn the_extensions_are_scanned() {
    for name in ["a.tta", "a.TTA", "a.wma", "a.dsf"] {
        assert!(is_audio_file(Path::new(name)), "{name}");
    }
    assert!(!is_audio_file(Path::new("a.dff")), "DSDIFF is not read");
    assert!(!is_audio_file(Path::new("a.wmv")), "video stays out");
}

#[test]
fn every_tag_and_the_length_are_read() {
    for (name, format, length) in FILES {
        let path = fixture(name);
        let meta = read_meta(&path).unwrap();
        let what = format!("{format}: {meta:?}");
        assert_eq!(meta.format, format, "{what}");
        assert!((meta.duration_ms - length).abs() <= 1, "{what}");
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
        assert!(meta.has_embedded_art, "{what}");
        assert!(
            embedded_art(&path).is_some_and(|bytes| bytes.starts_with(b"\x89PNG")),
            "{format}: embedded cover"
        );
    }
}

#[test]
fn audio_properties_and_wma_only_fields() {
    let tta = read_meta(&fixture("tagged.tta")).unwrap();
    assert_eq!((tta.sample_rate, tta.bit_depth), (Some(44100), Some(16)));

    let dsf = read_meta(&fixture("tagged.dsf")).unwrap();
    assert_eq!((dsf.sample_rate, dsf.bit_depth), (Some(2_822_400), Some(1)));
    assert_eq!(dsf.bitrate, Some(5644), "2.8224 MHz × 2 channels × 1 bit");

    let wma = read_meta(&fixture("tagged.wma")).unwrap();
    assert_eq!(wma.sample_rate, Some(44100));
    // From the Metadata Library object, not the Extended Content Description.
    assert_eq!(wma.lyricist.as_deref(), Some("ZUN"));
    // A bare custom field as foobar2000 writes it, and a QL/ one.
    assert_eq!(wma.vocalist.as_deref(), Some("なし"));
    assert_eq!(wma.event.as_deref(), Some("C64"));
}

#[test]
fn a_trueaudio_apev2_tag_wins_over_its_id3v2_tag() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("both.tta");
    std::fs::copy(fixture("tagged.tta"), &path).unwrap();
    let mut ape = ApeTag::new();
    for (key, value) in [
        ("Title", "APE title"),
        ("Artist", "APE artist"),
        ("EVENT", "C64"),
    ] {
        ape.insert(ApeItem::new(key.into(), ItemValue::Text(value.into())).unwrap());
    }
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    ape.dump_to(&mut file, WriteOptions::default()).unwrap();
    drop(file);

    let meta = read_meta(&path).unwrap();
    assert_eq!(meta.title.as_deref(), Some("APE title"));
    assert_eq!(meta.artist.as_deref(), Some("APE artist"));
    assert_eq!(meta.event.as_deref(), Some("C64"));
    assert_eq!(
        meta.album, None,
        "fields come from one tag, as for other formats"
    );
    assert_eq!(meta.duration_ms, 2000, "the tag is not mistaken for audio");
    assert!(meta.has_embedded_art, "the ID3v2 cover is still found");
    assert!(embedded_art(&path).is_some());

    // An APEv2 cover is read from the APE tag itself.
    let mut png = Vec::new();
    image::RgbImage::from_pixel(1, 1, image::Rgb([0, 0, 0]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let cover = Picture::unchecked(png.clone())
        .pic_type(PictureType::CoverFront)
        .mime_type(MimeType::Png)
        .build();
    let ape_only = dir.path().join("ape.tta");
    let audio = std::fs::read(fixture("tagged.tta")).unwrap();
    // Drop the ID3v2 tag: its size is synchsafe in bytes 6..10.
    let id3_len = 10
        + audio[6..10]
            .iter()
            .fold(0usize, |n, b| (n << 7) | usize::from(*b));
    std::fs::write(&ape_only, &audio[id3_len..]).unwrap();
    ape.insert(
        ApeItem::new(
            "Cover Art (Front)".into(),
            ItemValue::Binary(cover.as_ape_bytes()),
        )
        .unwrap(),
    );
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&ape_only)
        .unwrap();
    ape.dump_to(&mut file, WriteOptions::default()).unwrap();
    drop(file);
    let meta = read_meta(&ape_only).unwrap();
    assert_eq!(meta.title.as_deref(), Some("APE title"));
    assert!(meta.has_embedded_art);
    assert_eq!(embedded_art(&ape_only), Some(png));
}

#[test]
fn a_scan_indexes_them_with_their_covers_and_lyrics() {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("Music");
    std::fs::create_dir(&music).unwrap();
    for (name, _, _) in FILES {
        std::fs::copy(fixture(name), music.join(name)).unwrap();
    }
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
    let mut formats: Vec<&str> = tracks.iter().map(|t| t.format.as_str()).collect();
    formats.sort();
    assert_eq!(formats, ["DSF", "TTA", "WMA"]);
    let album = tracks[0].album_id.expect("an album row");
    assert!(tracks.iter().all(|t| t.album_id == Some(album)));
    let art = ArtStore::new(&dir.path().join("cache")).unwrap();
    for track in &tracks {
        assert!(track.duration_ms > 0, "{track:?}");
        let found = lyrics::for_track(&db, track)
            .unwrap()
            .expect("embedded lyrics");
        assert_eq!(found.source, "Embedded tag", "{}", track.format);
        assert!(
            matches!(art.track_art(&db, track.id).unwrap(), ArtData::Bytes(b) if b.starts_with(b"\x89PNG")),
            "{}",
            track.format
        );
    }
    assert!(art.album_thumb(&db, album).unwrap().is_file());
}

#[test]
fn damaged_files_are_refused_without_panicking() {
    // read_meta turns a panic into an error too, so this checks the readers
    // return errors themselves: the scan would log a panic as a parser bug.
    let dir = tempfile::tempdir().unwrap();
    for (name, _, _) in FILES {
        let bytes = std::fs::read(fixture(name)).unwrap();
        let ext = Path::new(name).extension().unwrap().to_str().unwrap();
        let path = dir.path().join(format!("cut.{ext}"));
        for len in (0..bytes.len()).step_by(97).chain([1, 10, 29, 30, 79, 80]) {
            std::fs::write(&path, &bytes[..len.min(bytes.len())]).unwrap();
            if let Err(e) = read_meta(&path) {
                assert!(
                    !e.to_string().contains("panicked"),
                    "{name} cut at {len}: {e}"
                );
            }
        }
        // Every header byte flipped in turn, then bytes throughout the file.
        for at in (0..bytes.len().min(600)).chain((600..bytes.len()).step_by(37)) {
            let mut damaged = bytes.clone();
            damaged[at] ^= 0xFF;
            std::fs::write(&path, &damaged).unwrap();
            if let Err(e) = read_meta(&path) {
                assert!(!e.to_string().contains("panicked"), "{name} byte {at}: {e}");
            }
        }
    }
}

#[test]
fn each_wma_codec_has_its_own_name() {
    // ffmpeg encodes only WMA 1 and 2, so the codec id in the fixture's
    // stream header (WAVEFORMATEX, 54 bytes into the Stream Properties body)
    // is rewritten instead.
    let bytes = std::fs::read(fixture("tagged.wma")).unwrap();
    let stream_properties = [
        0x91, 0x07, 0xDC, 0xB7, 0xB7, 0xA9, 0xCF, 0x11, 0x8E, 0xE6, 0x00, 0xC0, 0x0C, 0x20, 0x53,
        0x65,
    ];
    let at = bytes
        .windows(16)
        .position(|w| w == stream_properties)
        .unwrap()
        + 24
        + 54;
    assert_eq!(bytes[at..at + 2], [0x61, 0x01], "WMA 2 in the fixture");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("codec.wma");
    for (id, name) in [
        (0x0161u16, "WMA"),
        (0x0162, "WMA Pro"),
        (0x0163, "WMA Lossless"),
        (0x000A, "WMA Voice"),
    ] {
        let mut copy = bytes.clone();
        copy[at..at + 2].copy_from_slice(&id.to_le_bytes());
        std::fs::write(&path, &copy).unwrap();
        assert_eq!(read_meta(&path).unwrap().format, name);
    }
}
