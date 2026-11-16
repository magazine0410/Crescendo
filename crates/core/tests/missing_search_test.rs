//! `@(missing: tag)`: Quod Libet's query extension for tracks whose tag is
//! absent or blank.

use crescendo_core::db::Db;
use crescendo_core::model::BrowseMode;
use crescendo_core::search::{self, QueryKind};

/// (genre, year, composer, last_played)
type Row = (
    Option<&'static str>,
    Option<i64>,
    Option<&'static str>,
    Option<i64>,
);

/// Tracks 1..=5, one per row below.
fn make_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    let rows: [Row; 5] = [
        (Some("Jazz"), Some(1999), Some("Bach"), Some(1_700_000_000)),
        (None, Some(2001), None, None),
        (Some(""), None, Some("ZUN"), None),
        (Some("   "), None, None, None),
        (Some("Rock; Pop"), Some(1985), None, None),
    ];
    db.with(|conn| {
        for (i, (genre, year, composer, played)) in rows.iter().enumerate() {
            let id = i as i64 + 1;
            conn.execute(
                "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, duration_ms,
                                    genre, year, composer, last_played)
                 VALUES (?1, ?2, '/m', ?3, 0, 0, 'FLAC', 180000, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    id,
                    format!("/m/{id}.flac"),
                    format!("{id}.flac"),
                    genre,
                    year,
                    composer,
                    played
                ],
            )?;
        }
        Ok(())
    })
    .unwrap();
    (dir, db)
}

fn ids(db: &Db, query: &str) -> Vec<i64> {
    let filter = search::compile(query).unwrap();
    let mut ids: Vec<i64> = db
        .tracks_range_exact(Some(&filter), BrowseMode::Artist, &[], None, 0, 100)
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect();
    ids.sort();
    ids
}

#[test]
fn absent_empty_and_blank_text_tags_are_missing() {
    let (_dir, db) = make_db();
    assert_eq!(ids(&db, "@(missing: genre)"), [2, 3, 4]);
    assert_eq!(ids(&db, "@(missing: composer)"), [2, 4, 5]);
}

#[test]
fn number_and_time_tags_can_be_missing() {
    let (_dir, db) = make_db();
    assert_eq!(ids(&db, "@(missing: year)"), [3, 4]);
    assert_eq!(ids(&db, "@(missing: date)"), [3, 4], "date is year's alias");
    assert_eq!(
        ids(&db, "@(missing: lastplayed)"),
        [2, 3, 4, 5],
        "never played"
    );
}

#[test]
fn aliases_case_spacing_and_combining() {
    let (_dir, db) = make_db();
    assert_eq!(ids(&db, "@(missing: g)"), [2, 3, 4]);
    assert_eq!(ids(&db, "@( MISSING :Genre )"), [2, 3, 4]);
    assert_eq!(ids(&db, "!@(missing: genre)"), [1, 5]);
    assert_eq!(ids(&db, "&(@(missing: genre), year>2000)"), [2]);
    assert_eq!(
        ids(&db, "|(@(missing: genre), @(missing: year))"),
        [2, 3, 4]
    );
}

#[test]
fn unknown_extensions_and_tags_are_errors() {
    for bad in [
        "@(missing: nosuchtag)",
        "@(saved: Unrated)",
        "@(missing genre)",
        "@(missing: )",
        "@(missing: genre",
        "@(missing: length)",
    ] {
        assert!(search::validate(bad).is_some(), "{bad} should not parse");
    }
    // An @ that does not open an extension is ordinary text.
    assert!(search::validate("user@example").is_none());
}

#[test]
fn the_search_bar_tints_it_as_syntax() {
    let spans = search::highlight("jazz @(missing: year)");
    let kinds: Vec<QueryKind> = spans.iter().map(|s| s.kind).collect();
    assert_eq!(kinds, [QueryKind::Plain, QueryKind::Syntax]);
}
