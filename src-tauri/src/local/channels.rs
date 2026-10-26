use crate::{
    error::{AppError, AppResult},
    models::Target,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

struct Entry {
    socket: String,
    cancel: CancellationToken,
    projects: HashSet<String>,
    state: String,
    finished: tokio::task::JoinHandle<()>,
}
#[derive(Default)]
pub struct LocalChannels {
    entries: Mutex<HashMap<String, Entry>>,
}
impl LocalChannels {
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
    #[cfg(windows)]
    pub async fn enable(
        &self,
        state: &crate::state::AppState,
        app: tauri::AppHandle,
        target: &Target,
    ) -> AppResult<Value> {
        use crate::local::executor::{ExecutorResources, LocalExecutor};
        use tauri::{Emitter, Manager};
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
        let mut entries = self.entries.lock().await;
        if let Some(entry) = entries
            .get_mut(&target.connection_id)
            .filter(|e| !e.cancel.is_cancelled() && e.state == "ready")
        {
            entry.projects.insert(project_id.clone());
            return Ok(json!({"state":"ready","enabled":true}));
        }
        if let Some(previous) = entries.remove(&target.connection_id) {
            previous.cancel.cancel();
            let _ = previous.finished.await;
        }
        let owner = target.clone();
        let notify = |value: &str| {
            let _ = app.emit("device-state", json!({"target":owner,"state":value}));
        };
        notify("preparing");
        let client = state.hosts.client(&state.ssh, &state.db, target).await?;
        let registration = client.request("device.register", json!({})).await?;
        let socket = registration["tunnelSocket"]
            .as_str()
            .ok_or_else(|| AppError::new("E_PROTOCOL", "local.enable", "Device socket missing"))?
            .to_owned();
        let prepared = async {
            let resources = ExecutorResources::discover(
                &app.path()
                    .resource_dir()
                    .map_err(|e| AppError::io("local.resources", e))?,
            );
            LocalExecutor::start(resources).await
        }
        .await;
        let mut executor = match prepared {
            Ok(executor) => executor,
            Err(error) => {
                let _ = client.request("device.offline", json!({})).await;
                notify("error");
                return Err(error);
            }
        };
        let cancel = CancellationToken::new();
        let result = async {
            state
                .ssh
                .reverse_local(target, &socket, executor.port(), cancel.clone())
                .await?;
            notify("probing");
            let mut params = serde_json::to_value(executor.credentials()).map_err(|_| {
                AppError::new(
                    "E_EXECUTOR",
                    "local.credentials",
                    "Cannot encode execution credentials",
                )
            })?;
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
        }
        .await;
        if let Err(error) = result {
            let _ = state.ssh.cancel_reverse_local(target, &socket).await;
            let _ = client.request("device.offline", json!({})).await;
            executor.stop().await;
            notify("error");
            return Err(error);
        }
        let task_cancel = cancel.clone();
        let ssh = state.ssh.clone();
        let task_socket = socket.clone();
        let task_owner = target.clone();
        let finished = tokio::spawn(async move {
            let mut health = tokio::time::interval(std::time::Duration::from_secs(1));
            let failed = loop {
                tokio::select! {
                    _ = task_cancel.cancelled() => break false,
                    _ = client.cancel.cancelled() => break false,
                    _ = health.tick() => { if !executor.is_running() { break true; } }
                }
            };
            task_cancel.cancel();
            executor.stop().await;
            let _ = ssh.cancel_reverse_local(&task_owner, &task_socket).await;
            let _ = client.request("device.offline", json!({})).await;
            let _ = app.emit(
                "device-state",
                json!({"target":task_owner,"state":if failed {"error"} else {"closed"}}),
            );
        });
        entries.insert(
            target.connection_id.clone(),
            Entry {
                socket,
                cancel,
                projects: HashSet::from([project_id.clone()]),
                state: "ready".into(),
                finished,
            },
        );
        Ok(json!({"state":"ready","enabled":true}))
    }
    pub async fn close(&self, ssh: &crate::ssh::manager::SshManager, target: &Target) {
        if let Some(entry) = self.entries.lock().await.remove(&target.connection_id) {
            entry.cancel.cancel();
            let _ = ssh.cancel_reverse_local(target, &entry.socket).await;
            let _ = entry.finished.await;
        }
    }
}
