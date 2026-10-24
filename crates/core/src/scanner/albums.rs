use anyhow::Result;
use rusqlite::Connection;

/// Album identity: normalized (albumartist-or-artist, album) pair.
/// char(31) is an unambiguous separator (US control char).
const KEY_EXPR: &str =
    "ulower(COALESCE(albumartist, artist, '')) || char(31) || ulower(COALESCE(album, ''))";

/// Rebuild the albums table from tracks in one aggregate pass. Upserting on
/// `key` preserves album ids (thumbnail cache and cover_source stay valid).
pub fn rebuild(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    rebuild_in_transaction(&tx)?;
    tx.commit()?;
    Ok(())
}

/// Reuse a caller's transaction when track edits and album membership must
/// become visible together.
pub(crate) fn rebuild_in_transaction(conn: &Connection) -> Result<()> {
    conn.execute_batch(&format!(
        r#"
        INSERT INTO albums(key, album, albumartist, albumartist_sort, year, genre,
                           track_count, total_duration_ms, dir)
        SELECT {KEY_EXPR} AS key,
               MAX(album),
               MAX(COALESCE(albumartist, artist)),
               -- The sort tag of whichever name the album is shown under:
               -- an albumartist with no sort tag must not borrow the
               -- track artist's.
               MAX(CASE WHEN albumartist IS NOT NULL THEN albumartistsort
                        ELSE artistsort END),
               MAX(year),
               MAX(genre),
               COUNT(*),
               COALESCE(SUM(duration_ms), 0),
               MIN(dir)
        FROM tracks WHERE missing = 0
        GROUP BY 1
        ON CONFLICT(key) DO UPDATE SET
            album = excluded.album,
            albumartist = excluded.albumartist,
            albumartist_sort = excluded.albumartist_sort,
            year = excluded.year,
            genre = excluded.genre,
            track_count = excluded.track_count,
            total_duration_ms = excluded.total_duration_ms,
            dir = excluded.dir;

        DELETE FROM albums WHERE key NOT IN
            (SELECT DISTINCT {KEY_EXPR} FROM tracks WHERE missing = 0);

        -- Missing tracks keep their album only while it exists: a deleted
        -- album's id can be given to a new album later, and playlists would
        -- then show the missing tracks under that album's cover.
        UPDATE tracks SET album_id = NULL
        WHERE missing = 1 AND album_id IS NOT NULL
          AND album_id NOT IN (SELECT id FROM albums);

        UPDATE tracks SET album_id =
            (SELECT a.id FROM albums a WHERE a.key =
                ulower(COALESCE(tracks.albumartist, tracks.artist, '')) || char(31) ||
                ulower(COALESCE(tracks.album, '')))
        WHERE missing = 0;
        "#
    ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::rebuild;
    use crate::db::Db;

    #[test]
    fn unicode_case_variants_share_one_stable_album() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("albums.db")).unwrap();
        db.with(|conn| {
            for (id, artist, album) in [(1, "Écho", "Lumière"), (2, "écho", "lumière")] {
                conn.execute(
                    "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, \
                     artist, album, duration_ms) VALUES (?1, ?2, '/music', ?3, 0, 1, \
                     'FLAC', ?4, ?5, 1000)",
                    rusqlite::params![
                        id,
                        format!("/music/{id}.flac"),
                        format!("{id}.flac"),
                        artist,
                        album
                    ],
                )?;
            }
            rebuild(conn)?;
            let count: i64 = conn.query_row("SELECT COUNT(*) FROM albums", [], |r| r.get(0))?;
            let ids: i64 =
                conn.query_row("SELECT COUNT(DISTINCT album_id) FROM tracks", [], |r| {
                    r.get(0)
                })?;
            assert_eq!(count, 1);
            assert_eq!(ids, 1);

            let first_id: i64 = conn.query_row("SELECT id FROM albums", [], |r| r.get(0))?;
            conn.execute("UPDATE tracks SET artist = 'ÉCHO', album = 'LUMIÈRE'", [])?;
            rebuild(conn)?;
            let second_id: i64 = conn.query_row("SELECT id FROM albums", [], |r| r.get(0))?;
            assert_eq!(second_id, first_id);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn album_artist_sort_comes_from_the_album_artist_and_missing_albums_are_released() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("albums.db")).unwrap();
        db.with(|conn| {
            conn.execute(
                "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, artist, \
                 artistsort, albumartist, album) VALUES (1, '/m/1.flac', '/m', '1.flac', 0, 1, \
                 'FLAC', '上海アリス幻樂団', 'ZUN', '東方', 'Album')",
                [],
            )?;
            rebuild(conn)?;
            let sort: Option<String> =
                conn.query_row("SELECT albumartist_sort FROM albums", [], |r| r.get(0))?;
            assert_eq!(sort, None, "the album artist has no sort tag of its own");

            conn.execute("UPDATE tracks SET missing = 1", [])?;
            rebuild(conn)?;
            let album_id: Option<i64> =
                conn.query_row("SELECT album_id FROM tracks", [], |r| r.get(0))?;
            assert_eq!(album_id, None, "a deleted album's id may be reused");
            Ok(())
        })
        .unwrap();
    }
}
