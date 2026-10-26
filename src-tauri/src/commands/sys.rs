use crate::{
    error::{AppError, AppResult},
    models::Settings,
    session_log::{create_unique, Diagnostic},
    state::AppState,
};
use std::io::Write;
use tauri::{Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
#[tauri::command]
pub fn settings_get(state: tauri::State<'_, AppState>) -> AppResult<Settings> {
    state.storage.settings()
}
#[tauri::command]
pub fn settings_set(state: tauri::State<'_, AppState>, settings: Settings) -> AppResult<()> {
    state.storage.save_settings(&settings)
}
#[tauri::command]
pub fn diag_list(state: tauri::State<'_, AppState>) -> AppResult<Vec<Diagnostic>> {
    state.diagnostics.list()
}
#[tauri::command]
pub fn diag_export(state: tauri::State<'_, AppState>, path: String) -> AppResult<String> {
    state.diagnostics.export(std::path::Path::new(&path))
}
#[tauri::command]
pub fn editor_save_local(path: String, content: String) -> AppResult<String> {
    let (path, mut file) = create_unique(std::path::Path::new(&path))?;
    file.write_all(content.as_bytes())
        .map_err(|e| AppError::io("local-save", e))?;
    file.sync_all().map_err(|e| AppError::io("local-save", e))?;
    Ok(path.to_string_lossy().into_owned())
}
#[tauri::command]
pub fn sys_hide(app: tauri::AppHandle) -> AppResult<()> {
    app.get_webview_window("main")
        .ok_or_else(|| AppError::new("E_WINDOW", "window", "Main window unavailable"))?
        .hide()
        .map_err(|e| AppError::io("window", e))
}
#[tauri::command]
pub fn sys_exit(app: tauri::AppHandle) {
    app.exit(0);
}
#[tauri::command]
pub fn sys_notify(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    kind: String,
    title: String,
    body: String,
    project_id: Option<String>,
) -> AppResult<()> {
    let settings = state.storage.settings()?;
    let enabled = match kind.as_str() {
        "disconnected" => settings.notifications.disconnected,
        "completed" => settings.notifications.completed,
        "approval" => settings.notifications.approval,
        _ => {
            return Err(AppError::new(
                "E_INPUT",
                "notification",
                "Invalid notification category",
            ))
        }
    };
    if !enabled {
        return Ok(());
    }
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .extra("projectId", project_id)
        .show()
        .map_err(|e| AppError::io("notification", e))
}
pub fn restore(app: &tauri::AppHandle, project_id: Option<String>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    let _ = app.emit(
        "notification-open",
        serde_json::json!({"projectId":project_id}),
    );
}
