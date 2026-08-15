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
/// Bound both active and waiting requests. A virtualized wall normally stays
/// far below this; a compromised or runaway webview cannot park one task per
/// requested album indefinitely.
static ADMISSION_PERMITS: Semaphore = Semaphore::const_new(36);

fn respond_empty(responder: UriSchemeResponder, status: StatusCode) {
    let response = Response::builder()
        .status(status)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Vec::new())
        .unwrap_or_else(|_| Response::new(Vec::new()));
    responder.respond(response);
}

pub fn handle(
    ctx: UriSchemeContext<'_, tauri::Wry>,
    request: tauri::http::Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let admission = match ADMISSION_PERMITS.try_acquire() {
        Ok(permit) => permit,
        Err(_) => {
            respond_empty(responder, StatusCode::SERVICE_UNAVAILABLE);
            return;
        }
    };
    let app = ctx.app_handle().clone();
    let uri = request.uri().clone();
    tauri::async_runtime::spawn(async move {
        let _admission = admission;
        let _generation = match GEN_PERMITS.acquire().await {
            Ok(permit) => permit,
            Err(_) => {
                respond_empty(responder, StatusCode::SERVICE_UNAVAILABLE);
                return;
            }
        };
        let Some(state) = app.try_state::<AppState>() else {
            respond_empty(responder, StatusCode::SERVICE_UNAVAILABLE);
            return;
        };
        let db = state.db.clone();
        let art = state.art.clone();
        drop(state);
        let result = tauri::async_runtime::spawn_blocking(move || {
            serve(&db, &art, uri.path(), uri.query().unwrap_or(""))
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
                respond_empty(responder, StatusCode::NOT_FOUND);
            }
        }
    });
}

/// Path shapes: /album/{id} and /track/{id}, query `size=thumb|full`.
fn serve(
    db: &crescendo_core::db::Db,
    art_store: &crescendo_core::art::ArtStore,
    path: &str,
    query: &str,
) -> Option<(Vec<u8>, &'static str)> {
    let mut segs = path.trim_start_matches('/').split('/');
    let kind = segs.next()?;
    let id: i64 = segs.next()?.parse().ok()?;
    let full = query.contains("size=full");

    let art = match (kind, full) {
        ("album", false) => {
            let p = art_store.album_thumb(db, id).ok()?;
            ArtData::File(p)
        }
        ("album", true) => art_store.album_full(db, id).ok()?,
        ("track", _) => art_store.track_art(db, id).ok()?,
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
