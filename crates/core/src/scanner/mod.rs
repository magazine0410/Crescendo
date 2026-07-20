mod albums;
pub mod tags;

use crate::db::Db;
use crate::model::ScanProgress;
use anyhow::Result;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::UNIX_EPOCH;
use tags::TrackMeta;

const AUDIO_EXTS: &[&str] = &["flac", "mp3", "wav", "opus", "ogg", "m4a", "aac", "aiff"];
const BATCH_SIZE: usize = 1000;
const PROGRESS_EVERY: u64 = 500;

pub struct ScanOptions {
    pub root: PathBuf,
    /// Tag-reader threads. ~4 suits a spinning HDD; more only helps on SSDs.
    pub threads: usize,
    /// true = re-read tags for every file even if mtime/size unchanged.
    pub full: bool,
}

struct FileJob {
    path: String,
    dir: String,
    filename: String,
    mtime: i64,
    size: i64,
}

struct WriteJob {
    file: FileJob,
    meta: Result<TrackMeta>,
}

/// Run a scan. `progress` is called from the writer thread every few hundred
/// files and once at the end with `done = true`.
pub fn scan(
    db: &Db,
    opts: &ScanOptions,
    progress: &(dyn Fn(ScanProgress) + Sync),
    cancel: &AtomicBool,
) -> Result<ScanProgress> {
    let conn = db.side_connection()?;

    // Known files: path -> (mtime, size). Entries still present when the walk
    // finishes are missing from disk.
    let mut known: HashMap<String, (i64, i64)> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT path, mtime, size FROM tracks")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))
        })?;
        for row in rows {
            let (path, ms) = row?;
            known.insert(path, ms);
        }
    }

    let scanned = AtomicU64::new(0);
    let (job_tx, job_rx) = bounded::<FileJob>(1024);
    let (write_tx, write_rx) = bounded::<WriteJob>(1024);

    let mut result = ScanProgress {
        scanned: 0,
        added: 0,
        updated: 0,
        removed: 0,
        errors: 0,
        current_path: String::new(),
        done: false,
    };

    let leftover = std::thread::scope(|scope| -> Result<HashMap<String, (i64, i64)>> {
        // Walker
        let walker = scope.spawn({
            let root = opts.root.clone();
            let full = opts.full;
            let scanned = &scanned;
            move || walk(root, full, known, job_tx, scanned, cancel)
        });

        // Tag readers
        for _ in 0..opts.threads.max(1) {
            let job_rx: Receiver<FileJob> = job_rx.clone();
            let write_tx: Sender<WriteJob> = write_tx.clone();
            scope.spawn(move || {
                for file in job_rx.iter() {
                    let meta = tags::read_meta(Path::new(&file.path));
                    if write_tx.send(WriteJob { file, meta }).is_err() {
                        break;
                    }
                }
            });
        }
        // Writer owns the remaining clones; drop ours so the loop below ends.
        drop(job_rx);
        drop(write_tx);

        // Writer (this thread)
        write_loop(&conn, write_rx, &scanned, &mut result, progress)?;

        Ok(walker.join().expect("walker thread panicked"))
    })?;

    // Anything left in `known` was not seen on disk.
    if !cancel.load(Ordering::Relaxed) && !leftover.is_empty() {
        let mut mark = conn.prepare("UPDATE tracks SET missing = 1 WHERE path = ?1")?;
        conn.execute_batch("BEGIN")?;
        for path in leftover.keys() {
            mark.execute([path])?;
            result.removed += 1;
        }
        conn.execute_batch("COMMIT")?;
    }

    albums::rebuild(&conn)?;

    result.scanned = scanned.load(Ordering::Relaxed);
    result.done = true;
    progress(result.clone());
    Ok(result)
}

fn walk(
    root: PathBuf,
    full: bool,
    mut known: HashMap<String, (i64, i64)>,
    job_tx: Sender<FileJob>,
    scanned: &AtomicU64,
    cancel: &AtomicBool,
) -> HashMap<String, (i64, i64)> {
    for entry in walkdir::WalkDir::new(&root).follow_links(false).into_iter().flatten() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else { continue };
        if !AUDIO_EXTS.contains(&ext.to_lowercase().as_str()) {
            continue;
        }
        let Some(path_str) = path.to_str() else {
            log::warn!("skipping non-UTF-8 path: {}", path.display());
            continue;
        };
        let Ok(md) = entry.metadata() else { continue };
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let size = md.len() as i64;

        let prior = known.remove(path_str);
        if !full {
            if let Some((known_mtime, known_size)) = prior {
                if known_mtime == mtime && known_size == size {
                    scanned.fetch_add(1, Ordering::Relaxed);
                    continue; // unchanged: stat-only pass
                }
            }
        }
        let job = FileJob {
            path: path_str.to_string(),
            dir: path.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
            filename: path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default(),
            mtime,
            size,
        };
        if job_tx.send(job).is_err() {
            break;
        }
    }
    known
}

fn write_loop(
    conn: &rusqlite::Connection,
    write_rx: Receiver<WriteJob>,
    scanned: &AtomicU64,
    result: &mut ScanProgress,
    progress: &(dyn Fn(ScanProgress) + Sync),
) -> Result<()> {
    let mut upsert = conn.prepare(
        r#"
        INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms,
                           bitrate, sample_rate, bit_depth, title, artist, album,
                           albumartist, titlesort, artistsort, albumartistsort,
                           genre, grouping, track_no, disc_no, year,
                           date, has_embedded_art, search_blob, missing)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, 0)
        ON CONFLICT(path) DO UPDATE SET
            dir = excluded.dir, filename = excluded.filename,
            mtime = excluded.mtime, size = excluded.size,
            format = excluded.format, duration_ms = excluded.duration_ms,
            bitrate = excluded.bitrate, sample_rate = excluded.sample_rate,
            bit_depth = excluded.bit_depth, title = excluded.title,
            artist = excluded.artist, album = excluded.album,
            albumartist = excluded.albumartist,
            titlesort = excluded.titlesort, artistsort = excluded.artistsort,
            albumartistsort = excluded.albumartistsort, genre = excluded.genre,
            grouping = excluded.grouping, track_no = excluded.track_no,
            disc_no = excluded.disc_no, year = excluded.year,
            date = excluded.date, has_embedded_art = excluded.has_embedded_art,
            search_blob = excluded.search_blob, missing = 0
        "#,
    )?;

    let mut in_batch = 0usize;
    let mut existed_check = conn.prepare("SELECT 1 FROM tracks WHERE path = ?1")?;
    conn.execute_batch("BEGIN")?;
    let mut last_emit = 0u64;

    for job in write_rx.iter() {
        scanned.fetch_add(1, Ordering::Relaxed);
        let file = &job.file;
        // Files with unreadable tags still enter the library under their
        // filename — GStreamer can usually play them regardless.
        let meta = match job.meta {
            Ok(meta) => meta,
            Err(e) => {
                log::warn!("tag read failed for {} ({e}); using filename only", file.path);
                result.errors += 1;
                tags::fallback_meta(Path::new(&file.path))
            }
        };
        let existed = existed_check.exists([&file.path])?;
        let blob = tags::search_blob(&meta);
        upsert.execute(rusqlite::params![
            file.path,
            file.dir,
            file.filename,
            file.mtime,
            file.size,
            meta.format,
            meta.duration_ms,
            meta.bitrate,
            meta.sample_rate,
            meta.bit_depth,
            meta.title,
            meta.artist,
            meta.album,
            meta.albumartist,
            meta.titlesort,
            meta.artistsort,
            meta.albumartistsort,
            meta.genre,
            meta.grouping,
            meta.track_no,
            meta.disc_no,
            meta.year,
            meta.date,
            meta.has_embedded_art,
            blob,
        ])?;
        if existed {
            result.updated += 1;
        } else {
            result.added += 1;
        }

        in_batch += 1;
        if in_batch >= BATCH_SIZE {
            conn.execute_batch("COMMIT; BEGIN")?;
            in_batch = 0;
        }
        let total = scanned.load(Ordering::Relaxed);
        if total - last_emit >= PROGRESS_EVERY {
            last_emit = total;
            let mut p = result.clone();
            p.scanned = total;
            p.current_path = file.path.clone();
            progress(p);
        }
    }
    conn.execute_batch("COMMIT")?;
    Ok(())
}
