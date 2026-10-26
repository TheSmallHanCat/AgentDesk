use crate::{
    error::{AppError, AppResult},
    models::{Connection, ConnectionStatus, Reply, Target},
    ssh::config_import,
    state::AppState,
};
#[tauri::command]
pub fn conn_list(state: tauri::State<'_, AppState>) -> AppResult<Vec<Connection>> {
    state.db.connections()
}
#[tauri::command]
pub fn conn_save(
    state: tauri::State<'_, AppState>,
    mut connection: Connection,
) -> AppResult<Connection> {
    if state.ssh.is_active(&connection.id) {
        return Err(AppError::new(
            "E_BUSY",
            "connection",
            "Disconnect before editing this connection",
        ));
    }
    if connection.id.is_empty() {
        connection.id = uuid::Uuid::new_v4().to_string();
    }
    let (host, user, port) =
        config_import::normalize_endpoint(&connection.host, &connection.user, connection.port)?;
    connection.host = host;
    connection.user = user;
    connection.port = port;
    if let Ok(existing) = state.db.connection(&connection.id) {
        if (existing.host != connection.host
            || existing.port != connection.port
            || existing.user != connection.user)
            && state
                .db
                .projects()?
                .iter()
                .any(|p| p.connection_id == connection.id)
        {
            return Err(AppError::new("E_TARGET","connection","This connection owns project indexes. Add another connection to change its host, port or user without redirecting existing projects"));
        }
    }
    state.db.save_connection(&connection)?;
    Ok(connection)
}
#[tauri::command]
pub fn conn_delete(state: tauri::State<'_, AppState>, id: String) -> AppResult<()> {
    if state.ssh.is_active(&id) {
        return Err(AppError::new(
            "E_BUSY",
            "connection",
            "Disconnect before deleting this connection",
        ));
    }
    state.db.delete_connection(&id)?;
    crate::secrets::Secrets::clear(&id, "password")?;
    crate::secrets::Secrets::clear(&id, "passphrase")
}
#[tauri::command]
pub fn conn_statuses(state: tauri::State<'_, AppState>) -> AppResult<Vec<ConnectionStatus>> {
    state.ssh.statuses()
}
#[tauri::command]
pub async fn conn_connect(
    state: tauri::State<'_, AppState>,
    target: Target,
    credential: Option<String>,
    save_secret: bool,
) -> AppResult<Reply<ConnectionStatus>> {
    let status = state.ssh.connect(&target, credential, save_secret).await?;
    Ok(target.reply(status))
}
#[tauri::command]
pub async fn conn_disconnect(
    state: tauri::State<'_, AppState>,
    target: Target,
) -> AppResult<Reply<()>> {
    state.ssh.validate_target(&target)?;
    state
        .terminals
        .close_connection(&target.connection_id)
        .await;
    state
        .transfers
        .close_connection(&target.connection_id)
        .await;
    state.ssh.disconnect(&target).await?;
    Ok(target.reply(()))
}
#[tauri::command]
pub fn conn_trust_host_key(
    state: tauri::State<'_, AppState>,
    target: Target,
    fingerprint: String,
) -> AppResult<Reply<()>> {
    state.ssh.trust(&target, &fingerprint)?;
    Ok(target.reply(()))
}
#[tauri::command]
pub fn conn_import_ssh_config() -> AppResult<config_import::Imported> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| AppError::new("E_STORAGE", "import", "Home directory unavailable"))?;
    let text = std::fs::read_to_string(base.home_dir().join(".ssh/config"))
        .map_err(|e| AppError::io("import", e))?;
    config_import::parse(
        &text,
        &base.home_dir().to_string_lossy(),
        &std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_default(),
    )
}
