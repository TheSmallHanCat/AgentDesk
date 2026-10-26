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
        channel
            .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
            .await
            .map_err(|e| AppError::io("pty", e))?;
        channel
            .exec(true, command)
            .await
            .map_err(|e| AppError::io("pty", e))?;
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
            let mut bytes = Vec::with_capacity(32768);
            let mut tick = tokio::time::interval(Duration::from_millis(16));
            let mut exit_code = None;
            loop {
                tokio::select! {biased;
                    _=cancel.cancelled()=>break,
                    input=rx.recv()=>match input {Some(Input::Data(data))=>{if channel.data_bytes(data.into_bytes()).await.is_err(){break;}},Some(Input::Resize(c,r))=>{if channel.window_change(c,r,0,0).await.is_err(){break;}},None=>break},
                    _=tick.tick(),if !bytes.is_empty()=>{if !send(&output,&target,&terminal_id,&mut seq,&mut bytes,false,None,&window,&credits,&cancel).await{break;}},
                    message=channel.wait()=>match message {
                        Some(ChannelMsg::Data{data})|Some(ChannelMsg::ExtendedData{data,..})=>{bytes.extend_from_slice(&data);if bytes.len()>=32768&&!send(&output,&target,&terminal_id,&mut seq,&mut bytes,false,None,&window,&credits,&cancel).await{break;}},
                        Some(ChannelMsg::ExitStatus{exit_status})=>exit_code=Some(exit_status),
                        Some(ChannelMsg::Failure)|Some(ChannelMsg::Close)|None=>break,_=>{},
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
                .remove(&seq);
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
            entries.remove(id);
        }
        Ok(())
    }
    pub async fn close_connection(&self, id: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, t| {
                if t.target.connection_id == id {
                    t.cancel.cancel();
                    false
                } else {
                    true
                }
            });
        }
    }
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
        "shell" => "exec \"${SHELL:-/bin/sh}\" -l",
        "claude" => "exec claude",
        "codex" => "exec codex",
        _ => return Err(AppError::new("E_INPUT", "pty", "Unknown terminal kind")),
    };
    // A login, interactive shell loads nvm and shell rc configuration before starting the command.
    let script = format!("cd -- {} || exit 1; {action}", quote(path));
    Ok(format!(
        "exec \"${{SHELL:-/bin/sh}}\" -lic {}",
        quote(&script)
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
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
