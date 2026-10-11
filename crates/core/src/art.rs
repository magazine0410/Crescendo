//! Cover-art resolution and thumbnail cache.
//!
//! No image blobs in the database: per album we remember only a
//! `cover_source` string ("folder:<path>" or "embedded:<track_id>"), resolve
//! it lazily the first time art is requested, and cache a 256px JPEG in the
//! thumbnail directory keyed by album id.

use crate::db::Db;
use crate::scanner::tags::without_panics;
use anyhow::{anyhow, Result};
use rusqlite::OptionalExtension;
use std::collections::HashSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

pub const THUMB_SIZE: u32 = 256;
const MAX_COVER_DIMENSION: u32 = 8192;
const MAX_COVER_DECODE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_COVER_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EMBEDDED_ART_BYTES: usize = 32 * 1024 * 1024;

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
    /// Failed sources keyed by source + file size + mtime. A corrupt preferred
    /// folder image is skipped for the rest of the process, but replacing it
    /// changes the signature and makes it eligible immediately.
    invalid_sources: Mutex<HashSet<String>>,
}

pub enum ArtData {
    /// Serve this file directly from disk.
    File(PathBuf),
    /// Raw image bytes (embedded art).
    Bytes(Vec<u8>),
}

struct ResolvedArt {
    data: ArtData,
    image: image::DynamicImage,
    signature: String,
}

impl ArtStore {
    pub fn new(cache_dir: &Path) -> Result<Self> {
        let thumb_dir = cache_dir.join("thumbs");
        std::fs::create_dir_all(&thumb_dir)?;
        Ok(Self {
            thumb_dir,
            invalid_sources: Mutex::new(HashSet::new()),
        })
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
        let resolved = self.resolve_album_art(db, album_id)?;
        let generated_signature = resolved.signature;
        let thumb = resolved.image.thumbnail(THUMB_SIZE, THUMB_SIZE);
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
        let source_path = self.thumb_source_path(album_id);
        let source_tmp = unique_temp_path(&source_path);
        if let Err(e) = std::fs::write(&source_tmp, generated_signature)
            .and_then(|_| std::fs::rename(&source_tmp, &source_path))
        {
            let _ = std::fs::remove_file(&source_tmp);
            return Err(e.into());
        }
        Ok(cached)
    }

    /// Full-size art for an album (folder image preferred, else embedded art
    /// from one of its tracks). Resolution result is cached in
    /// `albums.cover_source`.
    pub fn album_full(&self, db: &Db, album_id: i64) -> Result<ArtData> {
        Ok(self.resolve_album_art(db, album_id)?.data)
    }

    fn resolve_album_art(&self, db: &Db, album_id: i64) -> Result<ResolvedArt> {
        // Always re-check the album directory. A new valid folder.jpg replaces
        // embedded art, while corrupt candidates fall through in name order.
        for path in self.album_folder_images(db, album_id)? {
            let source = format!("folder:{}", path.display());
            let Some(signature) = file_signature(&source, &path) else {
                continue;
            };
            if self.is_invalid(&signature) {
                continue;
            }
            let data = ArtData::File(path);
            match decode_art(&data) {
                Ok(image) => {
                    self.remember_source(db, album_id, &source)?;
                    return Ok(ResolvedArt {
                        data,
                        image,
                        signature,
                    });
                }
                Err(error) => {
                    log::warn!("ignoring invalid album cover {source}: {error}");
                    self.mark_invalid(signature);
                }
            }
        }

        if let Some(source) = self.cover_source(db, album_id)? {
            if let Some(track_id) = source
                .strip_prefix("embedded:")
                .and_then(|id| id.parse::<i64>().ok())
            {
                if let Some(resolved) = self.resolve_embedded(db, album_id, track_id)? {
                    self.remember_source(db, album_id, &source)?;
                    return Ok(resolved);
                }
            }
        }

        let candidates: Vec<(i64, String)> = db.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, path FROM tracks WHERE album_id = ?1 AND missing = 0 \
                 AND has_embedded_art = 1 ORDER BY disc_no, track_no LIMIT 3",
            )?;
            let rows = stmt.query_map([album_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.map(|r| Ok(r?)).collect()
        })?;
        for (track_id, _) in candidates {
            if let Some(resolved) = self.resolve_embedded(db, album_id, track_id)? {
                self.remember_source(db, album_id, &format!("embedded:{track_id}"))?;
                return Ok(resolved);
            }
        }
        self.clear_source(db, album_id)?;
        Err(anyhow!("no decodable cover art found for album {album_id}"))
    }

    fn resolve_embedded(
        &self,
        db: &Db,
        album_id: i64,
        track_id: i64,
    ) -> Result<Option<ResolvedArt>> {
        let path: Option<String> = db.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT path FROM tracks WHERE id = ?1 AND album_id = ?2 \
                     AND missing = 0 AND has_embedded_art = 1",
                    rusqlite::params![track_id, album_id],
                    |r| r.get(0),
                )
                .optional()?)
        })?;
        let Some(path) = path else { return Ok(None) };
        let path = PathBuf::from(path);
        let source = format!("embedded:{track_id}");
        let Some(signature) = file_signature(&source, &path) else {
            return Ok(None);
        };
        if self.is_invalid(&signature) {
            return Ok(None);
        }
        let Some(bytes) = embedded_art(&path) else {
            self.mark_invalid(signature);
            return Ok(None);
        };
        let data = ArtData::Bytes(bytes);
        match decode_art(&data) {
            Ok(image) => Ok(Some(ResolvedArt {
                data,
                image,
                signature,
            })),
            Err(error) => {
                log::warn!(
                    "ignoring invalid embedded cover in {}: {error}",
                    path.display()
                );
                self.mark_invalid(signature);
                Ok(None)
            }
        }
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

    fn clear_source(&self, db: &Db, album_id: i64) -> Result<()> {
        db.with(|conn| {
            conn.execute(
                "UPDATE albums SET cover_source = NULL WHERE id = ?1",
                [album_id],
            )?;
            Ok(())
        })
    }

    fn is_invalid(&self, signature: &str) -> bool {
        self.invalid_sources
            .lock()
            .is_ok_and(|invalid| invalid.contains(signature))
    }

    fn mark_invalid(&self, signature: String) {
        if let Ok(mut invalid) = self.invalid_sources.lock() {
            invalid.insert(signature);
        }
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
            let data = ArtData::Bytes(bytes);
            if decode_art(&data).is_ok() {
                return Ok(data);
            }
        }
        for path in folder_images(Path::new(&dir)) {
            let source = format!("folder:{}", path.display());
            let Some(signature) = file_signature(&source, &path) else {
                continue;
            };
            if self.is_invalid(&signature) {
                continue;
            }
            let data = ArtData::File(path);
            if decode_art(&data).is_ok() {
                return Ok(data);
            }
            self.mark_invalid(signature);
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

    fn album_folder_images(&self, db: &Db, album_id: i64) -> Result<Vec<PathBuf>> {
        let dir: Option<String> = db.with(|conn| {
            Ok(conn
                .query_row("SELECT dir FROM albums WHERE id = ?1", [album_id], |r| {
                    r.get(0)
                })
                .ok())
        })?;
        Ok(dir
            .map(|dir| folder_images(Path::new(&dir)))
            .unwrap_or_default())
    }

    /// Fingerprint the source selected for this album without decoding it.
    /// Folder art is checked first so adding or replacing folder.jpg invalidates
    /// an older embedded thumbnail. File size + nanosecond mtime catches normal
    /// in-place replacements while avoiding an image decode on every request.
    fn thumb_signature(&self, db: &Db, album_id: i64) -> Result<Option<String>> {
        for path in self.album_folder_images(db, album_id)? {
            let source = format!("folder:{}", path.display());
            let Some(signature) = file_signature(&source, &path) else {
                continue;
            };
            if !self.is_invalid(&signature) {
                return Ok(Some(signature));
            }
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
        let signature = path.and_then(|path| file_signature(&source, Path::new(&path)));
        Ok(signature.filter(|signature| !self.is_invalid(signature)))
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

fn folder_images(dir: &Path) -> Vec<PathBuf> {
    FOLDER_NAMES
        .iter()
        .map(|name| dir.join(name))
        .filter(|path| path.is_file())
        .collect()
}

pub fn embedded_art(path: &Path) -> Option<Vec<u8>> {
    // A file that makes lofty panic is treated as having no embedded cover.
    match without_panics(path, || Ok(read_embedded_art(path))) {
        Ok(art) => art,
        Err(e) => {
            log::warn!("embedded cover read failed: {e}");
            None
        }
    }
}

fn read_embedded_art(path: &Path) -> Option<Vec<u8>> {
    let file = lofty::read_from_path(path).ok()?;
    use lofty::file::TaggedFileExt;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    let pic = tag.pictures().first()?;
    if pic.data().len() > MAX_EMBEDDED_ART_BYTES {
        log::warn!(
            "embedded cover in {} exceeds the 32 MiB limit",
            path.display()
        );
        return None;
    }
    Some(pic.data().to_vec())
}

fn cover_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_COVER_DIMENSION);
    limits.max_image_height = Some(MAX_COVER_DIMENSION);
    limits.max_alloc = Some(MAX_COVER_DECODE_BYTES);
    limits
}

fn decode_file(path: &Path) -> Result<image::DynamicImage> {
    let size = std::fs::metadata(path)?.len();
    if size > MAX_COVER_FILE_BYTES {
        return Err(anyhow!("cover file exceeds the 64 MiB encoded-size limit"));
    }
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    reader.limits(cover_limits());
    Ok(reader.decode()?)
}

fn decode_bytes(bytes: &[u8]) -> Result<image::DynamicImage> {
    if bytes.len() > MAX_EMBEDDED_ART_BYTES {
        return Err(anyhow!(
            "embedded cover exceeds the 32 MiB encoded-size limit"
        ));
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(cover_limits());
    Ok(reader.decode()?)
}

fn decode_art(art: &ArtData) -> Result<image::DynamicImage> {
    match art {
        ArtData::File(path) => decode_file(path),
        ArtData::Bytes(bytes) => decode_bytes(bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_file, unique_temp_path, ArtStore, MAX_COVER_DIMENSION};
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

    #[test]
    fn corrupt_preferred_folder_art_falls_through_to_a_valid_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        let music = tmp.path().join("music");
        std::fs::create_dir(&music).unwrap();
        std::fs::write(music.join("folder.jpg"), b"not an image").unwrap();
        let fallback = music.join("cover.png");
        image::RgbImage::from_pixel(8, 8, image::Rgb([10, 20, 30]))
            .save(&fallback)
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
        assert!(store.album_thumb(&db, 1).unwrap().is_file());
        let source: String = db
            .with(|conn| {
                Ok(
                    conn.query_row("SELECT cover_source FROM albums WHERE id = 1", [], |row| {
                        row.get(0)
                    })?,
                )
            })
            .unwrap();
        assert!(source.ends_with("cover.png"));
    }

    #[test]
    fn cover_dimensions_are_bounded_before_full_decode() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("wide.png");
        image::RgbImage::new(MAX_COVER_DIMENSION + 1, 1)
            .save(&path)
            .unwrap();
        assert!(decode_file(&path).is_err());
    }
}
