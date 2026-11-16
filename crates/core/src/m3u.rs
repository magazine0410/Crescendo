use crate::db::Db;
use crate::model::Track;
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Far beyond any real playlist; a cap on what one import reads.
const MAX_M3U_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct M3uImportResult {
    pub playlist_id: i64,
    pub name: String,
    pub added: usize,
    pub missing: Vec<String>,
}

/// The playlist file formats [`import`] reads. Export writes M3U only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    M3u,
    Pls,
    Xspf,
}

fn format_of(path: &Path) -> Option<Format> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "m3u" | "m3u8" => Some(Format::M3u),
        "pls" => Some(Format::Pls),
        "xspf" => Some(Format::Xspf),
        _ => None,
    }
}

/// Import an .m3u/.m3u8, .pls or .xspf file as a new playlist. Entries resolve
/// absolute, or relative to the playlist's directory; `file://` URIs are
/// decoded. Entries not present in the library (web streams included) are
/// reported back, not silently dropped.
pub fn import(db: &Db, m3u_path: &Path) -> Result<M3uImportResult> {
    // The path comes from the webview. Its unmatched lines are returned as
    // `missing`, so without these checks any readable file could be read
    // back line by line, and /dev/zero or a FIFO would hang the import.
    let Some(format) = format_of(m3u_path) else {
        anyhow::bail!("only .m3u, .m3u8, .pls and .xspf files can be imported");
    };
    let meta = std::fs::metadata(m3u_path)?;
    if !meta.is_file() {
        anyhow::bail!("the playlist is not a regular file");
    }
    if meta.len() > MAX_M3U_BYTES {
        anyhow::bail!("the playlist is larger than 8 MiB");
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(std::fs::File::open(m3u_path)?, MAX_M3U_BYTES),
        &mut bytes,
    )?;
    // .m3u8 and XSPF are UTF-8 by definition; .m3u and .pls are assumed UTF-8
    // too (lossy fallback keeps imports working on stray encodings).
    let content = String::from_utf8_lossy(&bytes);
    let base = m3u_path.parent().unwrap_or_else(|| Path::new("/"));

    let (title, entries) = match format {
        Format::M3u => (None, m3u_entries(&content)),
        Format::Pls => (None, pls_entries(&content)),
        Format::Xspf => xspf_entries(&content)?,
    };
    let name = title.unwrap_or_else(|| {
        m3u_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Imported playlist".into())
    });

    let mut track_ids = Vec::new();
    let mut missing = Vec::new();

    // Resolution is read-only and can be substantial for a large imported
    // playlist, so use a WAL side connection rather than occupying the shared
    // UI connection for the whole file.
    {
        let conn = db.side_connection()?;
        let mut lookup = conn.prepare("SELECT id FROM tracks WHERE path = ?1")?;
        let mut windows_index: Option<HashMap<String, Vec<(i64, String)>>> = None;
        for entry in &entries {
            // XSPF locations are URIs, relative ones included; in M3U and PLS
            // only an explicit file:// entry is one.
            let decoded = match entry {
                Entry::Uri(uri) => uri_to_path(uri, base),
                Entry::Path(path) if path.starts_with("file:") => uri_to_path(path, base),
                Entry::Path(_) | Entry::Untitled(_) => None,
            };
            // A web address in an M3U or PLS is reported as written, not
            // taken for a relative path.
            let web = matches!(entry, Entry::Path(p) if is_web_address(p));
            let line = match (&decoded, entry) {
                (Some(path), _) => path.as_str(),
                (None, Entry::Path(text) | Entry::Uri(text) | Entry::Untitled(text)) => {
                    text.as_str()
                }
            };
            if web
                || matches!(entry, Entry::Untitled(_))
                || (matches!(entry, Entry::Uri(_)) && decoded.is_none())
            {
                // Not a file this computer has: a web address or a track
                // that names no location at all.
                missing.push(line.to_string());
                continue;
            }
            let (candidates, windows_suffix) = entry_candidates(line, base);
            let mut found = None;
            for candidate in &candidates {
                let path = candidate.to_string_lossy();
                match lookup.query_row([path.as_ref()], |r| r.get::<_, i64>(0)) {
                    Ok(id) => {
                        found = Some(id);
                        break;
                    }
                    Err(rusqlite::Error::QueryReturnedNoRows) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            if found.is_none() {
                if let Some(suffix) = windows_suffix {
                    if windows_index.is_none() {
                        let mut index: HashMap<String, Vec<(i64, String)>> = HashMap::new();
                        let mut paths = conn.prepare("SELECT id, path, filename FROM tracks")?;
                        let rows = paths.query_map([], |r| {
                            Ok((
                                r.get::<_, i64>(0)?,
                                r.get::<_, String>(1)?,
                                r.get::<_, String>(2)?,
                            ))
                        })?;
                        for row in rows {
                            let (id, path, filename) = row?;
                            index
                                .entry(filename.to_lowercase())
                                .or_default()
                                .push((id, path.to_lowercase()));
                        }
                        windows_index = Some(index);
                    }
                    let suffix_text = suffix.to_string_lossy().to_lowercase();
                    let filename = suffix
                        .file_name()
                        .map(|name| name.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    let ids: Vec<i64> = windows_index
                        .as_ref()
                        .and_then(|index| index.get(&filename))
                        .into_iter()
                        .flatten()
                        .filter_map(|(id, path)| path.ends_with(&suffix_text).then_some(*id))
                        .take(2)
                        .collect();
                    // A drive letter has no intrinsic Linux mount point. Accept
                    // a unique case-insensitive library suffix; ambiguity is
                    // safer to report than silently choosing the wrong track.
                    if ids.len() == 1 {
                        found = Some(ids[0]);
                    }
                }
            }
            match found {
                Some(id) => track_ids.push(id),
                None => missing.push(
                    candidates
                        .last()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| line.to_string()),
                ),
            }
        }
    }

    let playlist_id = db.playlist_create(&name)?;
    db.playlist_add_tracks(playlist_id, &track_ids)?;

    Ok(M3uImportResult {
        playlist_id,
        name,
        added: track_ids.len(),
        missing,
    })
}

/// One song a playlist names, before it is matched against the library.
#[derive(Debug, Clone, PartialEq)]
enum Entry {
    /// A filesystem path, absolute or relative (M3U and PLS).
    Path(String),
    /// A URI, absolute or relative to the playlist (XSPF `<location>`).
    Uri(String),
    /// An XSPF track without a location, reported by its title.
    Untitled(String),
}

fn m3u_entries(content: &str) -> Vec<Entry> {
    content
        .lines()
        .map(|line| line.trim().trim_start_matches('\u{feff}'))
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| Entry::Path(line.to_string()))
        .collect()
}

/// `File1=…`, `File2=…`, in their numbered order (the [playlist] section's
/// other keys, titles and lengths, are not needed to find the files).
fn pls_entries(content: &str) -> Vec<Entry> {
    let mut numbered: Vec<(u32, String)> = content
        .lines()
        .filter_map(|line| {
            let (key, value) = line.trim().trim_start_matches('\u{feff}').split_once('=')?;
            let key = key.trim().to_ascii_lowercase();
            let n: u32 = key.strip_prefix("file")?.parse().ok()?;
            let value = value.trim();
            (!value.is_empty()).then(|| (n, value.to_string()))
        })
        .collect();
    numbered.sort_by_key(|(n, _)| *n);
    numbered
        .into_iter()
        .map(|(_, value)| Entry::Path(value))
        .collect()
}

/// The playlist's `<title>` and each track's first `<location>`, read with
/// quick-xml, which never loads external entities.
fn xspf_entries(content: &str) -> Result<(Option<String>, Vec<Entry>)> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(content);
    // Element names from the root down, without namespace prefixes.
    let mut stack: Vec<String> = Vec::new();
    let mut text = String::new();
    let mut playlist_title = None;
    let mut entries = Vec::new();
    // Inside a <track>: its first location, and its title for the report.
    let mut location: Option<String> = None;
    let mut track_title: Option<String> = None;
    loop {
        let event = reader
            .read_event()
            .map_err(|e| anyhow::anyhow!("not a valid XSPF playlist: {e}"))?;
        match event {
            Event::Start(start) => {
                stack.push(start.local_name().as_ref().to_ascii_lowercase());
                text.clear();
                if stack.last().map(String::as_str) == Some("track") {
                    location = None;
                    track_title = None;
                }
            }
            Event::Text(t) => text.push_str(&t),
            Event::CData(c) => text.push_str(&c),
            Event::GeneralRef(r) => match r.resolve_char_ref() {
                Ok(Some(c)) => text.push(c),
                _ => match &*r {
                    "amp" => text.push('&'),
                    "lt" => text.push('<'),
                    "gt" => text.push('>'),
                    "quot" => text.push('"'),
                    "apos" => text.push('\''),
                    // An entity the file declared itself is not expanded.
                    _ => {}
                },
            },
            Event::End(_) => {
                let path: Vec<&str> = stack.iter().map(String::as_str).collect();
                let value = text.trim().to_string();
                match path.as_slice() {
                    [.., "playlist", "title"] if !value.is_empty() => playlist_title = Some(value),
                    [.., "track", "location"] if location.is_none() && !value.is_empty() => {
                        location = Some(value)
                    }
                    [.., "track", "title"] if !value.is_empty() => track_title = Some(value),
                    [.., "track"] => entries.push(match location.take() {
                        Some(uri) => Entry::Uri(uri),
                        None => Entry::Untitled(
                            track_title
                                .take()
                                .unwrap_or_else(|| "(a track with no location)".into()),
                        ),
                    }),
                    _ => {}
                }
                stack.pop();
                text.clear();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok((playlist_title, entries))
}

/// `http://…`, `https://…`, `mms://…` and the like: a scheme of letters,
/// digits, `+`, `-` or `.`, then "://".
fn is_web_address(entry: &str) -> bool {
    entry.split_once("://").is_some_and(|(scheme, _)| {
        !scheme.eq_ignore_ascii_case("file")
            && scheme.len() > 1
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// A file URI, or a URI relative to the playlist's folder, as a path. None
/// for anything that is not a file on this computer (http and the like).
fn uri_to_path(uri: &str, base: &Path) -> Option<String> {
    let parsed = match url::Url::parse(uri) {
        Ok(url) => url,
        Err(url::ParseError::RelativeUrlWithoutBase) => {
            url::Url::from_directory_path(base).ok()?.join(uri).ok()?
        }
        Err(_) => return None,
    };
    if parsed.scheme() != "file" {
        return None;
    }
    Some(parsed.to_file_path().ok()?.to_string_lossy().to_string())
}

/// Native path candidates plus an optional suffix for a Windows absolute path.
/// Native interpretation comes first so a legal Linux filename containing a
/// backslash continues to work; the slash-converted form is the fallback.
fn entry_candidates(line: &str, base: &Path) -> (Vec<PathBuf>, Option<PathBuf>) {
    let native = Path::new(line);
    let native = if native.is_absolute() {
        native.to_path_buf()
    } else {
        base.join(native)
    };
    let native = normalize(&native);
    let mut candidates = vec![native];

    let portable = line.replace('\\', "/");
    let bytes = portable.as_bytes();
    let drive_absolute =
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/';
    let unc_absolute = portable.starts_with("//");
    if drive_absolute || unc_absolute {
        let tail = if drive_absolute {
            &portable[2..]
        } else {
            portable.trim_start_matches('/')
        };
        let suffix = normalize(&PathBuf::from(format!("/{tail}")));
        return (candidates, Some(suffix));
    }

    if portable != line {
        let path = Path::new(&portable);
        let converted = if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        };
        let converted = normalize(&converted);
        if !candidates.contains(&converted) {
            candidates.push(converted);
        }
    }
    (candidates, None)
}

fn normalize(path: &Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[derive(Debug, Serialize)]
pub struct M3uExportResult {
    /// The files written.
    pub files: Vec<String>,
    /// Songs written across them.
    pub tracks: usize,
    /// Songs whose file was missing at export time; their paths are written
    /// anyway, so the playlist stays complete.
    pub missing: usize,
}

/// A playlist as extended M3U, in UTF-8: an `#EXTM3U` header, then for each
/// song an `#EXTINF:<seconds>,<Artist> - <Title>` line and its path. A path
/// below `dir` (the folder the file is written to) is written relative to it,
/// any other path in full, which is what [`import`] resolves back.
pub fn render(tracks: &[Track], dir: &Path) -> String {
    let mut out = String::from("#EXTM3U\n");
    for track in tracks {
        let seconds = if track.duration_ms > 0 {
            (track.duration_ms + 500) / 1000
        } else {
            -1
        };
        let title = track
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| {
                Path::new(&track.path)
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
                    .unwrap_or_default()
            });
        let artist = track
            .artist
            .as_ref()
            .or(track.albumartist.as_ref())
            .filter(|a| !a.trim().is_empty());
        let label = match artist {
            Some(artist) => format!("{artist} - {title}"),
            None => title,
        };
        // One entry per line: a line break in a tag would split it.
        let label = label.replace(['\r', '\n'], " ");
        let path = Path::new(&track.path);
        let line = match path.strip_prefix(dir) {
            Ok(relative) if !relative.as_os_str().is_empty() => relative.to_string_lossy(),
            _ => path.to_string_lossy(),
        };
        out.push_str(&format!("#EXTINF:{seconds},{label}\n{line}\n"));
    }
    out
}

/// `path`, ending in `.m3u`.
pub fn with_m3u_extension(path: &Path) -> PathBuf {
    let is_m3u = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("m3u"));
    if is_m3u {
        path.to_path_buf()
    } else {
        let mut name = path.as_os_str().to_owned();
        name.push(".m3u");
        PathBuf::from(name)
    }
}

/// Write one playlist to `file` (given a `.m3u` extension if it lacks one).
/// The file is written beside its target and renamed over it, so an existing
/// one is replaced whole or not at all.
pub fn export_playlist(db: &Db, playlist_id: i64, file: &Path) -> Result<M3uExportResult> {
    let file = with_m3u_extension(file);
    let dir = file
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .ok_or_else(|| anyhow::anyhow!("the playlist needs a folder to be saved in"))?;
    let tracks = db.playlist_tracks(playlist_id)?;
    let text = render(&tracks, dir);
    let tmp = dir.join(format!(
        ".{}.crescendo-tmp",
        file.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    ));
    std::fs::write(&tmp, text)?;
    if let Err(error) = std::fs::rename(&tmp, &file) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error.into());
    }
    Ok(M3uExportResult {
        files: vec![file.to_string_lossy().to_string()],
        tracks: tracks.len(),
        missing: tracks.iter().filter(|t| t.missing).count(),
    })
}

/// Write each playlist into `folder` as `<name>.m3u`. An existing file is
/// never replaced: the name gets " (2)", " (3)", … instead.
pub fn export_playlists_to_folder(db: &Db, ids: &[i64], folder: &Path) -> Result<M3uExportResult> {
    if !folder.is_dir() {
        anyhow::bail!("{} is not a folder", folder.display());
    }
    let names: HashMap<i64, String> = db
        .playlist_list()?
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let mut result = M3uExportResult {
        files: Vec::new(),
        tracks: 0,
        missing: 0,
    };
    for id in ids {
        let name = names
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("playlist {id} no longer exists"))?;
        let file = free_file_name(folder, &file_stem_for(name));
        let one = export_playlist(db, *id, &file)?;
        result.files.extend(one.files);
        result.tracks += one.tracks;
        result.missing += one.missing;
    }
    Ok(result)
}

/// A playlist name made safe as a file name: no path separators or control
/// characters, no leading dot, never empty.
pub fn file_stem_for(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim().to_string();
    if cleaned.is_empty() {
        "Playlist".into()
    } else {
        cleaned
    }
}

fn free_file_name(folder: &Path, stem: &str) -> PathBuf {
    let first = folder.join(format!("{stem}.m3u"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| folder.join(format!("{stem} ({n}).m3u")))
        .find(|candidate| !candidate.exists())
        .expect("some numbered name is free")
}

#[cfg(test)]
mod tests {
    use super::import;
    use crate::db::Db;
    use std::path::Path;

    #[test]
    fn only_regular_playlist_files_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let secret = dir.path().join("id_ed25519");
        std::fs::write(&secret, "PRIVATE KEY\n").unwrap();
        assert!(
            import(&db, &secret).is_err(),
            "any file read back as `missing`"
        );
        std::fs::create_dir(dir.path().join("folder.m3u")).unwrap();
        assert!(import(&db, &dir.path().join("folder.m3u")).is_err());
        let fifo = dir.path().join("fifo.m3u");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        assert!(import(&db, &fifo).is_err(), "a FIFO would block forever");
    }

    fn insert_track(db: &Db, id: i64, path: &Path) {
        db.with(|conn| {
            conn.execute(
                "INSERT INTO tracks(id, path, dir, filename, mtime, size, format) \
                 VALUES (?1, ?2, ?3, ?4, 0, 1, 'FLAC')",
                rusqlite::params![
                    id,
                    path.to_string_lossy(),
                    path.parent().unwrap().to_string_lossy(),
                    path.file_name().unwrap().to_string_lossy(),
                ],
            )?;
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn resolves_backslash_relative_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("m3u.db")).unwrap();
        let track = tmp.path().join("song.flac");
        insert_track(&db, 1, &track);
        let lists = tmp.path().join("lists");
        std::fs::create_dir(&lists).unwrap();
        let playlist = lists.join("relative.m3u8");
        std::fs::write(&playlist, "..\\song.flac\n").unwrap();

        let result = import(&db, &playlist).unwrap();
        assert_eq!(result.added, 1);
        assert!(result.missing.is_empty());
    }

    #[test]
    fn resolves_a_unique_windows_drive_suffix() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("m3u.db")).unwrap();
        let track = tmp
            .path()
            .join("mounted")
            .join("Music")
            .join("Album")
            .join("song.flac");
        insert_track(&db, 1, &track);
        let playlist = tmp.path().join("windows.m3u8");
        std::fs::write(&playlist, "D:\\Music\\Album\\song.flac\n").unwrap();

        let result = import(&db, &playlist).unwrap();
        assert_eq!(result.added, 1);
        assert!(result.missing.is_empty());
    }
}
