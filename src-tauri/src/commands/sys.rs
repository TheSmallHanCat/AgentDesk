use crate::{
    error::{AppError, AppResult},
    models::Settings,
    session_log::{create_unique, Diagnostic},
    state::AppState,
};
use std::io::Write;
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;
#[tauri::command]
pub fn settings_get(state: tauri::State<'_, AppState>) -> AppResult<Settings> {
    state.storage.settings()
}
#[tauri::command]
pub fn settings_set(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    settings: Settings,
) -> AppResult<()> {
    state.storage.save_settings(&settings)?;
    refresh_tray_menu(&app, &settings.language)
}
#[tauri::command]
pub fn diag_list(state: tauri::State<'_, AppState>) -> AppResult<Vec<Diagnostic>> {
    state.diagnostics.list()
}
async fn save_destination(
    app: tauri::AppHandle,
    name: &'static str,
) -> AppResult<Option<std::path::PathBuf>> {
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_file_name(name)
            .blocking_save_file()
            .map(|p| p.into_path().map_err(|e| AppError::io("save-dialog", e)))
            .transpose()
    })
    .await
    .map_err(|e| AppError::io("save-dialog", e))?
}
#[tauri::command]
pub async fn diag_export(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> AppResult<Option<String>> {
    let Some(path) = save_destination(app, "agentdesk-diagnostics.json").await? else {
        return Ok(None);
    };
    state.diagnostics.export(&path).map(Some)
}
#[tauri::command]
pub async fn editor_save_local(
    app: tauri::AppHandle,
    content: String,
) -> AppResult<Option<String>> {
    let Some(path) = save_destination(app, "agentdesk-file.txt").await? else {
        return Ok(None);
    };
    let (path, mut file) = create_unique(&path)?;
    let result = (|| {
        file.write_all(content.as_bytes())
            .map_err(|e| AppError::io("local-save", e))?;
        file.sync_all().map_err(|e| AppError::io("local-save", e))
    })();
    drop(file);
    if let Err(error) = result {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok(Some(path.to_string_lossy().into_owned()))
}
#[tauri::command]
pub fn sys_hide(app: tauri::AppHandle) -> AppResult<()> {
    app.get_webview_window("main")
        .ok_or_else(|| AppError::new("E_WINDOW", "window", "Main window unavailable"))?
        .hide()
        .map_err(|e| AppError::io("window", e))
}
#[tauri::command]
pub async fn sys_exit(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> AppResult<()> {
    state.transfers.shutdown().await?;
    for connection in state.db.connections()? {
        state.hosts.disconnect(&connection.id).await;
        state.terminals.close_connection(&connection.id).await;
        let target = crate::models::Target {
            connection_id: connection.id,
            project_id: None,
            session_id: None,
            request_seq: 0,
        };
        state.ssh.disconnect(&target).await?;
    }
    app.exit(0);
    Ok(())
}
#[tauri::command]
pub fn sys_notify(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    kind: String,
    title: String,
    body: String,
    project_id: Option<String>,
    session_id: Option<String>,
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
        .extra("sessionId", session_id)
        .show()
        .map_err(|e| AppError::io("notification", e))
}
pub fn restore(app: &tauri::AppHandle, project_id: Option<String>) {
    restore_session(app, project_id, None);
}
pub fn restore_session(
    app: &tauri::AppHandle,
    project_id: Option<String>,
    session_id: Option<String>,
) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    let _ = app.emit(
        "notification-open",
        serde_json::json!({"projectId":project_id,"sessionId":session_id}),
    );
}
pub fn native_label(language: &str, key: &str) -> String {
    static EN: std::sync::OnceLock<std::collections::HashMap<String, String>> =
        std::sync::OnceLock::new();
    static ZH: std::sync::OnceLock<std::collections::HashMap<String, String>> =
        std::sync::OnceLock::new();
    let catalog = if language == "en" {
        EN.get_or_init(|| {
            serde_json::from_str(include_str!("../../../src/i18n/en.json"))
                .expect("Embedded translations must be valid")
        })
    } else {
        ZH.get_or_init(|| {
            serde_json::from_str(include_str!("../../../src/i18n/zh-CN.json"))
                .expect("Embedded translations must be valid")
        })
    };
    catalog
        .get(key)
        .cloned()
        .unwrap_or_else(|| "AgentDesk".into())
}
pub fn refresh_tray_menu(app: &tauri::AppHandle, language: &str) -> AppResult<()> {
    if let Some(tray) = app.tray_by_id("main-tray") {
        let open = tauri::menu::MenuItem::with_id(
            app,
            "open",
            native_label(language, "tray.open"),
            true,
            None::<&str>,
        )
        .map_err(|e| AppError::io("tray", e))?;
        let exit = tauri::menu::MenuItem::with_id(
            app,
            "exit",
            native_label(language, "tray.exit"),
            true,
            None::<&str>,
        )
        .map_err(|e| AppError::io("tray", e))?;
        let menu = tauri::menu::Menu::with_items(app, &[&open, &exit])
            .map_err(|e| AppError::io("tray", e))?;
        tray.set_menu(Some(menu))
            .map_err(|e| AppError::io("tray", e))?;
    }
    Ok(())
}
