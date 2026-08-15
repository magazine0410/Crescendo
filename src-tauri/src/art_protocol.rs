//! `crescendo-art://` custom protocol: serves album thumbnails and full-size
//! art straight from the resolver/cache. The album wall is virtualized, so
//! only visible covers ever get requested — lazy generation for free.

use crate::state::AppState;
use crescendo_core::art::ArtData;
use tauri::http::{header, Response, StatusCode};
use tauri::{Manager, UriSchemeContext, UriSchemeResponder};
use tokio::sync::Semaphore;

/// Bound concurrent thumbnail generation (image decode is CPU-heavy and the
/// source reads hit the library HDD).
static GEN_PERMITS: Semaphore = Semaphore::const_new(4);

pub fn handle(
    ctx: UriSchemeContext<'_, tauri::Wry>,
    request: tauri::http::Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    let uri = request.uri().clone();
    tauri::async_runtime::spawn(async move {
        let _permit = GEN_PERMITS.acquire().await;
        let result = tauri::async_runtime::spawn_blocking(move || {
            let state = app.state::<AppState>();
            serve(&state, uri.path(), uri.query().unwrap_or(""))
        })
        .await;
        match result {
            Ok(Some((bytes, mime))) => {
                let resp = Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, mime)
                    // Disk-side source fingerprints keep thumbnails fresh; do
                    // not let the webview pin an older response for a day.
                    .header(header::CACHE_CONTROL, "no-cache")
                    .body(bytes)
                    .unwrap_or_else(|_| Response::new(Vec::new()));
                responder.respond(resp);
            }
            _ => {
                let resp = Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Vec::new())
                    .unwrap_or_else(|_| Response::new(Vec::new()));
                responder.respond(resp);
            }
        }
    });
}

/// Path shapes: /album/{id} and /track/{id}, query `size=thumb|full`.
fn serve(state: &AppState, path: &str, query: &str) -> Option<(Vec<u8>, &'static str)> {
    let mut segs = path.trim_start_matches('/').split('/');
    let kind = segs.next()?;
    let id: i64 = segs.next()?.parse().ok()?;
    let full = query.contains("size=full");

    let art = match (kind, full) {
        ("album", false) => {
            let p = state.art.album_thumb(&state.db, id).ok()?;
            ArtData::File(p)
        }
        ("album", true) => state.art.album_full(&state.db, id).ok()?,
        ("track", _) => state.art.track_art(&state.db, id).ok()?,
        _ => return None,
    };
    match art {
        ArtData::File(p) => {
            let mime = match p.extension().and_then(|e| e.to_str()) {
                Some("png") => "image/png",
                Some("webp") => "image/webp",
                Some("gif") => "image/gif",
                Some("bmp") => "image/bmp",
                _ => "image/jpeg",
            };
            Some((std::fs::read(p).ok()?, mime))
        }
        ArtData::Bytes(b) => {
            let mime = sniff_mime(&b);
            Some((b, mime))
        }
    }
}

fn sniff_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.len() > 11 && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/jpeg"
    }
}
