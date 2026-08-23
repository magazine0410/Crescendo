use crescendo_core::db::Db;
use crescendo_core::model::{BrowseMode, PlayContext};
use std::collections::HashMap;

/// A tiny library exercising the custom left-panel filter: an INTEGER column
/// (`year`, incl. a NULL), a CJK-tagged TEXT column (`genre`), and album
/// artists with a romanized sort name.
fn make_db() -> (tempfile::TempDir, Db) {
    // (year, genre, albumartist, albumartistsort)
    let rows: [(Option<i64>, Option<&str>, Option<&str>, Option<&str>); 4] = [
        (Some(1999), Some("ジャズ"), Some("上海アリス幻樂団"), Some("Shanghai Alice")),
        (Some(1999), Some("ジャズ"), Some("上海アリス幻樂団"), Some("Shanghai Alice")),
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

fn counts(vals: &[(String, i64, String)]) -> HashMap<String, i64> {
    vals.iter().map(|(v, n, _)| (v.clone(), *n)).collect()
}

fn custom(tag: &str) -> BrowseMode {
    BrowseMode::Custom(tag.to_string())
}

#[test]
fn fixed_albumartist_uses_romanized_sort_values() {
    let (_d, db) = make_db();

    let vals = db.browse_values(BrowseMode::Albumartist, None, false).unwrap();
    assert_eq!(
        vals,
        vec![
            ("".into(), 1, "".into()),
            ("Shanghai Alice".into(), 2, "".into()),
            ("Zulu".into(), 1, "".into()),
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

    let vals = db.browse_values(BrowseMode::Albumartist, None, false).unwrap();
    assert_eq!(
        vals.iter().find(|(v, _, _)| v == "Shanghai Alice"),
        Some(&("Shanghai Alice".into(), 4, "".into())),
    );
    assert_eq!(
        db.track_count(None, BrowseMode::Albumartist, &["Shanghai Alice".into()], false)
            .unwrap(),
        4,
        "selecting the merged row must include every raw albumartist"
    );

    let ctx = PlayContext::ValueTracks {
        mode: BrowseMode::Albumartist,
        values: vec!["Shanghai Alice".into()],
        query: None,
        collapse: false,
        sort: None,
    };
    assert_eq!(db.resolve_context(&ctx).unwrap().len(), 4);
}

#[test]
fn prefer_romanized_off_orders_album_artists_by_the_shown_name() {
    let (_d, db) = make_db();
    db.prefs_set("prefer_romanized", "false").unwrap();

    // Rows use and order by the untouched name: 上海… falls after the Latin ones.
    let vals = db.browse_values(BrowseMode::Albumartist, None, false).unwrap();
    assert_eq!(
        vals,
        vec![
            ("".into(), 1, "".into()),
            ("Zulu".into(), 1, "".into()),
            ("上海アリス幻樂団".into(), 2, "".into()),
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

    let vals = db.browse_values(BrowseMode::Albumartist, None, false).unwrap();
    let ara = vals.iter().position(|(v, _, _)| v == "ARA").unwrap();
    let shanghai = vals.iter().position(|(v, _, _)| v == "Shanghai Alice").unwrap();
    assert!(ara < shanghai, "ARA is displayed and therefore sorted under A, not C");
}

#[test]
fn custom_year_browses_integer_column_as_text() {
    let (_d, db) = make_db();

    // INTEGER `year` values come back as strings, NULL folds to the "" bucket.
    let vals = db.browse_values(custom("year"), None, false).unwrap();
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
    let year = counts(&db.browse_values(custom("  YeAr "), None, false).unwrap());
    let date = counts(&db.browse_values(custom("date"), None, false).unwrap());
    assert_eq!(year.get("1999"), Some(&2));
    assert_eq!(date, year);
}

#[test]
fn selecting_custom_integer_value_matches_via_text_compare() {
    let (_d, db) = make_db();

    // The selected value is a string ("1999") compared against an INTEGER column
    // — the CAST-to-TEXT is what makes this equal.
    assert_eq!(db.track_count(None, custom("year"), &["1999".into()], false).unwrap(), 2);
    assert_eq!(db.track_count(None, custom("year"), &["2000".into()], false).unwrap(), 1);
    assert_eq!(db.track_count(None, custom("year"), &["".into()], false).unwrap(), 1);
}

#[test]
fn custom_genre_handles_non_ascii_values() {
    let (_d, db) = make_db();

    let vals = db.browse_values(custom("genre"), None, false).unwrap();
    let c = counts(&vals);
    assert_eq!(c.get("ジャズ"), Some(&2));
    assert_eq!(c.get("Rock"), Some(&1));
    // Selecting a CJK value round-trips.
    assert_eq!(db.track_count(None, custom("genre"), &["ジャズ".into()], false).unwrap(), 2);
}

#[test]
fn unresolvable_custom_tag_yields_nothing() {
    let (_d, db) = make_db();

    // Unknown tag → empty value list …
    assert!(db.browse_values(custom("mood"), None, false).unwrap().is_empty());
    // … and selecting against it matches no tracks (the `AND 0` guard).
    assert_eq!(db.track_count(None, custom("nope"), &["x".into()], false).unwrap(), 0);
}

#[test]
fn resolve_context_plays_custom_value() {
    let (_d, db) = make_db();

    let ctx = PlayContext::ValueTracks {
        mode: custom("year"),
        values: vec!["1999".into()],
        query: None,
        collapse: false,
        sort: None,
    };
    let ids = db.resolve_context(&ctx).unwrap();
    assert_eq!(ids.len(), 2, "both 1999 tracks resolve for playback");

    // An unresolvable custom tag resolves to an empty queue rather than erroring.
    let empty = PlayContext::ValueTracks {
        mode: custom("composer"),
        values: vec!["whoever".into()],
        query: None,
        collapse: false,
        sort: None,
    };
    assert!(db.resolve_context(&empty).unwrap().is_empty());
}
