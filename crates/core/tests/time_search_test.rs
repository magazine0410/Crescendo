//! `added` and `lastplayed` compare as an age, with units of time, as in
//! Quod Libet: `added < 1 day` finds tracks added less than a day ago.

use crescendo_core::db::Db;
use crescendo_core::model::BrowseMode;
use crescendo_core::scanner::{scan, ScanOptions};
use crescendo_core::search;
use std::sync::atomic::AtomicBool;

const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Track 1: added an hour ago, played two hours ago. Track 2: added 10 days
/// ago, played 40 days ago. Track 3: added 2 years ago, never played.
/// Track 4: no date added yet (a row from before the column).
fn make_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    let now = now();
    let rows: [(i64, Option<i64>, Option<i64>); 4] = [
        (1, Some(now - HOUR), Some(now - 2 * HOUR)),
        (2, Some(now - 10 * DAY), Some(now - 40 * DAY)),
        (3, Some(now - 730 * DAY), None),
        (4, None, None),
    ];
    db.with(|conn| {
        for (id, added, played) in rows {
            conn.execute(
                "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, duration_ms,
                                    added_at, last_played, play_count)
                 VALUES (?1, ?2, '/m', ?3, 0, 0, 'FLAC', 180000, ?4, ?5, ?5 IS NOT NULL)",
                rusqlite::params![
                    id,
                    format!("/m/{id}.flac"),
                    format!("{id}.flac"),
                    added,
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
fn added_compares_as_an_age() {
    let (_dir, db) = make_db();
    assert_eq!(ids(&db, "added < 1 day"), [1]);
    assert_eq!(ids(&db, "#(added < 2 weeks)"), [1, 2]);
    assert_eq!(ids(&db, "added > 1 year"), [3]);
    assert_eq!(ids(&db, "#(1 day < added < 30 days)"), [2]);
}

#[test]
fn units_may_be_attached_plural_or_capitalised() {
    let (_dir, db) = make_db();
    assert_eq!(ids(&db, "added<1day"), [1]);
    assert_eq!(ids(&db, "added < 2 Hours"), [1]);
    assert_eq!(ids(&db, "added < 90 minutes"), [1]);
    assert!(ids(&db, "added < 3600 seconds").is_empty(), "an hour and a bit ago");
    assert_eq!(ids(&db, "#(added < 1 month)"), [1, 2]);
    assert_eq!(ids(&db, "added >= 1.5years"), [3]);
}

#[test]
fn never_played_is_older_than_any_span() {
    let (_dir, db) = make_db();
    assert_eq!(ids(&db, "lastplayed < 1 day"), [1]);
    assert_eq!(ids(&db, "lastplayed > 30 days"), [2, 3, 4]);
    assert_eq!(ids(&db, "&(lastplayed > 30 days, plays = 0)"), [3, 4]);
}

#[test]
fn units_of_time_need_a_time_tag() {
    assert!(search::validate("plays > 2 days").is_some());
    assert!(search::validate("#(year < 3 weeks)").is_some());
    // A word that is not a unit stays free text after the comparison.
    assert!(search::validate("plays > 2 jazz").is_none());
    assert!(search::validate("added < 3 fortnights").is_none());
}

#[test]
fn a_scan_fills_in_the_date_added_and_keeps_it() {
    let home = tempfile::tempdir().unwrap();
    let music = home.path().join("music");
    std::fs::create_dir(&music).unwrap();
    std::fs::write(music.join("a.flac"), b"not really flac").unwrap();
    let db = Db::open(&home.path().join("t.db")).unwrap();
    let run = || {
        let opts = ScanOptions {
            root: music.clone(),
            threads: 1,
            full: false,
        };
        scan(&db, &opts, &|_| {}, &AtomicBool::new(false)).unwrap();
    };
    let added = || -> Option<i64> {
        db.with(|conn| {
            Ok(conn.query_row("SELECT added_at FROM tracks", [], |r| r.get::<_, Option<i64>>(0))?)
        })
        .unwrap()
    };

    run();
    let first = added().expect("a new track gets a date added");
    assert!((now() - first).abs() < 60, "the file was just created");

    // A row from before the column: an unchanged file is filled in by the
    // next scan, without its tags being read again.
    db.with(|conn| Ok(conn.execute("UPDATE tracks SET added_at = NULL", [])?))
        .map(|_: usize| ())
        .unwrap();
    run();
    assert_eq!(added(), Some(first));

    // Once set, a later scan of a changed file leaves it alone.
    db.with(|conn| Ok(conn.execute("UPDATE tracks SET added_at = 1000", [])?))
        .map(|_: usize| ())
        .unwrap();
    std::fs::write(music.join("a.flac"), b"edited, and longer than before").unwrap();
    run();
    assert_eq!(added(), Some(1000));
}

#[test]
fn a_span_can_lead_a_range_without_the_hash_form() {
    let (_dir, db) = make_db();
    assert_eq!(ids(&db, "1 day < added < 30 days"), [2]);
    assert_eq!(ids(&db, "1day<added<30days"), [2]);
    assert_eq!(ids(&db, "1 year < added"), [3]);
    // Without a comparison after it, `1 day` is two words of text.
    assert_eq!(ids(&db, "1 day"), Vec::<i64>::new());
    assert!(search::validate("1 day < plays").is_some());
}
