//! Internet radio: saved stations, the Radio Browser directory, station
//! logos, and the rules for songs a station announces.
//!
//! Stations are not library tracks. They live in their own table, never enter
//! playlists or the queue, and reach the player as a stream address. Playing
//! one is its own phase in [`crate::playback::Playback`].

pub mod directory;
pub mod logo;

use crate::model::Track;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::io::Read;
use std::time::Duration;

/// A saved station.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Station {
    pub id: i64,
    pub name: String,
    /// What GStreamer opens: a direct stream, never a .pls/.m3u playlist.
    pub stream_url: String,
    pub homepage: Option<String>,
    /// Where the logo comes from. Only the backend fetches it, so the webview
    /// gets [`logo`](Self::logo), a key for `crescendo-art://…/logo/{key}`.
    #[serde(skip)]
    pub logo_url: Option<String>,
    pub logo: Option<String>,
    /// Comma-separated, as Radio Browser gives them.
    pub tags: Option<String>,
    pub country: Option<String>,
    pub codec: Option<String>,
    /// kbit/s.
    pub bitrate: Option<i64>,
    /// Radio Browser's station id, for stations saved from the directory.
    pub uuid: Option<String>,
}

/// A station to save.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewStation {
    pub name: String,
    pub stream_url: String,
    pub homepage: Option<String>,
    pub logo_url: Option<String>,
    pub tags: Option<String>,
    pub country: Option<String>,
    pub codec: Option<String>,
    pub bitrate: Option<i64>,
    pub uuid: Option<String>,
}

/// Only http and https streams are played or fetched.
pub fn check_stream_url(url: &str) -> Result<()> {
    let parsed = url::Url::parse(url.trim()).context("not a valid address")?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("a station address must start with http:// or https://");
    }
    Ok(())
}

/// The announced song, split the way Quod Libet splits it: "Artist - Title"
/// at the first " - ", else the whole text is the title.
pub fn split_stream_title(text: &str) -> (Option<String>, Option<String>) {
    let text = text.trim();
    if text.is_empty() {
        return (None, None);
    }
    match text.split_once(" - ") {
        Some((artist, title)) if !artist.trim().is_empty() && !title.trim().is_empty() => (
            Some(artist.trim().to_string()),
            Some(title.trim().to_string()),
        ),
        _ => (None, Some(text.to_string())),
    }
}

/// The announced song from a stream's tags. A separate artist tag (Ogg,
/// Opus and FLAC streams send one) is taken as it is; otherwise the title is
/// split, as Quod Libet does.
pub fn song_from_tags(title: &str, artist: Option<&str>) -> (Option<String>, Option<String>) {
    let artist = artist.map(str::trim).filter(|a| !a.is_empty());
    let title = title.trim();
    match artist {
        Some(artist) if !title.is_empty() => (Some(artist.to_string()), Some(title.to_string())),
        _ => split_stream_title(title),
    }
}

/// The saved station after (or before) `current` in `ids`, wrapping around.
/// A station not in the list, or none playing, starts from the first.
pub fn neighbour(ids: &[i64], current: Option<i64>, forward: bool) -> Option<i64> {
    let n = ids.len();
    if n == 0 {
        return None;
    }
    let Some(i) = current.and_then(|id| ids.iter().position(|&x| x == id)) else {
        return Some(ids[if forward { 0 } else { n - 1 }]);
    };
    Some(
        ids[if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        }],
    )
}

/// A song a station announced, while it is being heard.
#[derive(Debug, Clone, PartialEq)]
pub struct RadioSong {
    pub artist: Option<String>,
    pub title: String,
    pub started_at: i64,
    pub heard_ms: u64,
    /// Its title arrived as a change while the station played, so it was heard
    /// from its start. The first title after tuning in was already under way.
    pub from_start: bool,
    pub scrobbled: bool,
}

impl RadioSong {
    /// A listen counts after half of a song or 4 minutes heard, and the length of a
    /// radio song is unknown. So: 4 minutes at any time, or, once the song
    /// has ended, at least 30 seconds of a song heard from its start.
    pub fn scrobble_due(&self, ended: bool) -> bool {
        !self.scrobbled
            && self.artist.is_some()
            && (self.heard_ms >= 4 * 60_000
                || (ended && self.from_start && self.heard_ms >= 30_000))
    }

    /// The song as a track for the scrobbler. It is not in the library: id 0,
    /// no path, no length.
    pub fn as_track(&self) -> Track {
        Track {
            id: 0,
            path: String::new(),
            title: Some(self.title.clone()),
            artist: self.artist.clone(),
            album: None,
            albumartist: None,
            titlesort: None,
            artistsort: None,
            albumartistsort: None,
            genre: None,
            grouping: None,
            composer: None,
            arranger: None,
            lyricist: None,
            vocalist: None,
            catalognumber: None,
            event: None,
            bpm: None,
            track_no: None,
            disc_no: None,
            year: None,
            duration_ms: 0,
            format: String::new(),
            album_id: None,
            rating: None,
            play_count: 0,
            missing: false,
        }
    }
}

const PLAYLIST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PLAYLIST_BYTES: u64 = 256 * 1024;

/// The stream behind an address the user typed. A `.pls` or `.m3u` playlist
/// is fetched and its first http(s) entry used; anything else is taken as the
/// stream itself (HLS `.m3u8` included, which GStreamer plays directly). A
/// station on the local network is allowed here: the user chose the address.
pub fn resolve_stream_url(url: &str) -> Result<String> {
    let url = url.trim();
    check_stream_url(url)?;
    let path = url::Url::parse(url)?.path().to_ascii_lowercase();
    if !(path.ends_with(".pls") || path.ends_with(".m3u")) {
        return Ok(url.to_string());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(PLAYLIST_TIMEOUT))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("could not fetch the playlist {url}"))?;
    let mut text = String::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_PLAYLIST_BYTES)
        .read_to_string(&mut text)
        .context("could not read the playlist")?;
    playlist_entry(&text).ok_or_else(|| anyhow::anyhow!("the playlist names no http(s) stream"))
}

/// The first http(s) stream a .pls or .m3u lists.
pub fn playlist_entry(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .map(|line| match line.split_once('=') {
            // .pls: File1=http://…
            Some((key, value)) if key.to_ascii_lowercase().starts_with("file") => value.trim(),
            _ => line,
        })
        .find(|entry| check_stream_url(entry).is_ok())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_titles_split_at_the_first_spaced_dash() {
        assert_eq!(
            split_stream_title("Shibayan - Desert Years"),
            (Some("Shibayan".into()), Some("Desert Years".into()))
        );
        assert_eq!(
            split_stream_title("A - B - C"),
            (Some("A".into()), Some("B - C".into()))
        );
        assert_eq!(split_stream_title("Jingle"), (None, Some("Jingle".into())));
        assert_eq!(
            split_stream_title("Up-Tempo"),
            (None, Some("Up-Tempo".into()))
        );
        assert_eq!(
            split_stream_title(" - Title"),
            (None, Some("- Title".into()))
        );
        assert_eq!(split_stream_title("  "), (None, None));
    }

    #[test]
    fn a_separate_artist_tag_is_kept_and_the_title_left_whole() {
        assert_eq!(
            song_from_tags("Desert Years - Live", Some("Shibayan")),
            (Some("Shibayan".into()), Some("Desert Years - Live".into()))
        );
        assert_eq!(
            song_from_tags("Shibayan - Desert Years", Some("  ")),
            (Some("Shibayan".into()), Some("Desert Years".into()))
        );
        assert_eq!(
            song_from_tags("Shibayan - Desert Years", None).0.as_deref(),
            Some("Shibayan")
        );
        assert_eq!(song_from_tags("", Some("Shibayan")), (None, None));
    }

    #[test]
    fn next_and_previous_wrap_around_the_saved_list() {
        let ids = [4, 7, 9];
        assert_eq!(neighbour(&ids, Some(7), true), Some(9));
        assert_eq!(neighbour(&ids, Some(9), true), Some(4));
        assert_eq!(neighbour(&ids, Some(4), false), Some(9));
        assert_eq!(neighbour(&ids, None, true), Some(4));
        assert_eq!(neighbour(&ids, Some(99), false), Some(9));
        assert_eq!(neighbour(&[], Some(1), true), None);
    }

    fn song(heard_ms: u64, from_start: bool) -> RadioSong {
        RadioSong {
            artist: Some("A".into()),
            title: "T".into(),
            started_at: 0,
            heard_ms,
            from_start,
            scrobbled: false,
        }
    }

    #[test]
    fn radio_songs_scrobble_after_four_minutes_or_a_whole_song() {
        assert!(song(240_000, false).scrobble_due(false));
        assert!(!song(200_000, false).scrobble_due(true), "joined mid-song");
        assert!(song(30_000, true).scrobble_due(true));
        assert!(!song(30_000, true).scrobble_due(false), "still playing");
        assert!(!song(29_000, true).scrobble_due(true));
        let mut untitled = song(300_000, true);
        untitled.artist = None;
        assert!(!untitled.scrobble_due(true), "a listen needs an artist");
        let mut done = song(300_000, true);
        done.scrobbled = true;
        assert!(!done.scrobble_due(true));
    }

    #[test]
    fn playlists_give_their_first_stream() {
        let pls = "[playlist]\nNumberOfEntries=2\nFile1=https://a.example/stream\nTitle1=A\nFile2=http://b.example/\n";
        assert_eq!(
            playlist_entry(pls).as_deref(),
            Some("https://a.example/stream")
        );
        let m3u = "#EXTM3U\n#EXTINF:-1,Radio\nhttp://c.example:8000/live\n";
        assert_eq!(
            playlist_entry(m3u).as_deref(),
            Some("http://c.example:8000/live")
        );
        assert_eq!(playlist_entry("#EXTM3U\nfile:///etc/passwd\n"), None);
    }

    #[test]
    fn only_http_and_https_stations() {
        assert!(check_stream_url("https://stream.example/3").is_ok());
        assert!(check_stream_url("http://192.168.1.10:8000/live").is_ok());
        for bad in ["file:///music/a.flac", "rtsp://x/y", "stream.example", ""] {
            assert!(check_stream_url(bad).is_err(), "{bad}");
        }
    }
}
