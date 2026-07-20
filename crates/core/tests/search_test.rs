use crescendo_core::db::Db;
use crescendo_core::search;

fn test_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        let mut ins = conn.prepare(
            "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms,
                                title, artist, album, albumartist, titlesort, artistsort,
                                genre, track_no, year, search_blob)
             VALUES (?1, ?2, ?3, 0, 0, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        )?;
        type RowT<'a> =
            (&'a str, &'a str, &'a str, Option<&'a str>, Option<&'a str>, i64, i64, i64, &'a str);
        let rows: Vec<RowT> = vec![
            // title, artist, album, titlesort, artistsort, duration_ms, track, year, genre
            ("Now's The Time", "Charlie Parker", "The Essential", None, None, 184_000, 1, 1950, "Jazz"),
            ("Misery", "Maroon 5", "Hands All Over", None, None, 216_000, 1, 2010, "Pop Rock"),
            ("Bird Man", "Joe Hisaishi", "Nausicaä OST", None, None, 226_000, 5, 1984, "Soundtrack"),
            ("ネイティブフェイス", "ZUN", "東方風神録", Some("Native Face"), Some("ZUN"), 300_000, 8, 2007, "Doujin"),
        ];
        for (i, (title, artist, album, tsort, asort, dur, tn, year, genre)) in
            rows.iter().enumerate()
        {
            let mut blob = format!("{}\n{}\n{}\n{}\n", title, artist, album, genre);
            if let Some(ts) = tsort {
                blob.push_str(&format!("{ts}\n"));
            }
            let blob = blob.to_lowercase();
            ins.execute(rusqlite::params![
                format!("/m/{i}.flac"),
                "/m",
                format!("{i}.flac"),
                "FLAC",
                dur,
                title,
                artist,
                album,
                artist,
                tsort,
                asort,
                genre,
                tn,
                year,
                blob,
            ])?;
        }
        Ok(())
    })
    .unwrap();
    (dir, db)
}

fn count(db: &Db, q: &str) -> i64 {
    let filter = search::compile(q).unwrap();
    db.track_count(Some(&filter), crescendo_core::model::BrowseMode::Artist, None).unwrap()
}

#[test]
fn free_text_case_insensitive() {
    let (_d, db) = test_db();
    assert_eq!(count(&db, "parker"), 1);
    assert_eq!(count(&db, "PARKER"), 1);
    assert_eq!(count(&db, "parker misery"), 0); // AND semantics
}

#[test]
fn free_text_japanese() {
    let (_d, db) = test_db();
    assert_eq!(count(&db, "東方"), 1);
    assert_eq!(count(&db, "ネイティブ"), 1);
}

#[test]
fn tag_contains_and_exact() {
    let (_d, db) = test_db();
    assert_eq!(count(&db, "artist=charlie"), 1);
    assert_eq!(count(&db, "artist=CHARLIE"), 1);
    assert_eq!(count(&db, "artist=\"Charlie Parker\""), 1);
    assert_eq!(count(&db, "artist=\"charlie parker\""), 1); // exact, case-folded
    assert_eq!(count(&db, "artist=\"charlie parker\"c"), 0); // case-sensitive
    assert_eq!(count(&db, "artist=\"Charlie\""), 0); // exact ≠ substring
}

#[test]
fn negation() {
    let (_d, db) = test_db();
    assert_eq!(count(&db, "genre!=jazz"), 3);
    assert_eq!(count(&db, "!genre=jazz"), 3);
    assert_eq!(count(&db, "!parker"), 3);
}

#[test]
fn boolean_ops() {
    let (_d, db) = test_db();
    assert_eq!(count(&db, "|(parker, misery)"), 2);
    assert_eq!(count(&db, "&(genre=rock, artist=maroon)"), 1);
    assert_eq!(count(&db, "genre=|(jazz, soundtrack)"), 2);
}

#[test]
fn numeric_and_time() {
    let (_d, db) = test_db();
    assert_eq!(count(&db, "#(year >= 2000)"), 2);
    assert_eq!(count(&db, "#(1980 <= year < 2000)"), 1);
    assert_eq!(count(&db, "#(length > 3:00)"), 4); // shortest is 3:04
    assert_eq!(count(&db, "#(length >= 5:00)"), 1);
    assert_eq!(count(&db, "#(length < 3:10)"), 1);
    assert_eq!(count(&db, "#(track = 1)"), 2);
}

#[test]
fn regex_match() {
    let (_d, db) = test_db();
    assert_eq!(count(&db, "album=/essential/"), 1); // default case-insensitive
    assert_eq!(count(&db, "album=/essential/c"), 0);
    assert_eq!(count(&db, "title=/^bird/"), 1);
}

#[test]
fn sort_tags_are_searchable() {
    let (_d, db) = test_db();
    assert_eq!(count(&db, "titlesort=native"), 1); // explicit tag search
    assert_eq!(count(&db, "native face"), 1); // free text hits the blob
    assert_eq!(count(&db, "ネイティブ"), 1); // original title still matches
}

#[test]
fn unknown_tag_matches_nothing() {
    let (_d, db) = test_db();
    assert_eq!(count(&db, "bogus=parker"), 0);
}

#[test]
fn validate_flags_bad_queries() {
    assert!(search::validate("artist=\"unterminated").is_some());
    assert!(search::validate("#(year <)").is_some());
    assert!(search::validate("&(a, b").is_some());
    assert!(search::validate("artist=parker").is_none());
    assert!(search::validate("").is_none());
}
