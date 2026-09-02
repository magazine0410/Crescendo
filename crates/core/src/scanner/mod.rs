pub(crate) mod albums;
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

pub const AUDIO_EXTS: &[&str] = &["flac", "mp3", "wav", "opus", "ogg", "m4a", "aac", "aiff"];
const BATCH_SIZE: usize = 1000;
const PROGRESS_EVERY: u64 = 500;

/// true if the extension is one the scanner indexes. Case-insensitive.
pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| AUDIO_EXTS.contains(&ext.to_lowercase().as_str()))
}

/// Upsert statement for one `tracks` row, keyed on `path`. Shared with
/// `crate::import`, which adds dropped files one at a time.
pub(crate) const UPSERT_TRACK: &str = r#"
    INSERT INTO tracks(path, dir, filename, mtime, size, format, duration_ms,
                       bitrate, sample_rate, bit_depth, title, artist, album,
                       albumartist, titlesort, artistsort, albumartistsort,
                       genre, grouping, composer, arranger, lyricist, vocalist,
                       catalognumber, event, bpm, track_no, disc_no, year,
                       date, has_embedded_art, search_blob, missing)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
            ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28,
            ?29, ?30, ?31, ?32, 0)
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
        grouping = excluded.grouping, composer = excluded.composer,
        arranger = excluded.arranger, lyricist = excluded.lyricist,
        vocalist = excluded.vocalist,
        catalognumber = excluded.catalognumber, event = excluded.event,
        bpm = excluded.bpm, track_no = excluded.track_no,
        disc_no = excluded.disc_no, year = excluded.year,
        date = excluded.date, has_embedded_art = excluded.has_embedded_art,
        search_blob = excluded.search_blob, missing = 0
"#;

/// Bind one file onto a statement prepared from [`UPSERT_TRACK`]. Callers
/// prepare once and loop, so a batch pays for the parse only once.
pub(crate) fn upsert_row(
    stmt: &mut rusqlite::Statement<'_>,
    path: &str,
    dir: &str,
    filename: &str,
    mtime: i64,
    size: i64,
    meta: &TrackMeta,
) -> rusqlite::Result<()> {
    stmt.execute(rusqlite::params![
        path,
        dir,
        filename,
        mtime,
        size,
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
        meta.composer,
        meta.arranger,
        meta.lyricist,
        meta.vocalist,
        meta.catalognumber,
        meta.event,
        meta.bpm,
        meta.track_no,
        meta.disc_no,
        meta.year,
        meta.date,
        meta.has_embedded_art,
        tags::search_blob(meta),
    ])?;
    Ok(())
}

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

/// Shared progress counters. Both the walker (unchanged, stat-only files) and
/// the writer (changed/new files) bump these and emit snapshots, so the bar
/// moves during incremental scans where most files never reach the writer.
#[derive(Clone, Copy)]
struct Counters<'a> {
    scanned: &'a AtomicU64,
    added: &'a AtomicU64,
    updated: &'a AtomicU64,
    errors: &'a AtomicU64,
}

impl Counters<'_> {
    /// A progress snapshot. `removed` is only known once the walk finishes, so
    /// intermediate events report 0 for it (unchanged from the old behavior).
    fn snapshot(&self, current_path: String, done: bool) -> ScanProgress {
        ScanProgress {
            scanned: self.scanned.load(Ordering::Relaxed),
            added: self.added.load(Ordering::Relaxed),
            updated: self.updated.load(Ordering::Relaxed),
            removed: 0,
            errors: self.errors.load(Ordering::Relaxed),
            current_path,
            done,
        }
    }
}

/// Run a scan. `progress` is called from the writer thread every few hundred
/// files and once at the end with `done = true`.
///
/// The scan stops with an error if the root is unreachable or holds no audio
/// files while the library holds some. On a network share (an unmounted NAS, a
/// share that dropped) the walk would otherwise find nothing and the sweep at
/// the end would mark every track `missing = 1` — which reads as an empty
/// library, because all views filter on `missing = 0`.
pub fn scan(
    db: &Db,
    opts: &ScanOptions,
    progress: &(dyn Fn(ScanProgress) + Sync),
    cancel: &AtomicBool,
) -> Result<ScanProgress> {
    if !opts.root.is_dir() {
        anyhow::bail!(
            "music folder not found: {} (if it is on a network drive, mount it and try again)",
            opts.root.display()
        );
    }

    let conn = db.side_connection()?;

    // Known files: path -> (mtime, size). Entries still present when the walk
    // finishes are missing from disk.
    let mut known: HashMap<String, (i64, i64)> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT path, mtime, size FROM tracks")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?),
            ))
        })?;
        for row in rows {
            let (path, ms) = row?;
            known.insert(path, ms);
        }
    }

    let scanned = AtomicU64::new(0);
    let added = AtomicU64::new(0);
    let updated = AtomicU64::new(0);
    let errors = AtomicU64::new(0);
    let counters = Counters {
        scanned: &scanned,
        added: &added,
        updated: &updated,
        errors: &errors,
    };
    let (job_tx, job_rx) = bounded::<FileJob>(1024);
    let (write_tx, write_rx) = bounded::<WriteJob>(1024);

    let leftover = std::thread::scope(|scope| -> Result<HashMap<String, (i64, i64)>> {
        // Walker
        let walker = scope.spawn({
            let root = opts.root.clone();
            let full = opts.full;
            move || walk(root, full, known, job_tx, counters, progress, cancel)
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
        write_loop(&conn, write_rx, counters, progress)?;

        Ok(walker.join().expect("walker thread panicked"))
    })?;

    // A root that is present but empty gets the same treatment as a missing
    // one: an empty mount point is what a failed mount looks like.
    let seen = scanned.load(Ordering::Relaxed);
    if !cancel.load(Ordering::Relaxed) && seen == 0 && !leftover.is_empty() {
        anyhow::bail!(
            "music folder {} holds no audio files, but the library holds {}; \
             the library was left unchanged",
            opts.root.display(),
            leftover.len()
        );
    }

    // Anything left in `known` was not seen on disk.
    let mut removed = 0u64;
    if !cancel.load(Ordering::Relaxed) && !leftover.is_empty() {
        let mut mark = conn.prepare("UPDATE tracks SET missing = 1 WHERE path = ?1")?;
        conn.execute_batch("BEGIN")?;
        for path in leftover.keys() {
            mark.execute([path])?;
            removed += 1;
        }
        conn.execute_batch("COMMIT")?;
    }

    albums::rebuild(&conn)?;

    let mut result = counters.snapshot(String::new(), true);
    result.removed = removed;
    progress(result.clone());
    Ok(result)
}

fn walk(
    root: PathBuf,
    full: bool,
    mut known: HashMap<String, (i64, i64)>,
    job_tx: Sender<FileJob>,
    counters: Counters,
    progress: &(dyn Fn(ScanProgress) + Sync),
    cancel: &AtomicBool,
) -> HashMap<String, (i64, i64)> {
    let mut last_emit = 0u64;
    for entry in walkdir::WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !is_audio_file(path) {
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
                    // Unchanged: stat-only pass. These files never reach the
                    // writer, so emit progress here or an incremental scan
                    // would sit at 0% until it finishes.
                    let total = counters.scanned.fetch_add(1, Ordering::Relaxed) + 1;
                    if total - last_emit >= PROGRESS_EVERY {
                        last_emit = total;
                        progress(counters.snapshot(path_str.to_string(), false));
                    }
                    continue;
                }
            }
        }
        let job = FileJob {
            path: path_str.to_string(),
            dir: path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
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
    counters: Counters,
    progress: &(dyn Fn(ScanProgress) + Sync),
) -> Result<()> {
    let mut upsert = conn.prepare(UPSERT_TRACK)?;

    let mut in_batch = 0usize;
    let mut existed_check = conn.prepare("SELECT 1 FROM tracks WHERE path = ?1")?;
    conn.execute_batch("BEGIN")?;
    let mut last_emit = 0u64;

    for job in write_rx.iter() {
        counters.scanned.fetch_add(1, Ordering::Relaxed);
        let file = &job.file;
        // Files with unreadable tags still enter the library under their
        // filename — GStreamer can usually play them regardless.
        let meta = match job.meta {
            Ok(meta) => meta,
            Err(e) => {
                log::warn!(
                    "tag read failed for {} ({e}); using filename only",
                    file.path
                );
                counters.errors.fetch_add(1, Ordering::Relaxed);
                tags::fallback_meta(Path::new(&file.path))
            }
        };
        let existed = existed_check.exists([&file.path])?;
        upsert_row(
            &mut upsert,
            &file.path,
            &file.dir,
            &file.filename,
            file.mtime,
            file.size,
            &meta,
        )?;
        if existed {
            counters.updated.fetch_add(1, Ordering::Relaxed);
        } else {
            counters.added.fetch_add(1, Ordering::Relaxed);
        }

        in_batch += 1;
        if in_batch >= BATCH_SIZE {
            conn.execute_batch("COMMIT; BEGIN")?;
            in_batch = 0;
        }
        let total = counters.scanned.load(Ordering::Relaxed);
        if total - last_emit >= PROGRESS_EVERY {
            last_emit = total;
            progress(counters.snapshot(file.path.clone(), false));
        }
    }
    conn.execute_batch("COMMIT")?;
    Ok(())
}
