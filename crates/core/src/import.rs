//! Adding files and folders to a playlist — the target of a drag and drop
//! from the file manager.
//!
//! A dropped folder contributes every audio file below it. Files the library
//! does not know yet are read on the spot instead of through a full rescan:
//! a drop is a handful of files, and a rescan of a 136k-track library must
//! never be the price of adding an album to a playlist.

use crate::db::Db;
use crate::scanner::{self, tags};
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Default, Serialize)]
pub struct DropImportResult {
    /// Tracks appended to the playlist.
    pub added: usize,
    /// Files skipped because the playlist already holds that track.
    pub duplicates: usize,
    /// Files that were new to the library and were read into it.
    pub imported: usize,
    /// Audio files that could not be read at all.
    pub failed: Vec<String>,
}

/// Append the audio content of `paths` (files, folders, or both) to the
/// playlist. Tracks the playlist already holds are skipped, and so are
/// repeats inside the drop itself.
pub fn add_paths_to_playlist(
    db: &Db,
    playlist_id: i64,
    paths: &[PathBuf],
) -> Result<DropImportResult> {
    let exists = db.with(|conn| {
        Ok(conn.query_row("SELECT 1 FROM playlists WHERE id = ?1", [playlist_id], |_| Ok(()))
            .is_ok())
    })?;
    if !exists {
        bail!("playlist {playlist_id} does not exist");
    }

    let groups = expand(paths);
    let mut result = DropImportResult::default();
    let ids = resolve(db, &groups, &mut result)?;

    // Dedupe against the playlist's current members and against the drop.
    let existing: HashSet<i64> = db.with(|conn| {
        let mut stmt = conn.prepare("SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1")?;
        let set = stmt
            .query_map([playlist_id], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(set)
    })?;
    let mut batch: HashSet<i64> = HashSet::new();
    let mut to_add: Vec<i64> = Vec::new();
    for id in ids {
        if existing.contains(&id) || !batch.insert(id) {
            result.duplicates += 1;
        } else {
            to_add.push(id);
        }
    }

    db.playlist_add_tracks(playlist_id, &to_add)?;
    result.added = to_add.len();

    // New rows change album membership, so the album table (and with it the
    // thumbnail keys) has to catch up.
    if result.imported > 0 {
        db.with(|conn| scanner::albums::rebuild(conn))?;
    }
    Ok(result)
}

/// One dropped path. Folders keep their files together so they can be sorted
/// into album order once the tags are known; a loose file stands alone and
/// keeps the position it was dropped in.
struct Group {
    from_folder: bool,
    files: Vec<PathBuf>,
}

fn expand(paths: &[PathBuf]) -> Vec<Group> {
    let mut groups = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for path in paths {
        if path.is_dir() {
            let mut files: Vec<PathBuf> = walkdir::WalkDir::new(path)
                .follow_links(false)
                .into_iter()
                .flatten()
                .filter(|e| e.file_type().is_file())
                .map(|e| e.into_path())
                .filter(|p| scanner::is_audio_file(p))
                .filter(|p| seen.insert(p.clone()))
                .collect();
            // walkdir yields directory entries in whatever order the
            // filesystem hands them over.
            files.sort();
            if !files.is_empty() {
                groups.push(Group { from_folder: true, files });
            }
        } else if scanner::is_audio_file(path) && seen.insert(path.clone()) {
            groups.push(Group { from_folder: false, files: vec![path.clone()] });
        }
    }
    groups
}

/// One file of a dropped folder, with what the ordering needs.
struct Resolved<'a> {
    /// Albumartist (or artist) and album, the pair the albums table keys on.
    album: (String, String),
    disc: i64,
    track: i64,
    path: &'a Path,
    id: i64,
}

/// A `tracks` row as the ordering needs it: id, missing flag, album key,
/// disc and track number.
type Row = (i64, i64, String, String, i64, i64);

const LOOKUP: &str = "SELECT id, missing, COALESCE(albumartist, artist, ''), COALESCE(album, ''), \
                      COALESCE(disc_no, 0), COALESCE(track_no, 0) \
                      FROM tracks WHERE path = ?1";

/// Track ids for every file, importing the ones the library has not seen.
/// Files from a folder come back album by album, each album in disc/track
/// order.
fn resolve(db: &Db, groups: &[Group], result: &mut DropImportResult) -> Result<Vec<i64>> {
    db.with(|conn| {
        let mut lookup = conn.prepare(LOOKUP)?;
        let mut read = |path: &str| -> Option<Row> {
            lookup
                .query_row([path], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                })
                .ok()
        };
        let mut upsert = conn.prepare(scanner::UPSERT_TRACK)?;
        let mut out = Vec::new();

        for group in groups {
            let mut resolved: Vec<Resolved> = Vec::new();
            for file in &group.files {
                let Some(path_str) = file.to_str() else {
                    log::warn!("skipping non-UTF-8 path: {}", file.display());
                    result.failed.push(file.display().to_string());
                    continue;
                };
                let mut row = read(path_str);
                // Unknown, or known but marked missing and back on disk.
                if row.as_ref().is_none_or(|(_, missing, ..)| *missing != 0) {
                    if let Err(e) = import_file(&mut upsert, file, path_str) {
                        log::warn!("cannot import {path_str}: {e}");
                        result.failed.push(path_str.to_string());
                        continue;
                    }
                    result.imported += 1;
                    row = read(path_str);
                }
                match row {
                    Some((id, _, albumartist, album, disc, track)) => resolved.push(Resolved {
                        album: (albumartist, album),
                        disc,
                        track,
                        path: file.as_path(),
                        id,
                    }),
                    None => result.failed.push(path_str.to_string()),
                }
            }
            if group.from_folder {
                sort_by_album(&mut resolved);
            }
            out.extend(resolved.iter().map(|r| r.id));
        }
        Ok(out)
    })
}

/// Order the files of one dropped folder.
///
/// A folder is not always one album: dropping a parent folder, or a box set,
/// brings several albums in at once. Sorting the whole tree by disc/track
/// would interleave them, because every album has a track 1. So each album
/// stays together — in the order its files first appear on disk — and only
/// the tracks inside an album sort by disc and track number. That also keeps
/// a multi-disc album whole when its discs sit in separate subfolders.
fn sort_by_album(resolved: &mut [Resolved]) {
    // `resolved` arrives in path order, so first appearance is disk order.
    let mut first_seen: HashMap<(String, String), usize> = HashMap::new();
    for r in resolved.iter() {
        let next = first_seen.len();
        first_seen.entry(r.album.clone()).or_insert(next);
    }
    resolved.sort_by(|a, b| {
        (first_seen[&a.album], a.disc, a.track, a.path)
            .cmp(&(first_seen[&b.album], b.disc, b.track, b.path))
    });
}

/// Read one file's tags and write its row. Mirrors the scanner's writer,
/// including its fallback: a file with unreadable tags still enters the
/// library under its filename, because GStreamer can usually play it anyway.
fn import_file(
    upsert: &mut rusqlite::Statement<'_>,
    path: &Path,
    path_str: &str,
) -> Result<()> {
    let md = std::fs::metadata(path)?;
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let meta = tags::read_meta(path).unwrap_or_else(|e| {
        log::warn!("tag read failed for {path_str} ({e}); using filename only");
        tags::fallback_meta(path)
    });
    let dir = path.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let filename = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    scanner::upsert_row(upsert, path_str, &dir, &filename, mtime, md.len() as i64, &meta)?;
    Ok(())
}
