//! Cover-art resolution and thumbnail cache.
//!
//! No image blobs in the database: per album we remember only a
//! `cover_source` string ("folder:<path>" or "embedded:<track_id>"), resolve
//! it lazily the first time art is requested, and cache a 256px JPEG in the
//! thumbnail directory keyed by album id.

use crate::db::Db;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

pub const THUMB_SIZE: u32 = 256;

const FOLDER_NAMES: &[&str] = &[
    "folder.jpg",
    "cover.jpg",
    "front.jpg",
    "folder.png",
    "cover.png",
    "front.png",
    "Folder.jpg",
    "Cover.jpg",
    "Front.jpg",
];

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub struct ArtStore {
    thumb_dir: PathBuf,
}

pub enum ArtData {
    /// Serve this file directly from disk.
    File(PathBuf),
    /// Raw image bytes (embedded art).
    Bytes(Vec<u8>),
}

impl ArtStore {
    pub fn new(cache_dir: &Path) -> Result<Self> {
        let thumb_dir = cache_dir.join("thumbs");
        std::fs::create_dir_all(&thumb_dir)?;
        Ok(Self { thumb_dir })
    }

    pub fn thumb_path(&self, album_id: i64) -> PathBuf {
        self.thumb_dir.join(format!("{album_id}.jpg"))
    }

    fn thumb_source_path(&self, album_id: i64) -> PathBuf {
        self.thumb_dir.join(format!("{album_id}.jpg.source"))
    }

    /// Thumbnail for an album; generates and caches on first request.
    pub fn album_thumb(&self, db: &Db, album_id: i64) -> Result<PathBuf> {
        let cached = self.thumb_path(album_id);
        let signature = self.thumb_signature(db, album_id)?;
        if cached.exists()
            && signature.as_deref().is_some_and(|expected| {
                std::fs::read_to_string(self.thumb_source_path(album_id))
                    .is_ok_and(|actual| actual == expected)
            })
        {
            return Ok(cached);
        }
        let full = self.album_full(db, album_id)?;
        let generated_signature = self.thumb_signature(db, album_id)?;
        let img = match &full {
            ArtData::File(p) => image::open(p)?,
            ArtData::Bytes(b) => image::load_from_memory(b)?,
        };
        let thumb = img.thumbnail(THUMB_SIZE, THUMB_SIZE);
        let rgb = image::DynamicImage::ImageRgb8(thumb.to_rgb8());
        let tmp = unique_temp_path(&cached);
        if let Err(e) = rgb.save_with_format(&tmp, image::ImageFormat::Jpeg) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        if let Err(e) = std::fs::rename(&tmp, &cached) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        // Write the source marker only after the image is complete. Legacy or
        // interrupted cache entries have no matching marker and regenerate.
        if let Some(signature) = generated_signature {
            let source_path = self.thumb_source_path(album_id);
            let source_tmp = unique_temp_path(&source_path);
            if let Err(e) = std::fs::write(&source_tmp, signature)
                .and_then(|_| std::fs::rename(&source_tmp, &source_path))
            {
                let _ = std::fs::remove_file(&source_tmp);
                return Err(e.into());
            }
        }
        Ok(cached)
    }

    /// Full-size art for an album (folder image preferred, else embedded art
    /// from one of its tracks). Resolution result is cached in
    /// `albums.cover_source`.
    pub fn album_full(&self, db: &Db, album_id: i64) -> Result<ArtData> {
        // Always re-check the album directory. This notices a newly added
        // folder.jpg even when an older embedded-art choice was cached.
        if let Some(path) = self.album_folder_image(db, album_id)? {
            let source = format!("folder:{}", path.display());
            self.remember_source(db, album_id, &source)?;
            return Ok(ArtData::File(path));
        }
        if let Some(source) = self.cover_source(db, album_id)? {
            if let Some(art) = self.load_embedded_source(db, album_id, &source) {
                return Ok(art);
            }
            // stale cache entry (file moved) — re-resolve below
        }
        let source = self.resolve_embedded(db, album_id)?;
        self.remember_source(db, album_id, &source)?;
        self.load_embedded_source(db, album_id, &source)
            .ok_or_else(|| anyhow!("no cover art for album {album_id}"))
    }

    fn load_embedded_source(&self, db: &Db, album_id: i64, source: &str) -> Option<ArtData> {
        if let Some(id) = source.strip_prefix("embedded:") {
            let track_id: i64 = id.parse().ok()?;
            let path: String = db
                .with(|conn| {
                    Ok(conn
                        .query_row(
                            "SELECT path FROM tracks WHERE id = ?1 AND album_id = ?2 \
                             AND missing = 0 AND has_embedded_art = 1",
                            rusqlite::params![track_id, album_id],
                            |r| r.get(0),
                        )
                        .ok())
                })
                .ok()
                .flatten()?;
            return embedded_art(Path::new(&path)).map(ArtData::Bytes);
        }
        None
    }

    fn remember_source(&self, db: &Db, album_id: i64, source: &str) -> Result<()> {
        db.with(|conn| {
            conn.execute(
                "UPDATE albums SET cover_source = ?2 WHERE id = ?1 \
                 AND cover_source IS NOT ?2",
                rusqlite::params![album_id, source],
            )?;
            Ok(())
        })
    }

    /// Art for a single track (embedded first, then its folder) — used when a
    /// track has no album row yet.
    pub fn track_art(&self, db: &Db, track_id: i64) -> Result<ArtData> {
        let (path, dir): (String, String) = db.with(|conn| {
            Ok(conn.query_row(
                "SELECT path, dir FROM tracks WHERE id = ?1",
                [track_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })?;
        if let Some(bytes) = embedded_art(Path::new(&path)) {
            return Ok(ArtData::Bytes(bytes));
        }
        if let Some(p) = folder_image(Path::new(&dir)) {
            return Ok(ArtData::File(p));
        }
        Err(anyhow!("no art for track {track_id}"))
    }

    fn cover_source(&self, db: &Db, album_id: i64) -> Result<Option<String>> {
        db.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT cover_source FROM albums WHERE id = ?1",
                    [album_id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .unwrap_or(None))
        })
    }

    fn album_folder_image(&self, db: &Db, album_id: i64) -> Result<Option<PathBuf>> {
        let dir: Option<String> = db.with(|conn| {
            Ok(conn
                .query_row("SELECT dir FROM albums WHERE id = ?1", [album_id], |r| {
                    r.get(0)
                })
                .ok())
        })?;
        Ok(dir.and_then(|dir| folder_image(Path::new(&dir))))
    }

    fn resolve_embedded(&self, db: &Db, album_id: i64) -> Result<String> {
        // Try embedded art from the album's tracks (first few, in order).
        let candidates: Vec<(i64, String)> = db.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, path FROM tracks WHERE album_id = ?1 AND missing = 0 \
                 AND has_embedded_art = 1 ORDER BY disc_no, track_no LIMIT 3",
            )?;
            let rows = stmt.query_map([album_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.map(|r| Ok(r?)).collect()
        })?;
        for (track_id, path) in candidates {
            if embedded_art(Path::new(&path)).is_some() {
                return Ok(format!("embedded:{track_id}"));
            }
        }
        Err(anyhow!("no cover art found for album {album_id}"))
    }

    /// Fingerprint the source selected for this album without decoding it.
    /// Folder art is checked first so adding or replacing folder.jpg invalidates
    /// an older embedded thumbnail. File size + nanosecond mtime catches normal
    /// in-place replacements while avoiding an image decode on every request.
    fn thumb_signature(&self, db: &Db, album_id: i64) -> Result<Option<String>> {
        if let Some(path) = self.album_folder_image(db, album_id)? {
            return Ok(file_signature(&format!("folder:{}", path.display()), &path));
        }
        let Some(source) = self.cover_source(db, album_id)? else {
            return Ok(None);
        };
        let Some(id) = source
            .strip_prefix("embedded:")
            .and_then(|id| id.parse::<i64>().ok())
        else {
            return Ok(None);
        };
        let path: Option<String> = db.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT path FROM tracks WHERE id = ?1 AND album_id = ?2 \
                     AND missing = 0 AND has_embedded_art = 1",
                    rusqlite::params![id, album_id],
                    |r| r.get(0),
                )
                .ok())
        })?;
        Ok(path.and_then(|path| file_signature(&source, Path::new(&path))))
    }

    /// Remove only thumbnails whose selected source changed since generation.
    /// Called after scans/imports; `album_thumb` repeats the same check lazily in
    /// case cover files change between scans.
    pub fn invalidate_stale(&self, db: &Db) -> Result<()> {
        for entry in std::fs::read_dir(&self.thumb_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jpg") {
                continue;
            }
            let Some(album_id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse().ok())
            else {
                continue;
            };
            let expected = self.thumb_signature(db, album_id)?;
            let actual = std::fs::read_to_string(self.thumb_source_path(album_id)).ok();
            if expected.is_none() || expected != actual {
                self.invalidate(album_id);
            }
        }
        Ok(())
    }

    /// Drop a cached thumbnail (e.g. after a rescan changed the album).
    pub fn invalidate(&self, album_id: i64) {
        let _ = std::fs::remove_file(self.thumb_path(album_id));
        let _ = std::fs::remove_file(self.thumb_source_path(album_id));
    }
}

fn file_signature(source: &str, path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(format!("{source}\u{1f}{}\u{1f}{modified}", metadata.len()))
}

fn unique_temp_path(destination: &Path) -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let name = destination
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("cache");
    destination.with_file_name(format!("{name}.{}.{}.tmp", std::process::id(), id))
}

fn folder_image(dir: &Path) -> Option<PathBuf> {
    for name in FOLDER_NAMES {
        let p = dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

pub fn embedded_art(path: &Path) -> Option<Vec<u8>> {
    let file = lofty::read_from_path(path).ok()?;
    use lofty::file::TaggedFileExt;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    let pic = tag.pictures().first()?;
    Some(pic.data().to_vec())
}

#[cfg(test)]
mod tests {
    use super::{unique_temp_path, ArtStore};
    use crate::db::Db;

    #[test]
    fn thumbnail_refreshes_when_folder_art_changes_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let music = tmp.path().join("music");
        std::fs::create_dir(&music).unwrap();
        let cover = music.join("folder.png");
        image::RgbImage::from_pixel(8, 8, image::Rgb([255, 0, 0]))
            .save(&cover)
            .unwrap();

        let db = Db::open(&tmp.path().join("art.db")).unwrap();
        db.with(|conn| {
            conn.execute(
                "INSERT INTO albums(id, key, dir) VALUES (1, 'album', ?1)",
                [music.to_string_lossy().as_ref()],
            )?;
            Ok(())
        })
        .unwrap();
        let store = ArtStore::new(tmp.path()).unwrap();

        let thumb = store.album_thumb(&db, 1).unwrap();
        let first = std::fs::read(&thumb).unwrap();

        image::RgbImage::from_pixel(16, 16, image::Rgb([0, 0, 255]))
            .save(&cover)
            .unwrap();
        let thumb = store.album_thumb(&db, 1).unwrap();
        assert_ne!(std::fs::read(&thumb).unwrap(), first);

        image::RgbImage::from_pixel(32, 32, image::Rgb([0, 255, 0]))
            .save(&cover)
            .unwrap();
        store.invalidate_stale(&db).unwrap();
        assert!(!thumb.exists());
    }

    #[test]
    fn concurrent_writers_get_distinct_temp_paths() {
        let destination = std::path::Path::new("/tmp/1.jpg");
        assert_ne!(unique_temp_path(destination), unique_temp_path(destination));
    }
}
