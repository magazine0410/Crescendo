use crescendo_core::db::Db;
use crescendo_core::model::{BrowseMode, PlayContext, SortKey, TrackSort};

/// Tracks across two albumartists with distinct years, durations, and titlesort
/// tags, so every explicit sort key produces an order different from
/// LIBRARY_SORT.
fn test_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        let mut ins = conn.prepare(
            "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms,
                                title, titlesort, albumartist, artist, album, album_id,
                                disc_no, track_no, year)
             VALUES (?1, ?2, ?3, 0, 0, 'FLAC', ?4, ?5, ?6, ?7, ?7, ?8, ?9, 1, ?10, ?11)",
        )?;
        // (title, titlesort, albumartist, album, album_id, track_no, year, duration)
        let rows: Vec<(&str, Option<&str>, &str, &str, i64, i64, i64, i64)> = vec![
            ("Aria", None, "Artist A", "Alpha", 1, 1, 2014, 300_000),
            ("Coda", None, "Artist A", "Alpha", 1, 2, 2014, 100_000),
            ("ボーナス", Some("Bonus"), "Artist B", "Beta", 2, 1, 2010, 200_000),
        ];
        for (i, (title, tsort, aa, album, aid, tn, year, dur)) in rows.iter().enumerate() {
            ins.execute(rusqlite::params![
                format!("/m/{i}.flac"),
                "/m",
                format!("{i}.flac"),
                dur,
                title,
                tsort,
                aa,
                album,
                aid,
                tn,
                year,
            ])?;
        }
        Ok(())
    })
    .unwrap();
    (dir, db)
}

fn titles(db: &Db, sort: Option<TrackSort>) -> Vec<String> {
    db.tracks_range(None, BrowseMode::Artist, &[], false, sort, 0, 100)
        .unwrap()
        .into_iter()
        .map(|t| t.title.unwrap())
        .collect()
}

fn sort(key: SortKey, secondary: Option<SortKey>, descending: bool) -> Option<TrackSort> {
    Some(TrackSort { key, secondary, descending, secondary_descending: None })
}

#[test]
fn no_sort_keeps_library_order() {
    let (_d, db) = test_db();
    assert_eq!(titles(&db, None), ["Aria", "Coda", "ボーナス"]);
}

#[test]
fn primary_key_orders_and_descends() {
    let (_d, db) = test_db();
    // Year ascending: 2010 first; ties (Alpha, 2014) fall back to track_no.
    assert_eq!(
        titles(&db, sort(SortKey::Year, None, false)),
        ["ボーナス", "Aria", "Coda"]
    );
    assert_eq!(
        titles(&db, sort(SortKey::Year, None, true)),
        ["Aria", "Coda", "ボーナス"]
    );
    assert_eq!(
        titles(&db, sort(SortKey::Duration, None, false)),
        ["Coda", "ボーナス", "Aria"]
    );
}

#[test]
fn title_sort_uses_titlesort_tag() {
    let (_d, db) = test_db();
    // "ボーナス" sorts as "Bonus": between Aria and Coda instead of last.
    assert_eq!(
        titles(&db, sort(SortKey::Title, None, false)),
        ["Aria", "ボーナス", "Coda"]
    );
}

#[test]
fn prefer_romanized_off_sorts_titles_by_the_shown_tag() {
    let (_d, db) = test_db();
    db.prefs_set("prefer_romanized", "false").unwrap();
    // With the tag ignored, "ボーナス" sorts by itself again — after the Latin
    // titles, where SQLite's code-point order puts it.
    let s = sort(SortKey::Title, None, false);
    assert_eq!(titles(&db, s), ["Aria", "Coda", "ボーナス"]);

    // The view and the play queue must still agree on that order.
    let ids = db.resolve_context(&PlayContext::LibraryView { query: None, sort: s }).unwrap();
    let view = db.tracks_range(None, BrowseMode::Artist, &[], false, s, 0, 100).unwrap();
    assert_eq!(ids, view.iter().map(|t| t.id).collect::<Vec<_>>());
    for (i, t) in view.iter().enumerate() {
        let idx = db.track_index(None, BrowseMode::Artist, &[], false, s, t.id).unwrap();
        assert_eq!(idx, Some(i as i64));
    }
}

#[test]
fn secondary_key_breaks_primary_ties() {
    let (_d, db) = test_db();
    // Within the 2014 pair, duration (following the descending primary)
    // reverses the track order.
    assert_eq!(
        titles(&db, sort(SortKey::Year, Some(SortKey::Duration), true)),
        ["Aria", "Coda", "ボーナス"]
    );
    assert_eq!(
        titles(&db, sort(SortKey::Year, Some(SortKey::Duration), false)),
        ["ボーナス", "Coda", "Aria"]
    );
}

#[test]
fn secondary_direction_can_differ_from_primary() {
    let (_d, db) = test_db();
    // Year descending, but duration explicitly ascending: the 2014 pair runs
    // shortest-first while the years still descend.
    let s = Some(TrackSort {
        key: SortKey::Year,
        secondary: Some(SortKey::Duration),
        descending: true,
        secondary_descending: Some(false),
    });
    assert_eq!(titles(&db, s), ["Coda", "Aria", "ボーナス"]);
    // And the mirror case: year ascending, duration explicitly descending.
    let s = Some(TrackSort {
        key: SortKey::Year,
        secondary: Some(SortKey::Duration),
        descending: false,
        secondary_descending: Some(true),
    });
    assert_eq!(titles(&db, s), ["ボーナス", "Aria", "Coda"]);
}

#[test]
fn view_playback_and_index_agree_on_sorted_order() {
    let (_d, db) = test_db();
    let s = sort(SortKey::Duration, None, false);
    let ctx = PlayContext::LibraryView { query: None, sort: s };
    let ids = db.resolve_context(&ctx).unwrap();
    let view = db.tracks_range(None, BrowseMode::Artist, &[], false, s, 0, 100).unwrap();
    assert_eq!(ids, view.iter().map(|t| t.id).collect::<Vec<_>>());
    for (i, t) in view.iter().enumerate() {
        let idx = db
            .track_index(None, BrowseMode::Artist, &[], false, s, t.id)
            .unwrap();
        assert_eq!(idx, Some(i as i64));
    }
}

#[test]
fn track_groups_follow_sorted_order() {
    let (_d, db) = test_db();
    // Duration ascending interleaves the two albums: Coda (Alpha), ボーナス
    // (Beta), Aria (Alpha) -> three single-track runs, album 1 re-appearing.
    let groups = db
        .track_groups(None, BrowseMode::Artist, &[], false, sort(SortKey::Duration, None, false))
        .unwrap();
    let shape: Vec<(Option<i64>, i64)> = groups.iter().map(|g| (g.album_id, g.count)).collect();
    assert_eq!(shape, vec![(Some(1), 1), (Some(2), 1), (Some(1), 1)]);
}
