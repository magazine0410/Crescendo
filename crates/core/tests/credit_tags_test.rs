//! The credit / release tags added in schema v7: composer, arranger, lyricist,
//! vocalist, catalognumber, event and bpm. Doujin releases tag these heavily
//! and in Japanese, so every string case is checked against non-ASCII too.

use crescendo_core::db::Db;
use crescendo_core::model::BrowseMode;
use crescendo_core::scanner::tags::{search_blob, TrackMeta};
use crescendo_core::search;

type Row<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str, &'a str, &'a str, &'a str, i64);

fn test_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        let mut ins = conn.prepare(
            "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms,
                                title, artist, composer, arranger, lyricist, vocalist,
                                catalognumber, event, bpm, search_blob)
             VALUES (?1, ?2, ?3, 0, 0, 'FLAC', 1000, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )?;
        // title, artist, composer, arranger, lyricist, vocalist, catalog, event, bpm
        let rows: Vec<Row> = vec![
            ("LAST DESTINY", "Stack", "暁Records", "暁Records", "暁Records", "Stack",
             "DRCD-0002", "C83", 82),
            ("月齢11.3のキャンドルマジック", "NJK Record", "NJK Record", "ShibayanRecords",
             "やまざきさやか", "NJK Record", "NJK-007", "C91", 85),
            ("Bad Apple!!", "nomico", "ZUN", "Masayoshi Minoshima", "Haruka",
             "nomico", "ARCD0002", "C75", 138),
        ];
        for (i, r) in rows.iter().enumerate() {
            let (title, artist, composer, arranger, lyricist, vocalist, catalog, event, bpm) = *r;
            let meta = TrackMeta {
                title: Some(title.into()),
                artist: Some(artist.into()),
                composer: Some(composer.into()),
                arranger: Some(arranger.into()),
                lyricist: Some(lyricist.into()),
                vocalist: Some(vocalist.into()),
                catalognumber: Some(catalog.into()),
                event: Some(event.into()),
                ..Default::default()
            };
            ins.execute(rusqlite::params![
                format!("/m/{i}.flac"),
                "/m",
                format!("{i}.flac"),
                title,
                artist,
                composer,
                arranger,
                lyricist,
                vocalist,
                catalog,
                event,
                bpm,
                search_blob(&meta),
            ])?;
        }
        Ok(())
    })
    .unwrap();
    (dir, db)
}

fn count(db: &Db, q: &str) -> i64 {
    let filter = search::compile(q).unwrap();
    db.track_count(Some(&filter), BrowseMode::Artist, &[], false).unwrap()
}

#[test]
fn each_credit_tag_is_searchable() {
    let (_d, db) = test_db();
    assert_eq!(count(&db, "arranger=Shibayan"), 1);
    assert_eq!(count(&db, "composer=ZUN"), 1);
    assert_eq!(count(&db, "vocalist=nomico"), 1);
    assert_eq!(count(&db, "lyricist=Haruka"), 1);
    assert_eq!(count(&db, "event=C83"), 1);
    assert_eq!(count(&db, "catalognumber=NJK-007"), 1);
    // `catalog` is the short alias for the same column.
    assert_eq!(count(&db, "catalog=ARCD"), 1);
}

#[test]
fn credit_tags_match_non_ascii_case_insensitively() {
    let (_d, db) = test_db();
    // The Rust UDFs do the matching, so CJK substrings work where SQLite's
    // ASCII-only LIKE would not.
    assert_eq!(count(&db, "arranger=暁"), 1);
    assert_eq!(count(&db, "lyricist=やまざき"), 1);
    assert_eq!(count(&db, "composer=records"), 1); // vs. "NJK Record"
    assert_eq!(count(&db, "arranger=SHIBAYANRECORDS"), 1);
}

#[test]
fn bpm_compares_as_a_number() {
    let (_d, db) = test_db();
    assert_eq!(count(&db, "bpm>100"), 1);
    assert_eq!(count(&db, "bpm>=82"), 3);
    assert_eq!(count(&db, "bpm<85"), 1);
    assert_eq!(count(&db, "bpm=138"), 1);
}

#[test]
fn credits_reach_free_text_search() {
    let (_d, db) = test_db();
    assert_eq!(count(&db, "minoshima"), 1);
    assert_eq!(count(&db, "やまざきさやか"), 1);
    assert_eq!(count(&db, "drcd-0002"), 1);
    assert_eq!(count(&db, "c91"), 1);
}

#[test]
fn credit_tags_are_browsable() {
    let (_d, db) = test_db();
    let mode = BrowseMode::Custom("arranger".into());
    let values: Vec<String> =
        db.browse_values(mode.clone(), None, false).unwrap().into_iter().map(|v| v.0).collect();
    assert_eq!(values, ["Masayoshi Minoshima", "ShibayanRecords", "暁Records"]);
    // …and selecting one plays back the right count.
    assert_eq!(db.track_count(None, mode, &["暁Records".into()], false).unwrap(), 1);
}

#[test]
fn search_blob_stores_each_value_once() {
    // Doujin credits repeat the circle name across artist/composer/vocalist;
    // the blob is scanned in full per free-text query, so it must not grow
    // linearly with the repeats.
    let meta = TrackMeta {
        artist: Some("NJK Record".into()),
        composer: Some("NJK Record".into()),
        vocalist: Some("njk record".into()),
        arranger: Some("ShibayanRecords".into()),
        ..Default::default()
    };
    let blob = search_blob(&meta);
    assert_eq!(blob.matches("njk record").count(), 1);
    assert!(blob.contains("shibayanrecords"));
}

