//! Asks the profile sites about artists, as the Now Playing tab does, and
//! prints what would be shown. Answers are cached in the given database, so
//! use a scratch one:
//!
//!   cargo run -p crescendo-core --example profile_test -- <db-path> \
//!       "<name>[|<album>[|<album artist>]]"...
//!
//! Discogs is asked only when DISCOGS_TOKEN holds a personal access token.
use crescendo_core::artistinfo::profile::{profile, Hints, Sites};
use crescendo_core::db::Db;

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let mut args = std::env::args().skip(1);
    let db = Db::open(std::path::Path::new(&args.next().expect("a database path")))?;
    let sites = Sites {
        discogs_token: std::env::var("DISCOGS_TOKEN").ok(),
        ..Default::default()
    };
    for arg in args {
        let mut parts = arg.split('|');
        let name = parts.next().unwrap_or_default();
        let album = parts.next().filter(|s| !s.is_empty());
        let album_artist = parts.next().filter(|s| !s.is_empty());
        let hints = Hints {
            album,
            album_artist,
            trusted: name.chars().count() >= 5,
            ..Default::default()
        };
        let started = std::time::Instant::now();
        match profile(&db, &sites, name, hints) {
            Some(found) => println!(
                "{name} [{album:?}] -> {:?} {} ({:.1}s)\n    {}\n",
                found.source,
                found.url,
                started.elapsed().as_secs_f32(),
                found
                    .text
                    .chars()
                    .take(160)
                    .collect::<String>()
                    .replace('\n', " / ")
            ),
            None => println!(
                "{name} [{album:?}] -> nothing ({:.1}s)\n",
                started.elapsed().as_secs_f32()
            ),
        }
    }
    Ok(())
}
