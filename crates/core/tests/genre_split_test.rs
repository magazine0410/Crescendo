//! The genre facet lists every genre a stored value names: one value such as
//! `其他电子，House，Trance，J-Pop` counts toward each of its four genres and
//! never shows as a row of its own, and selecting one genre matches every
//! track that lists it — in counts, pages, albums and playback alike. The
//! scanner keeps all of a file's GENRE fields so they split the same way.

use crescendo_core::db::Db;
use crescendo_core::model::{BrowseMode, BrowseValue, PlayContext};
use crescendo_core::scanner::tags::read_meta;
use crescendo_core::search;
use std::collections::HashMap;

/// (id, genre, album_id). Albums: 1 = the combined-value album, 2 = plain
/// House, 3 = a slash list, 4 = untagged.
const ROWS: [(i64, Option<&str>, i64); 7] = [
    (1, Some("其他电子，House，Trance，J-Pop"), 1),
    (2, Some("其他电子，House，Trance，J-Pop"), 1),
    (3, Some("House"), 2),
    (4, Some("Bass House/Future House"), 3),
    (5, Some("Rock／Metal⧸J-Pop"), 3),
    (6, Some("Speedcore, Happy Hardcore; Speedcore"), 3),
    (7, None, 4),
];

fn make_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        for album in 1..=4 {
            conn.execute(
                "INSERT INTO albums(id, key, album, track_count) VALUES (?1, ?2, ?2, 1)",
                rusqlite::params![album, format!("album {album}")],
            )?;
        }
        let mut ins = conn.prepare(
            "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, duration_ms,
                                title, genre, album_id, track_no)
             VALUES (?1, ?2, '/m', ?3, 0, 0, 'FLAC', 1000, ?4, ?5, ?6, ?1)",
        )?;
        for (id, genre, album) in ROWS {
            ins.execute(rusqlite::params![
                id,
                format!("/m/{id}.flac"),
                format!("{id}.flac"),
                format!("Track {id}"),
                genre,
                album,
            ])?;
        }
        Ok(())
    })
    .unwrap();
    (dir, db)
}

fn counts(vals: &[BrowseValue]) -> HashMap<String, i64> {
    vals.iter().map(|v| (v.value.clone(), v.count)).collect()
}

fn genre_context(values: &[&str], query: Option<&str>) -> PlayContext {
    PlayContext::ValueTracks {
        mode: BrowseMode::Genre,
        values: values.iter().map(|v| v.to_string()).collect(),
        query: query.map(str::to_string),
        projection_revision: 0,
        sort: None,
    }
}

#[test]
fn each_listed_genre_counts_the_track_and_the_combined_value_is_hidden() {
    let (_d, db) = make_db();
    let vals = db.browse_values_exact(BrowseMode::Genre, None).unwrap();
    let c = counts(&vals);

    assert_eq!(c.get("其他电子"), Some(&2));
    assert_eq!(
        c.get("House"),
        Some(&3),
        "two from the combined value, one plain"
    );
    assert_eq!(c.get("Trance"), Some(&2));
    assert_eq!(
        c.get("J-Pop"),
        Some(&3),
        "fullwidth comma and big/fullwidth solidus"
    );
    assert_eq!(c.get("Bass House"), Some(&1));
    assert_eq!(c.get("Future House"), Some(&1));
    assert_eq!(c.get("Rock"), Some(&1));
    assert_eq!(c.get("Metal"), Some(&1));
    assert_eq!(
        c.get("Speedcore"),
        Some(&1),
        "a genre a value repeats counts its track once"
    );
    assert_eq!(c.get("Happy Hardcore"), Some(&1));
    assert_eq!(
        c.get(""),
        Some(&1),
        "the untagged track keeps its \"?\" row"
    );
    assert!(
        vals.iter()
            .all(|v| !v.value.contains(['，', '/', '／', '⧸', ',', ';'])),
        "no combined value is listed: {vals:?}"
    );
    // Still one alphabetical (case-insensitive) list.
    let names: Vec<&str> = vals.iter().map(|v| v.value.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_by_key(|s| s.to_lowercase());
    assert_eq!(names, sorted);
}

#[test]
fn selecting_one_genre_matches_every_track_that_lists_it() {
    let (_d, db) = make_db();
    let house = ["House".to_string()];

    assert_eq!(
        db.track_count_exact(None, BrowseMode::Genre, &house)
            .unwrap(),
        3
    );
    let rows = db
        .tracks_range_exact(None, BrowseMode::Genre, &house, None, 0, 50)
        .unwrap();
    let mut ids: Vec<i64> = rows.iter().map(|t| t.id).collect();
    ids.sort();
    assert_eq!(ids, [1, 2, 3]);
    assert_eq!(
        db.resolve_context_exact(&genre_context(&["House"], None))
            .unwrap(),
        [1, 2, 3]
    );
    let albums: Vec<i64> = db
        .browse_albums_exact(BrowseMode::Genre, &house, None)
        .unwrap()
        .iter()
        .map(|a| a.id)
        .collect();
    assert_eq!(albums, [1, 2]);

    // A multi-selection is a union, still one row per track.
    let both = ["Trance".to_string(), "Rock".to_string()];
    assert_eq!(
        db.track_count_exact(None, BrowseMode::Genre, &both)
            .unwrap(),
        3
    );
    // The combined value is no longer a selectable row of its own.
    let combined = ["其他电子，House，Trance，J-Pop".to_string()];
    assert_eq!(
        db.track_count_exact(None, BrowseMode::Genre, &combined)
            .unwrap(),
        0
    );
    // The untagged track is reachable through "".
    assert_eq!(
        db.resolve_context_exact(&genre_context(&[""], None))
            .unwrap(),
        [7]
    );
}

#[test]
fn the_custom_genre_filter_splits_the_same_way() {
    let (_d, db) = make_db();
    let mode = BrowseMode::Custom(" Genre ".into());
    let c = counts(&db.browse_values_exact(mode.clone(), None).unwrap());
    assert_eq!(c.get("House"), Some(&3));
    assert_eq!(c.get("Future House"), Some(&1));
    assert_eq!(
        db.track_count_exact(None, mode, &["Metal".into()]).unwrap(),
        1
    );
}

#[test]
fn a_search_filter_naming_shared_column_names_still_combines() {
    // `path` and `id` are also json_each columns; the split facet must keep
    // the search filter's bare column names unambiguous.
    let (_d, db) = make_db();
    let filter = search::compile(r#"path="/m/1.flac"|"/m/3.flac"|"/m/5.flac""#).unwrap();
    let c = counts(
        &db.browse_values_exact(BrowseMode::Genre, Some(&filter))
            .unwrap(),
    );
    assert_eq!(c.get("House"), Some(&2));
    assert_eq!(c.get("J-Pop"), Some(&2));
    assert_eq!(c.get("Bass House"), None);
    assert_eq!(
        db.track_count_exact(Some(&filter), BrowseMode::Genre, &["House".into()])
            .unwrap(),
        2
    );
    assert_eq!(
        db.resolve_context_exact(&genre_context(&["J-Pop"], Some(r#"path="/m/5.flac""#)))
            .unwrap(),
        [5]
    );
}

#[test]
fn other_facets_are_not_split() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        conn.execute(
            "INSERT INTO tracks(path, dir, filename, mtime, size, format, artist)
             VALUES ('/m/a.flac', '/m', 'a.flac', 0, 0, 'FLAC', 'AC/DC')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let c = counts(&db.browse_values_exact(BrowseMode::Artist, None).unwrap());
    assert_eq!(c.get("AC/DC"), Some(&1));
}

/// A FLAC stream with no audio frames: STREAMINFO plus one Vorbis comment
/// block holding `comments`. Enough for lofty to read the tags.
fn tiny_flac(comments: &[&str]) -> Vec<u8> {
    let mut out = b"fLaC".to_vec();
    // STREAMINFO (type 0), 34 bytes: block sizes 4096, frame sizes unknown,
    // 44.1 kHz, 2 channels, 16 bits, 0 samples, zero MD5.
    out.extend([0x00, 0x00, 0x00, 34]);
    out.extend([0x10, 0x00, 0x10, 0x00, 0, 0, 0, 0, 0, 0]);
    let packed: u64 = (44_100u64 << 44) | (1 << 41) | (15 << 36);
    out.extend(packed.to_be_bytes());
    out.extend([0u8; 16]);
    // VORBIS_COMMENT (type 4), marked as the last block.
    let mut vc = Vec::new();
    let vendor = b"crescendo-test";
    vc.extend((vendor.len() as u32).to_le_bytes());
    vc.extend(vendor);
    vc.extend((comments.len() as u32).to_le_bytes());
    for c in comments {
        vc.extend((c.len() as u32).to_le_bytes());
        vc.extend(c.as_bytes());
    }
    let len = vc.len() as u32;
    out.extend([0x84, (len >> 16) as u8, (len >> 8) as u8, len as u8]);
    out.extend(vc);
    out
}

#[test]
fn the_scanner_keeps_every_genre_field() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("multi.flac");
    std::fs::write(
        &path,
        tiny_flac(&[
            "TITLE=Multi",
            "GENRE=J-Pop",
            "GENRE=Rock",
            "GENRE= ",
            "GENRE=J-Pop",
            "GENRE=Bass House/Future House",
        ]),
    )
    .unwrap();
    let meta = read_meta(&path).unwrap();
    assert_eq!(meta.title.as_deref(), Some("Multi"));
    assert_eq!(
        meta.genre.as_deref(),
        Some("J-Pop; Rock; Bass House/Future House"),
        "blank and repeated fields are dropped, the rest kept in file order"
    );

    let single = dir.path().join("single.flac");
    std::fs::write(&single, tiny_flac(&["GENRE=  Jazz  "])).unwrap();
    assert_eq!(read_meta(&single).unwrap().genre.as_deref(), Some("Jazz"));
}
