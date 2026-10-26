use crate::{
    error::AppResult,
    models::{Reply, Target},
    ssh::pty::TerminalFrame,
    state::AppState,
};
#[tauri::command]
pub async fn term_open(
    state: tauri::State<'_, AppState>,
    target: Target,
    path: String,
    kind: String,
    cols: u32,
    rows: u32,
    on_output: tauri::ipc::Channel<TerminalFrame>,
) -> AppResult<Reply<serde_json::Value>> {
    let id = state
        .terminals
        .open(
            &state.ssh,
            target.clone(),
            &path,
            &kind,
            cols,
            rows,
            on_output,
        )
        .await?;
    Ok(target.reply(serde_json::json!({"terminalId":id})))
}
#[tauri::command]
pub async fn term_write(
    state: tauri::State<'_, AppState>,
    target: Target,
    terminal_id: String,
    data: String,
) -> AppResult<Reply<()>> {
    state.ssh.validate_target(&target)?;
    state.terminals.write(&target, &terminal_id, data).await?;
    Ok(target.reply(()))
}
#[tauri::command]
pub async fn term_resize(
    state: tauri::State<'_, AppState>,
    target: Target,
    terminal_id: String,
    cols: u32,
    rows: u32,
) -> AppResult<Reply<()>> {
    state.ssh.validate_target(&target)?;
    state
        .terminals
        .resize(&target, &terminal_id, cols, rows)
        .await?;
    Ok(target.reply(()))
}
#[tauri::command]
pub fn term_ack(
    state: tauri::State<'_, AppState>,
    target: Target,
    terminal_id: String,
    seq: u64,
) -> AppResult<()> {
    state.terminals.ack(&target, &terminal_id, seq)
}
#[tauri::command]
pub fn term_close(
    state: tauri::State<'_, AppState>,
    target: Target,
    terminal_id: String,
) -> AppResult<Reply<()>> {
    state.terminals.close(&target, &terminal_id)?;
    Ok(target.reply(()))
}
