//! Verify m3u import + thumbnail generation at scale against the real DB.
//! cargo run -p crescendo-core --example m3u_art_test -- <db> <m3u> <cache-dir>

use crescendo_core::art::ArtStore;
use crescendo_core::db::Db;
use crescendo_core::model::BrowseMode;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let db_path = args.next().expect("db path");
    let m3u = args.next().expect("m3u path");
    let cache = args.next().expect("cache dir");
    let db = Db::open(std::path::Path::new(&db_path))?;

    // --- m3u import ---
    let r = crescendo_core::m3u::import(&db, std::path::Path::new(&m3u))?;
    println!(
        "m3u import: playlist '{}' id={} added={} missing={}",
        r.name,
        r.playlist_id,
        r.added,
        r.missing.len()
    );
    for m in &r.missing {
        println!("  missing: {m}");
    }
    let tracks = db.playlist_tracks(r.playlist_id)?;
    println!("  playlist_tracks returns {} rows; first: {:?} - {:?}",
        tracks.len(),
        tracks.first().and_then(|t| t.artist.clone()),
        tracks.first().and_then(|t| t.title.clone()));
    // context resolution for playback
    let ids = db.resolve_context_exact(&crescendo_core::model::PlayContext::Playlist { id: r.playlist_id })?;
    println!("  resolve_context_exact: {} ids", ids.len());
    // clean up the test playlist
    db.playlist_delete(r.playlist_id)?;
    println!("  deleted test playlist");

    // --- thumbnails at scale: first 100 albums ---
    let albums = db.browse_albums_exact(BrowseMode::Album, &[], None)?;
    let store = ArtStore::new(std::path::Path::new(&cache))?;
    let n = albums.len().min(100);
    let t0 = Instant::now();
    let mut ok = 0;
    let mut fail = 0;
    for a in albums.iter().take(n) {
        match store.album_thumb(&db, a.id) {
            Ok(_) => ok += 1,
            Err(_) => fail += 1,
        }
    }
    println!(
        "thumbs: {ok} generated, {fail} without art, {n} albums in {:.1?} ({:.0} ms/album)",
        t0.elapsed(),
        t0.elapsed().as_millis() as f64 / n as f64
    );
    Ok(())
}
