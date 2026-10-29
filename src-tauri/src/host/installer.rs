use crate::{
    error::{AppError, AppResult},
    models::Target,
    ssh::{manager::SshManager, sftp},
};
use russh::ChannelMsg;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

const DATABASE_CHECK: &str = include_str!("check_database.sh");

#[derive(Clone)]
pub struct InstallPlan {
    pub target: Target,
    pub value: Value,
    pub resources: PathBuf,
    pub script: String,
    pub stage: String,
    pub cancel: CancellationToken,
    pub before: Option<HostLinks>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InstallPhase {
    Preparing,
    Committing,
}
struct ActiveInstall {
    target: Target,
    cancel: CancellationToken,
    phase: InstallPhase,
}
#[derive(Default)]
struct InstallState {
    plans: HashMap<String, InstallPlan>,
    active: HashMap<String, ActiveInstall>,
}
#[derive(Default)]
pub struct Installer {
    state: Mutex<InstallState>,
}

impl Installer {
    pub fn plan(
        &self,
        target: &Target,
        home: &str,
        resources: &Path,
        fake: bool,
    ) -> AppResult<Value> {
        let text = std::fs::read_to_string(resources.join("manifest.json")).map_err(|_| {
            AppError::new(
                "E_RESOURCES",
                "env.plan",
                "Run scripts/build/build_host_resources.py before installing Host",
            )
        })?;
        let manifest: Value =
            serde_json::from_str(&text).map_err(|e| AppError::io("env.plan", e))?;
        let files = manifest["files"]
            .as_array()
            .ok_or_else(|| AppError::new("E_RESOURCES", "env.plan", "Invalid resource manifest"))?;
        let get = |name: &str| -> AppResult<&Value> {
            files
                .iter()
                .find(|f| f["name"] == name)
                .ok_or_else(|| AppError::new("E_RESOURCES", "env.plan", "Missing release resource"))
        };
        let wheel = get("host")?;
        let deps = get("dependencies")?;
        let digest = wheel["sha256"]
            .as_str()
            .filter(|v| v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit()))
            .ok_or_else(|| AppError::new("E_RESOURCES", "env.plan", "Invalid wheel digest"))?;
        let version = manifest["version"]
            .as_str()
            .filter(|v| v.chars().all(|c| c.is_ascii_digit() || c == '.'))
            .ok_or_else(|| AppError::new("E_RESOURCES", "env.plan", "Invalid release version"))?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let build_id = format!("{version}-{}-{}", &digest[..12], &id[..8]);
        let stage = format!("{home}/.agentdesk/host/staging/{id}");
        let mut script = String::new();
        let mut set = |key: &str, value: &str| {
            script.push_str(&format!("export {key}={}\n", sftp::quote(value)));
        };
        set("AGENTDESK_PLAN_ID", &id);
        set("AGENTDESK_BUILD_ID", &build_id);
        set("AGENTDESK_WHEEL", wheel["path"].as_str().unwrap_or(""));
        set("AGENTDESK_WHEEL_SHA", digest);
        set("AGENTDESK_DEPS_SHA", deps["sha256"].as_str().unwrap_or(""));
        set(
            "AGENTDESK_PYTHON_METADATA_SHA",
            get("python-metadata")?["sha256"].as_str().unwrap_or(""),
        );
        for name in ["uv", "codex", "bwrap"] {
            let file = get(name)?;
            for (field, suffix) in [("url", "URL"), ("sha256", "SHA"), ("member", "MEMBER")] {
                set(
                    &format!("AGENTDESK_{}_{}", name.to_uppercase(), suffix),
                    file[field].as_str().unwrap_or(""),
                );
            }
        }
        script.push_str(DATABASE_CHECK);
        script.push_str(include_str!("install.sh"));
        let value = json!({"planId":id,"version":version,"buildId":build_id,"fakeAgent":fake,"artifacts":files,"paths":[stage,format!("{home}/.agentdesk/runtime"),format!("{home}/.agentdesk/host/versions/{build_id}")],"commands":script,"warning":"System compatibility and sandbox checks must pass before activation"});
        self.state
            .lock()
            .map_err(|e| AppError::io("env.plan", e))?
            .plans
            .insert(
                id,
                InstallPlan {
                    target: target.clone(),
                    value: value.clone(),
                    resources: resources.into(),
                    script,
                    stage,
                    cancel: CancellationToken::new(),
                    before: None,
                },
            );
        Ok(value)
    }
    pub fn take(&self, target: &Target, id: &str) -> AppResult<InstallPlan> {
        let mut state = self.state.lock().map_err(|e| AppError::io("env.plan", e))?;
        let plan = state
            .plans
            .get(id)
            .ok_or_else(|| AppError::new("E_PLAN", "env.install", "Installation plan expired"))?;
        if !crate::ssh::pty::same_resource(&plan.target, target) {
            return Err(AppError::new(
                "E_TARGET",
                "env.install",
                "Plan belongs to another target",
            ));
        }
        plan.check_cancelled()?;
        if state
            .active
            .values()
            .any(|entry| entry.target.connection_id == target.connection_id)
        {
            return Err(AppError::new(
                "E_BUSY",
                "env.install",
                "An installation is already running",
            ));
        }
        let plan = state.plans.remove(id).unwrap();
        state.active.insert(
            id.into(),
            ActiveInstall {
                target: target.clone(),
                cancel: plan.cancel.clone(),
                phase: InstallPhase::Preparing,
            },
        );
        Ok(plan)
    }
    pub fn finish(&self, id: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.active.remove(id);
        }
    }
    fn enter_commit(&self, id: &str) -> AppResult<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|e| AppError::io("env.commit", e))?;
        let entry = state.active.get_mut(id).ok_or_else(|| {
            AppError::new("E_PLAN", "env.commit", "Installation is no longer active")
        })?;
        if entry.cancel.is_cancelled() {
            return Err(cancelled());
        }
        entry.phase = InstallPhase::Committing;
        Ok(())
    }
    pub async fn execute(
        &self,
        ssh: &SshManager,
        target: &Target,
        plan: InstallPlan,
        progress: tauri::ipc::Channel<Value>,
    ) -> AppResult<Value> {
        let result = self.execute_prepared(ssh, target, &plan, &progress).await;
        self.finish(plan.value["planId"].as_str().unwrap());
        let _ = progress.send(json!({"target":target,"done":true,"success":result.is_ok()}));
        result
    }
    pub async fn execute_prepared(
        &self,
        ssh: &SshManager,
        target: &Target,
        plan: &InstallPlan,
        progress: &tauri::ipc::Channel<Value>,
    ) -> AppResult<Value> {
        plan.check_cancelled()?;
        self.execute_inner(ssh, target, plan, progress, &plan.cancel)
            .await
            .map_err(|error| ssh.install_error(target, error))
    }
    async fn execute_inner(
        &self,
        ssh: &SshManager,
        target: &Target,
        plan: &InstallPlan,
        progress: &tauri::ipc::Channel<Value>,
        cancel: &CancellationToken,
    ) -> AppResult<Value> {
        let q = sftp::quote(&plan.stage);
        ssh.exec_stage(
            target,
            "install.staging",
            &format!("set -eu\numask 077\nmkdir -p \"$HOME/.agentdesk/host/staging\"\nmkdir {q}"),
        )
        .await?;
        let session = sftp::session(ssh, target).await?;
        let upload_result = async {
            for file in plan.value["artifacts"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|v| v.get("path").is_some())
            {
                let filename = file["path"].as_str().unwrap();
                if filename.contains(['/', '\\']) || filename == ".." {
                    return Err(AppError::new(
                        "E_RESOURCES",
                        "env.upload",
                        "Invalid local artifact path",
                    ));
                }
                let bytes = tokio::fs::read(plan.resources.join(filename))
                    .await
                    .map_err(|e| AppError::io("env.upload", e))?;
                if cancel.is_cancelled() {
                    return Err(AppError::new(
                        "E_CANCELLED",
                        "env.upload",
                        "Installation cancelled",
                    ));
                }
                let mut destination = session
                    .create(format!("{}/{filename}", plan.stage))
                    .await
                    .map_err(sftp::error)?;
                destination
                    .write_all(&bytes)
                    .await
                    .map_err(|e| AppError::io("env.upload", e))?;
                destination
                    .shutdown()
                    .await
                    .map_err(|e| AppError::io("env.upload", e))?;
            }
            let mut destination = session
                .create(format!("{}/install.sh", plan.stage))
                .await
                .map_err(sftp::error)?;
            let mut script=String::new();
            if let Some(before)=&plan.before {
                script.push_str(&format!("export AGENTDESK_EXPECT_LINKS=1\nexport AGENTDESK_EXPECT_CURRENT={}\nexport AGENTDESK_EXPECT_PREVIOUS={}\n",sftp::quote(before.current.as_deref().unwrap_or("")),sftp::quote(before.previous.as_deref().unwrap_or(""))));
            }
            script.push_str(&plan.script);
            plan.check_cancelled()?;
            destination
                .write_all(script.as_bytes())
                .await
                .map_err(|e| AppError::io("env.upload", e))?;
            destination
                .shutdown()
                .await
                .map_err(|e| AppError::io("env.upload", e))?;
            Ok(())
        }
        .await;
        sftp::finish(session, upload_result).await?;
        plan.check_cancelled()?;
        let mut channel = ssh.open_channel(target).await?;
        channel
            .exec(true, "exec /bin/sh -s")
            .await
            .map_err(|e| AppError::io("env.install", e))?;
        let script = format!("set +m\numask 077\nsetsid /bin/sh {q}/install.sh </dev/null &\npid=$!\nprintf '%s' \"$pid\" >{q}/install.pid\ntrap 'kill -TERM -- -\"$pid\" 2>/dev/null || true' HUP TERM EXIT\nwait \"$pid\"\nstatus=$?\ntrap - HUP TERM EXIT\nexit \"$status\"\n");
        let mut diagnostic = InstallOutput::default();
        let mut exit = None;
        let result = tokio::time::timeout(Duration::from_secs(900), async {
            channel.data_bytes(script.into_bytes()).await.map_err(|e|AppError::io("env.install",e))?;
            channel.eof().await.map_err(|e|AppError::io("env.install",e))?;
            let mut emitted = 0usize;
            let mut prepared=PreparedMarker::new(plan.value["planId"].as_str().unwrap());
            let mut committed=false;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return Err(AppError::new("E_CANCELLED", "env.install", "Installation cancelled")),
                    message = channel.wait() => match message {
                        Some(ChannelMsg::ExtendedData { data, .. }) => {
                            diagnostic.stderr.extend_from_slice(&data);
                            let excess = diagnostic.stderr.len().saturating_sub(4096);
                            diagnostic.stderr.drain(..excess);
                        }
                        Some(ChannelMsg::Data { data }) => {
                            diagnostic.push(&data);
                            emitted += data.len();
                            if emitted > 2 * 1024 * 1024 { return Err(AppError::new("E_LIMIT", "env.install", "Installation log exceeded its limit")); }
                            progress.send(json!({"target":target,"message":String::from_utf8_lossy(&data),"done":false})).map_err(|e| AppError::io("env.progress", e))?;
                            if !committed && prepared.push(&data) {
                                self.enter_commit(plan.value["planId"].as_str().unwrap())?;
                                // The state transition and cancel share one mutex. Once
                                // committing wins, cancel reports E_COMMITTING.
                                ssh.exec_stage(target,"install.commit",&format!("set -eu\ntest -f {q}/prepared && test ! -f {q}/cancel\n: >{q}/commit")).await?;
                                committed=true;
                            }
                        }
                        Some(ChannelMsg::ExitStatus { exit_status }) => exit = Some(exit_status),
                        Some(ChannelMsg::Failure) => return Err(AppError::new("E_EXEC", "env.install", "Installation command was rejected")),
                        None | Some(ChannelMsg::Close) => break,
                        _ => {}
                    }
                }
            }
            if exit != Some(0) || !committed { return Err(AppError::new(if exit == Some(78) { "E_DATABASE_SCHEMA" } else { "E_INSTALL" }, "env.install", format!("Installation failed (status {exit:?})"))); }
            Ok(plan.value.clone())
        }).await.map_err(|_| AppError::new("E_TIMEOUT", "env.install", "Installation exceeded 15 minutes")).and_then(|r| r);
        let result = result.map_err(|mut error| {
            if error.details.is_none() {
                error.stage = format!("install.{}", diagnostic.stage);
                // Record through the shared SSH diagnostic path without retaining
                // arbitrary stderr or artifact contents.
                error.details = Some(serde_json::json!({"installStage":error.stage,"exitCode":exit,"stderrTail":crate::session_log::sanitized_install_tail(&diagnostic.stderr)}));
                ssh.record_install_failure(target, &error, exit, &diagnostic.stderr);
            }
            error
        });
        let _ = channel.close().await;
        if result.is_err() {
            // Inspect only our unique install script identity before killing its group.
            let cleanup = format!("test ! -d {q} || : >{q}/cancel\nif test -f {q}/install.pid; then pid=$(cat {q}/install.pid); case \"$pid\" in ''|*[!0-9]*) exit 1;; esac; if test -r /proc/\"$pid\"/cmdline && tr '\\0' '\\n' </proc/\"$pid\"/cmdline | grep -Fx -- {q}/install.sh >/dev/null; then kill -TERM -- -\"$pid\" 2>/dev/null || true; sleep 1; kill -KILL -- -\"$pid\" 2>/dev/null || true; fi; fi");
            // Cleanup failure must not hide the original failing phase.
            let _ = ssh.exec_stage(target, "install.cleanup", &cleanup).await;
        }
        result
    }
    pub fn cancel(&self, target: &Target, id: &str) -> AppResult<()> {
        let state = self
            .state
            .lock()
            .map_err(|e| AppError::io("env.cancel", e))?;
        if let Some(entry) = state.active.get(id) {
            if !crate::ssh::pty::same_resource(&entry.target, target) {
                return Err(AppError::new(
                    "E_TARGET",
                    "env.cancel",
                    "Installation belongs to another target",
                ));
            }
            if entry.phase == InstallPhase::Committing {
                return Err(AppError::new(
                    "E_COMMITTING",
                    "env.cancel",
                    "Installation is committing and cannot be cancelled",
                ));
            }
            entry.cancel.cancel();
            return Ok(());
        }
        if let Some(plan) = state.plans.get(id) {
            if !crate::ssh::pty::same_resource(&plan.target, target) {
                return Err(AppError::new(
                    "E_TARGET",
                    "env.cancel",
                    "Plan belongs to another target",
                ));
            }
            plan.cancel.cancel();
            return Ok(());
        }
        Err(AppError::new(
            "E_PLAN",
            "env.cancel",
            "Installation is not active",
        ))
    }
}

fn cancelled() -> AppError {
    AppError::new("E_CANCELLED", "env.install", "Installation cancelled")
}
impl InstallPlan {
    pub fn check_cancelled(&self) -> AppResult<()> {
        if self.cancel.is_cancelled() {
            Err(cancelled())
        } else {
            Ok(())
        }
    }
}

struct PreparedMarker {
    marker: Vec<u8>,
    tail: Vec<u8>,
}
struct InstallOutput {
    stage: String,
    tail: Vec<u8>,
    stderr: Zeroizing<Vec<u8>>,
}
impl Default for InstallOutput {
    fn default() -> Self {
        Self {
            stage: "launch".into(),
            tail: Vec::new(),
            stderr: Zeroizing::new(Vec::new()),
        }
    }
}
impl InstallOutput {
    fn push(&mut self, data: &[u8]) {
        self.tail.extend_from_slice(data);
        for line in self.tail.split(|c| *c == b'\n') {
            if let Some(stage) = line.strip_prefix(b"AGENTDESK_STAGE:") {
                if let Ok(stage) = std::str::from_utf8(stage) {
                    if [
                        "platform",
                        "prepare.paths",
                        "uv.download",
                        "uv.extract",
                        "python.install",
                        "venv.create",
                        "dependencies.install",
                        "host.install",
                        "database.check",
                        "codex.download",
                        "bwrap.download",
                        "codex.extract",
                        "host.import",
                        "codex.version",
                        "codex.sandbox",
                        "prepare.wait",
                        "activate.links",
                    ]
                    .contains(&stage)
                    {
                        self.stage = stage.into();
                    }
                }
            }
        }
        if let Some(end) = self.tail.iter().rposition(|c| *c == b'\n') {
            self.tail.drain(..=end);
        }
        if self.tail.len() > 128 {
            self.tail.clear();
        }
    }
}
impl PreparedMarker {
    fn new(id: &str) -> Self {
        Self {
            marker: format!("\nAGENTDESK_PREPARED:{id}\n").into_bytes(),
            tail: Vec::new(),
        }
    }
    fn push(&mut self, data: &[u8]) -> bool {
        self.tail.extend_from_slice(data);
        let found = self
            .tail
            .windows(self.marker.len())
            .any(|window| window == self.marker);
        if self.tail.len() > self.marker.len() {
            self.tail.drain(..self.tail.len() - self.marker.len());
        }
        found
    }
}

pub fn start_script(fake: bool) -> String {
    format!(
        r#"set -eu
umask 077
{database_check}
base="$HOME/.agentdesk"
test -x "$base/host/current/venv/bin/python"
mkdir -p "$base/run" "$base/state"
if test -S "$base/run/host.sock" && test -r "$base/run/host.pid"; then
  pid=$(cat "$base/run/host.pid")
  if kill -0 "$pid" 2>/dev/null; then exit 0; fi
fi
check_host_database "$base/host/current/venv/bin/python"
export AGENTDESK_CODEX_PATH="$base/host/current/bin/codex"
export AGENTDESK_FAKE_AGENT={fake}
if timeout 5 systemctl --user show-environment >/dev/null 2>&1; then
  mkdir -p "$HOME/.config/systemd/user"
  unit="$HOME/.config/systemd/user/agentdesk-host.service"
  if test -e "$unit" && ! grep -q '^# AgentDesk managed service$' "$unit"; then exit 74; fi
  printf '%s\n' '# AgentDesk managed service' '[Unit]' 'Description=AgentDesk Host' '[Service]' "Environment=AGENTDESK_CODEX_PATH=$base/host/current/bin/codex" 'Environment=AGENTDESK_FAKE_AGENT={fake}' "ExecStart=$base/host/current/venv/bin/python -m agentdesk_host" 'Restart=on-failure' 'UMask=0077' '[Install]' 'WantedBy=default.target' >"$unit"
  systemctl --user daemon-reload
  systemctl --user start agentdesk-host.service
else
  setsid nohup "$base/host/current/venv/bin/python" -m agentdesk_host </dev/null >/dev/null 2>/dev/null 9>&- &
fi
i=0
while test "$i" -lt 100; do test ! -S "$base/run/host.sock" || exit 0; sleep 0.1; i=$((i+1)); done
exit 75
"#,
        fake = if fake { "1" } else { "0" },
        database_check = DATABASE_CHECK,
    )
}

pub const STOP_SCRIPT: &str = r#"set -eu
base="$HOME/.agentdesk"
if test -r "$base/run/host.pid"; then
  pid=$(cat "$base/run/host.pid")
  case "$pid" in ''|*[!0-9]*) exit 1;; esac
  if test -r /proc/"$pid"/cmdline && tr '\0' '\n' </proc/"$pid"/cmdline | grep -Fx agentdesk_host >/dev/null; then
    if timeout 5 systemctl --user is-active agentdesk-host.service >/dev/null 2>&1; then systemctl --user stop agentdesk-host.service; else kill -TERM "$pid"; fi
    i=0
    while { test -S "$base/run/host.sock" || test -r "$base/run/host.pid"; } && test "$i" -lt 300; do sleep 0.1; i=$((i+1)); done
    test ! -S "$base/run/host.sock" && test ! -r "$base/run/host.pid"
  fi
fi
"#;

pub fn rollback_script() -> String {
    let marker = uuid::Uuid::new_v4().simple().to_string();
    format!(
        "{LINKS_HEADER}{DATABASE_CHECK}mkdir -p \"$base\"
exec 9>\"$base/install.lock\"
flock -n 9 || exit 76
"
    ) + &r#"test -L "$base/previous" && test -L "$base/current"
old=$(managed_link previous)
current=$(managed_link current)
test -x "$old/venv/bin/python"
check_host_database "$old/venv/bin/python"
ln -s "$old" "$base/current.__MARKER__"
ln -s "$current" "$base/previous.__MARKER__"
mv -Tf "$base/current.__MARKER__" "$base/current"
mv -Tf "$base/previous.__MARKER__" "$base/previous"
"#
    .replace("__MARKER__", &marker)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostLinks {
    pub current: Option<String>,
    pub previous: Option<String>,
}
impl HostLinks {
    pub fn parse(output: &[u8]) -> AppResult<Self> {
        let text = std::str::from_utf8(output).map_err(|e| AppError::io("host.links", e))?;
        let parts: Vec<_> = text.lines().collect();
        if parts.len() != 2 || parts.iter().any(|value| value.contains('\0')) {
            return Err(AppError::new(
                "E_PROTOCOL",
                "host.links",
                "Invalid Host link snapshot",
            ));
        }
        Ok(Self {
            current: (!parts[0].is_empty()).then(|| parts[0].to_owned()),
            previous: (!parts[1].is_empty()).then(|| parts[1].to_owned()),
        })
    }
}
const LINKS_HEADER: &str = r#"set -eu
umask 077
base="$HOME/.agentdesk/host"
managed_link() {
  if test -L "$base/$1"; then
    path=$(readlink "$base/$1")
    case "$path" in /*) ;; versions/*) path="$base/$path";; *) exit 73;; esac
    path=$(readlink -m -- "$path")
    case "$path" in "$base"/versions/*) printf '%s' "$path";; *) exit 73;; esac
  elif test -e "$base/$1"; then exit 73; fi
}
"#;
pub fn snapshot_script(require_previous: bool) -> String {
    let check = if require_previous {
        "test -n \"$previous\" && test -x \"$previous/venv/bin/python\"\n"
    } else {
        ""
    };
    format!("{LINKS_HEADER}current=$(managed_link current)\nprevious=$(managed_link previous)\n{check}printf '%s\n%s\n' \"$current\" \"$previous\"\n")
}
pub fn database_check_script(version: &str) -> String {
    format!(
        "set -eu\n{DATABASE_CHECK}check_host_database {}\n",
        sftp::quote(&format!("{version}/venv/bin/python"))
    )
}
pub fn restore_script(links: &HostLinks, expected: &str, restart_fake: Option<bool>) -> String {
    let script = r#"mkdir -p "$base"
exec 9>"$base/install.lock"
flock -n 9 || exit 76
current=$(managed_link current)
expected=__EXPECTED__
old_current=__CURRENT__
old_previous=__PREVIOUS__
if test "$current" = "$expected"; then
  if test -n "$old_current"; then
    check_host_database "$old_current/venv/bin/python"
  fi
__STOP__
  if test -n "$old_current"; then
    ln -s "$old_current" "$base/current.restore.__MARKER__"
    mv -Tf "$base/current.restore.__MARKER__" "$base/current"
  else
    test ! -L "$base/current" || rm -- "$base/current"
  fi
  if test -n "$old_previous"; then
    ln -s "$old_previous" "$base/previous.restore.__MARKER__"
    mv -Tf "$base/previous.restore.__MARKER__" "$base/previous"
  else
    test ! -L "$base/previous" || rm -- "$base/previous"
  fi
elif test "$current" != "$old_current"; then
  exit 77
fi
__RESTART__
"#;
    let restart = restart_fake
        .filter(|_| links.current.is_some())
        .map(start_script)
        .unwrap_or_default();
    // STOP_SCRIPT resets base to ~/.agentdesk; restore the link root afterward.
    let stop = format!("{}\nbase=\"$HOME/.agentdesk/host\"", STOP_SCRIPT);
    format!(
        "{LINKS_HEADER}{DATABASE_CHECK}{}",
        script
            .replace("__EXPECTED__", &sftp::quote(expected))
            .replace(
                "__CURRENT__",
                &sftp::quote(links.current.as_deref().unwrap_or(""))
            )
            .replace(
                "__PREVIOUS__",
                &sftp::quote(links.previous.as_deref().unwrap_or(""))
            )
            .replace("__MARKER__", &uuid::Uuid::new_v4().simple().to_string())
            .replace("__STOP__", &stop)
            .replace("__RESTART__", &restart)
    )
}
pub fn rollback_script_for(links: &HostLinks) -> String {
    let guard = format!(
        "test \"$(managed_link current)\" = {} && test \"$(managed_link previous)\" = {}\n",
        sftp::quote(links.current.as_deref().unwrap_or("")),
        sftp::quote(links.previous.as_deref().unwrap_or(""))
    );
    rollback_script().replacen(
        "old=$(managed_link",
        &format!("{guard}old=$(managed_link"),
        1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn phase_diagnostics_accept_only_known_names_across_transport_chunks() {
        let mut output = InstallOutput::default();
        output.push(b"AGENTDESK_STA");
        output.push(b"GE:codex.sandbox\n");
        assert_eq!(output.stage, "codex.sandbox");
        output.push(b"AGENTDESK_STAGE:credential-fixture\n");
        assert_eq!(output.stage, "codex.sandbox");
    }
    fn target(connection: &str) -> Target {
        Target {
            device_local: false,
            connection_id: connection.into(),
            project_id: None,
            session_id: None,
            request_seq: 1,
        }
    }
    fn fixture(id: &str, owner: Target) -> InstallPlan {
        InstallPlan {
            target: owner,
            value: json!({"planId":id,"buildId":"0.2.0-test"}),
            resources: PathBuf::new(),
            script: String::new(),
            stage: format!("/home/test/.agentdesk/host/staging/{id}"),
            cancel: CancellationToken::new(),
            before: None,
        }
    }
    fn pending(installer: &Installer, id: &str, owner: Target) {
        installer
            .state
            .lock()
            .unwrap()
            .plans
            .insert(id.into(), fixture(id, owner));
    }
    #[test]
    fn pending_cancel_and_preflight_cancel_cannot_be_lost() {
        let installer = Installer::default();
        let owner = target("a");
        pending(&installer, "pending", owner.clone());
        installer.cancel(&owner, "pending").unwrap();
        assert_eq!(
            installer.take(&owner, "pending").err().unwrap().code,
            "E_CANCELLED"
        );
        pending(&installer, "preflight", owner.clone());
        let plan = installer.take(&owner, "preflight").unwrap();
        assert!(installer
            .state
            .lock()
            .unwrap()
            .active
            .contains_key("preflight"));
        installer.cancel(&owner, "preflight").unwrap();
        assert!(plan.cancel.is_cancelled());
        assert_eq!(
            installer.enter_commit("preflight").unwrap_err().code,
            "E_CANCELLED"
        );
        installer.finish("preflight");
        assert_eq!(
            installer.cancel(&owner, "preflight").unwrap_err().code,
            "E_PLAN"
        );
    }
    #[test]
    fn commit_and_cancel_have_a_single_winner() {
        let installer = Installer::default();
        let owner = target("a");
        pending(&installer, "commit", owner.clone());
        let plan = installer.take(&owner, "commit").unwrap();
        installer.enter_commit("commit").unwrap();
        assert_eq!(
            installer.cancel(&owner, "commit").unwrap_err().code,
            "E_COMMITTING"
        );
        assert!(!plan.cancel.is_cancelled());
        installer.finish("commit");
        assert!(installer.state.lock().unwrap().active.is_empty());
    }
    #[test]
    fn ownership_and_busy_checks_preserve_the_original_plan() {
        let installer = Installer::default();
        let owner = target("a");
        let other = target("b");
        pending(&installer, "one", owner.clone());
        pending(&installer, "two", owner.clone());
        assert_eq!(
            installer.cancel(&other, "one").unwrap_err().code,
            "E_TARGET"
        );
        assert_eq!(
            installer.take(&other, "one").err().unwrap().code,
            "E_TARGET"
        );
        let first = installer.take(&owner, "one").unwrap();
        assert_eq!(
            installer.cancel(&other, "one").unwrap_err().code,
            "E_TARGET"
        );
        assert!(!first.cancel.is_cancelled());
        assert_eq!(installer.take(&owner, "two").err().unwrap().code, "E_BUSY");
        installer.finish("one");
        assert!(installer.take(&owner, "two").is_ok());
    }
    #[test]
    fn prepared_marker_handles_transport_chunk_boundaries() {
        let mut marker = PreparedMarker::new("owned");
        assert!(!marker.push(b"\nAGENTDESK_PREPARED:other\n"));
        assert!(!marker.push(b"progress\nAGENTDESK_PRE"));
        assert!(!marker.push(b"PARED:owned"));
        assert!(marker.push(b"\nmore output"));
        assert!(marker.tail.len() <= marker.marker.len());
    }
    #[test]
    fn link_snapshot_distinguishes_absent_links_from_malformed_output() {
        assert_eq!(
            HostLinks::parse(b"\n\n").unwrap(),
            HostLinks {
                current: None,
                previous: None
            }
        );
        assert_eq!(
            HostLinks::parse(b"/managed/versions/current\n\n")
                .unwrap()
                .current
                .as_deref(),
            Some("/managed/versions/current")
        );
        assert!(HostLinks::parse(b"single\n").is_err());
        assert!(HostLinks::parse(b"a\nb\nunexpected\n").is_err());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn rollback_and_recovery_modify_only_owned_links() {
        use std::io::Write;
        use std::os::unix::fs::{symlink, PermissionsExt};
        use std::process::{Command, Stdio};
        fn run(home: &Path, script: &str) -> std::process::Output {
            let mut child = Command::new("/bin/sh")
                .env("HOME", home)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(script.as_bytes())
                .unwrap();
            child.wait_with_output().unwrap()
        }
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join(".agentdesk/host");
        for name in ["old", "new", "other"] {
            let v = base.join("versions").join(name).join("venv/bin");
            std::fs::create_dir_all(&v).unwrap();
            std::fs::write(v.join("python"), "#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(v.join("python"), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let old = base.join("versions/old");
        let new = base.join("versions/new");
        let other = base.join("versions/other");
        symlink(&old, base.join("current")).unwrap();
        assert!(!run(dir.path(), &snapshot_script(true)).status.success());
        assert_eq!(std::fs::read_link(base.join("current")).unwrap(), old);
        symlink(&new, base.join("previous")).unwrap();
        let original = HostLinks::parse(&run(dir.path(), &snapshot_script(true)).stdout).unwrap();
        assert!(run(dir.path(), &rollback_script_for(&original))
            .status
            .success());
        assert_eq!(std::fs::read_link(base.join("current")).unwrap(), new);
        assert!(run(
            dir.path(),
            &restore_script(&original, new.to_str().unwrap(), None)
        )
        .status
        .success());
        assert_eq!(std::fs::read_link(base.join("current")).unwrap(), old);
        std::fs::write(old.join("venv/bin/python"), "#!/bin/sh\nexit 78\n").unwrap();
        assert!(run(dir.path(), &rollback_script_for(&original))
            .status
            .success());
        let run_dir = dir.path().join(".agentdesk/run");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("host.pid"), "invalid").unwrap();
        let rejected = run(
            dir.path(),
            &restore_script(&original, new.to_str().unwrap(), None),
        );
        assert_eq!(rejected.status.code(), Some(78));
        assert_eq!(std::fs::read_link(base.join("current")).unwrap(), new);
        assert_eq!(std::fs::read_link(base.join("previous")).unwrap(), old);
        assert_eq!(std::fs::read(run_dir.join("host.pid")).unwrap(), b"invalid");
        std::fs::remove_file(base.join("current")).unwrap();
        symlink(&other, base.join("current")).unwrap();
        assert!(!run(
            dir.path(),
            &restore_script(&original, new.to_str().unwrap(), None)
        )
        .status
        .success());
        assert_eq!(std::fs::read_link(base.join("current")).unwrap(), other);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn prepared_installation_requires_commit_and_holds_server_lock() {
        use std::os::unix::fs::symlink;
        use std::{
            io::{BufRead, BufReader, Write},
            process::{Command, Stdio},
        };
        fn start(home: &Path, script: &str, id: &str) -> std::process::Child {
            let mut child = Command::new("/bin/sh")
                .env("HOME", home)
                .env("AGENTDESK_PLAN_ID", id)
                .env("AGENTDESK_BUILD_ID", format!("new-{id}"))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(script.as_bytes())
                .unwrap();
            child
        }
        fn prepared(
            child: &mut std::process::Child,
            id: &str,
        ) -> BufReader<std::process::ChildStdout> {
            let mut reader = BufReader::new(child.stdout.take().unwrap());
            let mut line = String::new();
            loop {
                line.clear();
                assert!(
                    reader.read_line(&mut line).unwrap() > 0,
                    "Installer exited before preparation"
                );
                if line == format!("AGENTDESK_PREPARED:{id}\n") {
                    break;
                }
            }
            reader
        }
        let source = include_str!("install.sh");
        let prefix = source
            .split(r"printf '%s\n' 'Preparing verified uv runtime'")
            .next()
            .unwrap();
        let suffix = &source[source
            .find(r"printf '%s\n' 'Waiting for installation commit'")
            .unwrap()..];
        // Exercise the production barrier and activation, substituting only
        // expensive downloads and package installation with a prepared directory.
        let script = format!("{prefix}mkdir -p \"$version\"\n{suffix}");
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join(".agentdesk/host");
        let old = base.join("versions/old");
        std::fs::create_dir_all(&old).unwrap();
        for id in ["cancel", "concurrent", "commit"] {
            std::fs::create_dir_all(base.join("staging").join(id)).unwrap();
        }
        symlink(&old, base.join("current")).unwrap();
        let mut first = start(dir.path(), &script, "cancel");
        let _first_output = prepared(&mut first, "cancel");
        assert_eq!(std::fs::read_link(base.join("current")).unwrap(), old);
        assert_eq!(
            start(dir.path(), &script, "concurrent")
                .wait()
                .unwrap()
                .code(),
            Some(76)
        );
        std::fs::write(base.join("staging/cancel/cancel"), "").unwrap();
        assert_eq!(first.wait().unwrap().code(), Some(130));
        assert_eq!(std::fs::read_link(base.join("current")).unwrap(), old);
        let mut committing = start(dir.path(), &script, "commit");
        let _commit_output = prepared(&mut committing, "commit");
        assert_eq!(std::fs::read_link(base.join("current")).unwrap(), old);
        std::fs::write(base.join("staging/commit/commit"), "").unwrap();
        assert!(committing.wait().unwrap().success());
        assert_eq!(
            std::fs::read_link(base.join("current")).unwrap(),
            base.join("versions/new-commit")
        );
        assert_eq!(std::fs::read_link(base.join("previous")).unwrap(), old);
    }
}
