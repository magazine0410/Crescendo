use crescendo_core::db::Db;
use crescendo_core::model::{BrowseMode, PlayContext};
use std::collections::HashMap;

/// A tiny library exercising the custom left-panel filter: an INTEGER column
/// (`year`, incl. a NULL) and a CJK-tagged TEXT column (`genre`).
fn make_db() -> (tempfile::TempDir, Db) {
    // (year, genre)
    let rows: [(Option<i64>, Option<&str>); 4] = [
        (Some(1999), Some("ジャズ")),
        (Some(1999), Some("ジャズ")),
        (Some(2000), Some("Rock")),
        (None, None),
    ];
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        let mut ins = conn.prepare(
            "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms, year, genre)
             VALUES (?1, ?2, ?3, 0, 0, 'FLAC', 180000, ?4, ?5)",
        )?;
        for (i, (year, genre)) in rows.iter().enumerate() {
            ins.execute(rusqlite::params![
                format!("/m/{i}.flac"),
                "/m",
                format!("{i}.flac"),
                *year,
                *genre,
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
    assert_eq!(db.track_count(None, custom("year"), Some("1999"), false).unwrap(), 2);
    assert_eq!(db.track_count(None, custom("year"), Some("2000"), false).unwrap(), 1);
    assert_eq!(db.track_count(None, custom("year"), Some(""), false).unwrap(), 1);
}

#[test]
fn custom_genre_handles_non_ascii_values() {
    let (_d, db) = make_db();

    let vals = db.browse_values(custom("genre"), None, false).unwrap();
    let c = counts(&vals);
    assert_eq!(c.get("ジャズ"), Some(&2));
    assert_eq!(c.get("Rock"), Some(&1));
    // Selecting a CJK value round-trips.
    assert_eq!(db.track_count(None, custom("genre"), Some("ジャズ"), false).unwrap(), 2);
}

#[test]
fn unresolvable_custom_tag_yields_nothing() {
    let (_d, db) = make_db();

    // Unknown tag → empty value list …
    assert!(db.browse_values(custom("composer"), None, false).unwrap().is_empty());
    // … and selecting against it matches no tracks (the `AND 0` guard).
    assert_eq!(db.track_count(None, custom("nope"), Some("x"), false).unwrap(), 0);
}

#[test]
fn resolve_context_plays_custom_value() {
    let (_d, db) = make_db();

    let ctx = PlayContext::ValueTracks {
        mode: custom("year"),
        value: "1999".into(),
        query: None,
        collapse: false,
    };
    let ids = db.resolve_context(&ctx).unwrap();
    assert_eq!(ids.len(), 2, "both 1999 tracks resolve for playback");

    // An unresolvable custom tag resolves to an empty queue rather than erroring.
    let empty = PlayContext::ValueTracks {
        mode: custom("composer"),
        value: "whoever".into(),
        query: None,
        collapse: false,
    };
    assert!(db.resolve_context(&empty).unwrap().is_empty());
}
