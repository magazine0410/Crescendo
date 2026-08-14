//! Static lyrics for the Now Playing panel.
//!
//! Resolution order, first hit wins:
//!   1. sidecar `.lrc` next to the audio file (timestamps stripped for now),
//!   2. sidecar `.txt`,
//!   3. embedded `LYRICS`/`UNSYNCEDLYRICS` tag,
//!   4. VocaDB (romanized preferred, then original, then translation),
//!   5. LRCLIB `plainLyrics`.
//!
//! Local sources are read live on every track change — they are single small
//! file opens. Only the online lookups (VocaDB/LRCLIB) are cached, per track,
//! so a rescan or a newly dropped sidecar file is picked up immediately.
//! Online lookups can be switched off with the `lyrics_online` pref.

use crate::db::Db;
use crate::model::Track;
use anyhow::Result;
use lofty::file::TaggedFileExt;
use lofty::prelude::ItemKey;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::sync::LazyLock;

pub(crate) const CACHE_TTL_SECS: i64 = 30 * 24 * 3600;
/// How long a lookup that failed for transport reasons suppresses a retry.
/// Short, because it proves nothing about the track — but not zero, or an
/// offline session stalls on the request timeouts once per track change.
const FAILED_TTL_SECS: i64 = 10 * 60;
/// Cache `source` marking a row as "the lookup failed", as opposed to the
/// empty source of a real "this track has no lyrics" miss.
const FAILED_MARKER: &str = "\u{1f}failed";
const VOCADB_ROOT: &str = "https://vocadb.net/api/";
const LRCLIB_ROOT: &str = "https://lrclib.net/api/";
const USER_AGENT: &str = concat!("Crescendo/", env!("CARGO_PKG_VERSION"), " (lyrics lookup)");

/// `[mm:ss.xx]` line tags and enhanced-LRC `<mm:ss.xx>` word tags.
static LINE_TAG: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\[[0-9]{1,2}:[0-9]{1,2}(?:[.:][0-9]{1,3})?\]").unwrap());
static WORD_TAG: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"<[0-9]{1,2}:[0-9]{1,2}(?:[.:][0-9]{1,3})?>").unwrap());

#[derive(Debug, Clone, Serialize)]
pub struct LyricsResult {
    pub text: String,
    pub source: String,
}

/// Outcome of one online lookup: found lyrics, a definitive "no lyrics for
/// this track" answer, or a transport/server failure that means nothing at
/// all can be concluded. Only `NotFound` may be cached as a miss.
enum OnlineResult {
    Found(LyricsResult),
    NotFound,
    Failed,
}

pub fn for_track(db: &Db, track: &Track) -> Result<Option<LyricsResult>> {
    if let Some((text, ext)) = read_sidecar(&track.path) {
        return Ok(Some(LyricsResult {
            text: if ext == "lrc" { strip_lrc_timestamps(&text) } else { text },
            source: "Sidecar".into(),
        }));
    }
    if let Some(text) = embedded_lyrics(&track.path) {
        return Ok(Some(LyricsResult {
            text,
            source: "Embedded tag".into(),
        }));
    }

    if !db.lyrics_online() {
        return Ok(None);
    }
    if let Some((text, source, age)) = db.lyrics_cache_get(track.id)? {
        if age < cache_ttl(&source) {
            return Ok((!text.is_empty()).then(|| LyricsResult { text, source }));
        }
    }

    let mut any_failed = false;
    let mut found = None;
    for fetch in [vocadb_lyrics, lrclib_lyrics] {
        match fetch(track) {
            OnlineResult::Found(lyrics) => {
                found = Some(lyrics);
                break;
            }
            OnlineResult::NotFound => {}
            OnlineResult::Failed => any_failed = true,
        }
    }
    match found {
        Some(lyrics) => {
            db.lyrics_cache_put(track.id, &lyrics.text, &lyrics.source)?;
            Ok(Some(lyrics))
        }
        // A failed lookup proves nothing, so it must not mark the track as
        // having no lyrics for a month — but it is still worth remembering
        // briefly so an offline session does not pay the request timeouts
        // again on every track change.
        None if any_failed => {
            db.lyrics_cache_put(track.id, "", FAILED_MARKER)?;
            Ok(None)
        }
        None => {
            db.lyrics_cache_put(track.id, "", "")?;
            Ok(None)
        }
    }
}

/// A sidecar lyrics file next to the audio, returned with the extension it
/// was found under. Both the `song.<ext>` and the `song.flac.<ext>` name are
/// tried, `.lrc` before `.txt`, and each is matched case-insensitively.
///
/// Most tracks have no sidecar at all, so the cheap exact-name opens all run
/// first and the case-insensitive fallback is a *single* directory pass over
/// every candidate — this runs on every track change, on an HDD.
fn read_sidecar(audio_path: &str) -> Option<(String, &'static str)> {
    let path = Path::new(audio_path);
    let parent = path.parent()?;
    let file_name = path.file_name()?.to_string_lossy().into_owned();
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());

    // In precedence order: song.lrc, song.flac.lrc, song.txt, song.flac.txt.
    let mut names: Vec<(String, &'static str)> = Vec::with_capacity(4);
    for ext in ["lrc", "txt"] {
        if let Some(stem) = stem.as_deref() {
            names.push((format!("{stem}.{ext}"), ext));
        }
        names.push((format!("{file_name}.{ext}"), ext));
    }

    let mut hits: Vec<Option<std::path::PathBuf>> = names
        .iter()
        .map(|(name, _)| {
            let path = parent.join(name);
            path.is_file().then_some(path)
        })
        .collect();

    // Only a name that outranks the best exact-case hit is still worth
    // looking for, so a plain `song.lrc` costs no directory read at all and
    // the no-sidecar case — by far the most common — costs exactly one.
    let best = hits.iter().position(Option::is_some).unwrap_or(names.len());
    if best > 0 {
        // `song.LRC` and friends, e.g. files copied off a Windows share.
        let wanted: Vec<String> = names[..best].iter().map(|(n, _)| n.to_lowercase()).collect();
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                let lower = entry.file_name().to_string_lossy().to_lowercase();
                if let Some(i) = wanted.iter().position(|w| *w == lower) {
                    hits[i].get_or_insert_with(|| entry.path());
                }
            }
        }
    }

    // Keep the name precedence above rather than directory order.
    hits.into_iter()
        .zip(&names)
        .find_map(|(hit, (_, ext))| Some((read_lyrics_file(&hit?)?, *ext)))
}

/// How long a cache row stays authoritative, given its `source`.
fn cache_ttl(source: &str) -> i64 {
    if source == FAILED_MARKER {
        FAILED_TTL_SECS
    } else {
        CACHE_TTL_SECS
    }
}

fn read_lyrics_file(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn embedded_lyrics(audio_path: &str) -> Option<String> {
    let file = lofty::probe::Probe::open(audio_path)
        .ok()?
        .options(lofty::config::ParseOptions::new().parsing_mode(lofty::config::ParsingMode::Relaxed))
        .read()
        .ok()?;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    if let Some(s) = tag.get_string(&ItemKey::Lyrics) {
        return non_empty(s);
    }
    // foobar2000 writes unsynced lyrics to a Vorbis `UNSYNCEDLYRICS` field,
    // which lofty has no ItemKey for, so it comes back as an unknown key.
    for item in tag.items() {
        if let ItemKey::Unknown(key) = item.key() {
            if key.eq_ignore_ascii_case("UNSYNCEDLYRICS") {
                if let Some(s) = item.value().text() {
                    return non_empty(s);
                }
            }
        }
    }
    None
}

fn non_empty(s: &str) -> Option<String> {
    let trimmed = s.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn vocadb_lyrics(track: &Track) -> OnlineResult {
    let song_id = match find_vocadb_song(track) {
        Ok(Some(id)) => id,
        Ok(None) => return OnlineResult::NotFound,
        Err(()) => return OnlineResult::Failed,
    };
    let details = match vocadb_get(&format!("songs/{song_id}/details"), &[]) {
        Fetch::Ok(v) => v,
        Fetch::NotFound => return OnlineResult::NotFound,
        Fetch::Failed => return OnlineResult::Failed,
    };
    let Some(candidates) = details["lyricsFromParents"].as_array() else {
        return OnlineResult::Failed; // unexpected response shape, not "no lyrics"
    };

    // Prefer romanized, then the original, then a translation (usually English).
    let best = ["Romanized", "Original", "Translation"]
        .iter()
        .find_map(|ty| candidates.iter().find(|l| l["translationType"].as_str() == Some(ty)));
    let Some(best) = best else {
        return OnlineResult::NotFound;
    };
    let Some(lyrics_id) = best["id"].as_i64() else {
        return OnlineResult::Failed;
    };

    let full = match vocadb_get(&format!("songs/lyrics/{lyrics_id}"), &[]) {
        Fetch::Ok(v) => v,
        Fetch::NotFound => return OnlineResult::NotFound,
        Fetch::Failed => return OnlineResult::Failed,
    };
    let Some(text) = full["value"].as_str().map(str::trim) else {
        return OnlineResult::Failed;
    };
    if text.is_empty() {
        return OnlineResult::NotFound;
    }
    OnlineResult::Found(LyricsResult {
        text: text.to_string(),
        source: format!("VocaDB · {}", best["translationType"].as_str().unwrap_or("lyrics")),
    })
}

fn find_vocadb_song(track: &Track) -> Result<Option<i64>, ()> {
    let duration = track.duration_ms / 1000;
    let mut queries: Vec<&str> = Vec::with_capacity(2);
    if let Some(t) = track.title.as_deref() {
        queries.push(t);
    }
    if let Some(t) = track.titlesort.as_deref() {
        if !queries.contains(&t) {
            queries.push(t);
        }
    }

    for query in queries {
        let json = match vocadb_get(
            "songs",
            &[
                ("query", query),
                ("nameMatchMode", "Auto"),
                ("maxResults", "20"),
                ("getTotalCount", "false"),
                ("lang", "Default"),
            ],
        ) {
            Fetch::Ok(v) => v,
            Fetch::NotFound => continue,
            Fetch::Failed => return Err(()),
        };
        let Some(items) = json["items"].as_array() else {
            continue;
        };

        let mut best: Option<(i64, i64, bool)> = None; // (id, |Δlen|, has known length)
        for item in items {
            let artist = item["artistString"].as_str().unwrap_or("");
            if !artist_string_matches(track, artist) {
                continue;
            }
            let len = item["lengthSeconds"].as_i64().unwrap_or(0);
            let has_len = len > 0;
            let delta = if has_len { (len - duration).abs() } else { i64::MAX };
            let better = match best {
                None => true,
                Some((_, best_delta, best_has_len)) => {
                    (has_len && !best_has_len) || (has_len == best_has_len && delta < best_delta)
                }
            };
            if better {
                if let Some(id) = item["id"].as_i64() {
                    best = Some((id, delta, has_len));
                }
            }
        }
        if let Some((id, _, _)) = best {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

fn lrclib_lyrics(track: &Track) -> OnlineResult {
    let (Some(title), Some(artist)) = (track.title.as_deref(), track.artist.as_deref()) else {
        return OnlineResult::NotFound;
    };
    let req = ureq::get(&format!("{LRCLIB_ROOT}get"))
        .query("track_name", title)
        .query("artist_name", artist)
        .query("album_name", track.album.as_deref().unwrap_or(""))
        .query("duration", &(track.duration_ms / 1000).to_string())
        .set("User-Agent", USER_AGENT)
        .timeout(std::time::Duration::from_secs(15));
    let json: Value = match req.call() {
        Ok(r) => match r.into_json() {
            Ok(v) => v,
            Err(e) => {
                log::warn!("LRCLIB returned unreadable JSON: {e}");
                return OnlineResult::Failed;
            }
        },
        Err(ureq::Error::Status(404, _)) => return OnlineResult::NotFound,
        Err(e) => {
            log::warn!("LRCLIB request failed: {e}");
            return OnlineResult::Failed;
        }
    };
    if json["instrumental"].as_bool() == Some(true) {
        return OnlineResult::NotFound;
    }
    let Some(text) = json["plainLyrics"]
        .as_str()
        .or_else(|| json["syncedLyrics"].as_str())
    else {
        return OnlineResult::NotFound;
    };
    let text = strip_lrc_timestamps(text.trim());
    if text.is_empty() {
        return OnlineResult::NotFound;
    }
    OnlineResult::Found(LyricsResult {
        text,
        source: "LRCLIB".into(),
    })
}

enum Fetch {
    Ok(Value),
    NotFound,
    Failed,
}

fn vocadb_get(path: &str, query: &[(&str, &str)]) -> Fetch {
    let mut req = ureq::get(&format!("{VOCADB_ROOT}{path}"))
        .set("User-Agent", USER_AGENT)
        .timeout(std::time::Duration::from_secs(15));
    for (k, v) in query {
        req = req.query(k, v);
    }
    match req.call() {
        Ok(r) => match r.into_json() {
            Ok(v) => Fetch::Ok(v),
            Err(e) => {
                log::warn!("VocaDB returned unreadable JSON ({path}): {e}");
                Fetch::Failed
            }
        },
        Err(ureq::Error::Status(404, _)) => Fetch::NotFound,
        Err(e) => {
            log::warn!("VocaDB request failed ({path}): {e}");
            Fetch::Failed
        }
    }
}

fn artist_string_matches(track: &Track, artist_string: &str) -> bool {
    let hay = norm(artist_string);
    if hay.is_empty() {
        return false;
    }
    [
        track.artist.as_deref(),
        track.albumartist.as_deref(),
        track.artistsort.as_deref(),
        track.albumartistsort.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(norm)
    .any(|needle| !needle.is_empty() && artist_name_matches(&hay, &needle))
}

/// `needle` matches `hay` when the two are equal, or when every token of the
/// shorter name appears in the longer one. Comparing whole tokens keeps short
/// names ("IA", "nao") from matching as substrings of unrelated artist
/// strings, while still pairing a circle name with the VocaDB `artistString`
/// that appends its vocalist ("Alstroemeria Records" ⊂ "Alstroemeria Records
/// feat. nomico").
fn artist_name_matches(hay: &str, needle: &str) -> bool {
    if hay == needle {
        return true;
    }
    let hay_tokens = tokens(hay);
    let needle_tokens = tokens(needle);
    if hay_tokens.is_empty() || needle_tokens.is_empty() {
        return false;
    }
    let (short, long) = if needle_tokens.len() <= hay_tokens.len() {
        (&needle_tokens, &hay_tokens)
    } else {
        (&hay_tokens, &needle_tokens)
    };
    short.iter().all(|t| long.contains(t))
}

/// Words that only join two artists together, and so must never carry a match
/// on their own.
const JOINERS: [&str; 5] = ["feat", "ft", "featuring", "vs", "with"];

fn tokens(s: &str) -> Vec<&str> {
    // No '-', '/', "'" or '.': hyphenated, slash and dotted names (Kurousa-P,
    // DJ'TEKINA//SOMETHING, M.I.A) must stay a single token so they match an
    // artistString that uses them whole. A trailing '.' is trimmed instead, so
    // "feat." still reduces to the joiner "feat".
    s.split([' ', ',', '&', '(', ')', ';', '·', '、', '・', '：'])
        .map(|t| t.trim_end_matches('.'))
        .filter(|t| !t.is_empty() && !JOINERS.contains(t))
        .collect()
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Strip LRC timestamps so a synced file can render as plain text: leading
/// `[mm:ss.xx]` line tags and enhanced-LRC `<mm:ss.xx>` word tags are dropped
/// and the remaining text keeps its line breaks.
fn strip_lrc_timestamps(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.lines() {
        let line = WORD_TAG.replace_all(line, "");
        let line = LINE_TAG.replace_all(&line, "");
        let line = line.trim();
        if !line.is_empty() {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        artist_name_matches, artist_string_matches, cache_ttl, norm, read_sidecar,
        strip_lrc_timestamps, CACHE_TTL_SECS, FAILED_MARKER, FAILED_TTL_SECS,
    };
    use crate::model::Track;

    fn track_with_artist(artist: &str) -> Track {
        Track {
            id: 1,
            path: "/music/01.flac".into(),
            title: None,
            artist: Some(artist.into()),
            album: None,
            albumartist: None,
            titlesort: None,
            artistsort: None,
            albumartistsort: None,
            genre: None,
            grouping: None,
            track_no: None,
            disc_no: None,
            year: None,
            duration_ms: 0,
            format: "FLAC".into(),
            album_id: None,
            rating: None,
            play_count: 0,
            missing: false,
        }
    }

    #[test]
    fn strips_basic_lrc_timestamps() {
        let lrc = "[00:12.34] first line\n[01:02.03]second line\nplain third";
        assert_eq!(
            strip_lrc_timestamps(lrc),
            "first line\nsecond line\nplain third"
        );
    }

    #[test]
    fn strips_enhanced_word_timestamps() {
        let lrc = "[00:12.34] hel<00:12.34>lo wor<00:13.00>ld";
        assert_eq!(strip_lrc_timestamps(lrc), "hello world");
    }

    #[test]
    fn drops_blank_and_tag_only_lines() {
        let lrc = "[00:12.34] keep\n[00:15.00]\n\n[00:20.00] next";
        assert_eq!(strip_lrc_timestamps(lrc), "keep\nnext");
    }

    #[test]
    fn artist_match_whole_string_or_token() {
        // "黒うさP" appears as a token inside the VocaDB artist string.
        assert!(artist_string_matches(&track_with_artist("黒うさP"), "黒うさP feat. 初音ミク"));
        // ...and the other way around.
        assert!(artist_string_matches(&track_with_artist("黒うさP feat. 初音ミク"), "黒うさP"));
        assert!(artist_string_matches(&track_with_artist("nao"), "nao feat. 初音ミク"));
    }

    #[test]
    fn artist_match_short_names_do_not_match_substrings() {
        // "IA" must not match "asialand"; "nao" must not match "asialand" either.
        assert!(!artist_string_matches(&track_with_artist("IA"), "asialand feat. 初音ミク"));
        assert!(!artist_string_matches(&track_with_artist("nao"), "asialand feat. 初音ミク"));
        // A real token still matches a short name.
        assert!(artist_string_matches(&track_with_artist("IA"), "IA feat. 初音ミク"));
    }

    #[test]
    fn artist_match_multi_token_circle_names() {
        // The common doujin shape: the tag holds the circle, VocaDB's
        // artistString appends the vocalist.
        assert!(artist_string_matches(
            &track_with_artist("Alstroemeria Records"),
            "Alstroemeria Records feat. nomico"
        ));
        assert!(artist_string_matches(
            &track_with_artist("Yuuhei Satellite feat. senya"),
            "Yuuhei Satellite"
        ));
        // A shared word is not a shared name.
        assert!(!artist_string_matches(
            &track_with_artist("Amateras Records"),
            "Alstroemeria Records feat. nomico"
        ));
    }

    #[test]
    fn artist_match_ignores_joiner_words() {
        // "feat" alone must not pair two unrelated artists.
        assert!(!artist_string_matches(
            &track_with_artist("SOUND HOLIC feat. Nana Takahashi"),
            "ZYTOKINE feat. 隣人"
        ));
        // Hyphenated and dotted names stay whole.
        assert!(artist_name_matches("kurousa-p feat. miku", "kurousa-p"));
        assert!(!artist_name_matches("asia feat. miku", "m.i.a"));
    }

    #[test]
    fn norm_lowercases_and_collapses_whitespace() {
        assert_eq!(norm("  黒うさP   feat.   Miku "), "黒うさp feat. miku");
    }

    #[test]
    fn failed_lookups_expire_far_sooner_than_real_misses() {
        // A real "no lyrics exist" answer, and a hit, both hold for a month.
        assert_eq!(cache_ttl(""), CACHE_TTL_SECS);
        assert_eq!(cache_ttl("LRCLIB"), CACHE_TTL_SECS);
        // A failed request proves nothing, so it only suppresses the retry
        // long enough to keep an offline session off the request timeouts.
        assert_eq!(cache_ttl(FAILED_MARKER), FAILED_TTL_SECS);
        assert!(FAILED_TTL_SECS < CACHE_TTL_SECS);
    }

    #[test]
    fn sidecar_precedence_and_case_insensitivity() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("01 Song.flac");
        std::fs::write(&audio, b"").unwrap();
        let audio = audio.to_string_lossy().into_owned();

        // Nothing next to the track.
        assert!(read_sidecar(&audio).is_none());

        // A `.txt` is used when no `.lrc` exists...
        std::fs::write(dir.path().join("01 Song.txt"), "from txt").unwrap();
        assert_eq!(read_sidecar(&audio), Some(("from txt".into(), "txt")));

        // ...and loses to a `.lrc` found only by a case-insensitive match,
        // whose timestamps for_track then strips.
        std::fs::write(dir.path().join("01 Song.LRC"), "[00:01.00] from lrc").unwrap();
        assert_eq!(
            read_sidecar(&audio),
            Some(("[00:01.00] from lrc".into(), "lrc"))
        );

        // The exact `song.lrc` name still wins over the `song.flac.lrc` form.
        std::fs::write(dir.path().join("01 Song.flac.lrc"), "from double").unwrap();
        std::fs::write(dir.path().join("01 Song.lrc"), "from exact").unwrap();
        assert_eq!(read_sidecar(&audio), Some(("from exact".into(), "lrc")));
    }
}
