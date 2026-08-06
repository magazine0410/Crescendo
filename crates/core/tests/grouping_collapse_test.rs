use crescendo_core::db::Db;
use crescendo_core::model::BrowseMode;
use std::collections::HashMap;

fn grouping() -> BrowseMode {
    BrowseMode::Custom("grouping".into())
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
            "INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms, grouping)
             VALUES (?1, ?2, ?3, 0, 0, 'FLAC', 180000, ?4)",
        )?;
        for (i, g) in groupings.iter().enumerate() {
            ins.execute(rusqlite::params![
                format!("/m/{i}.flac"),
                "/m",
                format!("{i}.flac"),
                *g,
            ])?;
        }
        Ok(())
    })
    .unwrap();
    (dir, db)
}

fn value_counts(vals: &[(String, i64, String)]) -> HashMap<String, i64> {
    vals.iter().map(|(v, n, _)| (v.clone(), *n)).collect()
}

#[test]
fn browse_values_collapses_multi_theme_to_primary() {
    let (_d, db) = make_db();

    // Collapsed: the three `"Old Adam" Bar…` variants fold into one umbrella
    // whose count sums them; Other and the NULL ("") bucket stand alone.
    let collapsed = db.browse_values(grouping(), None, true).unwrap();
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
    let full = db.browse_values(grouping(), None, false).unwrap();
    assert_eq!(full.len(), 5);
    let c = value_counts(&full);
    assert_eq!(c.get("\"Old Adam\" Bar"), Some(&1));
    assert_eq!(c.get("\"Old Adam\" Bar; A Drunkard's Lemuria"), Some(&1));
}

#[test]
fn selecting_umbrella_matches_every_variant() {
    let (_d, db) = make_db();
    let umbrella = "\"Old Adam\" Bar";

    // Collapsed selection matches all three variants; uncollapsed matches only
    // the byte-exact tag.
    assert_eq!(
        db.track_count(None, grouping(), &[umbrella.into()], true).unwrap(),
        3
    );
    assert_eq!(
        db.track_count(None, grouping(), &[umbrella.into()], false).unwrap(),
        1
    );

    // tracks_range (the windowed list + play-queue order) agrees with the count.
    let rows = db
        .tracks_range(None, grouping(), &[umbrella.into()], true, None, 0, 100)
        .unwrap();
    assert_eq!(rows.len(), 3);
}
