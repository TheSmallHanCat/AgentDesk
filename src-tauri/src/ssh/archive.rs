use super::cancelled;
use crate::{
    error::{AppError, AppResult},
    models::Target,
    ssh::{manager::SshManager, sftp},
};
use russh::{client, Channel, ChannelMsg};
use std::time::Duration;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

const STREAM: &str = include_str!("archive_stream.sh");
const CLEANUP: &str = include_str!("archive_cleanup.sh");

fn launch_script(marker: &str, parent: &str, base: &str) -> String {
    // Noninteractive shells redirect an unqualified background job's stdin to
    // /dev/null. Preserve SSH stdin explicitly for the authorization gate. Parse
    // the whole block before launching, so the outer parser cannot consume GO.
    // With job control disabled the background PID differs from its inherited
    // PGID, allowing legacy setsid to exec without forking; wait retains status.
    format!(
        "{{\n  set +m\n  exec 3<&0\n  setsid /bin/sh -c {} {} {} {} <&3 3<&- &\n  archive_pid=$!\n  exec 3<&-\n  archive_status=0\n  wait \"$archive_pid\" || archive_status=$?\n  exit \"$archive_status\"\n}}\n",
        sftp::quote(STREAM),
        sftp::quote(marker),
        sftp::quote(parent),
        sftp::quote(base)
    )
}

pub(super) async fn copy(
    ssh: &SshManager,
    target: &Target,
    path: &str,
    file: &mut tokio::fs::File,
    cancel: &CancellationToken,
    progress: impl FnMut(u64),
) -> AppResult<u64> {
    let (parent, base) = path
        .rsplit_once('/')
        .ok_or_else(|| AppError::new("E_PATH", "archive", "Archive path must be absolute"))?;
    let parent = if parent.is_empty() { "/" } else { parent };
    let marker = format!("agentdesk-archive-{}", uuid::Uuid::new_v4());
    let script = launch_script(&marker, parent, base);
    let mut channel = ssh.open_channel(target).await?;
    let result = async {
        channel
            .exec(true, "exec /bin/sh -s")
            .await
            .map_err(|e| AppError::io("archive", e))?;
        channel
            .data_bytes(script.into_bytes())
            .await
            .map_err(|e| AppError::io("archive", e))?;
        receive(
            &mut channel,
            file,
            &marker,
            cancel,
            Duration::from_secs(60),
            progress,
        )
        .await
    }
    .await;
    // A non-PTY exec does not receive a reliable SIGHUP from channel close.
    // Explicitly close first, then terminate and check the owned process group.
    let _ = channel.close().await;
    if result.is_err() {
        let script = CLEANUP.replace("__MARKER__", &sftp::quote(&marker));
        tokio::time::timeout(Duration::from_secs(15), ssh.exec(target, &script))
            .await
            .map_err(|_| {
                AppError::new("E_CLEANUP", "archive", "Archive process cleanup timed out")
            })?
            .map_err(|_| {
                AppError::new(
                    "E_CLEANUP",
                    "archive",
                    "Could not confirm archive process cleanup",
                )
            })?;
    }
    result
}

async fn receive(
    channel: &mut Channel<client::Msg>,
    file: &mut (impl AsyncWrite + Unpin),
    marker: &str,
    cancel: &CancellationToken,
    idle: Duration,
    mut progress: impl FnMut(u64),
) -> AppResult<u64> {
    let mut deadline = tokio::time::Instant::now() + idle;
    let expected = format!("AGENTDESK_ARCHIVE_READY:{marker}\n");
    let mut header = Vec::new();
    let mut authorized = false;
    let mut transferred = 0;
    let mut status = None;
    loop {
        let message = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(cancelled()),
            _ = tokio::time::sleep_until(deadline) => return Err(AppError::new("E_TIMEOUT", "archive", "Archive download stopped responding")),
            message = channel.wait() => message,
        };
        match message {
            Some(ChannelMsg::Data { data }) => {
                if !authorized {
                    return Err(AppError::new(
                        "E_ARCHIVE",
                        "archive",
                        "Archive started before process confirmation",
                    ));
                }
                if !data.is_empty() {
                    file.write_all(&data)
                        .await
                        .map_err(|e| AppError::io("download", e))?;
                    deadline = tokio::time::Instant::now() + idle;
                    transferred += data.len() as u64;
                    progress(transferred);
                }
            }
            Some(ChannelMsg::ExtendedData { data, .. }) => {
                if !data.is_empty() {
                    deadline = tokio::time::Instant::now() + idle;
                }
                if !authorized {
                    header.extend_from_slice(&data);
                    if header.len() > 1024 || !expected.as_bytes().starts_with(&header) {
                        return Err(AppError::new(
                            "E_ARCHIVE",
                            "archive",
                            "Archive process confirmation was invalid",
                        ));
                    }
                    if header == expected.as_bytes() {
                        channel
                            .data_bytes(format!("{marker}\n").into_bytes())
                            .await
                            .map_err(|e| AppError::io("archive", e))?;
                        channel
                            .eof()
                            .await
                            .map_err(|e| AppError::io("archive", e))?;
                        authorized = true;
                    }
                }
            }
            Some(ChannelMsg::ExitStatus { exit_status }) => status = Some(exit_status),
            Some(ChannelMsg::Failure) => {
                return Err(AppError::new(
                    "E_ARCHIVE",
                    "archive",
                    "Remote archive command rejected",
                ))
            }
            Some(ChannelMsg::Close) | None => break,
            _ => {}
        }
    }
    if !authorized || status != Some(0) {
        return Err(AppError::new(
            "E_ARCHIVE",
            "archive",
            "Archive command did not exit successfully",
        ));
    }
    Ok(transferred)
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::{keys::PrivateKey, server, ChannelId};
    use std::sync::Arc;

    const MARKER: &str = "agentdesk-archive-regression";

    fn shell_program() -> std::path::PathBuf {
        if !cfg!(windows) {
            return "sh".into();
        }
        let output = std::process::Command::new("git")
            .arg("--exec-path")
            .output()
            .unwrap();
        assert!(output.status.success());
        let git_path = std::path::PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
        git_path
            .ancestors()
            .flat_map(|path| [path.join("bin/sh.exe"), path.join("usr/bin/sh.exe")])
            .find(|path| path.is_file())
            .expect("Windows archive regression requires Git Bash")
    }

    fn shell_path(path: &std::path::Path) -> String {
        let path = path.to_string_lossy().replace('\\', "/");
        if cfg!(windows) && path.as_bytes().get(1) == Some(&b':') {
            format!("/{}{}", path[..1].to_ascii_lowercase(), &path[2..])
        } else {
            path
        }
    }

    async fn actual_archive(authorize: bool, exists: bool) -> std::process::Output {
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt, BufReader};
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let shim = bin.join("setsid");
        // Model the legacy CentOS setsid interface while delegating actual
        // process/session creation to the installed executable.
        std::fs::write(&shim, "#!/bin/sh\ncase \"$1\" in -*) printf 'unsupported setsid option\\n' >&2; exit 64;; esac\nexec \"$REAL_SETSID\" \"$@\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::fs::write(directory.path().join("payload"), "archive regression").unwrap();
        let script = format!("REAL_SETSID=$(command -v setsid)\nexport REAL_SETSID\nPATH={}:\"$PATH\"\nexport PATH\n{}",
            sftp::quote(&shell_path(&bin)), launch_script(MARKER, &shell_path(directory.path()), if exists { "payload" } else { "missing" }));
        let mut child = tokio::process::Command::new(shell_program())
            .arg("-s")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut errors = BufReader::new(child.stderr.take().unwrap());
        input.write_all(script.as_bytes()).await.unwrap();
        let mut ready = String::new();
        tokio::time::timeout(Duration::from_secs(10), errors.read_line(&mut ready))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ready, format!("AGENTDESK_ARCHIVE_READY:{MARKER}\n"));
        if authorize {
            input
                .write_all(format!("{MARKER}\n").as_bytes())
                .await
                .unwrap();
        }
        drop(input);
        tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn legacy_setsid_without_wait_option_streams_and_propagates_exit_status() {
        use std::process::Stdio;
        let output = actual_archive(true, true).await;
        assert!(output.status.success());
        assert!(output.stdout.starts_with(&[0x1f, 0x8b]));
        let mut extract = tokio::process::Command::new(shell_program())
            .args(["-c", "tar -xOzf -"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut input = extract.stdin.take().unwrap();
        input.write_all(&output.stdout).await.unwrap();
        drop(input);
        let extracted = tokio::time::timeout(Duration::from_secs(10), extract.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(extracted.status.success());
        assert_eq!(extracted.stdout, b"archive regression");
        let failed = actual_archive(true, false).await;
        assert_eq!(failed.status.code(), Some(2));
    }

    #[tokio::test]
    async fn legacy_setsid_cancellation_before_authorization_never_starts_tar() {
        let output = actual_archive(false, true).await;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }

    struct TestClient;
    impl client::Handler for TestClient {
        type Error = russh::Error;
        async fn check_server_key(
            &mut self,
            _key: &russh::keys::PublicKeyOrCertificate,
        ) -> Result<bool, Self::Error> {
            // A fresh key is generated for each in-memory server; no network
            // endpoint or persistent trust store participates in these tests.
            Ok(true)
        }
    }

    struct TestServer {
        status: Option<u32>,
        noise: bool,
        channels: Vec<Channel<server::Msg>>,
        workers: Vec<tokio::task::JoinHandle<()>>,
    }
    impl Drop for TestServer {
        fn drop(&mut self) {
            for worker in &self.workers {
                worker.abort();
            }
        }
    }
    impl server::Handler for TestServer {
        type Error = russh::Error;
        async fn auth_none(&mut self, _user: &str) -> Result<server::Auth, Self::Error> {
            Ok(server::Auth::Accept)
        }
        async fn channel_open_session(
            &mut self,
            channel: Channel<server::Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            self.channels.push(channel);
            reply.accept().await;
            Ok(())
        }
        async fn exec_request(
            &mut self,
            id: ChannelId,
            _command: &[u8],
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            session.channel_success(id)?;
            session.extended_data(
                id,
                1,
                format!("AGENTDESK_ARCHIVE_READY:{MARKER}\n").into_bytes(),
            )?;
            Ok(())
        }
        async fn data(
            &mut self,
            id: ChannelId,
            data: &[u8],
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            assert_eq!(data, format!("{MARKER}\n").as_bytes());
            let handle = session.handle();
            let status = self.status;
            let noise = self.noise;
            self.workers.push(tokio::spawn(async move {
                for _ in 0..8 {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    if noise {
                        if handle.channel_success(id).await.is_err() {
                            return;
                        }
                    } else if handle.data(id, b"chunk".to_vec()).await.is_err() {
                        return;
                    }
                }
                if let Some(status) = status {
                    let _ = handle.exit_status_request(id, status).await;
                }
                let _ = handle.close(id).await;
            }));
            Ok(())
        }
    }

    async fn fixture(
        status: Option<u32>,
        noise: bool,
        cancel: CancellationToken,
    ) -> (AppResult<u64>, Vec<u8>) {
        let key = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
        let config = Arc::new(server::Config {
            keys: vec![key],
            ..Default::default()
        });
        let (client_stream, server_stream) = tokio::io::duplex(65536);
        let server = tokio::spawn(async move {
            if let Ok(session) = server::run_stream(
                config,
                server_stream,
                TestServer {
                    status,
                    noise,
                    channels: Vec::new(),
                    workers: Vec::new(),
                },
            )
            .await
            {
                let _ = session.await;
            }
        });
        let mut client = client::connect_stream(
            Arc::new(client::Config::default()),
            client_stream,
            TestClient,
        )
        .await
        .unwrap();
        client.authenticate_none("fixture").await.unwrap();
        let mut channel = client.channel_open_session().await.unwrap();
        channel.exec(true, "exec /bin/sh -s").await.unwrap();
        let mut bytes = Vec::new();
        let result = receive(
            &mut channel,
            &mut bytes,
            MARKER,
            &cancel,
            Duration::from_millis(100),
            |_| {},
        )
        .await;
        let _ = channel.close().await;
        let _ = client
            .disconnect(russh::Disconnect::ByApplication, "done", "en")
            .await;
        server.abort();
        (result, bytes)
    }

    #[tokio::test]
    async fn archive_idle_deadline_resets_for_each_data_chunk() {
        let started = std::time::Instant::now();
        let (result, bytes) = fixture(Some(0), false, CancellationToken::new()).await;
        assert_eq!(result.unwrap(), 40);
        assert_eq!(bytes, b"chunk".repeat(8));
        assert!(started.elapsed() > Duration::from_millis(100));
    }

    #[tokio::test]
    async fn archive_nonzero_or_missing_exit_status_never_succeeds() {
        for status in [Some(1), None] {
            let (result, _) = fixture(status, false, CancellationToken::new()).await;
            assert_eq!(result.unwrap_err().code, "E_ARCHIVE");
        }
    }

    #[tokio::test]
    async fn archive_protocol_noise_does_not_extend_the_data_idle_deadline() {
        let (result, bytes) = fixture(Some(0), true, CancellationToken::new()).await;
        assert_eq!(result.unwrap_err().code, "E_TIMEOUT");
        assert!(bytes.is_empty());
    }

    #[tokio::test]
    async fn archive_cancellation_does_not_write_output() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (result, bytes) = fixture(Some(0), false, cancel).await;
        assert_eq!(result.unwrap_err().code, "E_CANCELLED");
        assert!(bytes.is_empty());
    }

    #[test]
    fn archive_script_is_streaming_and_requires_client_authorization() {
        assert!(STREAM.contains("tar -czf - -C"));
        assert!(STREAM.find("read -r authorization").unwrap() < STREAM.find("tar -czf").unwrap());
        assert!(!STREAM.contains("mktemp"));
        assert!(!STREAM.contains("/tmp"));
        assert!(CLEANUP.contains("$1 == $2"));
        assert!(CLEANUP.contains("/bin/kill -KILL"));
    }
}
