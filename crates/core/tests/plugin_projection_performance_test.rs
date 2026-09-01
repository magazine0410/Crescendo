use crescendo_core::db::queries::{FacetPlan, FacetTransform};
use crescendo_core::db::Db;
use crescendo_core::model::{BrowseMode, PlayContext};
use std::time::{Duration, Instant};

const TRACKS: i64 = 150_000;

fn timed<T>(operation: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let result = operation();
    (result, started.elapsed())
}

/// Run explicitly for the Stage 4 performance gate:
///
/// `cargo test --release -p crescendo-core --test plugin_projection_performance_test -- --ignored --nocapture`
///
/// This uses the real SQLite query path at the target library size. The
/// transformed expression stays inside SQL; only bounded view rows leave the
/// database API. The generous ratio/fixed allowance catches accidental
/// per-track host processing without turning ordinary machine variance into a
/// flaky default test.
#[test]
#[ignore = "150k-track Stage 4 performance audit"]
fn transformed_grouping_projection_stays_comparable_at_target_library_size() {
    let directory = tempfile::tempdir().unwrap();
    let db = Db::open(&directory.path().join("projection.db")).unwrap();
    db.with(|connection| {
        connection.execute_batch("BEGIN IMMEDIATE")?;
        {
            let mut insert = connection.prepare(
                "INSERT INTO tracks(
                     id, path, dir, filename, mtime, size, format, duration_ms,
                     grouping, title, artist, albumartist, album, album_id,
                     disc_no, track_no
                 ) VALUES (
                     ?1, ?2, '/music', ?3, 0, 0, 'FLAC', 180000,
                     ?4, ?3, ?5, ?5, ?6, ?7, 1, ?8
                 )",
            )?;
            for id in 1..=TRACKS {
                let primary = id % 200;
                let variant = id % 5;
                let album_id = id % 100 + 1;
                insert.execute(rusqlite::params![
                    id,
                    format!("/music/{id}.flac"),
                    format!("{id}.flac"),
                    format!("Grouping {primary}; Variant {variant}"),
                    format!("Artist {}", id % 50),
                    format!("Album {album_id}"),
                    album_id,
                    id % 20 + 1,
                ])?;
            }
        }
        {
            let mut insert = connection.prepare(
                "INSERT INTO albums(
                     id, key, album, albumartist, track_count, total_duration_ms
                 ) VALUES (?1, ?2, ?3, 'Various', ?4, ?5)",
            )?;
            for id in 1..=100i64 {
                insert.execute(rusqlite::params![
                    id,
                    format!("album-{id}"),
                    format!("Album {id}"),
                    TRACKS / 100,
                    TRACKS / 100 * 180_000,
                ])?;
            }
        }
        connection.execute_batch("COMMIT")?;
        Ok(())
    })
    .unwrap();

    let mode = BrowseMode::Custom("grouping".into());
    let exact = FacetPlan::exact(&mode, true).unwrap();
    let transformed = FacetPlan::resolve(
        &mode,
        Some(&FacetTransform::TextFirstSegment {
            delimiter: ";".into(),
            trim_whitespace: true,
        }),
        true,
    )
    .unwrap();

    // Warm the SQLite page cache before comparing the two forms.
    db.browse_values_with_plan(Some(&exact), None).unwrap();
    db.browse_values_with_plan(Some(&transformed), None)
        .unwrap();
    let (exact_values, exact_time) =
        timed(|| db.browse_values_with_plan(Some(&exact), None).unwrap());
    let (transformed_values, transformed_time) = timed(|| {
        db.browse_values_with_plan(Some(&transformed), None)
            .unwrap()
    });
    eprintln!("150k browse_values: exact={exact_time:?}, transformed={transformed_time:?}");
    assert_eq!(
        exact_values.iter().map(|value| value.count).sum::<i64>(),
        TRACKS
    );
    assert_eq!(
        transformed_values
            .iter()
            .map(|value| value.count)
            .sum::<i64>(),
        TRACKS
    );
    assert!(transformed_time < Duration::from_secs(10));
    assert!(transformed_time <= exact_time.saturating_mul(8) + Duration::from_millis(500));

    let selection = vec!["Grouping 17".to_string()];
    let count = db
        .track_count_with_plan(None, Some(&transformed), &selection)
        .unwrap();
    let page = db
        .tracks_range_with_plan(None, Some(&transformed), &selection, true, None, 0, 200)
        .unwrap();
    let albums = db
        .browse_albums_with_plan(Some(&transformed), &selection, None, true)
        .unwrap();
    let groups = db
        .track_groups_with_plan(None, Some(&transformed), &selection, true, None)
        .unwrap();
    assert_eq!(count, TRACKS / 200);
    assert_eq!(page.len(), 200);
    assert!(!albums.is_empty());
    assert_eq!(groups.iter().map(|group| group.count).sum::<i64>(), count);

    let context = PlayContext::ValueTracks {
        mode,
        values: selection,
        query: None,
        projection_revision: 7,
        sort: None,
    };
    let ids = db
        .resolve_context_with_projection(&context, Some(&transformed), true)
        .unwrap();
    assert_eq!(ids.len() as i64, count);
    assert_eq!(ids[0], page[0].id);
}
