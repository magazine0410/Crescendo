use anyhow::Result;
use rusqlite::Connection;

/// Album identity: normalized (albumartist-or-artist, album) pair.
/// char(31) is an unambiguous separator (US control char).
const KEY_EXPR: &str =
    "lower(COALESCE(albumartist, artist, '')) || char(31) || lower(COALESCE(album, ''))";

/// Rebuild the albums table from tracks in one aggregate pass. Upserting on
/// `key` preserves album ids (thumbnail cache and cover_source stay valid).
pub fn rebuild(conn: &Connection) -> Result<()> {
    conn.execute_batch(&format!(
        r#"
        BEGIN;
        INSERT INTO albums(key, album, albumartist, albumartist_sort, year, genre,
                           track_count, total_duration_ms, dir)
        SELECT {KEY_EXPR} AS key,
               MAX(album),
               MAX(COALESCE(albumartist, artist)),
               MAX(COALESCE(albumartistsort, artistsort)),
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

        UPDATE tracks SET album_id =
            (SELECT a.id FROM albums a WHERE a.key =
                lower(COALESCE(tracks.albumartist, tracks.artist, '')) || char(31) ||
                lower(COALESCE(tracks.album, '')))
        WHERE missing = 0;
        COMMIT;
        "#
    ))?;
    Ok(())
}
