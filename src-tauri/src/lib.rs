pub mod commands;
pub mod db;
pub mod error;
pub mod host;
pub mod local;
pub mod models;
pub mod platform;
pub mod secrets;
pub mod session_log;
pub mod ssh;
pub mod state;
pub mod storage;

use tauri::{Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            commands::sys::restore(app, None)
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(state::AppState::new(handle.clone())?);
            let notify_handle = handle.clone();
            handle.notification().on_action(move |action| {
                let data = serde_json::to_value(action).unwrap_or_default();
                let project = data
                    .pointer("/notification/extra/projectId")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
                let session = data
                    .pointer("/notification/extra/sessionId")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned);
                commands::sys::restore_session(&notify_handle, project, session);
            })?;
            let settings = app
                .state::<state::AppState>()
                .storage
                .settings()
                .unwrap_or_default();
            let open = tauri::menu::MenuItem::with_id(
                app,
                "open",
                commands::sys::native_label(&settings.language, "tray.open"),
                true,
                None::<&str>,
            )?;
            let exit = tauri::menu::MenuItem::with_id(
                app,
                "exit",
                commands::sys::native_label(&settings.language, "tray.exit"),
                true,
                None::<&str>,
            )?;
            let menu = tauri::menu::Menu::with_items(app, &[&open, &exit])?;
            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))?;
            tauri::tray::TrayIconBuilder::with_id("main-tray")
                .icon(icon)
                .tooltip("AgentDesk")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => commands::sys::restore(app, None),
                    "exit" => {
                        let _ =
                            app.emit("app-close-requested", serde_json::json!({"forceExit":true}));
                        commands::sys::restore(app, None);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if matches!(event, tauri::tray::TrayIconEvent::DoubleClick { .. }) {
                        commands::sys::restore(tray.app_handle(), None);
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.emit(
                    "app-close-requested",
                    serde_json::json!({"forceExit":false}),
                );
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::device::device_status,
            commands::device::device_enable,
            commands::device::device_revoke,
            commands::env::env_probe,
            commands::env::env_install_plan,
            commands::env::env_install_run,
            commands::env::env_install_cancel,
            commands::env::host_start,
            commands::env::host_stop,
            commands::env::host_status,
            commands::env::host_rollback,
            commands::login::agent_status,
            commands::login::agent_login_start,
            commands::login::agent_login_respond,
            commands::login::agent_login_cancel,
            commands::login::agent_logout,
            commands::session::session_list,
            commands::session::session_draft_get,
            commands::session::session_draft_set,
            commands::session::session_create,
            commands::session::session_history,
            commands::session::session_rename,
            commands::session::session_delete,
            commands::session::session_restore,
            commands::session::session_subscribe,
            commands::session::session_unsubscribe,
            commands::session::event_blob,
            commands::session::run_send,
            commands::session::run_append,
            commands::session::run_stop,
            commands::session::approval_respond,
            commands::session::run_set_option,
            commands::conn::conn_list,
            commands::conn::conn_save,
            commands::conn::conn_delete,
            commands::conn::conn_statuses,
            commands::conn::conn_secret_status,
            commands::conn::conn_connect,
            commands::conn::conn_disconnect,
            commands::conn::conn_trust_host_key,
            commands::conn::conn_import_ssh_config,
            commands::sys::settings_get,
            commands::sys::settings_set,
            commands::sys::diag_list,
            commands::sys::diag_export,
            commands::sys::editor_save_local,
            commands::sys::sys_hide,
            commands::sys::sys_exit,
            commands::sys::sys_notify,
            commands::project::project_list,
            commands::project::project_save,
            commands::project::project_pick_local,
            commands::project::project_archive,
            commands::project::dir_recent,
            commands::project::dir_complete,
            commands::file::fs_list,
            commands::file::fs_read,
            commands::file::fs_stat,
            commands::file::fs_write,
            commands::file::fs_create,
            commands::file::fs_mkdir,
            commands::file::fs_rename,
            commands::file::fs_delete,
            commands::term::term_open,
            commands::term::term_write,
            commands::term::term_resize,
            commands::term::term_close,
            commands::term::term_ack,
            commands::term::term_reset,
            commands::transfer::transfer_list,
            commands::transfer::transfer_download,
            commands::transfer::transfer_cancel,
        ])
        .run(tauri::generate_context!())
        .expect("AgentDesk startup failed");
}
