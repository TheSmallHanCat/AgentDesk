use super::{
    manager::SshManager,
    sftp::{quote, remote_path},
};
use crate::{
    error::{AppError, AppResult},
    models::Target,
};
use russh::ChannelMsg;
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalFrame {
    pub target: Target,
    pub terminal_id: String,
    pub data: Vec<u8>,
    pub seq: u64,
    pub exited: bool,
    pub exit_code: Option<u32>,
}
enum Input {
    Data(String),
    Resize(u32, u32),
}
struct Terminal {
    target: Target,
    input: mpsc::Sender<Input>,
    cancel: CancellationToken,
    credits: Arc<Mutex<BTreeMap<u64, OwnedSemaphorePermit>>>,
}
#[derive(Default)]
pub struct PtyManager {
    entries: Mutex<HashMap<String, Terminal>>,
}
pub fn same_resource(a: &Target, b: &Target) -> bool {
    a.connection_id == b.connection_id
        && a.project_id == b.project_id
        && a.session_id == b.session_id
}
impl PtyManager {
    #[allow(clippy::too_many_arguments)]
    pub async fn open(
        self: &Arc<Self>,
        ssh: &SshManager,
        target: Target,
        path: &str,
        kind: &str,
        cols: u32,
        rows: u32,
        output: tauri::ipc::Channel<TerminalFrame>,
    ) -> AppResult<String> {
        let path = remote_path(path)?;
        let command = command(&path, kind)?;
        let mut channel = ssh.open_channel(&target).await?;
        let (cols, rows) = dimensions(cols, rows);
        let mut initial = Vec::with_capacity(32768);
        let startup = async {
            channel
                .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
                .await
                .map_err(|e| AppError::io("pty", e))?;
            confirm(&mut channel, "pty-request", &mut initial).await?;
            channel
                .exec(true, "exec /bin/sh -s")
                .await
                .map_err(|e| AppError::io("pty", e))?;
            confirm(&mut channel, "terminal-exec", &mut initial).await?;
            channel
                .data_bytes(command.into_bytes())
                .await
                .map_err(|e| AppError::io("terminal-exec", e))
        }
        .await;
        if let Err(error) = startup {
            let _ = channel.close().await;
            return Err(error);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(32);
        let credits = Arc::new(Mutex::new(BTreeMap::new()));
        self.entries
            .lock()
            .map_err(|e| AppError::io("pty", e))?
            .insert(
                id.clone(),
                Terminal {
                    target: target.clone(),
                    input: tx,
                    cancel: cancel.clone(),
                    credits: credits.clone(),
                },
            );
        let this = self.clone();
        let terminal_id = id.clone();
        tokio::spawn(async move {
            let window = Arc::new(Semaphore::new(8));
            let mut seq = 0;
            let mut bytes = initial;
            let mut tick = tokio::time::interval(Duration::from_millis(16));
            let mut exit_code = None;
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => break,
                    input = rx.recv() => match input {
                        Some(Input::Data(data)) => {
                            if channel.data_bytes(data.into_bytes()).await.is_err() { break; }
                        }
                        Some(Input::Resize(cols, rows)) => {
                            if channel.window_change(cols, rows, 0, 0).await.is_err() { break; }
                        }
                        None => break,
                    },
                    _ = tick.tick(), if !bytes.is_empty() => {
                        if !send(&output, &target, &terminal_id, &mut seq, &mut bytes,
                            false, None, &window, &credits, &cancel).await { break; }
                    }
                    message = channel.wait() => match message {
                        Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                            bytes.extend_from_slice(&data);
                            if bytes.len() >= 32768 && !send(&output, &target, &terminal_id,
                                &mut seq, &mut bytes, false, None, &window, &credits, &cancel).await {
                                break;
                            }
                        }
                        Some(ChannelMsg::ExitStatus { exit_status }) => exit_code = Some(exit_status),
                        Some(ChannelMsg::Failure) | Some(ChannelMsg::Close) | None => break,
                        _ => {}
                    }
                }
            }
            if !cancel.is_cancelled() {
                let _ = send(
                    &output,
                    &target,
                    &terminal_id,
                    &mut seq,
                    &mut bytes,
                    true,
                    exit_code,
                    &window,
                    &credits,
                    &cancel,
                )
                .await;
            }
            let _ = channel.close().await;
            if let Ok(mut outstanding) = credits.lock() {
                outstanding.clear();
            }
            if let Ok(mut entries) = this.entries.lock() {
                entries.remove(&terminal_id);
            }
        });
        Ok(id)
    }
    fn input(&self, target: &Target, id: &str) -> AppResult<mpsc::Sender<Input>> {
        let entries = self.entries.lock().map_err(|e| AppError::io("pty", e))?;
        let t = entries
            .get(id)
            .ok_or_else(|| AppError::new("E_NOT_FOUND", "pty", "Terminal has ended"))?;
        if !same_resource(target, &t.target) {
            return Err(AppError::new(
                "E_TARGET",
                "pty",
                "Terminal belongs to another target",
            ));
        }
        Ok(t.input.clone())
    }
    pub async fn write(&self, target: &Target, id: &str, data: String) -> AppResult<()> {
        if data.len() > 1024 * 1024 {
            return Err(AppError::new("E_LIMIT", "pty", "Input exceeds 1 MiB"));
        }
        self.input(target, id)?
            .send(Input::Data(data))
            .await
            .map_err(|e| AppError::io("pty", e))
    }
    pub async fn resize(&self, target: &Target, id: &str, cols: u32, rows: u32) -> AppResult<()> {
        let (c, r) = dimensions(cols, rows);
        self.input(target, id)?
            .send(Input::Resize(c, r))
            .await
            .map_err(|e| AppError::io("pty", e))
    }
    pub fn ack(&self, target: &Target, id: &str, seq: u64) -> AppResult<()> {
        let entries = self.entries.lock().map_err(|e| AppError::io("pty", e))?;
        if let Some(t) = entries.get(id) {
            if !same_resource(target, &t.target) {
                return Err(AppError::new(
                    "E_TARGET",
                    "pty",
                    "Terminal belongs to another target",
                ));
            }
            t.credits
                .lock()
                .map_err(|e| AppError::io("pty", e))?
                .retain(|sent, _| *sent > seq);
        }
        Ok(())
    }
    pub fn close(&self, target: &Target, id: &str) -> AppResult<()> {
        let mut entries = self.entries.lock().map_err(|e| AppError::io("pty", e))?;
        if let Some(t) = entries.get(id) {
            if !same_resource(target, &t.target) {
                return Err(AppError::new(
                    "E_TARGET",
                    "pty",
                    "Terminal belongs to another target",
                ));
            }
            t.cancel.cancel();
            t.credits
                .lock()
                .map_err(|e| AppError::io("pty", e))?
                .clear();
            entries.remove(id);
        }
        Ok(())
    }
    pub async fn close_connection(&self, id: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, t| {
                if t.target.connection_id == id {
                    t.cancel.cancel();
                    if let Ok(mut credits) = t.credits.lock() {
                        credits.clear();
                    }
                    false
                } else {
                    true
                }
            });
        }
    }
    pub fn reset_frontend(&self) -> AppResult<()> {
        let mut entries = self.entries.lock().map_err(|e| AppError::io("pty", e))?;
        for terminal in entries.values() {
            terminal.cancel.cancel();
            terminal
                .credits
                .lock()
                .map_err(|e| AppError::io("pty", e))?
                .clear();
        }
        entries.clear();
        Ok(())
    }
}
async fn confirm(
    channel: &mut russh::Channel<russh::client::Msg>,
    stage: &str,
    buffer: &mut Vec<u8>,
) -> AppResult<()> {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Success) => return Ok(()),
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    if buffer.len() + data.len() > 65536 {
                        return Err(AppError::new(
                            "E_LIMIT",
                            stage,
                            "Terminal startup output exceeds limit",
                        ));
                    }
                    buffer.extend_from_slice(&data);
                }
                Some(ChannelMsg::Failure) | Some(ChannelMsg::Close) | None => {
                    return Err(AppError::new(
                        "E_PTY",
                        stage,
                        "Server rejected the terminal request",
                    ))
                }
                _ => {}
            }
        }
    })
    .await
    .map_err(|_| {
        AppError::new(
            "E_TIMEOUT",
            stage,
            "Server did not confirm the terminal request",
        )
    })?
}
#[allow(clippy::too_many_arguments)]
async fn send(
    output: &tauri::ipc::Channel<TerminalFrame>,
    target: &Target,
    id: &str,
    seq: &mut u64,
    bytes: &mut Vec<u8>,
    exited: bool,
    code: Option<u32>,
    window: &Arc<Semaphore>,
    credits: &Arc<Mutex<BTreeMap<u64, OwnedSemaphorePermit>>>,
    cancel: &CancellationToken,
) -> bool {
    loop {
        let permit = tokio::select! {_=cancel.cancelled()=>return false,p=window.clone().acquire_owned()=>match p{Ok(p)=>p,Err(_)=>return false}};
        *seq += 1;
        let amount = bytes.len().min(32768);
        let data: Vec<_> = bytes.drain(..amount).collect();
        let last = bytes.is_empty();
        if let Ok(mut map) = credits.lock() {
            map.insert(*seq, permit);
        } else {
            return false;
        }
        if output
            .send(TerminalFrame {
                target: target.clone(),
                terminal_id: id.into(),
                data,
                seq: *seq,
                exited: exited && last,
                exit_code: code,
            })
            .is_err()
        {
            if let Ok(mut map) = credits.lock() {
                map.remove(seq);
            }
            return false;
        }
        if last {
            return true;
        }
    }
}
fn dimensions(cols: u32, rows: u32) -> (u32, u32) {
    (cols.clamp(2, 1000), rows.clamp(1, 500))
}
pub fn command(path: &str, kind: &str) -> AppResult<String> {
    let action = match kind {
        "shell" => "exec \"$shell\" -l".to_owned(),
        "claude" | "codex" => format!(
            "case \"${{shell##*/}}\" in\n  csh|tcsh) exec \"$shell\" -ic 'exec {kind}' ;;\n  *) exec \"$shell\" -lic 'exec {kind}' ;;\nesac"
        ),
        _ => return Err(AppError::new("E_INPUT", "pty", "Unknown terminal kind")),
    };
    // Only /bin/sh parses this script. The final interactive program remains the
    // user's shell; csh/tcsh require -l to be used without a -c command.
    Ok(format!(
        "cd {} || exit 1\nshell=${{SHELL:-/bin/sh}}\n{action}\n",
        quote(path)
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cumulative_ack_recovers_a_full_window_after_lost_acknowledgements() {
        let manager = PtyManager::default();
        let target = Target {
            connection_id: "connection".into(),
            project_id: None,
            session_id: None,
            request_seq: 1,
        };
        let window = Arc::new(Semaphore::new(8));
        let credits = Arc::new(Mutex::new(BTreeMap::new()));
        for seq in 1..=8 {
            let permit = window.clone().acquire_owned().await.unwrap();
            credits.lock().unwrap().insert(seq, permit);
        }
        let (input, _) = mpsc::channel(1);
        manager.entries.lock().unwrap().insert(
            "terminal".into(),
            Terminal {
                target: target.clone(),
                input,
                cancel: CancellationToken::new(),
                credits,
            },
        );
        manager.ack(&target, "terminal", 8).unwrap();
        assert_eq!(window.available_permits(), 8);
        manager.ack(&target, "terminal", 8).unwrap();
        assert_eq!(window.available_permits(), 8);
    }
    #[tokio::test]
    async fn frontend_reset_closes_old_terminals_and_releases_all_credit() {
        let manager = PtyManager::default();
        let target = Target {
            connection_id: "connection".into(),
            project_id: None,
            session_id: None,
            request_seq: 1,
        };
        let window = Arc::new(Semaphore::new(1));
        let credits = Arc::new(Mutex::new(BTreeMap::new()));
        let permit = window.clone().acquire_owned().await.unwrap();
        credits.lock().unwrap().insert(1, permit);
        let cancel = CancellationToken::new();
        let (input, _) = mpsc::channel(1);
        manager.entries.lock().unwrap().insert(
            "terminal".into(),
            Terminal {
                target: target.clone(),
                input,
                cancel: cancel.clone(),
                credits: credits.clone(),
            },
        );
        let mut other = target.clone();
        other.connection_id = "other".into();
        assert_eq!(
            manager.ack(&other, "terminal", 1).unwrap_err().code,
            "E_TARGET"
        );
        assert_eq!(window.available_permits(), 0);
        manager.reset_frontend().unwrap();
        assert!(cancel.is_cancelled());
        assert!(manager.entries.lock().unwrap().is_empty());
        assert!(credits.lock().unwrap().is_empty());
        assert_eq!(window.available_permits(), 1);
    }

    #[test]
    fn terminal_bootstrap_keeps_posix_syntax_out_of_the_login_shell() {
        let script = command("/work/a' b/中文", "shell").unwrap();
        assert!(script.starts_with("cd '/work/a'\\'' b/中文' || exit 1\n"));
        assert!(script.contains("shell=${SHELL:-/bin/sh}"));
        assert!(script.ends_with("exec \"$shell\" -l\n"));
        let script = command("/work", "codex").unwrap();
        assert!(script.contains("csh|tcsh) exec \"$shell\" -ic 'exec codex'"));
    }
    #[test]
    fn terminal_commands_only_launch_known_programs() {
        assert!(command("/a' b", "codex").unwrap().contains("-lic"));
        assert!(command("/a", "rm -rf /").is_err());
        assert_eq!(dimensions(0, 0), (2, 1));
    }
    #[test]
    fn resource_target_ignores_request_sequence_but_not_project() {
        let t = Target {
            connection_id: "a".into(),
            project_id: Some("p".into()),
            session_id: None,
            request_seq: 1,
        };
        let mut b = t.clone();
        b.request_seq = 2;
        assert!(same_resource(&t, &b));
        b.project_id = Some("q".into());
        assert!(!same_resource(&t, &b));
    }
}
