//! Lyrics for the Now Playing panel, synced to the track where the source
//! carries timings.
//!
//! Resolution order, first hit wins:
//!   1. sidecar `.lrc` next to the audio file,
//!   2. sidecar `.txt`,
//!   3. embedded `LYRICS`/`UNSYNCEDLYRICS` tag,
//!   4. TouhouDB, then VocaDB, then UtaiteDB — one client for all three, each
//!      preferring romanized, then the original, then a translation,
//!   5. LyricsTouHou (THBWiki), which serves synced LRC keyed on the title,
//!   6. LRCLIB (`syncedLyrics`, falling back to `plainLyrics`).
//!
//! Every source runs through the same LRC parser, so what decides whether the
//! lyrics are synced is the text itself, never the extension it arrived under:
//! a `.txt` holding timestamps is synced, an `.lrc` without them is not. The
//! result always carries a plain-text rendering; `lines` is non-empty only
//! when there were timestamps to keep.
//!
//! Local sources are read live on every track change — they are single small
//! file opens. Only the online lookups (VocaDB/LRCLIB) are cached, per track,
//! and cached *raw* so the timing survives; a rescan or a newly dropped
//! sidecar file is picked up immediately. Online lookups can be switched off
//! with the `lyrics_online` pref.

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
/// The VocaDB-family databases, in the order they are asked. All three run
/// the same software behind the same API, so one client covers them: only the
/// host and the name that lands in `source` differ. TouhouDB leads because
/// this library is doujin-heavy, then VocaDB, then UtaiteDB for covers.
const VOCADB_SITES: [(&str, &str); 3] = [
    ("https://touhoudb.com/api/", "TouhouDB"),
    ("https://vocadb.net/api/", "VocaDB"),
    ("https://utaitedb.net/api/", "UtaiteDB"),
];
/// LyricsTouHou (THBWiki), asked after the VocaDB family. Touhou-only, and
/// synced: entries are LRC files served straight off the title, `<title>.lrc`.
const THWIKI_ROOT: &str = "https://lyrics.thwiki.cc/";
const LRCLIB_ROOT: &str = "https://lrclib.net/api/";
/// How far a candidate's length may sit from the track's before it is treated
/// as a different recording rather than the same one. The API caps a search
/// at 100 results and we ask for 50, so on a title like "Bad Apple!!" — which
/// has hundreds of entries — this gate is what stops the closest of a bad
/// window being accepted.
const MAX_LENGTH_DELTA_SECS: i64 = 20;
const USER_AGENT: &str = concat!("Crescendo/", env!("CARGO_PKG_VERSION"), " (lyrics lookup)");

/// A leading `[mm:ss.xx]` line tag. Minutes are not capped at two digits:
/// long tracks do run past `[99:59]`.
static LINE_TAG: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\[([0-9]{1,3}):([0-9]{1,2})(?:[.:]([0-9]{1,3}))?\]").unwrap());
/// A leading `[ar:…]`-style ID tag. The letters are what separates it from a
/// timestamp, and the colon from a lyric line that opens with "[Chorus]".
static ID_TAG: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\[([a-zA-Z#]+):([^\]]*)\]").unwrap());
/// Enhanced-LRC `<mm:ss.xx>` word tags, anywhere in the line.
static WORD_TAG: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"<[0-9]{1,3}:[0-9]{1,2}(?:[.:][0-9]{1,3})?>").unwrap());

/// One timed line of an LRC file.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LyricLine {
    /// Milliseconds from the start of the track.
    pub ms: i64,
    /// The line itself, with any enhanced-LRC word timings removed. Empty for
    /// the blank lines an LRC file uses to mark an instrumental gap.
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LyricsResult {
    /// The lyrics as plain text — all a source without timestamps carries,
    /// and the fallback rendering for one that has them.
    pub text: String,
    /// Timed lines in time order, empty when the source had no timestamps.
    pub lines: Vec<LyricLine>,
    pub source: String,
}

/// Outcome of one online lookup: the raw lyrics as the service returned them
/// (LRC included, so the timing survives into the cache), a definitive "no
/// lyrics for this track" answer, or a transport/server failure that means
/// nothing at all can be concluded. Only `NotFound` may be cached as a miss.
enum OnlineResult {
    Found { raw: String, source: String },
    NotFound,
    Failed,
}

pub fn for_track(db: &Db, track: &Track) -> Result<Option<LyricsResult>> {
    // Every local source goes through the LRC parser: whether the lyrics are
    // synced is decided by what the text contains, not by where it came from
    // or what the file is called.
    if let Some(raw) = read_sidecar(&track.path) {
        if let Some(result) = parse_lyrics(&raw, "Sidecar") {
            return Ok(Some(result));
        }
    }
    if let Some(raw) = embedded_lyrics(&track.path) {
        if let Some(result) = parse_lyrics(&raw, "Embedded tag") {
            return Ok(Some(result));
        }
    }

    if !db.lyrics_online() {
        return Ok(None);
    }
    if let Some((raw, source, age)) = db.lyrics_cache_get(track.id)? {
        if age < cache_ttl(&source) {
            return Ok(parse_lyrics(&raw, &source));
        }
    }

    let mut any_failed = false;
    let mut found = None;
    for (root, name) in VOCADB_SITES {
        if take(vocadb_lyrics(track, root, name), &mut found, &mut any_failed) {
            break;
        }
    }
    if found.is_none() {
        take(thwiki_lyrics(track), &mut found, &mut any_failed);
    }
    if found.is_none() {
        take(lrclib_lyrics(track), &mut found, &mut any_failed);
    }
    match found {
        Some((raw, source)) => {
            db.lyrics_cache_put(track.id, &raw, &source)?;
            Ok(parse_lyrics(&raw, &source))
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

/// Fold one lookup into the running outcome. Returns true once something has
/// been found, so the remaining sources can be skipped; a failure is only
/// remembered, because another source may still answer.
fn take(result: OnlineResult, found: &mut Option<(String, String)>, any_failed: &mut bool) -> bool {
    match result {
        OnlineResult::Found { raw, source } => {
            *found = Some((raw, source));
            true
        }
        OnlineResult::NotFound => false,
        OnlineResult::Failed => {
            *any_failed = true;
            false
        }
    }
}

/// A sidecar lyrics file next to the audio. Both the `song.<ext>` and the
/// `song.flac.<ext>` name are tried, `.lrc` before `.txt`, and each is
/// matched case-insensitively. What comes back is the file as written —
/// whether it is synced is for the parser to decide.
///
/// Most tracks have no sidecar at all, so the cheap exact-name opens all run
/// first and the case-insensitive fallback is a *single* directory pass over
/// every candidate — this runs on every track change, on an HDD.
fn read_sidecar(audio_path: &str) -> Option<String> {
    let path = Path::new(audio_path);
    let parent = path.parent()?;
    let file_name = path.file_name()?.to_string_lossy().into_owned();
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());

    // In precedence order: song.lrc, song.flac.lrc, song.txt, song.flac.txt.
    let mut names: Vec<String> = Vec::with_capacity(4);
    for ext in ["lrc", "txt"] {
        if let Some(stem) = stem.as_deref() {
            names.push(format!("{stem}.{ext}"));
        }
        names.push(format!("{file_name}.{ext}"));
    }

    let mut hits: Vec<Option<std::path::PathBuf>> = names
        .iter()
        .map(|name| {
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
        let wanted: Vec<String> = names[..best].iter().map(|n| n.to_lowercase()).collect();
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
    hits.into_iter().find_map(|hit| read_lyrics_file(&hit?))
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

/// One VocaDB-family database. `root` is its API base, `name` the label that
/// ends up in the result's `source`.
fn vocadb_lyrics(track: &Track, root: &str, name: &str) -> OnlineResult {
    let song_id = match find_vocadb_song(track, root) {
        Ok(Some(id)) => id,
        Ok(None) => return OnlineResult::NotFound,
        Err(()) => return OnlineResult::Failed,
    };
    let details = match vocadb_get(root, &format!("songs/{song_id}/details"), &[]) {
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

    let full = match vocadb_get(root, &format!("songs/lyrics/{lyrics_id}"), &[]) {
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
    OnlineResult::Found {
        raw: text.to_string(),
        source: format!("{name} · {}", best["translationType"].as_str().unwrap_or("lyrics")),
    }
}

fn find_vocadb_song(track: &Track, root: &str) -> Result<Option<i64>, ()> {
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
            root,
            "songs",
            &[
                ("query", query),
                ("nameMatchMode", "Auto"),
                ("maxResults", "50"),
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
            // A title this common is shared by hundreds of arrangements, so a
            // length that far off is a different recording, not a near miss.
            // Only applies when both lengths are known — an entry with no
            // length recorded is not evidence of a mismatch, just weaker.
            if has_len && duration > 0 && delta >= MAX_LENGTH_DELTA_SECS {
                continue;
            }
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

/// LyricsTouHou keys entries on the title alone — there is no artist
/// parameter to narrow a search with. The entry it serves is therefore
/// verified after the fact against the `[ti:]` and `[ar:]` tags the file
/// carries, so a title several circles have used cannot quietly return the
/// wrong circle's words.
fn thwiki_lyrics(track: &Track) -> OnlineResult {
    let mut queries: Vec<&str> = Vec::with_capacity(2);
    if let Some(t) = track.title.as_deref() {
        queries.push(t);
    }
    if let Some(t) = track.titlesort.as_deref() {
        if !queries.contains(&t) {
            queries.push(t);
        }
    }

    let mut any_failed = false;
    for query in queries {
        let url = format!("{THWIKI_ROOT}{}.lrc", percent_encode(query));
        let raw = match ureq::get(&url)
            .set("User-Agent", USER_AGENT)
            .timeout(std::time::Duration::from_secs(15))
            .call()
        {
            Ok(r) => match r.into_string() {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("LyricsTouHou returned unreadable text ({query}): {e}");
                    any_failed = true;
                    continue;
                }
            },
            Err(ureq::Error::Status(404, _)) => continue,
            Err(e) => {
                log::warn!("LyricsTouHou request failed ({query}): {e}");
                any_failed = true;
                continue;
            }
        };

        if !thwiki_entry_matches(track, &raw) {
            continue;
        }
        let raw = strip_thwiki_translation(&raw);
        if raw.trim().is_empty() {
            continue;
        }
        return OnlineResult::Found { raw, source: "LyricsTouHou".into() };
    }
    if any_failed {
        OnlineResult::Failed
    } else {
        OnlineResult::NotFound
    }
}

/// The title and circle the served file names must both line up with the
/// track. `artist_name_matches` does the comparing for either: it is the same
/// "one name is a whole-token subset of the other" test, which lets a bare
/// "Bad Apple!!" match an entry titled "Bad Apple!! feat. nomico" while
/// refusing an unrelated song from the same album.
fn thwiki_entry_matches(track: &Track, raw: &str) -> bool {
    let Some(served_title) = lrc_id_tag(raw, "ti") else {
        return false;
    };
    let title_ok = [track.title.as_deref(), track.titlesort.as_deref()]
        .into_iter()
        .flatten()
        .any(|t| artist_name_matches(&norm(&served_title), &norm(t)));
    if !title_ok {
        return false;
    }
    // Not every entry names a circle. The database is Touhou-only and the
    // title already lined up, so a missing `[ar:]` is not cause to reject.
    match lrc_id_tag(raw, "ar") {
        Some(artist) => artist_string_matches(track, &artist),
        None => true,
    }
}

/// The value of an `[xx:…]` ID tag, if the file carries one.
fn lrc_id_tag(raw: &str, key: &str) -> Option<String> {
    for line in raw.lines() {
        if let Some(c) = ID_TAG.captures(line.trim_start()) {
            if c[1].eq_ignore_ascii_case(key) {
                let value = c[2].trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// LyricsTouHou puts the original and a Chinese translation on one line,
/// split by " // ". Keep the original half — the pane is one column, and the
/// rest of the pipeline has no concept of a second language. Entries without
/// a translation carry no separator and pass through untouched.
fn strip_thwiki_translation(raw: &str) -> String {
    raw.lines()
        .map(|line| {
            // Only timed lines carry translations. Leaving other lines alone
            // keeps the "//" inside a `[re:https://…]` URL out of it.
            if LINE_TAG.is_match(line.trim_start()) {
                line.split(" // ").next().unwrap_or(line).trim_end()
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Percent-encode a title for use as a URL path segment. Titles here are
/// Japanese far more often than not, so almost every byte needs escaping.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
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
    // Synced first: the timing is the point, and the parser derives the plain
    // rendering from it anyway.
    let Some(text) = json["syncedLyrics"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| json["plainLyrics"].as_str())
    else {
        return OnlineResult::NotFound;
    };
    let text = text.trim();
    if text.is_empty() {
        return OnlineResult::NotFound;
    }
    OnlineResult::Found {
        raw: text.to_string(),
        source: "LRCLIB".into(),
    }
}

enum Fetch {
    Ok(Value),
    NotFound,
    Failed,
}

fn vocadb_get(root: &str, path: &str, query: &[(&str, &str)]) -> Fetch {
    let mut req = ureq::get(&format!("{root}{path}"))
        .set("User-Agent", USER_AGENT)
        .timeout(std::time::Duration::from_secs(15));
    for (k, v) in query {
        req = req.query(k, v);
    }
    match req.call() {
        Ok(r) => match r.into_json() {
            Ok(v) => Fetch::Ok(v),
            Err(e) => {
                log::warn!("{root}{path} returned unreadable JSON: {e}");
                Fetch::Failed
            }
        },
        Err(ureq::Error::Status(404, _)) => Fetch::NotFound,
        Err(e) => {
            log::warn!("{root}{path} request failed: {e}");
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

/// Parse lyrics into a plain-text rendering plus, when the text carries
/// timestamps, the timed lines behind it. Text with no timestamps parses to
/// itself and no lines, so plain lyrics need no separate path.
fn parse_lyrics(raw: &str, source: &str) -> Option<LyricsResult> {
    let (text, lines) = parse_lrc(raw);
    (!text.is_empty()).then(|| LyricsResult { text, lines, source: source.to_string() })
}

/// The LRC format: `[mm:ss.xx]` line tags, `[ar:…]`-style ID tags, enhanced
/// `<mm:ss.xx>` word tags, and a line repeated at several times by carrying
/// several tags at once.
fn parse_lrc(s: &str) -> (String, Vec<LyricLine>) {
    let mut lines: Vec<LyricLine> = Vec::new();
    let mut plain = String::with_capacity(s.len());
    let mut offset_ms = 0i64;

    for raw in s.lines() {
        let mut rest = raw.trim_start();
        let mut stamps: Vec<i64> = Vec::new();
        loop {
            if let Some(end) = LINE_TAG.captures(rest).map(|c| {
                let min: i64 = c[1].parse().unwrap_or(0);
                let sec: i64 = c[2].parse().unwrap_or(0);
                // Two fraction digits are centiseconds, three milliseconds,
                // one tenths — all three spellings are in the wild.
                let frac = c.get(3).map_or(0, |f| {
                    let n: i64 = f.as_str().parse().unwrap_or(0);
                    match f.as_str().len() {
                        1 => n * 100,
                        2 => n * 10,
                        _ => n,
                    }
                });
                stamps.push(min * 60_000 + sec * 1000 + frac);
                c[0].len()
            }) {
                rest = &rest[end..];
                continue;
            }
            // ID tags only lead a line, never trail a timestamp.
            if stamps.is_empty() {
                if let Some(end) = ID_TAG.captures(rest).map(|c| {
                    if c[1].eq_ignore_ascii_case("offset") {
                        offset_ms = c[2].trim().parse().unwrap_or(0);
                    }
                    c[0].len()
                }) {
                    rest = &rest[end..];
                    continue;
                }
            }
            break;
        }

        let text = WORD_TAG.replace_all(rest, "");
        let text = text.trim();
        for ms in stamps {
            lines.push(LyricLine { ms, text: text.to_string() });
        }
        if !text.is_empty() {
            plain.push_str(text);
            plain.push('\n');
        }
    }

    // `[offset:+n]` means the lyrics run n ms ahead of the audio, so the times
    // move back by it.
    if offset_ms != 0 {
        for line in &mut lines {
            line.ms = (line.ms - offset_ms).max(0);
        }
    }
    lines.sort_by_key(|l| l.ms);
    (plain.trim_end().to_string(), lines)
}

#[cfg(test)]
mod tests {
    use super::{
        artist_name_matches, artist_string_matches, cache_ttl, lrc_id_tag, norm, parse_lrc,
        percent_encode, read_sidecar, strip_thwiki_translation, thwiki_entry_matches,
        CACHE_TTL_SECS, FAILED_MARKER, FAILED_TTL_SECS, VOCADB_SITES,
    };
    use crate::model::Track;

    /// The plain-text half of the parse, which is what a source without
    /// timestamps renders as.
    fn plain(s: &str) -> String {
        parse_lrc(s).0
    }

    /// The timed half, as `(ms, text)` pairs.
    fn timed(s: &str) -> Vec<(i64, String)> {
        parse_lrc(s).1.into_iter().map(|l| (l.ms, l.text)).collect()
    }

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
    fn parses_timestamps_into_timed_lines() {
        let lrc = "[00:12.34] first line\n[01:02.03]second line";
        assert_eq!(
            timed(lrc),
            [(12_340, "first line".into()), (62_030, "second line".into())]
        );
        assert_eq!(plain(lrc), "first line\nsecond line");
    }

    #[test]
    fn fraction_digits_are_tenths_centis_or_millis() {
        assert_eq!(timed("[00:01.5]a")[0].0, 1_500);
        assert_eq!(timed("[00:01.05]a")[0].0, 1_050);
        assert_eq!(timed("[00:01.005]a")[0].0, 1_005);
        // Some writers separate the fraction with a colon, and long tracks
        // run past [99:59].
        assert_eq!(timed("[00:01:50]a")[0].0, 1_500);
        assert_eq!(timed("[100:00.00]a")[0].0, 6_000_000);
        // No fraction at all is still a timestamp.
        assert_eq!(timed("[02:03]a")[0].0, 123_000);
    }

    #[test]
    fn text_with_no_timestamps_is_plain() {
        let txt = "first line\nsecond line";
        assert!(timed(txt).is_empty());
        assert_eq!(plain(txt), txt);
    }

    #[test]
    fn id_tags_are_dropped_from_both_halves() {
        let lrc = "[ti:Title]\n[ar:Artist]\n[length:03:24]\n[00:12.34]keep";
        assert_eq!(plain(lrc), "keep");
        assert_eq!(timed(lrc), [(12_340, "keep".into())]);
    }

    #[test]
    fn a_bracketed_lyric_line_is_not_an_id_tag() {
        // No colon, so "[Chorus]" is text, not metadata.
        assert_eq!(plain("[Chorus]\n[00:12.34]la"), "[Chorus]\nla");
    }

    #[test]
    fn offset_shifts_every_line_and_never_below_zero() {
        assert_eq!(timed("[offset:+500]\n[00:12.34]a")[0].0, 11_840);
        assert_eq!(timed("[offset:-500]\n[00:12.34]a")[0].0, 12_840);
        assert_eq!(timed("[offset:+5000]\n[00:01.00]a")[0].0, 0);
    }

    #[test]
    fn one_line_can_carry_several_timestamps() {
        // The chorus, sung three times.
        assert_eq!(
            timed("[00:20.00][01:40.00][02:30.00]chorus"),
            [
                (20_000, "chorus".into()),
                (100_000, "chorus".into()),
                (150_000, "chorus".into())
            ]
        );
        // ...but it reads once in the plain rendering.
        assert_eq!(plain("[00:20.00][01:40.00]chorus"), "chorus");
    }

    #[test]
    fn lines_come_back_in_time_order() {
        assert_eq!(
            timed("[00:30.00]third\n[00:10.00]first\n[00:20.00]second"),
            [
                (10_000, "first".into()),
                (20_000, "second".into()),
                (30_000, "third".into())
            ]
        );
    }

    #[test]
    fn strips_enhanced_word_timestamps() {
        let lrc = "[00:12.34] hel<00:12.34>lo wor<00:13.00>ld";
        assert_eq!(timed(lrc), [(12_340, "hello world".into())]);
        assert_eq!(plain(lrc), "hello world");
    }

    #[test]
    fn blank_lines_stay_timed_but_leave_the_plain_text() {
        // An empty timed line is an instrumental gap: worth keeping in the
        // synced view so the highlight moves off the last line, worth
        // dropping from a plain block of text.
        let lrc = "[00:12.34]keep\n[00:15.00]\n\n[00:20.00]next";
        assert_eq!(
            timed(lrc),
            [
                (12_340, "keep".into()),
                (15_000, String::new()),
                (20_000, "next".into())
            ]
        );
        assert_eq!(plain(lrc), "keep\nnext");
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

    /// A LyricsTouHou file, shortened but shaped exactly like the real thing.
    fn thwiki_file() -> String {
        [
            "[ti:Bad Apple!! feat. nomico]",
            "[al:Lovelight]",
            "[ar:Alstroemeria Records]",
            "[re:LyricsTouHou: https://lyrics.thwiki.cc/Bad_Apple!!.1.all.lrc]",
            "[00:57.02]流れてく 時の中ででも // 即便身处流逝的时光之中",
            "[01:10.73]",
            "[01:11.09]自分から 動くこともなく // 就算自己什么也不做",
        ]
        .join("\n")
    }

    fn track_named(title: &str, artist: &str) -> Track {
        let mut t = track_with_artist(artist);
        t.title = Some(title.into());
        t
    }

    #[test]
    fn thwiki_translation_is_dropped_but_the_original_kept() {
        let out = strip_thwiki_translation(&thwiki_file());
        assert!(out.contains("[00:57.02]流れてく 時の中ででも"));
        assert!(!out.contains("即便身处流逝的时光之中"));
        // A blank gap line survives, and a URL holding "//" is not a
        // translation separator.
        assert!(out.contains("[01:10.73]"));
        assert!(out.contains("https://lyrics.thwiki.cc/"));
        // An entry with no translation passes through untouched.
        let plain = "[ti:X]\n[00:00.00]終わることない祭典";
        assert_eq!(strip_thwiki_translation(plain), plain);
    }

    #[test]
    fn thwiki_id_tags_are_readable() {
        let raw = thwiki_file();
        assert_eq!(lrc_id_tag(&raw, "ti").as_deref(), Some("Bad Apple!! feat. nomico"));
        assert_eq!(lrc_id_tag(&raw, "ar").as_deref(), Some("Alstroemeria Records"));
        assert_eq!(lrc_id_tag(&raw, "zz"), None);
    }

    #[test]
    fn thwiki_entry_is_verified_against_its_own_tags() {
        let raw = thwiki_file();
        // The served title may be longer than the tag we searched with.
        assert!(thwiki_entry_matches(
            &track_named("Bad Apple!!", "Alstroemeria Records"),
            &raw
        ));
        // Right circle, wrong song — the site serving a neighbour off the same
        // album must not be accepted.
        assert!(!thwiki_entry_matches(
            &track_named("すごいえーりん・その１", "Alstroemeria Records"),
            &raw
        ));
        // Right title, wrong circle.
        assert!(!thwiki_entry_matches(
            &track_named("Bad Apple!!", "COOL&CREATE"),
            &raw
        ));
        // No `[ar:]` to check leaves the title match standing alone.
        let untagged = "[ti:Bad Apple!!]\n[00:01.00]x";
        assert!(thwiki_entry_matches(
            &track_named("Bad Apple!!", "Anyone At All"),
            untagged
        ));
    }

    #[test]
    fn titles_are_percent_encoded_for_the_url() {
        assert_eq!(percent_encode("Bad Apple!!"), "Bad%20Apple%21%21");
        assert_eq!(percent_encode("a-z_0.9~"), "a-z_0.9~");
        // Japanese titles are the common case: every byte escapes.
        assert_eq!(percent_encode("東方"), "%E6%9D%B1%E6%96%B9");
    }

    #[test]
    fn vocadb_family_is_asked_touhou_first() {
        // Order is a deliberate fit to a doujin-heavy library, not incidental.
        let names: Vec<&str> = VOCADB_SITES.iter().map(|(_, name)| *name).collect();
        assert_eq!(names, ["TouhouDB", "VocaDB", "UtaiteDB"]);
        // All three run the same software, so the paths the client builds must
        // hang off a root that ends in the API prefix.
        for (root, _) in VOCADB_SITES {
            assert!(root.starts_with("https://"), "{root} must be https");
            assert!(root.ends_with("/api/"), "{root} must end in /api/");
        }
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
        assert_eq!(read_sidecar(&audio).as_deref(), Some("from txt"));

        // ...and loses to a `.lrc` found only by a case-insensitive match.
        // The file comes back as written; parsing decides what it is.
        std::fs::write(dir.path().join("01 Song.LRC"), "[00:01.00] from lrc").unwrap();
        assert_eq!(read_sidecar(&audio).as_deref(), Some("[00:01.00] from lrc"));

        // The exact `song.lrc` name still wins over the `song.flac.lrc` form.
        std::fs::write(dir.path().join("01 Song.flac.lrc"), "from double").unwrap();
        std::fs::write(dir.path().join("01 Song.lrc"), "from exact").unwrap();
        assert_eq!(read_sidecar(&audio).as_deref(), Some("from exact"));
    }

    #[test]
    fn a_txt_sidecar_holding_lrc_is_still_synced() {
        // The extension does not decide; the content does.
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("01 Song.flac");
        std::fs::write(&audio, b"").unwrap();
        std::fs::write(dir.path().join("01 Song.txt"), "[00:05.00]synced anyway").unwrap();
        let raw = read_sidecar(&audio.to_string_lossy()).unwrap();
        assert_eq!(timed(&raw), [(5_000, "synced anyway".into())]);
    }
}
