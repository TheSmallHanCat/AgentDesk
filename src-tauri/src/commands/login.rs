use crate::{
    error::{AppError, AppResult},
    host::client::HostClient,
    models::{Reply, Target},
    state::AppState,
};
use serde_json::{json, Value};
use std::sync::Arc;

fn same_login_owner(owner: &Target, target: &Target) -> AppResult<()> {
    if owner != target {
        return Err(AppError::new(
            "E_TARGET",
            "login.owner",
            "Login belongs to another target or request",
        ));
    }
    Ok(())
}

fn started_login_id(
    cancel: &tokio_util::sync::CancellationToken,
    started: &Value,
    agent: &str,
) -> AppResult<String> {
    if let Some(identifier) = started["loginId"].as_str().filter(|id| !id.is_empty()) {
        if started["agent"] == agent {
            return Ok(identifier.to_owned());
        }
    }
    // The Host may already own a process. Without a valid owner response,
    // disconnect so it is reaped instead of abandoning an unknown login id.
    cancel.cancel();
    Err(AppError::new(
        "E_PROTOCOL",
        "login.start",
        "Host returned an invalid login owner",
    ))
}

fn login_client(state: &AppState, target: &Target, login_id: &str) -> AppResult<Arc<HostClient>> {
    state.ssh.validate_target(target)?;
    let entries = state
        .hosts
        .logins
        .lock()
        .map_err(|e| AppError::io("login.owner", e))?;
    let (owner, client) = entries.get(login_id).ok_or_else(|| {
        AppError::new(
            "E_NOT_FOUND",
            "login.owner",
            "Login session is no longer active",
        )
    })?;
    same_login_owner(owner, target)?;
    client
        .upgrade()
        .filter(|c| !c.cancel.is_cancelled())
        .ok_or_else(|| {
            AppError::new(
                "E_DISCONNECTED",
                "login.owner",
                "Login connection has closed",
            )
        })
}

async fn scoped_client(state: &AppState, target: &Target) -> AppResult<Arc<HostClient>> {
    if target.project_id.is_some() {
        crate::commands::session::project_client(state, target).await
    } else {
        state.hosts.client(&state.ssh, &state.db, target).await
    }
}
fn scoped_params(target: &Target, mut params: Value) -> Value {
    if let Some(id) = &target.project_id {
        params["projectId"] = json!(id);
    }
    params
}
#[tauri::command]
pub async fn agent_status(
    state: tauri::State<'_, AppState>,
    target: Target,
) -> AppResult<Reply<Value>> {
    let client = scoped_client(&state, &target).await?;
    Ok(target.reply(
        client
            .request("agent.status", scoped_params(&target, json!({})))
            .await?,
    ))
}

#[tauri::command]
pub async fn agent_login_start(
    state: tauri::State<'_, AppState>,
    target: Target,
    agent: String,
    on_events: tauri::ipc::Channel<Value>,
) -> AppResult<Reply<Value>> {
    let client = scoped_client(&state, &target).await?;
    // Subscribe before start: even prompts emitted before the RPC response remain live.
    let mut receiver = client.login_events.subscribe();
    let started = client
        .request(
            "login.start",
            scoped_params(&target, json!({"agent":agent})),
        )
        .await?;
    let login_id = started_login_id(&client.cancel, &started, &agent)?;
    state
        .hosts
        .logins
        .lock()
        .map_err(|e| {
            client.cancel.cancel();
            AppError::io("login.owner", e)
        })?
        .insert(login_id.clone(), (target.clone(), Arc::downgrade(&client)));
    let pool = state.hosts.clone();
    let owner = target.clone();
    tokio::spawn(async move {
        loop {
            let received = tokio::select! {
                _ = client.cancel.cancelled() => None,
                value = receiver.recv() => value.ok(),
            };
            let Some(frame) = received else {
                let _ = on_events.send(json!({"target":owner,"method":"login.state","data":{"loginId":login_id,"agent":agent,"state":"failed","reason":"E_DISCONNECTED"},"disconnected":true}));
                // A lagged login stream is invalidated, never replayed.
                if !client.cancel.is_cancelled() {
                    let _ = client
                        .request("login.cancel", json!({"loginId":login_id}))
                        .await;
                }
                break;
            };
            if frame["data"]["loginId"] != login_id || frame["data"]["agent"] != agent {
                continue;
            }
            let terminal = frame["method"] == "login.state"
                && matches!(
                    frame["data"]["state"].as_str(),
                    Some("succeeded" | "failed" | "expired" | "cancelled")
                );
            if on_events
                .send(json!({"target":owner,"method":frame["method"],"data":frame["data"]}))
                .is_err()
            {
                let _ = client
                    .request("login.cancel", json!({"loginId":login_id}))
                    .await;
                break;
            }
            if terminal {
                break;
            }
        }
        if let Ok(mut entries) = pool.logins.lock() {
            entries.remove(&login_id);
        }
    });
    Ok(target.reply(started))
}

#[tauri::command]
pub async fn agent_login_respond(
    state: tauri::State<'_, AppState>,
    target: Target,
    login_id: String,
    text: String,
) -> AppResult<Reply<Value>> {
    let client = login_client(&state, &target, &login_id)?;
    Ok(target.reply(
        client
            .request("login.respond", json!({"loginId":login_id,"text":text}))
            .await?,
    ))
}

#[tauri::command]
pub async fn agent_login_cancel(
    state: tauri::State<'_, AppState>,
    target: Target,
    login_id: String,
) -> AppResult<Reply<Value>> {
    let client = login_client(&state, &target, &login_id)?;
    Ok(target.reply(
        client
            .request("login.cancel", json!({"loginId":login_id}))
            .await?,
    ))
}

#[tauri::command]
pub async fn agent_logout(
    state: tauri::State<'_, AppState>,
    target: Target,
    agent: String,
) -> AppResult<Reply<Value>> {
    let client = scoped_client(&state, &target).await?;
    Ok(target.reply(
        client
            .request(
                "agent.logout",
                scoped_params(&target, json!({"agent":agent})),
            )
            .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_start_owner_disconnects_to_reap_unknown_login() {
        for started in [
            json!({}),
            json!({"loginId":"","agent":"claude"}),
            json!({"loginId":"id","agent":"codex"}),
        ] {
            let cancel = tokio_util::sync::CancellationToken::new();
            assert_eq!(
                started_login_id(&cancel, &started, "claude")
                    .unwrap_err()
                    .code,
                "E_PROTOCOL"
            );
            assert!(cancel.is_cancelled());
        }
        let cancel = tokio_util::sync::CancellationToken::new();
        assert_eq!(
            started_login_id(&cancel, &json!({"loginId":"id","agent":"claude"}), "claude").unwrap(),
            "id"
        );
        assert!(!cancel.is_cancelled());
    }
    #[test]
    fn login_owner_includes_request_and_all_target_fields() {
        let owner = Target {
            device_local: false,
            connection_id: "connection".into(),
            project_id: Some("project".into()),
            session_id: Some("session".into()),
            request_seq: 5,
        };
        assert!(same_login_owner(&owner, &owner).is_ok());
        for changed in [
            "connectionId",
            "projectId",
            "sessionId",
            "requestSeq",
            "deviceLocal",
        ] {
            let mut value = serde_json::to_value(&owner).unwrap();
            value[changed] = match changed {
                "requestSeq" => json!(6),
                "deviceLocal" => json!(true),
                _ => json!("other"),
            };
            let target: Target = serde_json::from_value(value).unwrap();
            assert_eq!(
                same_login_owner(&owner, &target).unwrap_err().code,
                "E_TARGET"
            );
        }
    }
}
