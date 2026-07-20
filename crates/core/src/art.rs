//! Cover-art resolution and thumbnail cache.
//!
//! No image blobs in the database: per album we remember only a
//! `cover_source` string ("folder:<path>" or "embedded:<track_id>"), resolve
//! it lazily the first time art is requested, and cache a 256px JPEG in the
//! thumbnail directory keyed by album id.

use crate::db::Db;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

pub const THUMB_SIZE: u32 = 256;

const FOLDER_NAMES: &[&str] = &[
    "folder.jpg", "cover.jpg", "front.jpg", "folder.png", "cover.png", "front.png",
    "Folder.jpg", "Cover.jpg", "Front.jpg",
];

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

    /// Thumbnail for an album; generates and caches on first request.
    pub fn album_thumb(&self, db: &Db, album_id: i64) -> Result<PathBuf> {
        let cached = self.thumb_path(album_id);
        if cached.exists() {
            return Ok(cached);
        }
        let full = self.album_full(db, album_id)?;
        let img = match &full {
            ArtData::File(p) => image::open(p)?,
            ArtData::Bytes(b) => image::load_from_memory(b)?,
        };
        let thumb = img.thumbnail(THUMB_SIZE, THUMB_SIZE);
        let rgb = image::DynamicImage::ImageRgb8(thumb.to_rgb8());
        let tmp = cached.with_extension("jpg.tmp");
        rgb.save_with_format(&tmp, image::ImageFormat::Jpeg)?;
        std::fs::rename(&tmp, &cached)?;
        Ok(cached)
    }

    /// Full-size art for an album (folder image preferred, else embedded art
    /// from one of its tracks). Resolution result is cached in
    /// `albums.cover_source`.
    pub fn album_full(&self, db: &Db, album_id: i64) -> Result<ArtData> {
        if let Some(source) = self.cover_source(db, album_id)? {
            if let Some(art) = self.load_source(db, &source) {
                return Ok(art);
            }
            // stale cache entry (file moved) — re-resolve below
        }
        let source = self.resolve(db, album_id)?;
        db.with(|conn| {
            conn.execute(
                "UPDATE albums SET cover_source = ?2 WHERE id = ?1",
                rusqlite::params![album_id, source],
            )?;
            Ok(())
        })?;
        self.load_source(db, &source)
            .ok_or_else(|| anyhow!("no cover art for album {album_id}"))
    }

    fn load_source(&self, db: &Db, source: &str) -> Option<ArtData> {
        if let Some(path) = source.strip_prefix("folder:") {
            let p = PathBuf::from(path);
            return p.exists().then_some(ArtData::File(p));
        }
        if let Some(id) = source.strip_prefix("embedded:") {
            let track_id: i64 = id.parse().ok()?;
            let path: String = db
                .with(|conn| {
                    Ok(conn
                        .query_row("SELECT path FROM tracks WHERE id = ?1", [track_id], |r| {
                            r.get(0)
                        })
                        .ok())
                })
                .ok()
                .flatten()?;
            return embedded_art(Path::new(&path)).map(ArtData::Bytes);
        }
        None
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
                .query_row("SELECT cover_source FROM albums WHERE id = ?1", [album_id], |r| {
                    r.get::<_, Option<String>>(0)
                })
                .unwrap_or(None))
        })
    }

    fn resolve(&self, db: &Db, album_id: i64) -> Result<String> {
        let dir: Option<String> = db.with(|conn| {
            Ok(conn
                .query_row("SELECT dir FROM albums WHERE id = ?1", [album_id], |r| r.get(0))
                .ok())
        })?;
        if let Some(dir) = dir {
            if let Some(p) = folder_image(Path::new(&dir)) {
                return Ok(format!("folder:{}", p.display()));
            }
        }
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

    /// Drop a cached thumbnail (e.g. after a rescan changed the album).
    pub fn invalidate(&self, album_id: i64) {
        let _ = std::fs::remove_file(self.thumb_path(album_id));
    }
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
