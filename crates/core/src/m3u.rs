use crate::db::Db;
use anyhow::Result;
use serde::Serialize;
use std::path::Path;

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

    db.with(|conn| {
        let mut lookup = conn.prepare("SELECT id FROM tracks WHERE path = ?1")?;
        for line in content.lines() {
            let line = line.trim().trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let path = if Path::new(line).is_absolute() {
                Path::new(line).to_path_buf()
            } else {
                base.join(line)
            };
            // Normalize ./ and ../ segments without touching the filesystem
            // (missing files should still resolve to a canonical-ish path).
            let path = normalize(&path);
            let path_str = path.to_string_lossy();
            match lookup.query_row([path_str.as_ref()], |r| r.get::<_, i64>(0)) {
                Ok(id) => track_ids.push(id),
                Err(_) => missing.push(path_str.to_string()),
            }
        }
        Ok(())
    })?;

    let playlist_id = db.playlist_create(&name)?;
    db.playlist_add_tracks(playlist_id, &track_ids)?;

    Ok(M3uImportResult { playlist_id, name, added: track_ids.len(), missing })
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
