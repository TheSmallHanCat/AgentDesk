use crate::{
    error::AppResult,
    models::{Reply, Target},
    ssh::transfer::Transfer,
    state::AppState,
};
#[tauri::command]
pub fn transfer_list(state: tauri::State<'_, AppState>) -> AppResult<Vec<Transfer>> {
    state.transfers.list()
}
#[tauri::command]
pub fn transfer_download(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    target: Target,
    path: String,
    destination: String,
) -> AppResult<Reply<serde_json::Value>> {
    if crate::local::fs::local(&state.db, &target)? {
        return Err(crate::error::AppError::new(
            "E_CAPABILITY",
            "transfer.download",
            "Local project files are already on this device",
        ));
    }
    let id = state
        .transfers
        .download(app, state.ssh.clone(), target.clone(), path, destination)?;
    Ok(target.reply(serde_json::json!({"transferId":id})))
}
#[tauri::command]
pub fn transfer_cancel(
    state: tauri::State<'_, AppState>,
    target: Target,
    transfer_id: String,
) -> AppResult<Reply<()>> {
    state.transfers.cancel(&target, &transfer_id)?;
    Ok(target.reply(()))
}
