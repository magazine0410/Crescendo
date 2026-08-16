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
    db.track_count(Some(&filter), crescendo_core::model::BrowseMode::Artist, &[], false).unwrap()
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
    assert_eq!(count(&db, "artist=\"charlie parker\""), 1); // substring, case-folded
    assert_eq!(count(&db, "artist=\"Charlie Parker\"c"), 1); // case-sensitive, exact case
    assert_eq!(count(&db, "artist=\"charlie parker\"c"), 0); // case-sensitive miss
    assert_eq!(count(&db, "artist=\"Charlie\""), 1); // quoted value is a substring match
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
fn bare_comparison_without_the_hash_wrapper() {
    let (_d, db) = test_db();
    assert_eq!(count(&db, "year>=2000"), 2);
    assert_eq!(count(&db, "year >= 2000"), 2);
    assert_eq!(count(&db, "1980<=year<2000"), 1);
    assert_eq!(count(&db, "track=1"), 2); // numeric tag never matches as text
    assert_eq!(count(&db, "year!=1950"), 3);
    // '<' and '>' after anything else are ordinary free-text characters.
    assert_eq!(count(&db, "bird>man"), 0);
    assert_eq!(search::validate("bird>man"), None);
}

#[test]
fn play_counts() {
    let (_d, db) = test_db();
    let id_of = |title: &str| -> i64 {
        db.with(|c| Ok(c.query_row("SELECT id FROM tracks WHERE title = ?1", [title], |r| r.get(0))?))
            .unwrap()
    };
    // 3 plays, 2 plays, 1 play, and one track that was never played.
    for _ in 0..3 {
        db.record_play(id_of("Now's The Time")).unwrap();
    }
    for _ in 0..2 {
        db.record_play(id_of("Bird Man")).unwrap();
    }
    db.record_play(id_of("Misery")).unwrap();

    assert_eq!(count(&db, "plays=3"), 1);
    assert_eq!(count(&db, "plays>2"), 1);
    assert_eq!(count(&db, "plays>=2"), 2);
    assert_eq!(count(&db, "playcount>2"), 1); // alias
    assert_eq!(count(&db, "#(plays > 2)"), 1); // the #( ) form still works

    // "Under N" holds back the never-played tracks — they are not low counts,
    // they are no count at all.
    assert_eq!(count(&db, "plays<3"), 2);
    assert_eq!(count(&db, "plays<=2"), 2);
    assert_eq!(count(&db, "plays!=3"), 2);
    assert_eq!(count(&db, "1<=plays<3"), 2);

    // Naming 0 asks for them on purpose.
    assert_eq!(count(&db, "plays=0"), 1);
    assert_eq!(count(&db, "plays<1"), 1);
    assert_eq!(count(&db, "!plays>0"), 1);
    assert_eq!(count(&db, "plays>0"), 3);

    // Combines with the rest of the syntax.
    assert_eq!(count(&db, "&(plays>0, genre=jazz)"), 1);
    assert_eq!(count(&db, "plays>1 artist=parker"), 1);
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
fn track_index_locates_row_in_view() {
    use crescendo_core::model::BrowseMode;
    let (_d, db) = test_db();
    let id_of = |title: &str| -> i64 {
        db.with(|c| Ok(c.query_row("SELECT id FROM tracks WHERE title = ?1", [title], |r| r.get(0))?))
            .unwrap()
    };
    let index_of = |title: &str| {
        db.track_index(None, BrowseMode::Artist, &[], false, None, id_of(title)).unwrap()
    };
    // Full library, LIBRARY_SORT by albumartist: Charlie Parker, Joe Hisaishi,
    // Maroon 5, ZUN.
    assert_eq!(index_of("Now's The Time"), Some(0));
    assert_eq!(index_of("Bird Man"), Some(1));
    assert_eq!(index_of("Misery"), Some(2));
    assert_eq!(index_of("ネイティブフェイス"), Some(3));

    // A track outside the filtered view has no index.
    let filter = search::compile("artist=zun").unwrap();
    let charlie = id_of("Now's The Time");
    let zun = id_of("ネイティブフェイス");
    assert_eq!(db.track_index(Some(&filter), BrowseMode::Artist, &[], false, None, charlie).unwrap(), None);
    assert_eq!(db.track_index(Some(&filter), BrowseMode::Artist, &[], false, None, zun).unwrap(), Some(0));
}

#[test]
fn classify_tints_queries() {
    use search::QueryKind;
    assert_eq!(search::classify(""), QueryKind::Plain);
    assert_eq!(search::classify("nausicaa wind"), QueryKind::Plain); // free text only
    assert_eq!(search::classify("\"exact phrase\""), QueryKind::Plain); // quoted free text
    assert_eq!(search::classify("#(year >= 2010)"), QueryKind::Syntax);
    assert_eq!(search::classify("artist=parker"), QueryKind::Syntax);
    assert_eq!(search::classify("!ambient"), QueryKind::Syntax);
    assert_eq!(search::classify("|(a, b)"), QueryKind::Syntax);
    assert_eq!(search::classify("plays>2"), QueryKind::Syntax);
    assert_eq!(search::classify("bird>man"), QueryKind::Plain); // free text, not a comparison
    assert_eq!(search::classify("#(year <)"), QueryKind::Invalid);
    assert_eq!(search::classify("plays>"), QueryKind::Invalid);
    assert_eq!(search::classify("artist=\"oops"), QueryKind::Invalid);
}

#[test]
fn validate_flags_bad_queries() {
    assert!(search::validate("artist=\"unterminated").is_some());
    assert!(search::validate("#(year <)").is_some());
    assert!(search::validate("&(a, b").is_some());
    assert!(search::validate("artist=parker").is_none());
    assert!(search::validate("").is_none());
}
