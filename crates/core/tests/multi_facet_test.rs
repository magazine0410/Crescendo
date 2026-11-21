//! Selections in several left-panel categories combine: a track must match
//! every category that has a selection, and matches one category by any of its
//! selected values. Counts, pages, indexes, album runs, albums, playback and
//! file drags all read the same combined set.

use crescendo_core::db::queries::{FacetFilter, FacetPlan};
use crescendo_core::db::Db;
use crescendo_core::model::{BrowseMode, FacetSelection, PlayContext};

/// (id, artist, album, album_id, genre). Track 1 credits two artists, so the
/// artist facet's split is part of what combines.
const ROWS: [(i64, &str, &str, i64, &str); 5] = [
    (1, "A; B", "Red", 1, "House"),
    (2, "A", "Red", 1, "Trance"),
    (3, "A", "Blue", 2, "House"),
    (4, "B", "Blue", 2, "House"),
    (5, "C", "Blue", 2, "Trance"),
];

fn make_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        for (id, album) in [(1, "Red"), (2, "Blue")] {
            conn.execute(
                "INSERT INTO albums(id, key, album, track_count) VALUES (?1, ?2, ?2, 1)",
                rusqlite::params![id, album],
            )?;
        }
        let mut ins = conn.prepare(
            "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, duration_ms,
                                title, artist, album, album_id, genre, track_no)
             VALUES (?1, ?2, '/m', ?3, 0, 0, 'FLAC', 1000, ?4, ?5, ?6, ?7, ?8, ?1)",
        )?;
        for (id, artist, album, album_id, genre) in ROWS {
            ins.execute(rusqlite::params![
                id,
                format!("/m/{id}.flac"),
                format!("{id}.flac"),
                format!("Track {id}"),
                artist,
                album,
                album_id,
                genre,
            ])?;
        }
        Ok(())
    })
    .unwrap();
    (dir, db)
}

fn selection(mode: BrowseMode, values: &[&str]) -> FacetSelection {
    FacetSelection {
        mode,
        values: values.iter().map(|v| v.to_string()).collect(),
    }
}

fn context(filters: Vec<FacetSelection>) -> PlayContext {
    PlayContext::ValueTracks {
        filters,
        query: None,
        projection_revision: 0,
        sort: None,
    }
}

/// The ids playback resolves for these selections, after checking that every
/// other query of the same view agrees with them.
fn view(db: &Db, filters: Vec<FacetSelection>) -> Vec<i64> {
    let facets: Vec<FacetFilter> = filters
        .iter()
        .map(|filter| FacetFilter {
            plan: FacetPlan::exact(&filter.mode, false),
            values: &filter.values,
        })
        .collect();
    let ids = db.resolve_context_exact(&context(filters.clone())).unwrap();

    assert_eq!(
        db.track_count_with_plan(None, &facets).unwrap(),
        ids.len() as i64
    );
    let page: Vec<i64> = db
        .tracks_range_with_plan(None, &facets, false, None, 0, 100)
        .unwrap()
        .iter()
        .map(|track| track.id)
        .collect();
    assert_eq!(page, ids);
    for (index, id) in ids.iter().enumerate() {
        assert_eq!(
            db.track_index_with_plan(None, &facets, false, None, *id)
                .unwrap(),
            Some(index as i64)
        );
    }
    let groups = db
        .track_groups_with_plan(None, &facets, false, None)
        .unwrap();
    assert_eq!(
        groups.iter().map(|group| group.count).sum::<i64>(),
        ids.len() as i64
    );
    let mut album_ids: Vec<i64> = ROWS
        .iter()
        .filter(|row| ids.contains(&row.0))
        .map(|row| row.3)
        .collect();
    album_ids.sort();
    album_ids.dedup();
    let mut albums: Vec<i64> = db
        .browse_albums_with_plan(&facets, None, false)
        .unwrap()
        .iter()
        .map(|album| album.id)
        .collect();
    albums.sort();
    assert_eq!(albums, album_ids);
    let paths = db
        .resolve_context_paths_exact(&context(filters), 100)
        .unwrap();
    assert_eq!(
        paths,
        ids.iter()
            .map(|id| format!("/m/{id}.flac"))
            .collect::<Vec<_>>()
    );
    ids
}

#[test]
fn a_track_must_match_every_category() {
    let (_d, db) = make_db();
    assert_eq!(
        view(
            &db,
            vec![
                selection(BrowseMode::Artist, &["A"]),
                selection(BrowseMode::Album, &["Red"]),
            ]
        ),
        [1, 2]
    );
    // Track 1 lists B beside A, so the split artist facet still finds it.
    assert_eq!(
        view(
            &db,
            vec![
                selection(BrowseMode::Artist, &["B"]),
                selection(BrowseMode::Album, &["Red"]),
            ]
        ),
        [1]
    );
    assert_eq!(
        view(
            &db,
            vec![
                selection(BrowseMode::Genre, &["House"]),
                selection(BrowseMode::Artist, &["A"]),
                selection(BrowseMode::Album, &["Blue"]),
            ]
        ),
        [3]
    );
}

#[test]
fn values_in_one_category_are_alternatives() {
    let (_d, db) = make_db();
    assert_eq!(
        view(
            &db,
            vec![
                selection(BrowseMode::Artist, &["B", "C"]),
                selection(BrowseMode::Album, &["Blue"]),
            ]
        ),
        [4, 5]
    );
}

#[test]
fn selections_that_share_no_track_match_nothing() {
    let (_d, db) = make_db();
    assert!(view(
        &db,
        vec![
            selection(BrowseMode::Artist, &["C"]),
            selection(BrowseMode::Album, &["Red"]),
        ]
    )
    .is_empty());
    // A custom tag that names no column matches nothing, whatever else is
    // selected beside it.
    assert!(view(
        &db,
        vec![
            selection(BrowseMode::Artist, &["A"]),
            selection(BrowseMode::Custom("nosuchtag".into()), &["x"]),
        ]
    )
    .is_empty());
}

/// The frontend's descriptor (`PlayContext` in api.ts), as it arrives over IPC.
#[test]
fn the_frontend_descriptor_names_each_category() {
    let (_d, db) = make_db();
    let ctx: PlayContext = serde_json::from_value(serde_json::json!({
        "type": "value_tracks",
        "filters": [
            { "mode": "artist", "values": ["A"] },
            { "mode": { "custom": "genre" }, "values": ["House"] },
        ],
        "query": null,
        "projection_revision": 0,
        "sort": null,
    }))
    .unwrap();
    assert_eq!(db.resolve_context_exact(&ctx).unwrap(), [3, 1]);
}

#[test]
fn no_selection_is_the_whole_library() {
    let (_d, db) = make_db();
    assert_eq!(view(&db, vec![]), [3, 4, 5, 1, 2]);
}
