//! The artist facet lists every artist a value credits: `めらみぽっぷ; 七誌`
//! counts toward both artists and never shows as a row of its own, and
//! selecting one artist matches every track that credits it — in counts,
//! pages, albums and playback alike. The split runs on the label the panel
//! shows, so a romanized sort tag that credits two artists splits too.

use crescendo_core::db::Db;
use crescendo_core::model::{BrowseMode, BrowseValue, PlayContext};
use std::collections::HashMap;

/// (id, artist, artistsort, albumartist, album_id). Albums: 1 = the
/// two-artist value, 2 = solo, 3 = mixed lists, 4 = untagged.
#[allow(clippy::type_complexity)]
const ROWS: [(i64, Option<&str>, Option<&str>, Option<&str>, i64); 9] = [
    (1, Some("めらみぽっぷ; 七誌"), None, Some("A; B"), 1),
    (2, Some("めらみぽっぷ; 七誌"), None, Some("A; B"), 1),
    (3, Some("七誌"), None, None, 2),
    (4, Some("天宮みや，姫城碧海、七誌"), None, None, 3),
    // The panel shows the sort tag for a CJK artist, and splits that.
    (5, Some("黒崎朔夜"), Some("Amane; Kurosaki Sakuya"), None, 3),
    (
        6,
        Some("愚者トリオ（みあ、io、mikage）; COOL&CREATE"),
        None,
        None,
        3,
    ),
    (7, Some("AC/DC, Tyler & Co"), None, None, 3),
    (8, None, None, None, 4),
    (9, Some("IOSYS feat. 七誌 & あまね"), None, None, 3),
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
                                title, artist, artistsort, albumartist, album_id, track_no)
             VALUES (?1, ?2, '/m', ?3, 0, 0, 'FLAC', 1000, ?4, ?5, ?6, ?7, ?8, ?1)",
        )?;
        for (id, artist, artistsort, albumartist, album) in ROWS {
            ins.execute(rusqlite::params![
                id,
                format!("/m/{id}.flac"),
                format!("{id}.flac"),
                format!("Track {id}"),
                artist,
                artistsort,
                albumartist,
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

fn artist_context(values: &[&str]) -> PlayContext {
    PlayContext::ValueTracks {
        mode: BrowseMode::Artist,
        values: values.iter().map(|v| v.to_string()).collect(),
        query: None,
        projection_revision: 0,
        sort: None,
    }
}

#[test]
fn each_credited_artist_counts_the_track_and_the_combined_value_is_hidden() {
    let (_d, db) = make_db();
    let vals = db.browse_values_exact(BrowseMode::Artist, None).unwrap();
    let c = counts(&vals);

    assert_eq!(c.get("めらみぽっぷ"), Some(&2));
    assert_eq!(
        c.get("七誌"),
        Some(&4),
        "semicolon, solo, ideographic comma"
    );
    assert_eq!(c.get("天宮みや"), Some(&1));
    assert_eq!(c.get("姫城碧海"), Some(&1), "fullwidth comma");
    assert_eq!(c.get("Amane"), Some(&1), "romanized sort tag is split");
    assert_eq!(c.get("Kurosaki Sakuya"), Some(&1));
    assert_eq!(
        c.get("愚者トリオ（みあ、io、mikage）"),
        Some(&1),
        "separators inside brackets do not split"
    );
    assert_eq!(c.get("COOL&CREATE"), Some(&1), "& is part of the name");
    assert_eq!(
        c.get("AC/DC, Tyler & Co"),
        Some(&1),
        "ASCII comma, slash and & are not separators"
    );
    assert_eq!(c.get("IOSYS"), Some(&1), "feat. separates");
    assert_eq!(c.get("七誌 & あまね"), Some(&1));
    assert_eq!(
        c.get(""),
        Some(&1),
        "the untagged track keeps its \"?\" row"
    );
    assert_eq!(c.get("めらみぽっぷ; 七誌"), None);
    assert_eq!(c.get("Amane; Kurosaki Sakuya"), None);
}

#[test]
fn selecting_one_artist_matches_every_track_that_credits_it() {
    let (_d, db) = make_db();
    let nanashi = ["七誌".to_string()];

    assert_eq!(
        db.track_count_exact(None, BrowseMode::Artist, &nanashi)
            .unwrap(),
        4
    );
    let rows = db
        .tracks_range_exact(None, BrowseMode::Artist, &nanashi, None, 0, 50)
        .unwrap();
    let mut ids: Vec<i64> = rows.iter().map(|t| t.id).collect();
    ids.sort();
    assert_eq!(ids, [1, 2, 3, 4]);
    // Playback resolves the same set, in the view's order.
    let mut played = db
        .resolve_context_exact(&artist_context(&["七誌"]))
        .unwrap();
    played.sort();
    assert_eq!(played, [1, 2, 3, 4]);
    let albums: Vec<i64> = db
        .browse_albums_exact(BrowseMode::Artist, &nanashi, None)
        .unwrap()
        .iter()
        .map(|a| a.id)
        .collect();
    assert_eq!(albums, [1, 2, 3]);
    assert_eq!(
        db.resolve_context_exact(&artist_context(&["Amane"]))
            .unwrap(),
        [5]
    );
    // The combined value is no longer a selectable row of its own.
    assert_eq!(
        db.track_count_exact(None, BrowseMode::Artist, &["めらみぽっぷ; 七誌".into()])
            .unwrap(),
        0
    );
}

#[test]
fn with_romanization_off_the_raw_artist_is_split() {
    let (_d, db) = make_db();
    db.prefs_set("prefer_romanized", "false").unwrap();
    let c = counts(&db.browse_values_exact(BrowseMode::Artist, None).unwrap());
    assert_eq!(c.get("黒崎朔夜"), Some(&1));
    assert_eq!(c.get("Amane"), None);
}

#[test]
fn the_custom_artist_filter_splits_the_same_way() {
    let (_d, db) = make_db();
    let mode = BrowseMode::Custom(" Artist ".into());
    let c = counts(&db.browse_values_exact(mode.clone(), None).unwrap());
    assert_eq!(c.get("七誌"), Some(&4));
    assert_eq!(
        db.track_count_exact(None, mode, &["姫城碧海".into()])
            .unwrap(),
        1
    );
}

#[test]
fn the_album_artist_facet_is_not_split() {
    let (_d, db) = make_db();
    let c = counts(
        &db.browse_values_exact(BrowseMode::Albumartist, None)
            .unwrap(),
    );
    assert_eq!(c.get("A; B"), Some(&2));
    assert_eq!(c.get("A"), None);
}
