use crate::{
    error::{AppError, AppResult},
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
    state.ssh.validate_target(&target)?;
    if kind != "shell" {
        let client = crate::commands::session::project_client(&state, &target).await?;
        let plan = client
            .request("agent.terminal.start", serde_json::json!({"agent":kind}))
            .await?;
        let command = plan["command"]
            .as_str()
            .ok_or_else(|| AppError::new("E_PROTOCOL", "term.open", "Agent command unavailable"))?;
        let working_path = if crate::local::fs::local(&state.db, &target)? {
            let project = state
                .db
                .project(target.project_id.as_deref().unwrap_or_default())?;
            let registered = client.request("project.upsert", serde_json::json!({"id":project.id,"name":project.name,"path":project.path,"kind":"local"})).await?;
            registered["nativeRoot"]
                .as_str()
                .ok_or_else(|| {
                    AppError::new(
                        "E_PROTOCOL",
                        "term.open",
                        "Native working directory missing",
                    )
                })?
                .to_owned()
        } else {
            crate::ssh::sftp::remote_path(&path)?
        };
        let script = format!(
            "cd {} || exit 1\nstty echo || exit 1\nexec {command}\n",
            crate::ssh::sftp::quote(&working_path)
        );
        let id = state
            .terminals
            .open_script(&state.ssh, target.clone(), script, cols, rows, on_output)
            .await?;
        return Ok(target.reply(serde_json::json!({"terminalId":id})));
    }
    if crate::local::fs::local(&state.db, &target)? {
        let path = crate::local::fs::checked(&state.db, &target, &path, false, true)?;
        let shell = state.storage.settings()?.local_shell;
        let id = state
            .local_terminals
            .open(
                target.clone(),
                &path.to_string_lossy(),
                &shell,
                cols,
                rows,
                on_output,
            )
            .await?;
        return Ok(target.reply(serde_json::json!({"terminalId":id})));
    }
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
    if terminal_id.starts_with("local-") {
        state
            .local_terminals
            .write(&target, &terminal_id, data)
            .await?;
    } else {
        state.terminals.write(&target, &terminal_id, data).await?;
    }
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
    if terminal_id.starts_with("local-") {
        state
            .local_terminals
            .resize(&target, &terminal_id, cols, rows)
            .await?;
    } else {
        state
            .terminals
            .resize(&target, &terminal_id, cols, rows)
            .await?;
    }
    Ok(target.reply(()))
}
#[tauri::command]
pub fn term_ack(
    state: tauri::State<'_, AppState>,
    target: Target,
    terminal_id: String,
    seq: u64,
) -> AppResult<()> {
    state.ssh.validate_target(&target)?;
    if terminal_id.starts_with("local-") {
        state.local_terminals.ack(&target, &terminal_id, seq)
    } else {
        state.terminals.ack(&target, &terminal_id, seq)
    }
}
#[tauri::command]
pub fn term_close(
    state: tauri::State<'_, AppState>,
    target: Target,
    terminal_id: String,
) -> AppResult<Reply<()>> {
    state.ssh.validate_target(&target)?;
    if terminal_id.starts_with("local-") {
        state.local_terminals.close(&target, &terminal_id)?;
    } else {
        state.terminals.close(&target, &terminal_id)?;
    }
    Ok(target.reply(()))
}
#[tauri::command]
pub fn term_reset(state: tauri::State<'_, AppState>) -> AppResult<()> {
    state.local_terminals.reset_frontend()?;
    state.terminals.reset_frontend()
}
