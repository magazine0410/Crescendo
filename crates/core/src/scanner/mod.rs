pub(crate) mod albums;
pub mod tags;

use crate::db::Db;
use crate::model::ScanProgress;
use anyhow::Result;
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};
use tags::TrackMeta;

pub const AUDIO_EXTS: &[&str] = &["flac", "mp3", "wav", "opus", "ogg", "m4a", "aac", "aiff"];
const BATCH_SIZE: usize = 1000;
const PROGRESS_EVERY: u64 = 500;
/// Longest the writer holds the database write lock in one transaction. Play
/// history, playlist edits and prefs on the main connection wait this long at
/// most.
const TXN_BUDGET: Duration = Duration::from_millis(200);
/// How long the writer waits for the next file before committing what it has.
const IDLE_COMMIT: Duration = Duration::from_millis(50);
/// Pause after a commit forced by `BATCH_SIZE` or `TXN_BUDGET`. SQLite's busy
/// handler polls every 100 ms once it has waited a while, so the gap has to be
/// longer than that to let a waiting writer in.
const WRITER_YIELD: Duration = Duration::from_millis(120);

struct KnownFile {
    mtime: i64,
    size: i64,
    missing: bool,
}

struct WalkResult {
    leftover: HashMap<String, KnownFile>,
    error: Option<String>,
}

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

    // Only this root's paths can be declared missing by this walk. Playlist
    // imports and other roots are not evidence of a missing file here.
    let scan_root = std::path::absolute(&opts.root)?;
    let mut known = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT path, mtime, size, missing FROM tracks")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                KnownFile {
                    mtime: r.get(1)?,
                    size: r.get(2)?,
                    missing: r.get(3)?,
                },
            ))
        })?;
        for row in rows {
            let (path, ms) = row?;
            if std::path::absolute(&path)?.starts_with(&scan_root) {
                known.insert(path, ms);
            }
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

    let walk_result = std::thread::scope(|scope| -> Result<WalkResult> {
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

    if let Some(error) = walk_result.error {
        // Successfully read files may already have been upserted in batches.
        // Keep their album membership consistent, but never infer absence from
        // a partial traversal (including a single failed metadata read).
        albums::rebuild(&conn)?;
        anyhow::bail!("scan incomplete; missing-file cleanup skipped: {error}");
    }
    let leftover = walk_result.leftover;

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
        conn.execute_batch("BEGIN IMMEDIATE")?;
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
    mut known: HashMap<String, KnownFile>,
    job_tx: Sender<FileJob>,
    counters: Counters,
    progress: &(dyn Fn(ScanProgress) + Sync),
    cancel: &AtomicBool,
) -> WalkResult {
    let mut last_emit = 0u64;
    let mut error = None;
    let mut record_error = |detail: String| {
        counters.errors.fetch_add(1, Ordering::Relaxed);
        log::warn!("scan traversal failed: {detail}");
        error.get_or_insert(detail);
    };
    for entry in walkdir::WalkDir::new(&root).follow_links(false) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                record_error(error.to_string());
                continue;
            }
        };
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
        let md = match entry.metadata() {
            Ok(md) => md,
            Err(error) => {
                record_error(error.to_string());
                continue;
            }
        };
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let size = md.len() as i64;

        let prior = known.remove(path_str);
        if !full {
            if let Some(ref prior) = prior {
                if !prior.missing && prior.mtime == mtime && prior.size == size {
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
    WalkResult {
        leftover: known,
        error,
    }
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
    // When the open transaction began. The write lock is held only while it
    // is Some, so the main connection can write whenever the readers are
    // slower than this loop, which on an HDD is most of the time.
    let mut opened: Option<Instant> = None;
    let mut last_emit = 0u64;

    loop {
        let job = if opened.is_some() {
            match write_rx.recv_timeout(IDLE_COMMIT) {
                Ok(job) => job,
                Err(RecvTimeoutError::Timeout) => {
                    // No work queued: commit rather than wait holding the lock.
                    conn.execute_batch("COMMIT")?;
                    opened = None;
                    in_batch = 0;
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match write_rx.recv() {
                Ok(job) => job,
                Err(_) => break,
            }
        };
        if opened.is_none() {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            opened = Some(Instant::now());
        }
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
        let total = counters.scanned.load(Ordering::Relaxed);
        if total - last_emit >= PROGRESS_EVERY {
            last_emit = total;
            progress(counters.snapshot(file.path.clone(), false));
        }
        if in_batch >= BATCH_SIZE || opened.is_some_and(|t| t.elapsed() >= TXN_BUDGET) {
            conn.execute_batch("COMMIT")?;
            opened = None;
            in_batch = 0;
            // Work is still queued, so the next BEGIN would follow at once and
            // a writer polling from the main connection's busy handler could
            // miss every gap.
            std::thread::sleep(WRITER_YIELD);
        }
    }
    if opened.is_some() {
        conn.execute_batch("COMMIT")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn job(i: usize) -> WriteJob {
        let path = format!("/music/{i:05}.flac");
        WriteJob {
            meta: Ok(tags::fallback_meta(Path::new(&path))),
            file: FileJob {
                filename: format!("{i:05}.flac"),
                dir: "/music".into(),
                path,
                mtime: 0,
                size: 0,
            },
        }
    }

    fn counters(c: &[AtomicU64; 4]) -> Counters<'_> {
        Counters {
            scanned: &c[0],
            added: &c[1],
            updated: &c[2],
            errors: &c[3],
        }
    }

    /// An incremental scan sends a changed file early and then walks for a
    /// long time. The writer must not sit on the write lock meanwhile, or play
    /// history and playlist edits on the main connection fail.
    #[test]
    fn idle_writer_releases_the_write_lock() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("library.db")).unwrap();
        let conn = db.side_connection().unwrap();
        let playlist = db.playlist_create("Test").unwrap();
        let c: [AtomicU64; 4] = Default::default();
        let (tx, rx) = bounded::<WriteJob>(4);

        std::thread::scope(|s| {
            let cs = counters(&c);
            let writer = s.spawn(move || write_loop(&conn, rx, cs, &|_| {}));
            tx.send(job(0)).unwrap();

            // The sender stays open, as it does mid-walk. The row must still
            // be committed and visible to another connection.
            let deadline = Instant::now() + Duration::from_secs(5);
            let id = loop {
                let id: Option<i64> = db
                    .with(|conn| {
                        Ok(conn
                            .query_row(
                                "SELECT id FROM tracks WHERE path = '/music/00000.flac'",
                                [],
                                |r| r.get(0),
                            )
                            .ok())
                    })
                    .unwrap();
                if let Some(id) = id {
                    break id;
                }
                assert!(Instant::now() < deadline, "the writer never committed");
                std::thread::sleep(Duration::from_millis(10));
            };

            // Reads the playlist, then inserts: the path that used to fail at
            // once with "database is locked".
            db.playlist_add_tracks(playlist, &[id]).unwrap();
            let other = db.side_connection().unwrap();
            other
                .execute_batch("PRAGMA busy_timeout = 0; BEGIN IMMEDIATE; COMMIT")
                .expect("the idle writer still holds the write lock");

            drop(tx);
            writer.join().unwrap().unwrap();
        });
        assert_eq!(c[1].load(Ordering::Relaxed), 1);
    }

    /// During a full scan work is always queued. A main-connection write must
    /// still get in within a few transaction budgets, not wait out its whole
    /// busy timeout.
    #[test]
    fn busy_writer_lets_the_main_connection_in() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("library.db")).unwrap();
        let conn = db.side_connection().unwrap();
        let c: [AtomicU64; 4] = Default::default();
        let (tx, rx) = bounded::<WriteJob>(1024);
        let stop = AtomicBool::new(false);

        std::thread::scope(|s| {
            let cs = counters(&c);
            let writer = s.spawn(move || write_loop(&conn, rx, cs, &|_| {}));
            s.spawn(|| {
                let mut i = 0;
                while !stop.load(Ordering::Relaxed) && tx.send(job(i)).is_ok() {
                    i += 1;
                }
                drop(tx);
            });
            // Let the writer reach a steady state with the queue full.
            while c[0].load(Ordering::Relaxed) < 100 {
                std::thread::sleep(Duration::from_millis(5));
            }

            let started = Instant::now();
            db.playlist_create("During scan").unwrap();
            let waited = started.elapsed();
            stop.store(true, Ordering::Relaxed);
            writer.join().unwrap().unwrap();
            assert!(
                waited < Duration::from_secs(2),
                "main-connection write waited {waited:?}"
            );
        });
    }
}
