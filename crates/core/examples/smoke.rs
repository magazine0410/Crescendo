//! Manual smoke test: scan a directory into a scratch DB, print stats, and
//! exercise search + art resolution.
//!
//! cargo run -p crescendo-core --example smoke -- <music-dir> <db-path> [--full]

use crescendo_core::art::ArtStore;
use crescendo_core::db::Db;
use crescendo_core::model::BrowseMode;
use crescendo_core::scanner::{scan, ScanOptions};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("usage: smoke <music-dir> <db-path>"));
    let db_path = PathBuf::from(args.next().expect("usage: smoke <music-dir> <db-path>"));
    let full = args.next().as_deref() == Some("--full");

    let db = Db::open(&db_path)?;
    let cancel = AtomicBool::new(false);

    let t0 = Instant::now();
    let result = scan(
        &db,
        &ScanOptions {
            root,
            threads: 4,
            full,
        },
        &|p| {
            if p.scanned % 5000 == 0 && !p.done {
                eprintln!(
                    "  ... scanned={} added={} updated={} errors={}",
                    p.scanned, p.added, p.updated, p.errors
                );
            }
        },
        &cancel,
    )?;
    eprintln!(
        "scan done in {:.1?}: scanned={} added={} updated={} removed={} errors={}",
        t0.elapsed(),
        result.scanned,
        result.added,
        result.updated,
        result.removed,
        result.errors
    );

    let stats = db.library_stats()?;
    eprintln!(
        "stats: {} tracks, {} albums, {:.1} hours total; formats={:?}",
        stats.tracks,
        stats.albums,
        stats.total_duration_ms as f64 / 3_600_000.0,
        stats.formats
    );

    for mode in [BrowseMode::Genre, BrowseMode::Artist] {
        let t = Instant::now();
        let values = db.browse_values_exact(mode.clone(), None)?;
        eprintln!(
            "browse {:?}: {} values in {:.1?} (first 5: {:?})",
            mode,
            values.len(),
            t.elapsed(),
            values.iter().take(5).collect::<Vec<_>>()
        );
    }

    for q in [
        "parker",
        "genre=jazz",
        "#(1994 <= year < 2000)",
        "&(artist=hisaishi, #(length < 2:00))",
        "album=/shadow/",
        "!genre=rock",
    ] {
        let t = Instant::now();
        match crescendo_core::search::compile(q) {
            Ok(filter) => {
                let n = db.track_count_exact(Some(&filter), BrowseMode::Artist, &[])?;
                eprintln!("search {q:40} -> {n} tracks in {:.1?}", t.elapsed());
            }
            Err(e) => eprintln!("search {q:40} -> PARSE ERROR: {e}"),
        }
    }

    // Art resolution for the first few albums
    let albums = db.browse_albums_exact(BrowseMode::Album, &[], None)?;
    let cache_dir = db_path.parent().unwrap().join("art-cache");
    let store = ArtStore::new(&cache_dir)?;
    let mut ok = 0;
    let mut fail = 0;
    let t = Instant::now();
    for album in albums.iter().take(20) {
        match store.album_thumb(&db, album.id) {
            Ok(_) => ok += 1,
            Err(_) => fail += 1,
        }
    }
    eprintln!(
        "art: {ok} thumbs generated, {fail} without art, in {:.1?}",
        t.elapsed()
    );

    Ok(())
}
