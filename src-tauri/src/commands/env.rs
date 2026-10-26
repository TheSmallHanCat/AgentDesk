use crate::{
    error::{AppError, AppResult},
    host::{
        client::{remote_home, HostClient},
        installer::{
            restore_script, rollback_script_for, snapshot_script, start_script, HostLinks,
            STOP_SCRIPT,
        },
    },
    models::{Reply, Target},
    state::AppState,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::Manager;

fn install_cancelled() -> AppError {
    AppError::new("E_CANCELLED", "env.install", "Installation cancelled")
}
fn has_fake(client: &Arc<HostClient>) -> AppResult<bool> {
    Ok(client
        .hello
        .lock()
        .map_err(|e| AppError::io("host.status", e))?["capabilities"]
        .get("fake")
        .is_some())
}
async fn prepare_shutdown(client: &HostClient) -> AppResult<()> {
    let result = client.request("host.prepareShutdown", json!({})).await?;
    if result["prepared"] != true {
        return Err(AppError::new(
            "E_PROTOCOL",
            "host.shutdown",
            "Host did not confirm its maintenance barrier",
        ));
    }
    Ok(())
}
async fn host_or_inactive(state: &AppState, target: &Target) -> AppResult<Option<Arc<HostClient>>> {
    match state.hosts.client(&state.ssh, &state.db, target).await {
        Ok(client) => Ok(Some(client)),
        Err(error) => {
            if error.code == "E_HOST_ID_CHANGED" {
                return Err(error);
            }
            // A live but unreachable Host cannot safely be treated as idle.
            state
                .ssh
                .exec(
                    target,
                    r#"set -eu
pidfile="$HOME/.agentdesk/run/host.pid"
if test -r "$pidfile"; then
  pid=$(cat "$pidfile")
  case "$pid" in ''|*[!0-9]*) exit 74;; esac
  if kill -0 "$pid" 2>/dev/null; then exit 74; fi
fi
"#,
                )
                .await
                .map_err(|_| error)?;
            Ok(None)
        }
    }
}
async fn recover_host(
    state: &AppState,
    target: &Target,
    before: &HostLinks,
    expected: &str,
    restart_fake: Option<bool>,
) -> AppResult<()> {
    let client = state.hosts.client(&state.ssh, &state.db, target).await.ok();
    if let Some(client) = &client {
        prepare_shutdown(client).await?;
    }
    state.hosts.disconnect(&target.connection_id).await;
    let recovered = state
        .ssh
        .exec_stage(
            target,
            "install.recover",
            &restore_script(before, expected, restart_fake),
        )
        .await;
    if let Some(client) = client {
        let _ = client.request("host.cancelShutdown", json!({})).await;
    }
    recovered?;
    state.hosts.disconnect(&target.connection_id).await;
    if restart_fake.is_some() && before.current.is_some() {
        state
            .hosts
            .client(&state.ssh, &state.db, target)
            .await?
            .request("env.probe", json!({}))
            .await?;
    }
    Ok(())
}

fn resources(app: &tauri::AppHandle) -> AppResult<std::path::PathBuf> {
    if cfg!(debug_assertions) {
        return Ok(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../resources/host"));
    }
    app.path()
        .resource_dir()
        .map(|path| path.join("resources/host"))
        .map_err(|e| AppError::io("env.resources", e))
}
#[tauri::command]
pub async fn env_probe(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    target: Target,
) -> AppResult<Reply<Value>> {
    let output = state
        .ssh
        .exec(&target, include_str!("../host/probe.sh"))
        .await?;
    let text = String::from_utf8_lossy(&output);
    let values: std::collections::HashMap<_, _> = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();
    let mut layers = Vec::new();
    for name in [
        "system",
        "architecture",
        "glibc",
        "uv",
        "python",
        "host",
        "sdk",
        "bwrap",
        "systemd",
    ] {
        let detail = values.get(name).copied().unwrap_or("unknown");
        let status = match (name, detail) {
            (_, "missing") => "missing",
            (_, "unknown") => "unknown",
            (_, "unsupported") => "unsupported",
            ("system", value) if value != "Linux" => "unsupported",
            ("architecture", value) if value != "x86_64" => "unsupported",
            ("glibc", value) if !glibc_supported(value) => "unsupported",
            _ => "ready",
        };
        layers.push(json!({"name":name,"status":status,"detail":detail}));
    }
    let version = values.get("host").copied().filter(|v| *v != "missing");
    let mut result = json!({"layers":layers,"agents":{},"host":{"installed":version.is_some(),"running":false,"version":version}});
    #[cfg(windows)]
    {
        use tauri::Manager;
        let resources = crate::local::executor::ExecutorResources::discover(
            &app.path()
                .resource_dir()
                .map_err(|e| AppError::io("local.resources", e))?,
        );
        for check in resources.probe().await {
            result["layers"].as_array_mut().unwrap().push(json!({"name":check.name,"status":if check.available {"ready"} else {"missing"},"detail":check.error.map(|reason| format!("{}: {reason}",check.path)).unwrap_or(check.path)}));
        }
    }
    if values.get("socket") == Some(&"present") {
        match state.hosts.client(&state.ssh, &state.db, &target).await {
            Ok(client) => {
                let probe = client.request("env.probe", json!({})).await?;
                result["agents"] = probe["agents"].clone();
                result["host"] = probe["host"].clone();
                let mut additional = Vec::new();
                for (agent, caps) in probe["agents"].as_object().into_iter().flatten() {
                    let prefix = agent;
                    additional.push(json!({"name":format!("{prefix}Cli"),"status":if caps["installed"]==true {"ready"} else {"missing"},"detail":caps["version"].as_str().unwrap_or("unknown")}));
                    let login_status = match caps["authStatus"].as_str() {
                        Some("failed") => "failed",
                        Some("checking") => "unknown",
                        _ if caps["loggedIn"] == true => "ready",
                        _ => "missing",
                    };
                    additional.push(json!({"name":format!("{prefix}Login"),"status":login_status,"detail":caps["reason"].as_str().unwrap_or("ready")}));
                }
                let caps = &probe["agents"]["codex"];
                for (name, field) in [
                    ("codexResources", "resourcesAvailable"),
                    ("codexSandbox", "sandboxAvailable"),
                ] {
                    additional.push(json!({"name":name,"status":if caps[field]==true {"ready"} else {"failed"},"detail":if caps[field]==true {"ready"} else {"E_SANDBOX"}}));
                }
                if let Some(layers) = result["layers"].as_array_mut() {
                    layers.extend(additional);
                }
                result["sameHostConnections"] =
                    json!(state.db.same_host_connections(&target.connection_id)?);
            }
            Err(error) => {
                result["host"]["error"] = json!(error.code);
            }
        }
    }
    Ok(target.reply(result))
}
fn glibc_supported(value: &str) -> bool {
    let version = value.split_whitespace().last().unwrap_or("");
    let mut parts = version
        .split('.')
        .filter_map(|part| part.parse::<u32>().ok());
    matches!((parts.next(),parts.next()), (Some(major),Some(minor)) if major>2 || (major==2 && minor>=28))
}
#[tauri::command]
pub async fn env_install_plan(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    target: Target,
    fake_agent: Option<bool>,
) -> AppResult<Reply<Value>> {
    state.ssh.validate_target(&target)?;
    let home = remote_home(&state.ssh, &target).await?;
    Ok(target.reply(state.installer.plan(
        &target,
        &home,
        &resources(&app)?,
        fake_agent.unwrap_or(false),
    )?))
}
#[tauri::command]
pub async fn env_install_run(
    state: tauri::State<'_, AppState>,
    target: Target,
    plan_id: String,
    on_progress: tauri::ipc::Channel<Value>,
) -> AppResult<Reply<Value>> {
    Ok(target.reply(install_host(&state, target.clone(), plan_id, on_progress).await?))
}

pub async fn install_host(
    state: &AppState,
    target: Target,
    plan_id: String,
    on_progress: tauri::ipc::Channel<Value>,
) -> AppResult<Value> {
    state.ssh.validate_target(&target)?;
    // take registers cancellation before the first asynchronous preflight.
    let plan = state.installer.take(&target, &plan_id)?;
    execute_host_install(state, target, plan, on_progress).await
}
fn recovery_error(mut original: AppError, recovery: AppError) -> AppError {
    let mut details = original.details.take().unwrap_or_else(|| json!({}));
    details["recoveryCode"] = json!(recovery.code);
    details["recoveryStage"] = json!(recovery.stage);
    details["hint"] = json!("Host recovery failed; inspect the retained versions before retrying");
    original.with_details(details)
}

pub async fn execute_host_install(
    state: &AppState,
    target: Target,
    mut plan: crate::host::installer::InstallPlan,
    on_progress: tauri::ipc::Channel<Value>,
) -> AppResult<Value> {
    let plan_id = plan.value["planId"].as_str().unwrap().to_owned();
    let mut before = None;
    let mut old_client = None;
    let mut previous_fake = false;
    let mut recovery_needed = false;
    let fake = plan.value["fakeAgent"] == true;
    let root = plan.stage.rsplit_once("/staging/").unwrap().0;
    let expected = format!(
        "{root}/versions/{}",
        plan.value["buildId"].as_str().unwrap()
    );
    let operation = async {
        let snapshot = snapshot_script(false);
        let links = tokio::select! {
            _=plan.cancel.cancelled()=>return Err(install_cancelled()),
            output=state.ssh.exec_stage(&target,"install.snapshot",&snapshot)=>HostLinks::parse(&output?)?,
        };
        before = Some(links.clone());
        plan.before = Some(links);
        old_client = tokio::select! {
            _=plan.cancel.cancelled()=>return Err(install_cancelled()),
            client=host_or_inactive(state,&target)=>client?,
        };
        if let Some(client) = &old_client {
            previous_fake = has_fake(client)?;
            tokio::select! {
                _=plan.cancel.cancelled()=>return Err(install_cancelled()),
                result=prepare_shutdown(client)=>result?,
            };
            let observed =
                HostLinks::parse(&state.ssh.exec_stage(&target, "install.snapshot", &snapshot_script(false)).await?)?;
            if Some(&observed) != before.as_ref() {
                return Err(AppError::new(
                    "E_CONFLICT",
                    "env.install",
                    "Host version changed during installation preflight",
                ));
            }
            recovery_needed = true;
            // Finish STOP even if cancellation arrives during its execution.
            // Recovery then restarts the original version deterministically.
            state.hosts.disconnect(&target.connection_id).await;
            state.ssh.exec_stage(&target, "install.stop", STOP_SCRIPT).await?;
        }
        plan.check_cancelled()?;
        recovery_needed = true;
        let installed = state
            .installer
            .execute_prepared(&state.ssh, &target, &plan, &on_progress)
            .await?;
        state.ssh.exec_stage(&target, "install.start", &start_script(fake)).await?;
        let client = state.hosts.client(&state.ssh, &state.db, &target).await?;
        client.request("env.probe", json!({})).await?;
        let current = HostLinks::parse(&state.ssh.exec_stage(&target, "install.snapshot", &snapshot_script(false)).await?)?;
        if current.current.as_deref() != Some(expected.as_str()) {
            return Err(AppError::new(
                "E_CONFLICT",
                "env.install",
                "Host activation changed concurrently",
            ));
        }
        Ok(installed)
    }
    .await;
    if let Some(client) = &old_client {
        let _ = client.request("host.cancelShutdown", json!({})).await;
    }
    let result = match operation {
        Err(error) if recovery_needed => {
            match recover_host(
                state,
                &target,
                before.as_ref().unwrap(),
                &expected,
                old_client.is_some().then_some(previous_fake),
            )
            .await
            {
                Ok(()) => Err(error),
                Err(recovery) => Err(recovery_error(error, recovery)),
            }
        }
        other => other,
    };
    state.installer.finish(&plan_id);
    let _ = on_progress.send(json!({"target":target,"done":true,"success":result.is_ok()}));
    result.map_err(|error| state.ssh.install_error(&target, error))
}
#[tauri::command]
pub fn env_install_cancel(
    state: tauri::State<'_, AppState>,
    target: Target,
    plan_id: String,
) -> AppResult<Reply<()>> {
    state.ssh.validate_target(&target)?;
    state.installer.cancel(&target, &plan_id)?;
    Ok(target.reply(()))
}
#[tauri::command]
pub async fn host_start(
    state: tauri::State<'_, AppState>,
    target: Target,
    fake_agent: Option<bool>,
) -> AppResult<Reply<Value>> {
    state
        .ssh
        .exec_stage(
            &target,
            "host.start",
            &start_script(fake_agent.unwrap_or(false)),
        )
        .await?;
    let client = state.hosts.client(&state.ssh, &state.db, &target).await?;
    let hello = client
        .hello
        .lock()
        .map_err(|e| AppError::io("host.status", e))?
        .clone();
    Ok(target.reply(hello))
}
#[tauri::command]
pub async fn host_stop(state: tauri::State<'_, AppState>, target: Target) -> AppResult<Reply<()>> {
    state.ssh.validate_target(&target)?;
    let client = host_or_inactive(&state, &target).await?;
    if let Some(client) = &client {
        prepare_shutdown(client).await?;
    }
    state.hosts.disconnect(&target.connection_id).await;
    if let Err(error) = state
        .ssh
        .exec_stage(&target, "install.stop", STOP_SCRIPT)
        .await
    {
        if let Some(client) = client {
            let _ = client.request("host.cancelShutdown", json!({})).await;
        }
        return Err(error);
    }
    state.hosts.disconnect(&target.connection_id).await;
    Ok(target.reply(()))
}
#[tauri::command]
pub async fn host_status(
    state: tauri::State<'_, AppState>,
    target: Target,
) -> AppResult<Reply<Value>> {
    let client = state.hosts.client(&state.ssh, &state.db, &target).await?;
    Ok(target.reply(client.request("env.probe", json!({})).await?))
}

#[tauri::command]
pub async fn host_rollback(
    state: tauri::State<'_, AppState>,
    target: Target,
    fake_agent: Option<bool>,
) -> AppResult<Reply<Value>> {
    Ok(target.reply(rollback_host(&state, target.clone(), fake_agent).await?))
}

pub async fn rollback_host(
    state: &AppState,
    target: Target,
    fake_agent: Option<bool>,
) -> AppResult<Value> {
    state.ssh.validate_target(&target)?;
    // Validate the retained executable before touching the running Host.
    let before = HostLinks::parse(
        &state
            .ssh
            .exec_stage(&target, "rollback.snapshot", &snapshot_script(true))
            .await?,
    )?;
    let old_client = host_or_inactive(state, &target).await?;
    let previous_fake = old_client
        .as_ref()
        .map(has_fake)
        .transpose()?
        .unwrap_or(false);
    if let Some(client) = &old_client {
        prepare_shutdown(client).await?;
    }
    let observed = state
        .ssh
        .exec_stage(&target, "rollback.snapshot", &snapshot_script(true))
        .await
        .and_then(|output| HostLinks::parse(&output));
    if observed.as_ref().ok() != Some(&before) {
        if let Some(client) = &old_client {
            let _ = client.request("host.cancelShutdown", json!({})).await;
        }
        return Err(observed.err().unwrap_or_else(|| {
            AppError::new(
                "E_CONFLICT",
                "host.rollback",
                "Host version changed during rollback preflight",
            )
        }));
    }
    let result = async {
        state.hosts.disconnect(&target.connection_id).await;
        state
            .ssh
            .exec_stage(&target, "install.stop", STOP_SCRIPT)
            .await?;
        state
            .ssh
            .exec_stage(&target, "rollback.activate", &rollback_script_for(&before))
            .await?;
        state
            .ssh
            .exec_stage(
                &target,
                "host.start",
                &start_script(fake_agent.unwrap_or(false)),
            )
            .await?;
        let client = state.hosts.client(&state.ssh, &state.db, &target).await?;
        client.request("env.probe", json!({})).await
    }
    .await;
    if let Some(client) = &old_client {
        let _ = client.request("host.cancelShutdown", json!({})).await;
    }
    match result {
        Ok(probe) => Ok(probe),
        Err(error) => match recover_host(
            state,
            &target,
            &before,
            before.previous.as_deref().unwrap(),
            old_client.is_some().then_some(previous_fake),
        )
        .await
        {
            Ok(()) => Err(error),
            Err(recovery) => Err(recovery_error(error, recovery)),
        },
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn glibc_baseline() {
        assert!(glibc_supported("glibc 2.28"));
        assert!(!glibc_supported("glibc 2.17"));
        assert!(!glibc_supported("unknown"));
    }
}
