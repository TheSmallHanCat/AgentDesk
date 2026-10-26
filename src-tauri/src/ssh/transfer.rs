use crate::{
    error::{AppError, AppResult},
    models::Target,
    ssh::{manager::SshManager, pty::same_resource, sftp},
};
use serde::Serialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::Emitter;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Semaphore,
};
use tokio_util::sync::CancellationToken;
#[path = "archive.rs"]
mod archive;
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transfer {
    pub id: String,
    pub target: Target,
    pub path: String,
    pub destination: String,
    pub local_path: Option<String>,
    pub state: String,
    pub transferred: u64,
    pub total: u64,
    pub speed: u64,
    pub error: Option<String>,
}
struct Job {
    value: Transfer,
    cancel: CancellationToken,
    done: CancellationToken,
}
pub struct TransferManager {
    jobs: Mutex<HashMap<String, Job>>,
    slots: Arc<Semaphore>,
}
impl Default for TransferManager {
    fn default() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(3)),
        }
    }
}
impl TransferManager {
    pub fn list(&self) -> AppResult<Vec<Transfer>> {
        Ok(self
            .jobs
            .lock()
            .map_err(|e| AppError::io("transfer", e))?
            .values()
            .map(|j| j.value.clone())
            .collect())
    }
    fn update(&self, app: &Option<tauri::AppHandle>, id: &str, change: impl FnOnce(&mut Transfer)) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(j) = jobs.get_mut(id) {
                let previous = j.value.state.clone();
                change(&mut j.value);
                if let Some(app) = app {
                    let _ = app.emit("transfer-state", &j.value);
                    if previous != j.value.state {
                        let count = jobs
                            .values()
                            .filter(|j| matches!(j.value.state.as_str(), "queued" | "running"))
                            .count();
                        if let Some(tray) = app.tray_by_id("main-tray") {
                            let _ = tray.set_tooltip(Some(if count > 0 {
                                format!("AgentDesk · {count}")
                            } else {
                                "AgentDesk".into()
                            }));
                            let rgba = if count > 0 {
                                [208, 171, 112, 255].repeat(32 * 32)
                            } else {
                                [63, 112, 212, 255].repeat(32 * 32)
                            };
                            let _ =
                                tray.set_icon(Some(tauri::image::Image::new_owned(rgba, 32, 32)));
                        }
                    }
                }
            }
        }
    }
    pub fn cancel(&self, target: &Target, id: &str) -> AppResult<()> {
        let jobs = self.jobs.lock().map_err(|e| AppError::io("transfer", e))?;
        let j = jobs
            .get(id)
            .ok_or_else(|| AppError::new("E_NOT_FOUND", "transfer", "Transfer not found"))?;
        if !same_resource(target, &j.value.target) {
            return Err(AppError::new(
                "E_TARGET",
                "transfer",
                "Transfer belongs to another target",
            ));
        }
        j.cancel.cancel();
        Ok(())
    }
    pub async fn close_connection(&self, id: &str) -> AppResult<()> {
        let waits = {
            let jobs = self.jobs.lock().map_err(|e| AppError::io("transfer", e))?;
            jobs.values()
                .filter(|job| {
                    job.value.target.connection_id == id
                        && matches!(job.value.state.as_str(), "queued" | "running")
                })
                .map(|job| {
                    job.cancel.cancel();
                    job.done.clone()
                })
                .collect::<Vec<_>>()
        };
        tokio::time::timeout(Duration::from_secs(20), async {
            for done in waits {
                done.cancelled().await;
            }
        })
        .await
        .map_err(|_| {
            AppError::new(
                "E_TIMEOUT",
                "transfer",
                "Transfers are still cleaning up; wait before disconnecting",
            )
        })
    }
    pub async fn shutdown(&self) -> AppResult<()> {
        let waits = {
            let jobs = self.jobs.lock().map_err(|e| AppError::io("shutdown", e))?;
            jobs.values()
                .filter(|j| matches!(j.value.state.as_str(), "queued" | "running"))
                .map(|j| {
                    j.cancel.cancel();
                    j.done.clone()
                })
                .collect::<Vec<_>>()
        };
        tokio::time::timeout(Duration::from_secs(20), async {
            for done in waits {
                done.cancelled().await;
            }
        })
        .await
        .map_err(|_| {
            AppError::new(
                "E_TIMEOUT",
                "shutdown",
                "Transfers are still cleaning up; wait and try exiting again",
            )
        })
    }
    pub fn download(
        self: &Arc<Self>,
        app: impl Into<Option<tauri::AppHandle>>,
        ssh: Arc<SshManager>,
        target: Target,
        path: String,
        destination: String,
    ) -> AppResult<String> {
        let app = app.into();
        ssh.validate_target(&target)?;
        let path = sftp::remote_path(&path)?;
        if path == "/" {
            return Err(AppError::new(
                "E_PROTECTED",
                "download",
                "Root directory cannot be downloaded",
            ));
        }
        if !Path::new(&destination).is_absolute() || !Path::new(&destination).is_dir() {
            return Err(AppError::new(
                "E_DESTINATION",
                "download",
                "Choose an existing destination directory",
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        let done = CancellationToken::new();
        let value = Transfer {
            id: id.clone(),
            target: target.clone(),
            path: path.clone(),
            destination: destination.clone(),
            local_path: None,
            state: "queued".into(),
            transferred: 0,
            total: 0,
            speed: 0,
            error: None,
        };
        {
            let mut jobs = self.jobs.lock().map_err(|e| AppError::io("transfer", e))?;
            if jobs
                .values()
                .filter(|j| matches!(j.value.state.as_str(), "queued" | "running"))
                .count()
                >= 32
            {
                return Err(AppError::new(
                    "E_LIMIT",
                    "transfer",
                    "Transfer queue is full",
                ));
            }
            if jobs.len() >= 200 {
                jobs.retain(|_, j| matches!(j.value.state.as_str(), "queued" | "running"));
            }
            jobs.insert(
                id.clone(),
                Job {
                    value,
                    cancel: cancel.clone(),
                    done: done.clone(),
                },
            );
        }
        let this = self.clone();
        let job_id = id.clone();
        tokio::spawn(async move {
            let slot = tokio::select! {_=cancel.cancelled()=>None,p=this.slots.clone().acquire_owned()=>p.ok()};
            let result = if slot.is_none() {
                Err(cancelled())
            } else {
                this.update(&app, &job_id, |v| v.state = "running".into());
                this.run(&app, &job_id, &ssh, &target, &path, &destination, &cancel)
                    .await
            };
            this.update(&app, &job_id, |v| match result {
                Ok(path) => {
                    v.state = "completed".into();
                    v.local_path = Some(path.to_string_lossy().into_owned());
                    v.speed = 0;
                }
                Err(e) => {
                    v.state = if e.code == "E_CANCELLED" {
                        "cancelled"
                    } else {
                        "failed"
                    }
                    .into();
                    v.error = if e.code == "E_CANCELLED" {
                        None
                    } else {
                        Some(e.to_string())
                    };
                    v.speed = 0;
                }
            });
            done.cancel();
        });
        Ok(id)
    }
    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        app: &Option<tauri::AppHandle>,
        id: &str,
        ssh: &SshManager,
        target: &Target,
        path: &str,
        destination: &str,
        cancel: &CancellationToken,
    ) -> AppResult<PathBuf> {
        let session = sftp::session(ssh, target).await?;
        let result = async {
            let meta = session.metadata(path).await.map_err(sftp::error)?;
            if !meta.is_dir() && !meta.is_regular() {
                return Err(AppError::new("E_FILE_TYPE", "download", "Only files and directories can be downloaded"));
            }
            let name = safe_name(path.rsplit('/').next().unwrap_or("download"));
            let name = if meta.is_dir() { format!("{name}.tar.gz") } else { name };
            let temp = tempfile::Builder::new()
                .prefix(".agentdesk-")
                .suffix(".part")
                .tempfile_in(destination)
                .map_err(|e| AppError::io("download", e))?;
            let mut file = tokio::fs::File::from_std(temp.reopen().map_err(|e| AppError::io("download", e))?);
            let started = Instant::now();
            let mut last = Instant::now();
            let transferred = if meta.is_dir() {
                archive::copy(ssh, target, path, &mut file, cancel, |bytes| {
                    self.progress(app, id, bytes, started, &mut last);
                }).await?
            } else {
                let total = meta.size.unwrap_or(0);
                self.update(app, id, |value| value.total = total);
                let mut remote = session.open(path).await.map_err(sftp::error)?;
                let copied = async {
                    let mut transferred = 0u64;
                    let mut buffer = vec![0u8; 65536];
                    loop {
                        let amount = tokio::select! {
                            _ = cancel.cancelled() => return Err(cancelled()),
                            read = tokio::time::timeout(Duration::from_secs(60), remote.read(&mut buffer)) => {
                                read.map_err(|_| AppError::new("E_TIMEOUT", "download", "File download stopped responding"))?
                                    .map_err(|e| AppError::io("download", e))?
                            }
                        };
                        if amount == 0 { break; }
                        file.write_all(&buffer[..amount]).await.map_err(|e| AppError::io("download", e))?;
                        transferred += amount as u64;
                        self.progress(app, id, transferred, started, &mut last);
                    }
                    if transferred != total {
                        return Err(AppError::new("E_CONFLICT", "download", "Remote file size changed during download"));
                    }
                    let after = session.metadata(path).await.map_err(sftp::error)?;
                    if after.mtime != meta.mtime || after.size != meta.size {
                        return Err(AppError::new("E_CONFLICT", "download", "Remote file changed during download"));
                    }
                    Ok(transferred)
                }.await;
                let _ = remote.close().await;
                copied?
            };
            if cancel.is_cancelled() { return Err(cancelled()); }
            file.flush().await.map_err(|e| AppError::io("download", e))?;
            file.sync_all().await.map_err(|e| AppError::io("download", e))?;
            drop(file);
            self.update(app, id, |value| {
                value.transferred = transferred;
                value.total = transferred;
            });
            persist(temp, Path::new(destination), &name)
        }.await;
        sftp::finish(session, result).await
    }
    fn progress(
        &self,
        app: &Option<tauri::AppHandle>,
        id: &str,
        bytes: u64,
        start: Instant,
        last: &mut Instant,
    ) {
        if last.elapsed() >= Duration::from_millis(100) {
            let secs = start.elapsed().as_secs_f64().max(0.001);
            self.update(app, id, |v| {
                v.transferred = bytes;
                v.speed = (bytes as f64 / secs) as u64;
            });
            *last = Instant::now();
        }
    }
}
fn cancelled() -> AppError {
    AppError::new("E_CANCELLED", "download", "Transfer cancelled")
}
fn safe_name(name: &str) -> String {
    let mut n: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    n = n.trim_end_matches([' ', '.']).to_string();
    if n.is_empty() {
        n = "download".into();
    }
    let base = n.split('.').next().unwrap().to_ascii_uppercase();
    if [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ]
    .contains(&base.as_str())
    {
        n.insert(0, '_');
    }
    n
}
fn persist(mut temp: tempfile::NamedTempFile, dir: &Path, name: &str) -> AppResult<PathBuf> {
    let (stem, ext) = if let Some(s) = name.strip_suffix(".tar.gz") {
        (s.to_string(), ".tar.gz".into())
    } else {
        let p = Path::new(name);
        (
            p.file_stem().unwrap().to_string_lossy().into_owned(),
            p.extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default(),
        )
    };
    for n in 0..10000 {
        let p = dir.join(if n == 0 {
            name.to_string()
        } else {
            format!("{stem} ({n}){ext}")
        });
        match temp.persist_noclobber(&p) {
            Ok(_) => return Ok(p),
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => temp = e.file,
            Err(e) => return Err(AppError::io("download", e)),
        }
    }
    Err(AppError::new(
        "E_DESTINATION",
        "download",
        "No available destination name",
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collision_never_overwrites() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.tar.gz"), "keep").unwrap();
        let temp = tempfile::NamedTempFile::new_in(d.path()).unwrap();
        let path = persist(temp, d.path(), "a.tar.gz").unwrap();
        assert_eq!(path.file_name().unwrap(), "a (1).tar.gz");
        assert_eq!(
            std::fs::read_to_string(d.path().join("a.tar.gz")).unwrap(),
            "keep"
        );
    }
    #[test]
    fn windows_names_are_safe() {
        assert_eq!(safe_name("CON.txt"), "_CON.txt");
        assert_eq!(safe_name("a:b?. "), "a_b_");
    }
}
