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
    let script = format!(
        "exec setsid -w /bin/sh -c {} {} {} {}\n",
        sftp::quote(STREAM),
        sftp::quote(&marker),
        sftp::quote(parent),
        sftp::quote(base)
    );
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
