use crate::player::{Cmd, PlayerState};
use crate::state::AppState;
use crescendo_core::model::{PlayContext, Track};
use tauri::State;

#[tauri::command]
pub async fn player_play_context(
    state: State<'_, AppState>,
    ctx: PlayContext,
    start_index: usize,
) -> Result<(), String> {
    state.player.send(Cmd::PlayContext { ctx, start_index });
    Ok(())
}

#[tauri::command]
pub async fn player_jump_to(state: State<'_, AppState>, index: usize) -> Result<(), String> {
    state.player.send(Cmd::JumpTo(index));
    Ok(())
}

#[tauri::command]
pub async fn player_toggle_pause(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::TogglePause);
    Ok(())
}

#[tauri::command]
pub async fn player_stop(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::Stop);
    Ok(())
}

#[tauri::command]
pub async fn player_next(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::Next);
    Ok(())
}

#[tauri::command]
pub async fn player_prev(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::Prev);
    Ok(())
}

#[tauri::command]
pub async fn player_seek(state: State<'_, AppState>, ms: u64) -> Result<(), String> {
    state.player.send(Cmd::Seek(ms));
    Ok(())
}

#[tauri::command]
pub async fn player_set_volume(state: State<'_, AppState>, volume: f64) -> Result<(), String> {
    state.player.send(Cmd::SetVolume(volume));
    let _ = state.db.prefs_set("volume", &format!("{volume:.3}"));
    Ok(())
}

#[tauri::command]
pub async fn player_set_muted(state: State<'_, AppState>, muted: bool) -> Result<(), String> {
    state.player.send(Cmd::SetMuted(muted));
    Ok(())
}

#[tauri::command]
pub async fn player_set_shuffle(state: State<'_, AppState>, shuffle: bool) -> Result<(), String> {
    state.player.send(Cmd::SetShuffle(shuffle));
    Ok(())
}

#[tauri::command]
pub async fn player_cycle_repeat(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::CycleRepeat);
    Ok(())
}

#[tauri::command]
pub async fn player_set_stop_after_current(
    state: State<'_, AppState>,
    on: bool,
) -> Result<(), String> {
    state.player.send(Cmd::SetStopAfterCurrent(on));
    Ok(())
}

#[tauri::command]
pub async fn player_set_spectrum_enabled(
    state: State<'_, AppState>,
    on: bool,
) -> Result<(), String> {
    state.player.send(Cmd::SetSpectrumEnabled(on));
    Ok(())
}

#[tauri::command]
pub async fn player_get_state(state: State<'_, AppState>) -> Result<PlayerState, String> {
    Ok(state.player.state())
}

#[tauri::command]
pub async fn queue_add(state: State<'_, AppState>, track_ids: Vec<i64>) -> Result<(), String> {
    state.player.send(Cmd::QueueAdd(track_ids));
    Ok(())
}

#[tauri::command]
pub async fn queue_remove(state: State<'_, AppState>, positions: Vec<usize>) -> Result<(), String> {
    state.player.send(Cmd::QueueRemove(positions));
    Ok(())
}

#[tauri::command]
pub async fn queue_clear(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::QueueClear);
    Ok(())
}

#[tauri::command]
pub async fn queue_list(state: State<'_, AppState>) -> Result<Vec<Track>, String> {
    let ids = state.player.queue_ids();
    state.db.tracks_by_ids(&ids).map_err(|e| e.to_string())
}
