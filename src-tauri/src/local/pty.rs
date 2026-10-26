//! Local interactive terminals use ConPTY on Windows, with bounded frontend backpressure.

use crate::{
    error::{AppError, AppResult},
    models::Target,
    ssh::pty::TerminalFrame,
};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::{
    collections::{BTreeMap, HashMap},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

enum Input {
    Data(String),
    Resize(PtySize),
}

struct Control {
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    #[cfg(windows)]
    job: crate::platform::executor::Job,
}

impl Control {
    fn kill(&self) {
        #[cfg(windows)]
        self.job.terminate();
        if let Ok(mut killer) = self.killer.lock() {
            let _ = killer.kill();
        }
    }
}

struct Terminal {
    target: Target,
    input: mpsc::Sender<Input>,
    cancel: CancellationToken,
    finished: CancellationToken,
    control: Arc<Control>,
    credits: Arc<Mutex<BTreeMap<u64, OwnedSemaphorePermit>>>,
}

#[derive(Default)]
pub struct PtyManager {
    entries: Mutex<HashMap<String, Terminal>>,
}

fn same_resource(first: &Target, second: &Target) -> bool {
    first.device_local == second.device_local && crate::ssh::pty::same_resource(first, second)
}

fn size(cols: u32, rows: u32) -> PtySize {
    PtySize {
        cols: cols.clamp(2, 1000) as u16,
        rows: rows.clamp(1, 500) as u16,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn shell_command(shell: &str, path: &Path) -> AppResult<(CommandBuilder, PathBuf)> {
    let executable = match shell {
        "" | "pwsh" => {
            let executable =
                std::env::current_exe().map_err(|error| AppError::io("local.pty", error))?;
            super::executor::ExecutorResources::discover(
                executable.parent().unwrap_or(Path::new(".")),
            )
            .powershell
        }
        "cmd" => PathBuf::from(
            std::env::var_os("COMSPEC").unwrap_or_else(|| "C:\\Windows\\System32\\cmd.exe".into()),
        ),
        configured if Path::new(configured).is_absolute() => PathBuf::from(configured),
        _ => {
            return Err(AppError::new(
                "E_INPUT",
                "local.pty",
                "Select pwsh, cmd, or an absolute shell executable path",
            ))
        }
    };
    if !executable.is_file() {
        return Err(AppError::new(
            "E_LOCAL_COMPONENT",
            "local.pty",
            format!(
                "Local shell is missing: {}; prepare components or inspect security software",
                executable.display()
            ),
        ));
    }
    let mut command = CommandBuilder::new(&executable);
    match executable
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "pwsh" | "powershell" => {
            command.args(["-NoLogo", "-NoProfile"]);
        }
        "cmd" => command.arg("/d"),
        _ => (),
    }
    command.cwd(path);
    command.env("TERM", "xterm-256color");
    // Test infrastructure credentials must not flow into user shells.
    for (key, _) in std::env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("AGENTDESK_E2E_")
        {
            command.env_remove(key);
        }
    }
    Ok((command, executable))
}

impl PtyManager {
    #[allow(clippy::too_many_arguments)]
    pub async fn open(
        self: &Arc<Self>,
        target: Target,
        path: &str,
        shell: &str,
        cols: u32,
        rows: u32,
        output: tauri::ipc::Channel<TerminalFrame>,
    ) -> AppResult<String> {
        if !target.device_local {
            return Err(AppError::new(
                "E_TARGET",
                "local.pty",
                "Local terminal requires a local project target",
            ));
        }
        let path = std::fs::canonicalize(path).map_err(|error| AppError::io("local.pty", error))?;
        if !path.is_dir() {
            return Err(AppError::new(
                "E_INPUT",
                "local.pty",
                "Terminal working directory must be a directory",
            ));
        }
        let (command, executable) = shell_command(shell, &path)?;
        let dimensions = size(cols, rows);
        let (pair, mut child, reader, writer, control) = tokio::task::spawn_blocking(move || {
            let pair = native_pty_system()
                .openpty(dimensions)
                .map_err(|error| AppError::io("local.pty", error))?;
            let mut child = pair
                .slave
                .spawn_command(command)
                .map_err(|error| AppError::io("local.pty", error))?;
            #[cfg(windows)]
            let job = {
                let job = crate::platform::executor::Job::new()?;
                let result = child
                    .as_raw_handle()
                    .ok_or_else(|| {
                        AppError::new(
                            "E_LOCAL_PTY",
                            "local.pty",
                            "ConPTY did not return a process handle",
                        )
                    })
                    .and_then(|handle| {
                        // The child retains ownership until after Job assignment.
                        let handle =
                            unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(handle) };
                        job.assign_process(handle, &executable)
                    });
                if let Err(error) = result {
                    let _ = child.kill();
                    return Err(error);
                }
                job
            };
            #[cfg(not(windows))]
            let _ = executable;
            let reader = pair
                .master
                .try_clone_reader()
                .map_err(|error| AppError::io("local.pty", error))?;
            let writer = pair
                .master
                .take_writer()
                .map_err(|error| AppError::io("local.pty", error))?;
            let control = Arc::new(Control {
                killer: Mutex::new(child.clone_killer()),
                #[cfg(windows)]
                job,
            });
            Ok::<_, AppError>((pair, child, reader, writer, control))
        })
        .await
        .map_err(|error| AppError::io("local.pty", error))??;
        drop(pair.slave);
        let master = pair.master;
        let id = format!("local-{}", uuid::Uuid::new_v4());
        let cancel = CancellationToken::new();
        let finished = CancellationToken::new();
        let io_stop = CancellationToken::new();
        let credits = Arc::new(Mutex::new(BTreeMap::new()));
        let (input, rx) = mpsc::channel(32);
        let (data_tx, mut data_rx) = mpsc::channel(8);
        let (exit_tx, mut exit_rx) = tokio::sync::oneshot::channel();
        self.entries
            .lock()
            .map_err(|error| AppError::io("local.pty", error))?
            .insert(
                id.clone(),
                Terminal {
                    target: target.clone(),
                    input,
                    cancel: cancel.clone(),
                    finished: finished.clone(),
                    control: control.clone(),
                    credits: credits.clone(),
                },
            );
        read_output(reader, data_tx, cancel.clone());
        write_input(master, writer, rx, io_stop.clone(), control.clone());
        tokio::task::spawn_blocking(move || {
            let _ = exit_tx.send(child.wait().ok().map(|status| status.exit_code()));
        });
        let this = self.clone();
        let terminal_id = id.clone();
        tokio::spawn(async move {
            let window = Arc::new(Semaphore::new(8));
            let mut seq = 0;
            let mut bytes = Vec::new();
            let mut tick = tokio::time::interval(Duration::from_millis(16));
            let mut exit_code = None;
            let mut child_done = false;
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => break,
                    result = &mut exit_rx, if !child_done => {
                        child_done = true;
                        exit_code = result.ok().flatten();
                        io_stop.cancel();
                    }
                    data = data_rx.recv() => match data {
                        Some(data) => {
                            bytes.extend_from_slice(&data);
                            if bytes.len() >= 32768 && !send(&output, &target, &terminal_id, &mut seq, &mut bytes, false, None, &window, &credits, &cancel).await { break; }
                        }
                        None => break,
                    },
                    _ = tick.tick(), if !bytes.is_empty() => {
                        if !send(&output, &target, &terminal_id, &mut seq, &mut bytes, false, None, &window, &credits, &cancel).await { break; }
                    }
                }
            }
            if !child_done && !cancel.is_cancelled() {
                exit_code = tokio::time::timeout(Duration::from_secs(1), &mut exit_rx)
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .flatten();
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
            control.kill();
            io_stop.cancel();
            if let Ok(mut credits) = credits.lock() {
                credits.clear();
            }
            if let Ok(mut entries) = this.entries.lock() {
                entries.remove(&terminal_id);
            }
            finished.cancel();
        });
        Ok(id)
    }

    fn input(&self, target: &Target, id: &str) -> AppResult<mpsc::Sender<Input>> {
        let entries = self
            .entries
            .lock()
            .map_err(|error| AppError::io("local.pty", error))?;
        let terminal = entries
            .get(id)
            .ok_or_else(|| AppError::new("E_NOT_FOUND", "local.pty", "Terminal has ended"))?;
        if !same_resource(target, &terminal.target) {
            return Err(AppError::new(
                "E_TARGET",
                "local.pty",
                "Terminal belongs to another target",
            ));
        }
        Ok(terminal.input.clone())
    }

    pub async fn write(&self, target: &Target, id: &str, data: String) -> AppResult<()> {
        if data.len() > 1024 * 1024 {
            return Err(AppError::new("E_LIMIT", "local.pty", "Input exceeds 1 MiB"));
        }
        self.input(target, id)?
            .send(Input::Data(data))
            .await
            .map_err(|error| AppError::io("local.pty", error))
    }

    pub async fn resize(&self, target: &Target, id: &str, cols: u32, rows: u32) -> AppResult<()> {
        self.input(target, id)?
            .send(Input::Resize(size(cols, rows)))
            .await
            .map_err(|error| AppError::io("local.pty", error))
    }

    pub fn ack(&self, target: &Target, id: &str, seq: u64) -> AppResult<()> {
        let entries = self
            .entries
            .lock()
            .map_err(|error| AppError::io("local.pty", error))?;
        if let Some(terminal) = entries.get(id) {
            if !same_resource(target, &terminal.target) {
                return Err(AppError::new(
                    "E_TARGET",
                    "local.pty",
                    "Terminal belongs to another target",
                ));
            }
            terminal
                .credits
                .lock()
                .map_err(|error| AppError::io("local.pty", error))?
                .retain(|sent, _| *sent > seq);
        }
        Ok(())
    }

    pub fn close(&self, target: &Target, id: &str) -> AppResult<()> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|error| AppError::io("local.pty", error))?;
        if let Some(terminal) = entries.get(id) {
            if !same_resource(target, &terminal.target) {
                return Err(AppError::new(
                    "E_TARGET",
                    "local.pty",
                    "Terminal belongs to another target",
                ));
            }
        }
        if let Some(terminal) = entries.remove(id) {
            stop(&terminal);
        }
        Ok(())
    }

    pub async fn close_connection(&self, connection_id: &str) {
        let finished: Vec<_> = if let Ok(mut entries) = self.entries.lock() {
            let ids: Vec<_> = entries
                .iter()
                .filter(|(_, terminal)| terminal.target.connection_id == connection_id)
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| entries.remove(&id))
                .map(|terminal| {
                    stop(&terminal);
                    terminal.finished
                })
                .collect()
        } else {
            Vec::new()
        };
        for finished in finished {
            let _ = tokio::time::timeout(Duration::from_secs(5), finished.cancelled()).await;
        }
    }

    pub fn reset_frontend(&self) -> AppResult<()> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|error| AppError::io("local.pty", error))?;
        for terminal in entries.values() {
            stop(terminal);
        }
        entries.clear();
        Ok(())
    }
}

fn stop(terminal: &Terminal) {
    terminal.cancel.cancel();
    terminal.control.kill();
    if let Ok(mut credits) = terminal.credits.lock() {
        credits.clear();
    }
}

fn read_output(
    mut reader: Box<dyn Read + Send>,
    output: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,
) {
    tokio::task::spawn_blocking(move || {
        let mut buffer = [0; 32768];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(amount) => {
                    // Keep draining while ConPTY closes to avoid a blocked ClosePseudoConsole.
                    if !cancel.is_cancelled()
                        && output.blocking_send(buffer[..amount].to_vec()).is_err()
                    {
                        cancel.cancel();
                    }
                }
            }
        }
    });
}

fn write_input(
    mut master: Box<dyn MasterPty + Send>,
    mut writer: Box<dyn Write + Send>,
    mut input: mpsc::Receiver<Input>,
    stop: CancellationToken,
    control: Arc<Control>,
) {
    tokio::spawn(async move {
        while !stop.is_cancelled() {
            let value = tokio::select! { _ = stop.cancelled() => break, value = input.recv() => match value { Some(value) => value, None => break } };
            let result = tokio::task::spawn_blocking(move || {
                let result = match value {
                    Input::Data(data) => writer
                        .write_all(data.as_bytes())
                        .and_then(|_| writer.flush())
                        .map_err(|error| error.to_string()),
                    Input::Resize(size) => master.resize(size).map_err(|error| error.to_string()),
                };
                (master, writer, result)
            })
            .await;
            match result {
                Ok((next_master, next_writer, result)) => {
                    master = next_master;
                    writer = next_writer;
                    if result.is_err() {
                        break;
                    }
                }
                Err(_) => {
                    control.kill();
                    return;
                }
            }
        }
        // ClosePseudoConsole may wait for pending output; keep it off the async runtime.
        tokio::task::spawn_blocking(move || {
            drop(writer);
            drop(master);
        });
    });
}

#[allow(clippy::too_many_arguments)]
async fn send(
    output: &tauri::ipc::Channel<TerminalFrame>,
    target: &Target,
    id: &str,
    seq: &mut u64,
    bytes: &mut Vec<u8>,
    exited: bool,
    exit_code: Option<u32>,
    window: &Arc<Semaphore>,
    credits: &Arc<Mutex<BTreeMap<u64, OwnedSemaphorePermit>>>,
    cancel: &CancellationToken,
) -> bool {
    loop {
        let permit = tokio::select! { _ = cancel.cancelled() => return false, permit = window.clone().acquire_owned() => match permit { Ok(permit) => permit, Err(_) => return false } };
        *seq += 1;
        let amount = bytes.len().min(32768);
        let data = bytes.drain(..amount).collect();
        let last = bytes.is_empty();
        if let Ok(mut credits) = credits.lock() {
            credits.insert(*seq, permit);
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
                exit_code,
            })
            .is_err()
        {
            if let Ok(mut credits) = credits.lock() {
                credits.remove(seq);
            }
            return false;
        }
        if last {
            return true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "Starts real local pwsh and cmd ConPTY processes; requires prepared Windows resources"]
    async fn conpty_real_shells_frames_target_and_cleanup() {
        for shell in ["pwsh", "cmd"] {
            let manager = Arc::new(PtyManager::default());
            let target = Target {
                device_local: true,
                connection_id: "local-test".into(),
                project_id: Some("project".into()),
                session_id: None,
                request_seq: 1,
            };
            let directory = tempfile::tempdir().unwrap();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let output = tauri::ipc::Channel::new(move |body| {
                if let tauri::ipc::InvokeResponseBody::Json(body) = body {
                    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
                    let _ = tx.send(value);
                }
                Ok(())
            });
            let id = manager
                .open(
                    target.clone(),
                    directory.path().to_str().unwrap(),
                    shell,
                    80,
                    24,
                    output,
                )
                .await
                .unwrap();
            manager.resize(&target, &id, 120, 35).await.unwrap();
            let mut other = target.clone();
            other.device_local = false;
            assert_eq!(manager.ack(&other, &id, 1).unwrap_err().code, "E_TARGET");
            assert_eq!(
                manager
                    .write(&other, &id, "exit\r\n".into())
                    .await
                    .unwrap_err()
                    .code,
                "E_TARGET"
            );
            let source = if shell == "pwsh" {
                "Write-Output '本机PTY成功'; exit 23\r\n"
            } else {
                "echo local-pty-pass & exit 23\r\n"
            };
            manager.write(&target, &id, source.into()).await.unwrap();
            let mut all_output = Vec::new();
            let code = tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    let frame = rx.recv().await.expect("ConPTY frame stream ended");
                    let bytes: Vec<u8> = serde_json::from_value(frame["data"].clone()).unwrap();
                    if bytes.windows(4).any(|part| part == b"\x1b[6n") {
                        let _ = manager.write(&target, &id, "\x1b[1;1R".into()).await;
                    }
                    all_output.extend(bytes);
                    manager
                        .ack(&target, &id, frame["seq"].as_u64().unwrap())
                        .unwrap();
                    if frame["exited"] == true {
                        break frame["exitCode"].as_u64();
                    }
                }
            })
            .await
            .unwrap();
            assert_eq!(code, Some(23));
            let text = String::from_utf8_lossy(&all_output);
            assert!(text.contains(if shell == "pwsh" {
                "本机PTY成功"
            } else {
                "local-pty-pass"
            }));
            manager.close_connection(&target.connection_id).await;
            assert!(manager.entries.lock().unwrap().is_empty());
        }
    }
}
