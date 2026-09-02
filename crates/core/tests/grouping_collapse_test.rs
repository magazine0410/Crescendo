use crescendo_core::db::queries::{FacetPlan, FacetTransform};
use crescendo_core::db::Db;
use crescendo_core::model::{BrowseMode, BrowseValue, PlayContext};
use std::collections::HashMap;

fn grouping() -> BrowseMode {
    BrowseMode::Custom("grouping".into())
}

fn collapsed_plan() -> FacetPlan {
    FacetPlan::resolve(
        &grouping(),
        Some(&FacetTransform::TextFirstSegment {
            delimiter: ";".into(),
            trim_whitespace: true,
        }),
        true,
    )
    .unwrap()
}

/// A tiny library keyed on `grouping` tags: three variants sharing the primary
/// theme `"Old Adam" Bar` (one plain, two semicolon-joined — one with a space
/// before the `;` to exercise the TRIM), plus `Other` and an untagged (NULL)
/// track.
fn make_db() -> (tempfile::TempDir, Db) {
    let groupings: [Option<&str>; 5] = [
        Some("\"Old Adam\" Bar"),
        Some("\"Old Adam\" Bar; A Drunkard's Lemuria"),
        Some("\"Old Adam\" Bar ; Hangover of Bedfellows Dreaming Differently"),
        Some("Other"),
        None,
    ];
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("t.db")).unwrap();
    db.with(|conn| {
        let mut ins = conn.prepare(
            "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms, grouping,
                                title, artist, albumartist, album, album_id, disc_no, track_no)
             VALUES (?1, ?2, ?3, 0, 0, 'FLAC', 180000, ?4,
                     ?3, 'Artist', 'Artist', 'Album', 1, 1, ?5)",
        )?;
        for (i, g) in groupings.iter().enumerate() {
            ins.execute(rusqlite::params![
                format!("/m/{i}.flac"),
                "/m",
                format!("{i}.flac"),
                *g,
                i as i64 + 1,
            ])?;
        }
        conn.execute(
            "INSERT INTO albums(id, key, album, albumartist, track_count)
             VALUES (1, 'album', 'Album', 'Artist', 5)",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    (dir, db)
}

fn value_counts(vals: &[BrowseValue]) -> HashMap<String, i64> {
    vals.iter().map(|v| (v.value.clone(), v.count)).collect()
}

#[test]
fn browse_values_collapses_multi_theme_to_primary() {
    let (_d, db) = make_db();

    // Collapsed: the three `"Old Adam" Bar…` variants fold into one umbrella
    // whose count sums them; Other and the NULL ("") bucket stand alone.
    let plan = collapsed_plan();
    let collapsed = db.browse_values_with_plan(Some(&plan), None).unwrap();
    assert_eq!(collapsed.len(), 3);
    let c = value_counts(&collapsed);
    assert_eq!(c.get("\"Old Adam\" Bar"), Some(&3));
    assert_eq!(c.get("Other"), Some(&1));
    assert_eq!(c.get(""), Some(&1));
}

#[test]
fn browse_values_uncollapsed_keeps_full_strings() {
    let (_d, db) = make_db();

    // Uncollapsed: each distinct full tag is its own row (today's behavior).
    let full = db.browse_values_exact(grouping(), None).unwrap();
    assert_eq!(full.len(), 5);
    let c = value_counts(&full);
    assert_eq!(c.get("\"Old Adam\" Bar"), Some(&1));
    assert_eq!(c.get("\"Old Adam\" Bar; A Drunkard's Lemuria"), Some(&1));
}

#[test]
fn selecting_umbrella_matches_every_variant() {
    let (_d, db) = make_db();
    let umbrella = "\"Old Adam\" Bar";
    let plan = collapsed_plan();

    // Collapsed selection matches all three variants; uncollapsed matches only
    // the byte-exact tag.
    assert_eq!(
        db.track_count_with_plan(None, Some(&plan), &[umbrella.into()])
            .unwrap(),
        3
    );
    assert_eq!(
        db.track_count_exact(None, grouping(), &[umbrella.into()])
            .unwrap(),
        1
    );

    // tracks_range (the windowed list + play-queue order) agrees with the count.
    let rows = db
        .tracks_range_with_plan(None, Some(&plan), &[umbrella.into()], true, None, 0, 100)
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(
        db.browse_albums_with_plan(Some(&plan), &[umbrella.into()], None, true)
            .unwrap()
            .len(),
        1
    );

    // Indexes, album runs, and playback resolution use the same FacetPlan as
    // the value list and windowed rows.
    let ids: Vec<_> = rows.iter().map(|track| track.id).collect();
    for (expected, id) in ids.iter().enumerate() {
        assert_eq!(
            db.track_index_with_plan(None, Some(&plan), &[umbrella.into()], true, None, *id,)
                .unwrap(),
            Some(expected as i64)
        );
    }
    let groups = db
        .track_groups_with_plan(None, Some(&plan), &[umbrella.into()], true, None)
        .unwrap();
    assert_eq!(groups.iter().map(|group| group.count).sum::<i64>(), 3);
    let context = PlayContext::ValueTracks {
        mode: grouping(),
        values: vec![umbrella.into()],
        query: None,
        projection_revision: 0,
        sort: None,
    };
    assert_eq!(
        db.resolve_context_with_projection(&context, Some(&plan), true)
            .unwrap(),
        ids
    );
}

#[test]
fn first_segment_composes_with_the_host_romanized_artist_projection() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("artists.db")).unwrap();
    db.with(|conn| {
        for (id, artist, artistsort) in [
            (1, "テーマ甲", "Theme; First"),
            (2, "テーマ乙", "Theme; Second"),
        ] {
            conn.execute(
                "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, duration_ms, artist, artistsort)
                 VALUES (?1, ?2, '/m', ?3, 0, 0, 'FLAC', 1, ?4, ?5)",
                rusqlite::params![
                    id,
                    format!("/m/{id}.flac"),
                    format!("{id}.flac"),
                    artist,
                    artistsort,
                ],
            )?;
        }
        Ok(())
    })
    .unwrap();

    let transform = FacetTransform::TextFirstSegment {
        delimiter: ";".into(),
        trim_whitespace: true,
    };
    let plan = FacetPlan::resolve(&BrowseMode::Artist, Some(&transform), true).unwrap();
    assert_eq!(
        db.browse_values_with_plan(Some(&plan), None).unwrap(),
        vec![BrowseValue {
            value: "Theme".into(),
            count: 2,
            sort_value: None,
        }]
    );
    assert_eq!(
        db.track_count_with_plan(None, Some(&plan), &["Theme".into()])
            .unwrap(),
        2
    );
}
