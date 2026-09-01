use crescendo_core::db::Db;
use crescendo_core::model::{BrowseMode, BrowseValue, PlayContext, SortKey, TrackSort};

fn custom(tag: &str) -> BrowseMode {
    BrowseMode::Custom(tag.into())
}

fn browse_value(value: &str, count: i64) -> BrowseValue {
    BrowseValue { value: value.into(), count, sort_value: None }
}

/// Nine tracks, one per interesting distance from today. `days` is how long ago
/// the track was last played; `None` = never played.
const TRACKS: [(&str, Option<i64>); 9] = [
    ("Today", Some(0)),
    ("ThreeDays", Some(3)),
    ("SixDays", Some(6)),
    ("SevenDays", Some(7)),
    ("TwentyNine", Some(29)),
    ("Thirty", Some(30)),
    ("LastYear", Some(364)),
    ("Ancient", Some(365)),
    ("Never", None),
];

/// File a play `days` ago at local noon — the row `record_play` would have
/// written that day — and refresh the track's `last_played` cache from it.
fn play_days_ago(db: &Db, id: i64, days: i64) {
    db.with(|conn| {
        conn.execute(
            "INSERT INTO play_history(track_id, played_at, year, month, day) \
             SELECT ?1, e, \
                    CAST(strftime('%Y', e, 'unixepoch', 'localtime') AS INTEGER), \
                    CAST(strftime('%m', e, 'unixepoch', 'localtime') AS INTEGER), \
                    CAST(strftime('%d', e, 'unixepoch', 'localtime') AS INTEGER) \
             FROM (SELECT unixepoch(datetime('now', 'localtime', 'start of day', ?2, \
                                             '+12 hours'), 'utc') AS e)",
            rusqlite::params![id, format!("-{days} days")],
        )?;
        conn.execute(
            "UPDATE tracks SET play_count = play_count + 1, \
                    last_played = (SELECT MAX(played_at) FROM play_history WHERE track_id = ?1) \
             WHERE id = ?1",
            [id],
        )?;
        Ok(())
    })
    .unwrap();
}

fn test_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        let mut ins = conn.prepare(
            "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms,
                                title, artist, albumartist, album, album_id, disc_no,
                                track_no, year)
             VALUES (?1, '/m', ?2, 0, 0, 'FLAC', 180000, ?3, 'Artist', 'Artist',
                     'Album', 1, 1, ?4, 2020)",
        )?;
        for (i, (title, _)) in TRACKS.iter().enumerate() {
            ins.execute(rusqlite::params![
                format!("/m/{i}.flac"),
                format!("{i}.flac"),
                title,
                i as i64 + 1,
            ])?;
        }
        conn.execute(
            "INSERT INTO albums(id, key, album, albumartist, year, track_count)
             VALUES (1, 'k', 'Album', 'Artist', 2020, 9)",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let id_of = |title: &str| -> i64 {
        db.with(|c| Ok(c.query_row("SELECT id FROM tracks WHERE title = ?1", [title], |r| r.get(0))?))
            .unwrap()
    };
    for (title, days) in TRACKS {
        if let Some(days) = days {
            play_days_ago(&db, id_of(title), days);
        }
    }
    (dir, db)
}

fn titles(rows: Vec<crescendo_core::model::Track>) -> Vec<String> {
    rows.into_iter().map(|t| t.title.unwrap()).collect()
}

#[test]
fn buckets_group_by_days_since_the_last_play() {
    let (_d, db) = test_db();
    let values = db.browse_values_exact(custom("lastplayed"), None).unwrap();
    // "0" today, "1" within 7 days, "2" within 30, "3" within 365, "4" longer
    // ago, "5" never — newest bucket first, and every track in exactly one.
    assert_eq!(
        values,
        vec![
            browse_value("0", 1), // Today
            browse_value("1", 2), // 3 and 6 days
            browse_value("2", 2), // 7 and 29 days
            browse_value("3", 2), // 30 and 364 days
            browse_value("4", 1), // 365 days
            browse_value("5", 1), // never played
        ]
    );
    assert_eq!(values.iter().map(|value| value.count).sum::<i64>(), TRACKS.len() as i64);
    // The %last_played% spelling browses identically.
    assert_eq!(db.browse_values_exact(custom("last_played"), None).unwrap(), values);
}

#[test]
fn bucket_edges_fall_on_whole_days() {
    let (_d, db) = test_db();
    let in_bucket = |b: &str| {
        titles(db.tracks_range_exact(None, custom("lastplayed"), &[b.into()], None, 0, 100).unwrap())
    };
    // A span holds the days below its edge, and the edge itself moves on.
    assert_eq!(in_bucket("0"), ["Today"]);
    assert_eq!(in_bucket("1"), ["ThreeDays", "SixDays"]);
    assert_eq!(in_bucket("2"), ["SevenDays", "TwentyNine"]);
    assert_eq!(in_bucket("3"), ["Thirty", "LastYear"]);
    assert_eq!(in_bucket("4"), ["Ancient"]);
    assert_eq!(in_bucket("5"), ["Never"]);
}

#[test]
fn several_buckets_select_together() {
    let (_d, db) = test_db();
    let both = &["0".to_string(), "1".to_string()];
    assert_eq!(db.track_count_exact(None, custom("lastplayed"), both).unwrap(), 3);

    // Playback resolves the same set, in the same order the view shows.
    let ctx = PlayContext::ValueTracks {
        mode: custom("lastplayed"),
        values: both.to_vec(),
        query: None,
        projection_revision: 0,
        sort: None,
    };
    let ids = db.resolve_context_exact(&ctx).unwrap();
    let view = db.tracks_range_exact(None, custom("lastplayed"), both, None, 0, 100).unwrap();
    assert_eq!(ids, view.iter().map(|t| t.id).collect::<Vec<_>>());
}

#[test]
fn play_count_lists_most_played_first() {
    let (_d, db) = test_db();
    // Every track in TRACKS has one play except "Never"; give two of them more
    // so the list has several counts to order.
    let id_of = |title: &str| -> i64 {
        db.with(|c| Ok(c.query_row("SELECT id FROM tracks WHERE title = ?1", [title], |r| r.get(0))?))
            .unwrap()
    };
    for _ in 0..2 {
        db.record_play(id_of("Today")).unwrap(); // 3 plays
    }
    db.record_play(id_of("SixDays")).unwrap(); // 2 plays

    let values = db.browse_values_exact(custom("playcount"), None).unwrap();
    assert_eq!(
        values,
        vec![
            browse_value("3", 1), // Today
            browse_value("2", 1), // SixDays
            browse_value("1", 6), // the rest of the played ones
            browse_value("0", 1), // Never — always at the bottom
        ]
    );
    // The %plays% spelling browses identically.
    assert_eq!(db.browse_values_exact(custom("plays"), None).unwrap(), values);

    // Selecting a count narrows the view to exactly those tracks.
    let rows = db.tracks_range_exact(None, custom("playcount"), &["0".into()], None, 0, 100).unwrap();
    assert_eq!(titles(rows), ["Never"]);
    assert_eq!(db.track_count_exact(None, custom("playcount"), &["1".into()]).unwrap(), 6);
}

#[test]
fn album_wall_narrows_to_the_selected_span() {
    let (_d, db) = test_db();
    // The default Library view lists albums, not tracks: a span with plays in it
    // shows its album, an empty one shows nothing.
    let albums = db.browse_albums_exact(custom("lastplayed"), &["0".into()], None).unwrap();
    assert_eq!(albums.len(), 1);
    // Every track here shares one album, so no span is empty; a filter that
    // matches nothing still has to come back empty.
    let none = db
        .browse_albums_exact(custom("lastplayed"), &["4".into()], Some(&search_for("nothingmatches")))
        .unwrap();
    assert_eq!(none.len(), 0);
}

fn search_for(q: &str) -> crescendo_core::search::SearchFilter {
    crescendo_core::search::compile(q).unwrap()
}

#[test]
fn sort_orders_by_the_time_of_the_last_play() {
    let (_d, db) = test_db();
    let sorted = |descending: bool| {
        let sort = TrackSort {
            key: SortKey::LastPlayed,
            secondary: None,
            descending,
            secondary_descending: None,
        };
        titles(
            db.tracks_range_exact(None, BrowseMode::Artist, &[], Some(sort), 0, 100).unwrap(),
        )
    };
    // Descending: the most recent play first, never-played tracks at the bottom.
    assert_eq!(
        sorted(true),
        [
            "Today", "ThreeDays", "SixDays", "SevenDays", "TwentyNine", "Thirty", "LastYear",
            "Ancient", "Never",
        ]
    );
    // Ascending is the exact reverse: a track with no play at all counts as the
    // least recently played.
    let mut reversed = sorted(true);
    reversed.reverse();
    assert_eq!(sorted(false), reversed);
}

#[test]
fn a_new_play_moves_a_track_to_today() {
    let (_d, db) = test_db();
    let ancient = db
        .with(|c| Ok(c.query_row("SELECT id FROM tracks WHERE title = 'Ancient'", [], |r| r.get(0))?))
        .unwrap();
    db.record_play(ancient).unwrap();

    let today =
        db.tracks_range_exact(None, custom("lastplayed"), &["0".into()], None, 0, 100).unwrap();
    assert_eq!(titles(today), ["Today", "Ancient"]);
    // Its older play is still on the record — the bucket reads the newest one.
    let plays: i64 = db
        .with(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM play_history WHERE track_id = ?1",
                [ancient],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(plays, 2);
    assert_eq!(
        db.tracks_range_exact(None, custom("lastplayed"), &["4".into()], None, 0, 100)
            .unwrap()
            .len(),
        0
    );
}
