use crate::commands::library::{projection_error, ProjectionCommandError};
use crate::state::AppState;
use crescendo_core::model::PlayContext;
use serde::{Deserialize, Serialize};
#[cfg(not(target_os = "linux"))]
use std::path::PathBuf;
use std::sync::mpsc::sync_channel;
use tauri::{AppHandle, State, Window};

/// Native drag payloads larger than this are intentionally clipped. Besides
/// protecting GTK and the receiving application, this keeps a whole-library
/// drag from defeating Crescendo's windowed-library design.
pub const FILE_DRAG_LIMIT: usize = 2_000;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileDragSource {
    /// Explicit rows (track-list multi-selection, queue, now playing).
    TrackIds { ids: Vec<i64> },
    /// A backend-resolved album, playlist, library view, or facet value.
    Context { ctx: PlayContext },
}

#[derive(Debug, Serialize)]
pub struct FileDragResult {
    pub count: usize,
    pub truncated: bool,
}

#[tauri::command]
pub async fn plugin_file_drag_start(
    app: AppHandle,
    window: Window,
    state: State<'_, AppState>,
    source: FileDragSource,
) -> Result<FileDragResult, ProjectionCommandError> {
    let provider =
        state
            .plugins
            .file_drag_provider()
            .ok_or_else(|| ProjectionCommandError::Operation {
                message: "No loaded plugin provides file drag support".into(),
            })?;
    log::debug!("File drag requested through plugin {provider}");

    let (paths, truncated) = match source {
        FileDragSource::TrackIds { ids } => {
            let truncated = ids.len() > FILE_DRAG_LIMIT;
            let paths = state
                .db
                .track_paths_by_ids(&ids, FILE_DRAG_LIMIT)
                .map_err(operation_error)?;
            (paths, truncated)
        }
        FileDragSource::Context { ctx } => {
            let expected_revision = match &ctx {
                PlayContext::LibraryView {
                    projection_revision,
                    ..
                }
                | PlayContext::ValueTracks {
                    projection_revision,
                    ..
                } => Some(*projection_revision),
                _ => None,
            };
            let projection = state
                .plugins
                .projection(expected_revision)
                .map_err(projection_error)?;
            let plan = match &ctx {
                PlayContext::ValueTracks { mode, .. } => projection.facet_plan(mode),
                _ => None,
            };
            let mut paths = state
                .db
                .resolve_context_paths_with_projection(
                    &ctx,
                    plan.as_ref(),
                    projection.prefer_romanized(),
                    FILE_DRAG_LIMIT + 1,
                )
                .map_err(operation_error)?;
            let truncated = paths.len() > FILE_DRAG_LIMIT;
            paths.truncate(FILE_DRAG_LIMIT);
            (paths, truncated)
        }
    };

    if paths.is_empty() {
        return Err(ProjectionCommandError::Operation {
            message: "No available files to drag".into(),
        });
    }

    let count = paths.len();
    let image_bytes = include_bytes!("../../icons/128x128.png").to_vec();

    // GTK requires drag initiation on the application thread. Keep the public
    // command async, but wait only for native drag setup, not for the eventual
    // drop/cancel callback.
    let (tx, rx) = sync_channel(1);
    app.run_on_main_thread(move || {
        let result = start_native_drag(window, paths, image_bytes);
        let _ = tx.send(result);
    })
    .map_err(|error| ProjectionCommandError::Operation {
        message: error.to_string(),
    })?;
    rx.recv()
        .map_err(|error| ProjectionCommandError::Operation {
            message: error.to_string(),
        })?
        .map_err(|message| ProjectionCommandError::Operation { message })?;

    Ok(FileDragResult { count, truncated })
}

#[cfg(target_os = "linux")]
fn start_native_drag(
    window: Window,
    paths: Vec<String>,
    image_bytes: Vec<u8>,
) -> Result<(), String> {
    use gtk::{
        gdk,
        glib::{ObjectExt, SignalHandlerId},
        prelude::{DragContextExtManual, PixbufLoaderExt, WidgetExt, WidgetExtManual},
    };
    use std::{cell::RefCell, rc::Rc};

    let gtk_window = window.gtk_window().map_err(|error| error.to_string())?;
    let uris = linux_file_uris(&paths)?;
    let handler_ids = Rc::new(RefCell::new(Vec::<SignalHandlerId>::new()));

    log::debug!("Starting GTK file drag with {} paths", uris.len());
    gtk_window.drag_source_set(gdk::ModifierType::BUTTON1_MASK, &[], gdk::DragAction::COPY);
    gtk_window.drag_source_add_uri_targets();

    let drag_uris = uris.clone();
    handler_ids
        .borrow_mut()
        .push(gtk_window.connect_drag_data_get(move |_, _, data, _, _| {
            let refs: Vec<&str> = drag_uris.iter().map(String::as_str).collect();
            log::debug!("Serving GTK drag URIs: {:?}", refs);
            if !data.set_uris(&refs) {
                log::warn!("GTK rejected the file URI drag payload");
            }
        }));

    let target_list = gtk_window
        .drag_source_get_target_list()
        .ok_or_else(|| "GTK produced an empty drag target list".to_string())?;

    // `button` is a mouse button number, not GDK's BUTTON1_MASK bitmask. The
    // upstream drag crate passes the latter (256), which makes the pointer grab
    // unreliable. A real event is unavailable after the webview IPC hop, so
    // GTK uses its documented current-time/current-pointer fallback.
    let drag_context = gtk_window
        .drag_begin_with_coordinates(&target_list, gdk::DragAction::COPY, 1, None, -1, -1)
        .ok_or_else(|| "GTK failed to create a drag context".to_string())?;

    let loader = gtk::gdk_pixbuf::PixbufLoader::new();
    if loader.write(&image_bytes).is_ok() && loader.close().is_ok() {
        if let Some(icon) = loader.pixbuf() {
            drag_context.drag_set_icon_pixbuf(&icon, 0, 0);
        }
    }

    let cleanup_window = gtk_window.clone();
    let cleanup_handlers = handler_ids.clone();
    handler_ids
        .borrow_mut()
        .push(gtk_window.connect_drag_end(move |_, context| {
            log::debug!(
                "GTK file drag finished with action {:?}",
                context.selected_action()
            );

            // Do not clean up at GdkDragContext::drop-performed: that signal
            // occurs when the mouse is released, before the destination has
            // necessarily finished requesting selection data. `drag-end` is
            // emitted only after those transfers complete.
            let ids: Vec<_> = cleanup_handlers.borrow_mut().drain(..).collect();
            for id in ids {
                cleanup_window.disconnect(id);
            }
            cleanup_window.drag_source_unset();
            log::debug!("GTK file drag handlers cleaned up after data transfer");
        }));

    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn start_native_drag(
    window: Window,
    paths: Vec<String>,
    image_bytes: Vec<u8>,
) -> Result<(), String> {
    // Construct DragItem on the destination thread: its alternative custom
    // data-provider variant is intentionally !Send, even though this command
    // always uses the files variant.
    let item = drag::DragItem::Files(paths.into_iter().map(PathBuf::from).collect());
    let image = drag::Image::Raw(image_bytes);
    drag::start_drag(
        &window,
        item,
        image,
        |_result, _cursor_position| {},
        drag::Options::default(),
    )
    .map_err(|error| error.to_string())
}

fn operation_error(error: impl std::fmt::Display) -> ProjectionCommandError {
    ProjectionCommandError::Operation {
        message: error.to_string(),
    }
}

#[cfg(target_os = "linux")]
fn linux_file_uris(paths: &[String]) -> Result<Vec<String>, String> {
    paths
        .iter()
        .map(|path| {
            glib::filename_to_uri(path, None)
                .map(String::from)
                .map_err(|error| format!("Cannot make a file URI from {path}: {error}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn gtk_drag_paths_form_valid_encoded_file_uris() {
        let raw = "/mnt/Mythic_Meme/Music/Circle [例大祭7]/2 特典トーク琴音 .mp3";
        let advertised = super::linux_file_uris(&[raw.into()]).unwrap().remove(0);

        assert_eq!(
            advertised,
            glib::filename_to_uri(raw, None).unwrap(),
            "the GTK payload must use GLib's canonical file URI"
        );
        assert!(!advertised.contains(' '));
        assert!(advertised.contains("%5B"));
        assert!(advertised.contains("%5D"));
        assert!(advertised.contains("%E4%BE%8B%E5%A4%A7%E7%A5%AD7"));
    }
}
