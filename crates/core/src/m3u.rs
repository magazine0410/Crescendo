use crate::db::Db;
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct M3uImportResult {
    pub playlist_id: i64,
    pub name: String,
    pub added: usize,
    pub missing: Vec<String>,
}

/// Import an .m3u/.m3u8 file as a new playlist. Lines resolve absolute, or
/// relative to the m3u's directory; entries not present in the library are
/// reported back, not silently dropped.
pub fn import(db: &Db, m3u_path: &Path) -> Result<M3uImportResult> {
    let bytes = std::fs::read(m3u_path)?;
    // .m3u8 is UTF-8 by definition; plain .m3u is assumed UTF-8 too (lossy
    // fallback keeps imports working on stray encodings).
    let content = String::from_utf8_lossy(&bytes);
    let base = m3u_path.parent().unwrap_or_else(|| Path::new("/"));

    let name = m3u_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Imported playlist".into());

    let mut track_ids = Vec::new();
    let mut missing = Vec::new();

    // Resolution is read-only and can be substantial for a large imported
    // playlist, so use a WAL side connection rather than occupying the shared
    // UI connection for the whole file.
    {
        let conn = db.side_connection()?;
        let mut lookup = conn.prepare("SELECT id FROM tracks WHERE path = ?1")?;
        let mut windows_index: Option<HashMap<String, Vec<(i64, String)>>> = None;
        for line in content.lines() {
            let line = line.trim().trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with('#') {
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

#[cfg(test)]
mod tests {
    use super::import;
    use crate::db::Db;
    use std::path::Path;

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
