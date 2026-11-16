//! Writing playlists to files through a plugin's playlist exporter.
//!
//! Crescendo owns the parts that touch the filesystem: which file a playlist
//! goes to, never overwriting in a folder export, and writing it atomically.
//! The plugin only renders text. Each playlist is handed to its command in
//! chunks of at most [`CHUNK_BYTES`] of JSON, so a long playlist stays within
//! the 1 MiB limit on a command's input and output, and the chunks' text is
//! written in order:
//!
//! ```json
//! { "playlist": { "id": 3, "name": "Mix" },
//!   "file": "/home/me/Music/Mix.m3u", "directory": "/home/me/Music",
//!   "offset": 0, "total": 120,
//!   "tracks": [ { "path": "...", "title": "...", "artist": "...",
//!                 "album_artist": "...", "duration_ms": 245400,
//!                 "missing": false } ] }
//! ```
//!
//! The command answers `{ "text": "..." }`: the lines for those tracks, plus
//! whatever header the format needs when `offset` is 0.

use super::manager::PluginManager;
use crate::db::Db;
use crate::model::Track;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Serialized tracks per command call. Leaves room under the 1 MiB input
/// limit for the rest of the request, and keeps the rendered text, which is
/// about as long as the paths it contains, under the 1 MiB output limit.
pub const CHUNK_BYTES: usize = 512 * 1024;

#[derive(Debug, Serialize)]
pub struct PlaylistExportResult {
    /// The files written.
    pub files: Vec<String>,
    /// Songs written across them.
    pub tracks: usize,
    /// Songs whose file was missing at export time. The exporter is handed
    /// them anyway, so the playlist stays complete.
    pub missing: usize,
}

fn track_entry(track: &Track) -> Value {
    json!({
        "path": track.path,
        "title": track.title,
        "artist": track.artist,
        "album_artist": track.albumartist,
        "duration_ms": track.duration_ms,
        "missing": track.missing,
    })
}

/// `path`, ending in `.<extension>` (any case).
pub fn with_extension(path: &Path, extension: &str) -> PathBuf {
    let matches = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(extension));
    if matches {
        path.to_path_buf()
    } else {
        let mut name = path.as_os_str().to_owned();
        name.push(".");
        name.push(extension);
        PathBuf::from(name)
    }
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

fn free_file_name(folder: &Path, stem: &str, extension: &str) -> PathBuf {
    let first = folder.join(format!("{stem}.{extension}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| folder.join(format!("{stem} ({n}).{extension}")))
        .find(|candidate| !candidate.exists())
        .expect("some numbered name is free")
}

/// Write one playlist to `file`, given the exporter's extension if it lacks
/// it. The text goes to a temporary file beside the target that is renamed
/// over it, so an existing file is replaced whole or not at all.
pub fn export_playlist(
    manager: &PluginManager,
    db: &Db,
    plugin_id: &str,
    exporter_id: &str,
    playlist_id: i64,
    file: &Path,
) -> Result<PlaylistExportResult> {
    let exporter = manager
        .playlist_exporter(plugin_id, exporter_id)
        .ok_or_else(|| anyhow!("{plugin_id} has no loaded exporter {exporter_id:?}"))?;
    let name = db
        .playlist_list()?
        .into_iter()
        .find(|p| p.id == playlist_id)
        .map(|p| p.name)
        .ok_or_else(|| anyhow!("playlist {playlist_id} no longer exists"))?;
    let file = with_extension(file, &exporter.extension);
    let dir = file
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .ok_or_else(|| anyhow!("the playlist needs a folder to be saved in"))?;
    let tracks = db.playlist_tracks(playlist_id)?;

    let tmp = dir.join(format!(
        ".{}.crescendo-tmp",
        file.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    ));
    let written = (|| -> Result<()> {
        let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        let mut offset = 0;
        // An empty playlist still gets one call, for the format's header.
        while offset < tracks.len() || (offset == 0 && tracks.is_empty()) {
            let mut chunk = Vec::new();
            let mut bytes = 0;
            for track in &tracks[offset..] {
                let entry = track_entry(track);
                let size = entry.to_string().len() + 1;
                if !chunk.is_empty() && bytes + size > CHUNK_BYTES {
                    break;
                }
                bytes += size;
                chunk.push(entry);
            }
            let input = json!({
                "playlist": { "id": playlist_id, "name": name },
                "file": file.to_string_lossy(),
                "directory": dir.to_string_lossy(),
                "offset": offset,
                "total": tracks.len(),
                "tracks": chunk,
            });
            let output = manager
                .invoke(plugin_id, &exporter.command, &input)
                .with_context(|| format!("{plugin_id} could not export the playlist"))?;
            let text = output
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("{plugin_id} returned no text for the playlist"))?;
            out.write_all(text.as_bytes())?;
            if tracks.is_empty() {
                break;
            }
            offset += chunk.len();
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        Ok(())
    })();
    if let Err(error) = written.and_then(|()| Ok(std::fs::rename(&tmp, &file)?)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(PlaylistExportResult {
        files: vec![file.to_string_lossy().to_string()],
        tracks: tracks.len(),
        missing: tracks.iter().filter(|t| t.missing).count(),
    })
}

/// Write each playlist into `folder` as `<name>.<extension>`. An existing file
/// is never replaced: the name gets " (2)", " (3)", … instead.
pub fn export_playlists_to_folder(
    manager: &PluginManager,
    db: &Db,
    plugin_id: &str,
    exporter_id: &str,
    ids: &[i64],
    folder: &Path,
) -> Result<PlaylistExportResult> {
    if !folder.is_dir() {
        bail!("{} is not a folder", folder.display());
    }
    let exporter = manager
        .playlist_exporter(plugin_id, exporter_id)
        .ok_or_else(|| anyhow!("{plugin_id} has no loaded exporter {exporter_id:?}"))?;
    let names: HashMap<i64, String> = db
        .playlist_list()?
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let mut result = PlaylistExportResult {
        files: Vec::new(),
        tracks: 0,
        missing: 0,
    };
    for id in ids {
        let name = names
            .get(id)
            .ok_or_else(|| anyhow!("playlist {id} no longer exists"))?;
        let file = free_file_name(folder, &file_stem_for(name), &exporter.extension);
        let one = export_playlist(manager, db, plugin_id, exporter_id, *id, &file)?;
        result.files.extend(one.files);
        result.tracks += one.tracks;
        result.missing += one.missing;
    }
    Ok(result)
}
