use crate::{
    error::{AppError, AppResult},
    models::Target,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use tokio::sync::{watch, Mutex};
use tokio_util::sync::CancellationToken;

struct Entry {
    id: String,
    cancel: CancellationToken,
    projects: HashSet<String>,
    state: String,
    finished: watch::Receiver<bool>,
}
#[cfg(any(windows, test))]
struct Completion(watch::Sender<bool>);
#[cfg(any(windows, test))]
impl Drop for Completion {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}
#[derive(Default)]
pub struct LocalChannels {
    entries: Mutex<HashMap<String, Entry>>,
    operations: Mutex<HashMap<String, std::sync::Arc<Mutex<()>>>>,
}
#[cfg(windows)]
fn cancelled() -> AppError {
    AppError::new(
        "E_CANCELLED",
        "local.enable",
        "Execution channel preparation was cancelled",
    )
}
impl LocalChannels {
    async fn project_operation(&self, target: &Target) -> tokio::sync::OwnedMutexGuard<()> {
        let operation = self
            .operations
            .lock()
            .await
            .entry(target.connection_id.clone())
            .or_default()
            .clone();
        operation.lock_owned().await
    }
    pub async fn status(&self, target: &Target) -> Value {
        let entries = self.entries.lock().await;
        if let Some(entry) = entries
            .get(&target.connection_id)
            .filter(|e| !e.cancel.is_cancelled())
        {
            json!({"state":entry.state,"enabled":target.project_id.as_ref().is_some_and(|p| entry.projects.contains(p))})
        } else {
            json!({"state":"closed","enabled":false})
        }
    }
    pub async fn require(&self, target: &Target) -> AppResult<()> {
        let status = self.status(target).await;
        if status["state"] != "ready" || status["enabled"] != true {
            return Err(AppError::new(
                "E_DEVICE_NOT_READY",
                "local.run",
                "Enable this project's execution channel before starting a task",
            ));
        }
        Ok(())
    }
    #[cfg(any(windows, test))]
    async fn reserve(
        &self,
        target: &Target,
    ) -> AppResult<(String, CancellationToken, Option<Completion>)> {
        let mut entries = self.entries.lock().await;
        if let Some(entry) = entries
            .get(&target.connection_id)
            .filter(|e| !*e.finished.borrow())
        {
            if entry.state != "ready" || entry.cancel.is_cancelled() {
                return Err(AppError::new(
                    "E_BUSY",
                    "local.enable",
                    "Execution channel preparation or cleanup is already running",
                ));
            }
            return Ok((entry.id.clone(), entry.cancel.clone(), None));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        let (done, finished) = watch::channel(false);
        entries.insert(
            target.connection_id.clone(),
            Entry {
                id: id.clone(),
                cancel: cancel.clone(),
                projects: HashSet::new(),
                state: "preparing".into(),
                finished,
            },
        );
        Ok((id, cancel, Some(Completion(done))))
    }
    async fn remove_finished(&self, target: &Target, id: &str) {
        let mut entries = self.entries.lock().await;
        if entries
            .get(&target.connection_id)
            .is_some_and(|e| e.id == id && *e.finished.borrow())
        {
            entries.remove(&target.connection_id);
        }
    }
    #[cfg(windows)]
    pub async fn enable(
        &self,
        state: &crate::state::AppState,
        app: tauri::AppHandle,
        target: &Target,
    ) -> AppResult<Value> {
        use crate::local::executor::{ExecutorResources, LocalExecutor};
        use tauri::{Emitter, Manager};
        let _operation = self.project_operation(target).await;
        if !crate::local::fs::local(&state.db, target)? {
            return Err(AppError::new(
                "E_TARGET",
                "local.enable",
                "Local project required",
            ));
        }
        let project_id = target
            .project_id
            .as_ref()
            .ok_or_else(|| AppError::new("E_TARGET", "local.enable", "Project target required"))?;
        let (id, cancel, completion) = self.reserve(target).await?;
        let reuse = completion.is_none();
        let mut client = None;
        let mut registration = Value::Null;
        let mut socket = String::new();
        let mut executor = None;
        let prepare = async {
            client = Some(crate::commands::session::project_client(state, target).await?);
            let client = client.as_ref().unwrap();
            if !reuse {
                let _ = app.emit("device-state", json!({"target":target,"state":"preparing"}));
                registration = client.request("device.register", json!({})).await?;
                socket = registration["tunnelSocket"]
                    .as_str()
                    .ok_or_else(|| {
                        AppError::new("E_PROTOCOL", "local.enable", "Device socket missing")
                    })?
                    .to_owned();
            }
            client
                .request("device.project.enable", json!({"projectId":project_id}))
                .await?;
            if reuse {
                return Ok(());
            }
            let resources = ExecutorResources::discover(
                &app.path()
                    .resource_dir()
                    .map_err(|e| AppError::io("local.resources", e))?,
            );
            executor = Some(LocalExecutor::start(resources).await?);
            let executor = executor.as_mut().unwrap();
            state
                .ssh
                .reverse_local(target, &socket, executor.port(), cancel.clone())
                .await?;
            {
                let mut entries = self.entries.lock().await;
                let entry = entries
                    .get_mut(&target.connection_id)
                    .filter(|e| e.id == id && !e.cancel.is_cancelled())
                    .ok_or_else(cancelled)?;
                entry.state = "probing".into();
            }
            let _ = app.emit("device-state", json!({"target":target,"state":"probing"}));
            let mut params = executor.take_credentials();
            params["generation"] = registration["generation"].clone();
            let ready = client.request("device.tunnel.ready", params).await?;
            if ready["state"] != "ready" {
                return Err(AppError::new(
                    "E_DEVICE_PROBE",
                    "local.probe",
                    "Execution probe did not complete",
                ));
            }
            Ok(())
        };
        let result = tokio::select! { biased;
            _ = cancel.cancelled() => Err(cancelled()),
            result = prepare => result,
        };
        let mut entries = self.entries.lock().await;
        let result = result.and_then(|()| {
            if entries
                .get(&target.connection_id)
                .is_some_and(|e| e.id == id && !e.cancel.is_cancelled())
            {
                Ok(())
            } else {
                Err(cancelled())
            }
        });
        if let Err(error) = result {
            drop(entries);
            if !reuse {
                cancel.cancel();
                if let Some(mut executor) = executor {
                    executor.stop().await;
                }
                if !socket.is_empty() {
                    let _ = state.ssh.cancel_reverse_local(target, &socket).await;
                }
                if let Some(client) = client {
                    if registration["generation"].is_null() {
                        client.cancel.cancel();
                    } else {
                        let offline = client.request(
                            "device.offline",
                            json!({"generation":registration["generation"]}),
                        );
                        if tokio::time::timeout(std::time::Duration::from_secs(2), offline)
                            .await
                            .is_err()
                        {
                            client.cancel.cancel();
                        }
                    }
                }
                let _ = app.emit("device-state", json!({"target":target,"state":"closed"}));
                drop(completion);
                self.remove_finished(target, &id).await;
            }
            return Err(error);
        }
        // The short commit section cannot race close's cancellation transition.
        let entry = entries
            .get_mut(&target.connection_id)
            .filter(|e| e.id == id && !e.cancel.is_cancelled())
            .ok_or_else(cancelled)?;
        entry.projects.insert(project_id.clone());
        if reuse {
            return Ok(json!({"state":"ready","enabled":true}));
        }
        let mut executor = executor.unwrap();
        let client = client.unwrap();
        let task_cancel = cancel.clone();
        let ssh = state.ssh.clone();
        let owner = target.clone();
        let generation = registration["generation"].clone();
        let mut host_events = client.events.subscribe();
        tokio::spawn(async move {
            let _completion = completion;
            let mut health = tokio::time::interval(std::time::Duration::from_secs(1));
            let failed = loop {
                tokio::select! {
                    _ = task_cancel.cancelled() => break false,
                    _ = client.cancel.cancelled() => break false,
                    event = host_events.recv() => {
                        if matches!(event, Ok(ref value) if value["type"] == "device.state" && value["payload"]["state"] == "closed") {
                            break false;
                        }
                    }
                    _ = health.tick() => {
                        if client.hello.lock().ok().is_some_and(|hello| hello["deviceState"]["state"] == "closed") {
                            break false;
                        }
                        if !executor.is_running() { break true; }
                    }
                }
            };
            task_cancel.cancel();
            executor.stop().await;
            let _ = ssh.cancel_reverse_local(&owner, &socket).await;
            let offline = client.request("device.offline", json!({"generation":generation}));
            if tokio::time::timeout(std::time::Duration::from_secs(2), offline)
                .await
                .is_err()
            {
                client.cancel.cancel();
            }
            let _ = app.emit(
                "device-state",
                json!({"target":owner,"state":if failed {"error"} else {"closed"}}),
            );
        });
        entry.state = "ready".into();
        Ok(json!({"state":"ready","enabled":true}))
    }
    pub async fn revoke(
        &self,
        state: &crate::state::AppState,
        target: &Target,
    ) -> AppResult<Value> {
        let _operation = self.project_operation(target).await;
        if !crate::local::fs::local(&state.db, target)? {
            return Err(AppError::new(
                "E_TARGET",
                "local.revoke",
                "Local project required",
            ));
        }
        let client = crate::commands::session::project_client(state, target).await?;
        let reply = client
            .request(
                "device.project.revoke",
                json!({"projectId":target.project_id}),
            )
            .await?;
        let last = {
            let mut entries = self.entries.lock().await;
            if let Some(entry) = entries.get_mut(&target.connection_id) {
                if let Some(project) = &target.project_id {
                    entry.projects.remove(project);
                }
                entry.projects.is_empty()
            } else {
                false
            }
        };
        if last || reply["state"] == "closed" {
            self.close(&state.ssh, target).await;
        }
        Ok(self.status(target).await)
    }
    async fn cancel_entry(&self, target: &Target) -> Option<(String, watch::Receiver<bool>)> {
        let mut entries = self.entries.lock().await;
        let entry = entries.get_mut(&target.connection_id)?;
        entry.cancel.cancel();
        entry.projects.clear();
        entry.state = "closed".into();
        Some((entry.id.clone(), entry.finished.clone()))
    }
    pub async fn close(&self, _ssh: &crate::ssh::manager::SshManager, target: &Target) {
        if let Some((id, mut finished)) = self.cancel_entry(target).await {
            while !*finished.borrow_and_update() {
                if finished.changed().await.is_err() {
                    break;
                }
            }
            self.remove_finished(target, &id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target(id: &str) -> Target {
        Target {
            connection_id: id.into(),
            project_id: Some("p".into()),
            session_id: None,
            request_seq: 1,
            device_local: true,
        }
    }
    #[tokio::test]
    async fn project_operations_serialize_only_the_same_connection() {
        let channels = LocalChannels::default();
        let first = channels.project_operation(&target("a")).await;
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(20),
            channels.project_operation(&target("a"))
        )
        .await
        .is_err());
        let other = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            channels.project_operation(&target("b")),
        )
        .await
        .unwrap();
        drop(first);
        let _next = channels.project_operation(&target("a")).await;
        drop(other);
    }
    #[tokio::test]
    async fn preparing_does_not_block_status_or_cancellation_and_stale_cleanup_is_rejected() {
        let channels = LocalChannels::default();
        let owner = target("a");
        let (old, cancel, completion) = channels.reserve(&owner).await.unwrap();
        assert_eq!(channels.reserve(&owner).await.err().unwrap().code, "E_BUSY");
        assert_eq!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                channels.status(&target("b"))
            )
            .await
            .unwrap()["state"],
            "closed"
        );
        assert_eq!(channels.status(&owner).await["state"], "preparing");
        let (_, mut finished) = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            channels.cancel_entry(&owner),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(cancel.is_cancelled());
        assert_eq!(channels.reserve(&owner).await.err().unwrap().code, "E_BUSY");
        drop(completion);
        finished.changed().await.unwrap();
        let (new, _, _) = channels.reserve(&owner).await.unwrap();
        assert_ne!(old, new);
        channels.remove_finished(&owner, &old).await;
        assert_eq!(channels.status(&owner).await["state"], "preparing");
        assert!(channels.require(&owner).await.is_err());
    }
}
