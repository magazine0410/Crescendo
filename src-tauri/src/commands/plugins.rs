use crate::state::AppState;
use crescendo_core::plugins::{PluginMutationResult, PluginSnapshot};
use serde_json::Value;
use tauri::{path::BaseDirectory, AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

fn plugin_error(error: anyhow::Error) -> String {
    format!("{error:#}")
}

#[tauri::command]
pub async fn plugin_list(state: State<'_, AppState>) -> Result<PluginSnapshot, String> {
    state.plugins.snapshot().map_err(plugin_error)
}

#[tauri::command]
pub async fn plugin_choose_install(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<PluginMutationResult, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Install Crescendo plugin")
        .add_filter("Crescendo plugin", &["crescendo-plugin"])
        .pick_file(move |path| {
            let _ = sender.send(path);
        });
    let Some(selected) = receiver.await.map_err(|error| error.to_string())? else {
        return Ok(PluginMutationResult {
            changed: false,
            message: None,
            snapshot: state.plugins.snapshot().map_err(plugin_error)?,
        });
    };
    let path = selected.into_path().map_err(|error| error.to_string())?;
    let plugins = state.plugins.clone();
    tauri::async_runtime::spawn_blocking(move || plugins.install(&path))
        .await
        .map_err(|error| error.to_string())?
        .map_err(plugin_error)
}

#[tauri::command]
pub async fn plugin_set_enabled(
    state: State<'_, AppState>,
    plugin_id: String,
    enabled: bool,
) -> Result<PluginMutationResult, String> {
    let plugins = state.plugins.clone();
    tauri::async_runtime::spawn_blocking(move || plugins.set_enabled(&plugin_id, enabled))
        .await
        .map_err(|error| error.to_string())?
        .map_err(plugin_error)
}

#[tauri::command]
pub async fn plugin_set_setting(
    state: State<'_, AppState>,
    plugin_id: String,
    key: String,
    value: Value,
) -> Result<PluginMutationResult, String> {
    let plugins = state.plugins.clone();
    tauri::async_runtime::spawn_blocking(move || plugins.set_setting(&plugin_id, &key, value))
        .await
        .map_err(|error| error.to_string())?
        .map_err(plugin_error)
}

#[tauri::command]
pub async fn plugin_uninstall(
    state: State<'_, AppState>,
    plugin_id: String,
    remove_data: bool,
) -> Result<PluginMutationResult, String> {
    let plugins = state.plugins.clone();
    tauri::async_runtime::spawn_blocking(move || plugins.uninstall(&plugin_id, remove_data))
        .await
        .map_err(|error| error.to_string())?
        .map_err(plugin_error)
}

#[tauri::command]
pub async fn plugin_grouping_migration(
    app: AppHandle,
    state: State<'_, AppState>,
    action: String,
) -> Result<PluginMutationResult, String> {
    let archive = match action.as_str() {
        "install" => Some(
            app.path()
                .resolve(
                    "resources/plugins/dev.crescendo.primary-grouping.crescendo-plugin",
                    BaseDirectory::Resource,
                )
                .map_err(|error| error.to_string())?,
        ),
        "dismiss" => None,
        _ => return Err("grouping migration action must be install or dismiss".into()),
    };
    let plugins = state.plugins.clone();
    tauri::async_runtime::spawn_blocking(move || {
        plugins.complete_grouping_migration(archive.as_deref())
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(plugin_error)
}

#[tauri::command]
pub async fn plugin_restart_app(app: AppHandle) -> Result<(), String> {
    app.restart()
}

#[tauri::command]
pub async fn plugin_approve_grants(
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<PluginMutationResult, String> {
    let plugins = state.plugins.clone();
    tauri::async_runtime::spawn_blocking(move || plugins.approve_grants(&plugin_id))
        .await
        .map_err(|error| error.to_string())?
        .map_err(plugin_error)
}

#[tauri::command]
pub async fn plugin_invoke(
    state: State<'_, AppState>,
    plugin_id: String,
    command_id: String,
    input: Value,
) -> Result<Value, String> {
    let plugins = state.plugins.clone();
    tauri::async_runtime::spawn_blocking(move || plugins.invoke(&plugin_id, &command_id, &input))
        .await
        .map_err(|error| error.to_string())?
        .map_err(plugin_error)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    const PLUGIN_PERMISSIONS: [&str; 10] = [
        "allow-plugin-list",
        "allow-plugin-choose-install",
        "allow-plugin-set-enabled",
        "allow-plugin-set-setting",
        "allow-plugin-uninstall",
        "allow-plugin-grouping-migration",
        "allow-plugin-restart-app",
        "allow-plugin-approve-grants",
        "allow-plugin-invoke",
        "allow-plugin-file-drag-start",
    ];

    fn capability(source: &str) -> Value {
        serde_json::from_str(source).unwrap()
    }

    fn permissions(capability: &Value) -> Vec<&str> {
        capability["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect()
    }

    #[test]
    fn plugin_commands_are_granted_only_to_the_main_window() {
        let main = capability(include_str!("../../capabilities/main.json"));
        let about = capability(include_str!("../../capabilities/about.json"));
        let base = capability(include_str!("../../capabilities/default.json"));
        assert_eq!(main["windows"], serde_json::json!(["main"]));
        let main_permissions = permissions(&main);
        let about_permissions = permissions(&about);
        let base_permissions = permissions(&base);
        for permission in PLUGIN_PERMISSIONS {
            assert!(main_permissions.contains(&permission));
            assert!(!about_permissions.contains(&permission));
            assert!(!base_permissions.contains(&permission));
        }
    }
}
