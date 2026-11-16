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
/// Bound both active and waiting requests, so a compromised or runaway webview
/// cannot park one task per album in the library. A refused request shows the
/// missing-cover tile until its row is rendered again, so this must cover
/// everything one screen asks for at once: a maximized wall on a 4K display
/// shows about 300 tiles, and lazy loading also fetches the rows just past
/// the edge. A waiting request costs a few KB; decoding is bounded below.
static ADMISSION_PERMITS: Semaphore = Semaphore::const_new(1024);
/// Station logos are fetched from the stations' websites, which can be slow;
/// they get their own permits so they never hold up album covers.
static LOGO_PERMITS: Semaphore = Semaphore::const_new(2);

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
        if let Some(key) = uri.path().strip_prefix("/logo/").map(str::to_string) {
            serve_logo(&app, key, responder).await;
            return;
        }
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

/// `/logo/{key}`: a station logo the radio commands registered.
async fn serve_logo(app: &tauri::AppHandle, key: String, responder: UriSchemeResponder) {
    let Ok(_permit) = LOGO_PERMITS.acquire().await else {
        respond_empty(responder, StatusCode::SERVICE_UNAVAILABLE);
        return;
    };
    let Some(logos) = app
        .try_state::<AppState>()
        .map(|state| state.radio.logos.clone())
    else {
        respond_empty(responder, StatusCode::SERVICE_UNAVAILABLE);
        return;
    };
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        std::fs::read(logos.get(&key)?).ok()
    })
    .await
    .ok()
    .flatten();
    match bytes {
        Some(bytes) => {
            let resp = Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "image/png")
                .header(header::CACHE_CONTROL, "max-age=86400")
                .body(bytes)
                .unwrap_or_else(|_| Response::new(Vec::new()));
            responder.respond(resp);
        }
        None => respond_empty(responder, StatusCode::NOT_FOUND),
    }
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
