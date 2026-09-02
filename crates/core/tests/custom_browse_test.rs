use crescendo_core::db::Db;
use crescendo_core::model::{BrowseMode, BrowseValue, PlayContext};
use std::collections::HashMap;

/// A tiny library exercising the custom left-panel filter: an INTEGER column
/// (`year`, incl. a NULL), a CJK-tagged TEXT column (`genre`), and album
/// artists with a romanized sort name.
fn make_db() -> (tempfile::TempDir, Db) {
    // (year, genre, albumartist, albumartistsort)
    let rows: [(Option<i64>, Option<&str>, Option<&str>, Option<&str>); 4] = [
        (
            Some(1999),
            Some("ジャズ"),
            Some("上海アリス幻樂団"),
            Some("Shanghai Alice"),
        ),
        (
            Some(1999),
            Some("ジャズ"),
            Some("上海アリス幻樂団"),
            Some("Shanghai Alice"),
        ),
        (Some(2000), Some("Rock"), Some("Zulu"), None),
        (None, None, None, None),
    ];
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        let mut ins = conn.prepare(
            "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms, year, genre,
                                albumartist, albumartistsort)
             VALUES (?1, ?2, ?3, 0, 0, 'FLAC', 180000, ?4, ?5, ?6, ?7)",
        )?;
        for (i, (year, genre, albumartist, albumartistsort)) in rows.iter().enumerate() {
            ins.execute(rusqlite::params![
                format!("/m/{i}.flac"),
                "/m",
                format!("{i}.flac"),
                *year,
                *genre,
                *albumartist,
                *albumartistsort,
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

fn browse_value(value: &str, count: i64) -> BrowseValue {
    BrowseValue {
        value: value.into(),
        count,
        sort_value: None,
    }
}

fn custom(tag: &str) -> BrowseMode {
    BrowseMode::Custom(tag.to_string())
}

#[test]
fn fixed_albumartist_uses_romanized_sort_values() {
    let (_d, db) = make_db();

    let vals = db
        .browse_values_exact(BrowseMode::Albumartist, None)
        .unwrap();
    assert_eq!(
        vals,
        vec![
            browse_value("", 1),
            browse_value("Shanghai Alice", 2),
            browse_value("Zulu", 1),
        ],
    );
}

#[test]
fn fixed_albumartist_merges_raw_names_that_render_as_the_same_name() {
    let (_d, db) = make_db();
    db.with(|conn| {
        conn.execute(
            "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms,
                                albumartist, albumartistsort)
             VALUES ('/m/other.flac', '/m', 'other.flac', 0, 0, 'FLAC', 180000,
                     '上海愛麗絲', 'Shanghai Alice')",
            [],
        )?;
        conn.execute(
            "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms,
                                albumartist)
             VALUES ('/m/latin.flac', '/m', 'latin.flac', 0, 0, 'FLAC', 180000,
                     'Shanghai Alice')",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let vals = db
        .browse_values_exact(BrowseMode::Albumartist, None)
        .unwrap();
    assert_eq!(
        vals.iter().find(|v| v.value == "Shanghai Alice"),
        Some(&browse_value("Shanghai Alice", 4)),
    );
    assert_eq!(
        db.track_count_exact(None, BrowseMode::Albumartist, &["Shanghai Alice".into()])
            .unwrap(),
        4,
        "selecting the merged row must include every raw albumartist"
    );

    let ctx = PlayContext::ValueTracks {
        mode: BrowseMode::Albumartist,
        values: vec!["Shanghai Alice".into()],
        query: None,
        projection_revision: 0,
        sort: None,
    };
    assert_eq!(db.resolve_context_exact(&ctx).unwrap().len(), 4);
}

#[test]
fn prefer_romanized_off_orders_album_artists_by_the_shown_name() {
    let (_d, db) = make_db();
    db.prefs_set("prefer_romanized", "false").unwrap();

    // Rows use and order by the untouched name: 上海… falls after the Latin ones.
    let vals = db
        .browse_values_exact(BrowseMode::Albumartist, None)
        .unwrap();
    assert_eq!(
        vals,
        vec![
            browse_value("", 1),
            browse_value("Zulu", 1),
            browse_value("上海アリス幻樂団", 2),
        ],
    );
}

#[test]
fn latin_albumartist_is_ordered_by_its_visible_name_not_its_sort_tag() {
    let (_d, db) = make_db();
    db.with(|conn| {
        conn.execute(
            "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms,
                                albumartist, albumartistsort)
             VALUES ('/m/ara.flac', '/m', 'ara.flac', 0, 0, 'FLAC', 180000,
                     'ARA', 'CarrotWine.')",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let vals = db
        .browse_values_exact(BrowseMode::Albumartist, None)
        .unwrap();
    let ara = vals.iter().position(|v| v.value == "ARA").unwrap();
    let shanghai = vals
        .iter()
        .position(|v| v.value == "Shanghai Alice")
        .unwrap();
    assert!(
        ara < shanghai,
        "ARA is displayed and therefore sorted under A, not C"
    );
}

#[test]
fn custom_year_browses_integer_column_as_text() {
    let (_d, db) = make_db();

    // INTEGER `year` values come back as strings, NULL folds to the "" bucket.
    let vals = db.browse_values_exact(custom("year"), None).unwrap();
    let c = counts(&vals);
    assert_eq!(vals.len(), 3);
    assert_eq!(c.get("1999"), Some(&2));
    assert_eq!(c.get("2000"), Some(&1));
    assert_eq!(c.get(""), Some(&1), "NULL year is the '?' bucket");
}

#[test]
fn custom_tag_is_normalized_and_date_aliases_year() {
    let (_d, db) = make_db();

    // Leading/trailing space + case are normalized, and `date` aliases `year`.
    let year = counts(&db.browse_values_exact(custom("  YeAr "), None).unwrap());
    let date = counts(&db.browse_values_exact(custom("date"), None).unwrap());
    assert_eq!(year.get("1999"), Some(&2));
    assert_eq!(date, year);
}

#[test]
fn selecting_custom_integer_value_matches_via_text_compare() {
    let (_d, db) = make_db();

    // The selected value is a string ("1999") compared against an INTEGER column
    // — the CAST-to-TEXT is what makes this equal.
    assert_eq!(
        db.track_count_exact(None, custom("year"), &["1999".into()])
            .unwrap(),
        2
    );
    assert_eq!(
        db.track_count_exact(None, custom("year"), &["2000".into()])
            .unwrap(),
        1
    );
    assert_eq!(
        db.track_count_exact(None, custom("year"), &["".into()])
            .unwrap(),
        1
    );
}

#[test]
fn custom_genre_handles_non_ascii_values() {
    let (_d, db) = make_db();

    let vals = db.browse_values_exact(custom("genre"), None).unwrap();
    let c = counts(&vals);
    assert_eq!(c.get("ジャズ"), Some(&2));
    assert_eq!(c.get("Rock"), Some(&1));
    // Selecting a CJK value round-trips.
    assert_eq!(
        db.track_count_exact(None, custom("genre"), &["ジャズ".into()])
            .unwrap(),
        2
    );
}

#[test]
fn unresolvable_custom_tag_yields_nothing() {
    let (_d, db) = make_db();

    // Unknown tag → empty value list …
    assert!(db
        .browse_values_exact(custom("mood"), None)
        .unwrap()
        .is_empty());
    // … and selecting against it matches no tracks (the `AND 0` guard).
    assert_eq!(
        db.track_count_exact(None, custom("nope"), &["x".into()])
            .unwrap(),
        0
    );
}

#[test]
fn resolve_context_plays_custom_value() {
    let (_d, db) = make_db();

    let ctx = PlayContext::ValueTracks {
        mode: custom("year"),
        values: vec!["1999".into()],
        query: None,
        projection_revision: 0,
        sort: None,
    };
    let ids = db.resolve_context_exact(&ctx).unwrap();
    assert_eq!(ids.len(), 2, "both 1999 tracks resolve for playback");

    // An unresolvable custom tag resolves to an empty queue rather than erroring.
    let empty = PlayContext::ValueTracks {
        mode: custom("composer"),
        values: vec!["whoever".into()],
        query: None,
        projection_revision: 0,
        sort: None,
    };
    assert!(db.resolve_context_exact(&empty).unwrap().is_empty());
}

#[test]
fn grouping_is_reachable_only_through_the_custom_wire_variant() {
    assert!(serde_json::from_str::<BrowseMode>(r#""grouping""#).is_err());
    assert_eq!(
        serde_json::from_str::<BrowseMode>(r#"{"custom":"grouping"}"#).unwrap(),
        custom("grouping")
    );
}

/// Queueing a facet value must resolve the *value*, not the albums it happens
/// to touch. The frontend used to expand each matching album and queue every
/// track in it, which quietly added the rest of a mixed-genre album; both Play
/// and Add-to-queue now go through this one resolution.
#[test]
fn a_value_context_excludes_other_tracks_from_the_same_album() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        // One album, two genres — a compilation, or a mistagged rip.
        let mut ins = conn.prepare(
            "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, duration_ms,
                                title, genre, album, albumartist)
             VALUES (?1, ?2, '/m', ?3, 0, 0, 'FLAC', 1000, ?4, ?5, 'Mixed', 'Various')",
        )?;
        for (id, genre) in [(1, "Rock"), (2, "Jazz"), (3, "Rock")] {
            ins.execute(rusqlite::params![
                id,
                format!("/m/{id}.flac"),
                format!("{id}.flac"),
                format!("Track {id}"),
                genre,
            ])?;
        }
        Ok(())
    })
    .unwrap();

    let ctx = PlayContext::ValueTracks {
        mode: BrowseMode::Genre,
        values: vec!["Rock".into()],
        query: None,
        projection_revision: 0,
        sort: None,
    };
    assert_eq!(
        db.resolve_context_exact(&ctx).unwrap(),
        vec![1, 3],
        "the Jazz track shares the album but not the selected value"
    );

    // Selecting a value the album does not carry queues nothing at all, rather
    // than the album that merely sits next to it.
    let ctx = PlayContext::ValueTracks {
        mode: BrowseMode::Genre,
        values: vec!["Ambient".into()],
        query: None,
        projection_revision: 0,
        sort: None,
    };
    assert!(db.resolve_context_exact(&ctx).unwrap().is_empty());
}
