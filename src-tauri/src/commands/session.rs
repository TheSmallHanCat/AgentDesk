use crate::{
    error::{AppError, AppResult},
    host::client::HostClient,
    models::{Reply, Target},
    state::AppState,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

enum HostStreamEvent {
    Event(Value),
    Resync,
    Disconnected,
}
async fn next_host_event(
    receiver: &mut tokio::sync::broadcast::Receiver<Value>,
) -> HostStreamEvent {
    match receiver.recv().await {
        Ok(event) => HostStreamEvent::Event(event),
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => HostStreamEvent::Resync,
        Err(tokio::sync::broadcast::error::RecvError::Closed) => HostStreamEvent::Disconnected,
    }
}

pub async fn project_client(state: &AppState, target: &Target) -> AppResult<Arc<HostClient>> {
    state.ssh.validate_target(target)?;
    let project_id = target
        .project_id
        .as_ref()
        .ok_or_else(|| AppError::new("E_TARGET", "session", "A project target is required"))?;
    let project = state.db.project(project_id)?;
    let client = state.hosts.client(&state.ssh, &state.db, target).await?;
    if project.runtime_mode == "isolated"
        && client
            .hello
            .lock()
            .map_err(|e| AppError::io("runtime.hello", e))?["features"]["isolatedRuntime"]
            != 1
    {
        return Err(AppError::new(
            "E_RUNTIME_UNAVAILABLE",
            "runtime.hello",
            "Host does not support isolated workspaces",
        ));
    }
    let registered = client.request("project.upsert", json!({"id":project.id,"path":project.path,"name":project.name,"defaultAgent":project.default_agent,"runtimeMode":project.runtime_mode,"kind":if project.device_local {"local"} else {"remote"}})).await?;
    if project.runtime_mode == "isolated"
        && (registered["runtimeMode"] != "isolated"
            || registered["runtimeId"]
                .as_str()
                .is_none_or(|id| !id.starts_with("rt_") || id.len() != 35))
    {
        return Err(AppError::new(
            "E_TARGET",
            "runtime.project",
            "Host did not preserve the project runtime",
        ));
    }
    Ok(client)
}
fn session_id(target: &Target) -> AppResult<&str> {
    target
        .session_id
        .as_deref()
        .ok_or_else(|| AppError::new("E_TARGET", "session", "A session target is required"))
}

#[tauri::command]
pub fn session_draft_get(
    state: tauri::State<'_, AppState>,
    target: Target,
) -> AppResult<Reply<String>> {
    Ok(target.reply(state.db.draft(&target)?))
}
#[tauri::command]
pub fn session_draft_set(
    state: tauri::State<'_, AppState>,
    target: Target,
    text: String,
) -> AppResult<Reply<()>> {
    state.db.save_draft(&target, &text)?;
    Ok(target.reply(()))
}

#[tauri::command]
pub async fn session_list(
    state: tauri::State<'_, AppState>,
    target: Target,
    archived: Option<bool>,
) -> AppResult<Reply<Value>> {
    let client = project_client(&state, &target).await?;
    let sessions = client
        .request(
            "session.list",
            json!({"projectId":target.project_id,"archived":archived.unwrap_or(false)}),
        )
        .await?;
    let rows = sessions
        .as_array()
        .ok_or_else(|| AppError::new("E_PROTOCOL", "session.list", "Invalid session list"))?;
    for session in rows {
        state.db.register_session(&target, session)?;
    }
    Ok(target.reply(sessions))
}
#[tauri::command]
pub async fn session_create(
    state: tauri::State<'_, AppState>,
    target: Target,
    agent: String,
    model: String,
    permission_mode: String,
    title: Option<String>,
) -> AppResult<Reply<Value>> {
    let client = project_client(&state, &target).await?;
    let mut params = json!({"projectId":target.project_id,"agent":agent,"model":model,"permissionMode":permission_mode});
    if let Some(title) = title {
        params["title"] = json!(title);
    }
    let session = client.request("session.create", params).await?;
    state.db.register_session(&target, &session)?;
    Ok(target.reply(session))
}
#[tauri::command]
pub async fn session_history(
    state: tauri::State<'_, AppState>,
    target: Target,
    before_seq: Option<u64>,
    limit: Option<u64>,
) -> AppResult<Reply<Value>> {
    let client = project_client(&state, &target).await?;
    let mut params =
        json!({"sessionId":session_id(&target)?,"limit":limit.unwrap_or(50).clamp(1,200)});
    if let Some(before) = before_seq {
        params["beforeSeq"] = json!(before);
    }
    Ok(target.reply(client.request("session.history", params).await?))
}
#[tauri::command]
pub async fn session_rename(
    state: tauri::State<'_, AppState>,
    target: Target,
    title: String,
) -> AppResult<Reply<Value>> {
    let client = project_client(&state, &target).await?;
    Ok(target.reply(
        client
            .request(
                "session.rename",
                json!({"sessionId":session_id(&target)?,"title":title}),
            )
            .await?,
    ))
}
#[tauri::command]
pub async fn session_delete(
    state: tauri::State<'_, AppState>,
    target: Target,
) -> AppResult<Reply<Value>> {
    let client = project_client(&state, &target).await?;
    let deleted = client
        .request("session.delete", json!({"sessionId":session_id(&target)?}))
        .await?;
    if deleted["deleted"] == true {
        state.db.forget_session(&target)?;
    }
    Ok(target.reply(deleted))
}
#[tauri::command]
pub async fn session_restore(
    state: tauri::State<'_, AppState>,
    target: Target,
) -> AppResult<Reply<Value>> {
    let client = project_client(&state, &target).await?;
    Ok(target.reply(
        client
            .request("session.restore", json!({"sessionId":session_id(&target)?}))
            .await?,
    ))
}
#[tauri::command]
pub async fn event_blob(
    state: tauri::State<'_, AppState>,
    target: Target,
    seq: u64,
    offset: Option<u64>,
    limit: Option<u64>,
) -> AppResult<Reply<Value>> {
    let client = project_client(&state, &target).await?;
    Ok(target.reply(client.request("event.blob", json!({"sessionId":session_id(&target)?,"seq":seq,"offset":offset.unwrap_or(0),"limit":limit.unwrap_or(65536)})).await?))
}
#[tauri::command]
pub async fn run_send(
    state: tauri::State<'_, AppState>,
    target: Target,
    text: String,
    client_msg_id: Option<String>,
    script: Option<Value>,
) -> AppResult<Reply<Value>> {
    if crate::local::fs::local(&state.db, &target)? {
        state.local_channels.require(&target).await?;
    }
    let client = project_client(&state, &target).await?;
    let id = client_msg_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut params = json!({"sessionId":session_id(&target)?,"text":text,"clientMsgId":id});
    if let Some(script) = script {
        params["script"] = script;
    }
    Ok(target.reply(client.request("run.send", params).await?))
}
#[tauri::command]
pub async fn run_append(
    state: tauri::State<'_, AppState>,
    target: Target,
    run_id: String,
    text: String,
    client_msg_id: String,
) -> AppResult<Reply<Value>> {
    if crate::local::fs::local(&state.db, &target)? {
        state.local_channels.require(&target).await?;
    }
    let client = project_client(&state, &target).await?;
    Ok(target.reply(
        client
            .request(
                "run.append",
                json!({"sessionId":session_id(&target)?,"runId":run_id,"text":text,"clientMsgId":client_msg_id}),
            )
            .await?,
    ))
}
#[tauri::command]
pub async fn run_stop(
    state: tauri::State<'_, AppState>,
    target: Target,
    run_id: String,
) -> AppResult<Reply<Value>> {
    let client = project_client(&state, &target).await?;
    Ok(target.reply(
        client
            .request(
                "run.stop",
                json!({"sessionId":session_id(&target)?,"runId":run_id}),
            )
            .await?,
    ))
}
#[tauri::command]
pub async fn approval_respond(
    state: tauri::State<'_, AppState>,
    target: Target,
    approval_id: String,
    decision: Value,
) -> AppResult<Reply<Value>> {
    let client = project_client(&state, &target).await?;
    Ok(target.reply(client.request("approval.respond", json!({"sessionId":session_id(&target)?,"approvalId":approval_id,"decision":decision})).await?))
}
#[tauri::command]
pub async fn run_set_option(
    state: tauri::State<'_, AppState>,
    target: Target,
) -> AppResult<Reply<Value>> {
    state.ssh.validate_target(&target)?;
    Err(AppError::new(
        "E_CAPABILITY",
        "run.option",
        "Run options are unavailable in M2",
    ))
}

#[tauri::command]
pub async fn session_subscribe(
    state: tauri::State<'_, AppState>,
    target: Target,
    last_seq: u64,
    on_events: tauri::ipc::Channel<Value>,
) -> AppResult<Reply<Value>> {
    let initial = project_client(&state, &target).await?;
    session_id(&target)?;
    let id = uuid::Uuid::new_v4().to_string();
    let cancel = tokio_util::sync::CancellationToken::new();
    state
        .hosts
        .subscriptions
        .lock()
        .map_err(|e| AppError::io("session.subscribe", e))?
        .insert(id.clone(), (target.clone(), cancel.clone()));
    let (pool, ssh, db) = (state.hosts.clone(), state.ssh.clone(), state.db.clone());
    let owner = target.clone();
    let subscription_id = id.clone();
    tokio::spawn(async move {
        let mut last_seq = last_seq;
        let mut client = initial;
        let mut delay = 1;
        'connection: loop {
            let mut receiver = client.events.subscribe();
            let subscribe = async {
                client
                    .request(
                        "sync.subscribe",
                        json!({"sessionId":owner.session_id,"subscriptionId":subscription_id}),
                    )
                    .await?;
                loop {
                    let page = client
                        .request(
                            "sync.since",
                            json!({"sessionId":owner.session_id,"lastSeq":last_seq,"limit":100}),
                        )
                        .await?;
                    let mut events = page["events"].as_array().cloned().unwrap_or_default();
                    events.extend(page["snapshots"].as_array().cloned().unwrap_or_default());
                    if !events.is_empty() {
                        on_events
                            .send(json!({"target":owner,"events":events}))
                            .map_err(|e| AppError::io("session.channel", e))?;
                    }
                    last_seq = page["currentSeq"].as_u64().unwrap_or(last_seq);
                    if page["hasMore"] != true {
                        break;
                    }
                }
                AppResult::Ok(())
            };
            let synced =
                tokio::select! { _ = cancel.cancelled() => break, result = subscribe => result };
            if synced.is_ok() {
                delay = 1;
                let mut lagged = false;
                let mut batch = Vec::new();
                let mut size = 0;
                let mut tick = tokio::time::interval(Duration::from_millis(16));
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = client.cancel.cancelled() => break,
                        _ = tick.tick() => {
                            if !batch.is_empty() {
                                if on_events.send(json!({"target":owner,"events":std::mem::take(&mut batch)})).is_err() { cancel.cancel(); break; }
                                size=0;
                            }
                        }
                        event = next_host_event(&mut receiver) => match event {
                            HostStreamEvent::Event(event) => {
                                if event["sessionId"].as_str() != owner.session_id.as_deref() { continue; }
                                let seq = event["seq"].as_u64().unwrap_or(0);
                                if seq <= last_seq { continue; }
                                last_seq = seq;
                                size += serde_json::to_vec(&event).map(|v| v.len()).unwrap_or(0);
                                batch.push(event);
                                if size >= 32768 {
                                    if on_events.send(json!({"target":owner,"events":std::mem::take(&mut batch)})).is_err() { cancel.cancel(); break; }
                                    size=0;
                                }
                            }
                            HostStreamEvent::Resync => { lagged = true; break; }
                            HostStreamEvent::Disconnected => break,
                        }
                    }
                }
                if !batch.is_empty() {
                    let _ = on_events.send(json!({"target":owner,"events":batch}));
                }
                if lagged && !cancel.is_cancelled() {
                    continue 'connection;
                }
            }
            if cancel.is_cancelled() {
                break;
            }
            if on_events
                .send(json!({"target":owner,"events":[],"disconnected":true}))
                .is_err()
            {
                break;
            }
            tokio::select! { _ = cancel.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(delay)) => {} }
            delay = (delay * 2).min(30);
            match pool.client(&ssh, &db, &owner).await {
                Ok(next) => client = next,
                Err(_) => continue,
            }
        }
        let _ = client
            .request(
                "sync.unsubscribe",
                json!({"sessionId":owner.session_id,"subscriptionId":subscription_id}),
            )
            .await;
        let _ =
            on_events.send(json!({"target":owner,"events":[],"disconnected":true,"closed":true}));
        let _ = pool.unsubscribe(&owner, &subscription_id);
    });
    Ok(target.reply(json!({"subscriptionId":id})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn broadcast_overflow_requests_replay_and_keeps_the_stream_usable() {
        let (sender, mut receiver) = tokio::sync::broadcast::channel(1);
        sender.send(json!({"seq":1})).unwrap();
        sender.send(json!({"seq":2})).unwrap();
        assert!(matches!(
            next_host_event(&mut receiver).await,
            HostStreamEvent::Resync
        ));
        assert!(
            matches!(next_host_event(&mut receiver).await, HostStreamEvent::Event(event) if event["seq"] == 2)
        );
        drop(sender);
        assert!(matches!(
            next_host_event(&mut receiver).await,
            HostStreamEvent::Disconnected
        ));
    }
}
#[tauri::command]
pub fn session_unsubscribe(
    state: tauri::State<'_, AppState>,
    target: Target,
    subscription_id: String,
) -> AppResult<Reply<()>> {
    state.ssh.validate_target(&target)?;
    state.hosts.unsubscribe(&target, &subscription_id)?;
    Ok(target.reply(()))
}
