use crate::{
    error::AppResult,
    models::{Reply, Target},
    state::AppState,
};
use serde_json::Value;

#[tauri::command]
pub async fn device_status(
    state: tauri::State<'_, AppState>,
    target: Target,
) -> AppResult<Reply<Value>> {
    state.ssh.validate_target(&target)?;
    Ok(target.reply(state.local_channels.status(&target).await))
}
#[tauri::command]
pub async fn device_enable(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    target: Target,
    confirmed: bool,
) -> AppResult<Reply<Value>> {
    state.ssh.validate_target(&target)?;
    if !confirmed {
        return Err(crate::error::AppError::new(
            "E_CONFIRMATION",
            "local.enable",
            "Explicit project authorization required",
        ));
    }
    #[cfg(windows)]
    {
        Ok(target.reply(state.local_channels.enable(&state, app, &target).await?))
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Err(crate::error::AppError::new(
            "E_PLATFORM",
            "local.enable",
            "Local execution is currently supported on Windows",
        ))
    }
}
