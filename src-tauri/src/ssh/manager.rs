use super::{
    auth,
    known_hosts::{KeyDecision, KnownHosts},
};
use crate::{
    db::Db,
    error::{AppError, AppResult},
    models::{Connection, ConnectionStatus, Target},
    secrets::Secrets,
    session_log::Diagnostics,
};
use russh::{
    client,
    keys::{HashAlg, PublicKey, PublicKeyOrCertificate},
    Channel, ChannelMsg, Disconnect,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::Emitter;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

struct PendingKey {
    host: String,
    port: u16,
    key: PublicKey,
}
#[derive(Default)]
struct Observation {
    key: Option<PublicKey>,
    previous: Option<String>,
    trusted: bool,
    error: Option<AppError>,
}
pub struct Client {
    known: Arc<KnownHosts>,
    host: String,
    port: u16,
    observed: Arc<Mutex<Observation>>,
}
impl client::Handler for Client {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let mut observed = self.observed.lock().unwrap_or_else(|e| e.into_inner());
        if key.certificate().is_some() {
            observed.error = Some(AppError::new(
                "E_HOST_CERT",
                "host-key",
                "Host certificates are not supported; use a pinned public key",
            ));
            return Ok(false);
        }
        let public = key.public_key();
        observed.key = Some(public.clone());
        match self.known.check(&self.host, self.port, &public) {
            Ok(KeyDecision::Trusted) => {
                observed.trusted = true;
                Ok(true)
            }
            Ok(KeyDecision::Unknown) => Ok(false),
            Ok(KeyDecision::Changed(old)) => {
                observed.previous = Some(old);
                Ok(false)
            }
            Err(error) => {
                observed.error = Some(error);
                Ok(false)
            }
        }
    }
}
type Session = Arc<AsyncMutex<client::Handle<Client>>>;
pub struct SshManager {
    db: Arc<Db>,
    known: Arc<KnownHosts>,
    diag: Arc<Diagnostics>,
    app: Option<tauri::AppHandle>,
    sessions: Mutex<HashMap<String, Session>>,
    statuses: Mutex<HashMap<String, ConnectionStatus>>,
    pending: Mutex<HashMap<String, PendingKey>>,
    controls: Mutex<HashMap<String, CancellationToken>>,
    attempts: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}
impl SshManager {
    pub fn new(
        db: Arc<Db>,
        known: Arc<KnownHosts>,
        diag: Arc<Diagnostics>,
        app: Option<tauri::AppHandle>,
    ) -> Self {
        Self {
            db,
            known,
            diag,
            app,
            sessions: Mutex::new(HashMap::new()),
            statuses: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            controls: Mutex::new(HashMap::new()),
            attempts: Mutex::new(HashMap::new()),
        }
    }
    fn lock_error(e: impl std::fmt::Display) -> AppError {
        AppError::io("connection-state", e)
    }
    fn attempt(&self, id: &str) -> AppResult<Arc<AsyncMutex<()>>> {
        Ok(self
            .attempts
            .lock()
            .map_err(Self::lock_error)?
            .entry(id.into())
            .or_default()
            .clone())
    }
    fn status(&self, status: ConnectionStatus) {
        if let Ok(mut states) = self.statuses.lock() {
            states.insert(status.connection_id.clone(), status.clone());
        }
        if let Some(app) = &self.app {
            let _ = app.emit("connection-state", status);
        }
    }
    pub fn statuses(&self) -> AppResult<Vec<ConnectionStatus>> {
        Ok(self
            .statuses
            .lock()
            .map_err(Self::lock_error)?
            .values()
            .cloned()
            .collect())
    }
    pub fn is_active(&self, id: &str) -> bool {
        self.controls.lock().is_ok_and(|c| c.contains_key(id))
            || self.sessions.lock().is_ok_and(|s| s.contains_key(id))
    }
    pub fn validate_target(&self, target: &Target) -> AppResult<()> {
        self.db.validate_target(target)
    }
    pub async fn connect(
        self: &Arc<Self>,
        target: &Target,
        credential: Option<String>,
        save: Option<bool>,
    ) -> AppResult<ConnectionStatus> {
        self.validate_target(target)?;
        let connection = self.db.connection(&target.connection_id)?;
        if connection
            .proxy_jump
            .as_ref()
            .is_some_and(|s| !s.is_empty())
        {
            return Err(AppError::new(
                "E_PROXYJUMP",
                "connection",
                "ProxyJump is imported but not supported in this milestone",
            ));
        }
        if self.is_active(&connection.id) {
            return Err(AppError::new(
                "E_BUSY",
                "connection",
                "Disconnect or stop reconnecting before connecting again",
            ));
        }
        let attempt = self.attempt(&connection.id)?;
        let _guard = attempt.lock().await;
        if self.is_active(&connection.id) {
            return Err(AppError::new(
                "E_BUSY",
                "connection",
                "Connection is already active",
            ));
        }
        let kind = secret_kind(&connection);
        let credential = match credential.filter(|s| !s.is_empty()) {
            Some(s) => Some(Zeroizing::new(s)),
            None if connection.auth_method != "agent" => Secrets::get(&connection.id, kind)?,
            None => None,
        };
        let cancel = CancellationToken::new();
        self.controls
            .lock()
            .map_err(Self::lock_error)?
            .insert(connection.id.clone(), cancel.clone());
        self.status(ConnectionStatus::new(&connection.id, "connecting"));
        let result = tokio::select! {biased; _=cancel.cancelled()=>Err(AppError::new("E_CANCELLED","connection","Connection cancelled")), result=self.establish(&connection,credential.as_deref().map(|s|s.as_str()))=>result};
        match result {
            Ok(status) if status.state == "connected" => {
                if save == Some(true) {
                    if let Some(secret) = credential.as_ref() {
                        if let Err(error) = Secrets::set(&connection.id, kind, secret) {
                            self.disconnect_locked(&connection.id).await?;
                            return Err(error);
                        }
                    }
                } else if save == Some(false) {
                    if let Err(error) = Secrets::clear(&connection.id, kind) {
                        self.disconnect_locked(&connection.id).await?;
                        return Err(error);
                    }
                }
                self.status(status.clone());
                let this = Arc::clone(self);
                tokio::spawn(async move {
                    this.supervise(connection, credential, cancel).await;
                });
                Ok(status)
            }
            Ok(status) => {
                self.controls
                    .lock()
                    .map_err(Self::lock_error)?
                    .remove(&connection.id);
                self.status(status.clone());
                Ok(status)
            }
            Err(error) => {
                self.disconnect_locked(&connection.id).await?;
                let mut status = ConnectionStatus::new(
                    &connection.id,
                    if error.code == "E_CANCELLED" {
                        "disconnected"
                    } else {
                        "failed"
                    },
                );
                status.error = Some(error.code.clone());
                self.status(status);
                Err(error)
            }
        }
    }
    async fn establish(
        &self,
        connection: &Connection,
        credential: Option<&str>,
    ) -> AppResult<ConnectionStatus> {
        let start = Instant::now();
        let socket = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::net::TcpStream::connect((connection.host.as_str(), connection.port)),
        )
        .await
        .map_err(|_| AppError::new("E_TIMEOUT", "tcp", "SSH connection timed out"))
        .and_then(|result| result.map_err(|e| AppError::io("tcp", e)));
        self.diag.record(
            &connection.id,
            "tcp",
            start.elapsed().as_millis() as u64,
            socket.as_ref().err().map(|e| e.code.as_str()),
        );
        let socket = socket?;
        let observed = Arc::new(Mutex::new(Observation::default()));
        let handler = Client {
            known: self.known.clone(),
            host: connection.host.clone(),
            port: connection.port,
            observed: observed.clone(),
        };
        let config = Arc::new(client::Config {
            preferred: self.known.preferred(&connection.host, connection.port)?,
            keepalive_interval: Some(Duration::from_secs(15)),
            keepalive_max: 2,
            nodelay: true,
            channel_buffer_size: 32,
            ..Default::default()
        });
        let start = Instant::now();
        let handshake = tokio::time::timeout(
            Duration::from_secs(15),
            client::connect_stream(config, socket, handler),
        )
        .await
        .map_err(|_| AppError::new("E_TIMEOUT", "host-key", "SSH handshake timed out"))
        .inspect_err(|e| {
            self.diag.record(
                &connection.id,
                "host-key",
                start.elapsed().as_millis() as u64,
                Some(&e.code),
            )
        })?;
        let mut status = ConnectionStatus::new(&connection.id, "connected");
        {
            let mut observed = observed.lock().map_err(Self::lock_error)?;
            if let Some(error) = observed.error.take() {
                self.diag.record(
                    &connection.id,
                    "host-key",
                    start.elapsed().as_millis() as u64,
                    Some(&error.code),
                );
                return Err(error);
            }
            if let Some(key) = &observed.key {
                status.fingerprint = Some(key.fingerprint(HashAlg::Sha256).to_string());
                status.key_type = Some(key.algorithm().to_string());
                if let Some(old) = &observed.previous {
                    status.state = "failed".into();
                    status.previous_fingerprint = Some(old.clone());
                    status.error = Some("E_HOST_KEY_CHANGED".into());
                } else if !observed.trusted {
                    status.state = "awaitingTrust".into();
                    self.pending.lock().map_err(Self::lock_error)?.insert(
                        connection.id.clone(),
                        PendingKey {
                            host: connection.host.clone(),
                            port: connection.port,
                            key: key.clone(),
                        },
                    );
                }
            }
        }
        self.diag.record(
            &connection.id,
            "host-key",
            start.elapsed().as_millis() as u64,
            status
                .error
                .as_deref()
                .or(if handshake.is_err() && status.state == "connected" {
                    Some("E_IO")
                } else {
                    None
                }),
        );
        if status.state != "connected" {
            return Ok(status);
        }
        let mut handle = handshake.map_err(|e| AppError::io("ssh-handshake", e))?;
        let start = Instant::now();
        let auth_result = tokio::time::timeout(
            Duration::from_secs(30),
            auth::authenticate(&mut handle, connection, credential),
        )
        .await
        .unwrap_or_else(|_| {
            Err(AppError::new(
                "E_TIMEOUT",
                "authentication",
                "SSH authentication timed out",
            ))
        });
        self.diag.record(
            &connection.id,
            "authentication",
            start.elapsed().as_millis() as u64,
            auth_result.as_ref().err().map(|e| e.code.as_str()),
        );
        if let Err(error) = auth_result {
            let _ = handle
                .disconnect(Disconnect::ByApplication, "Authentication failed", "en")
                .await;
            return Err(error);
        }
        let mut updated = connection.clone();
        updated.last_used_at = Some(crate::models::now());
        self.db.save_connection(&updated)?;
        self.sessions
            .lock()
            .map_err(Self::lock_error)?
            .insert(connection.id.clone(), Arc::new(AsyncMutex::new(handle)));
        Ok(status)
    }
    async fn supervise(
        self: Arc<Self>,
        connection: Connection,
        credential: Option<Zeroizing<String>>,
        cancel: CancellationToken,
    ) {
        let mut failures = 0u32;
        loop {
            tokio::select! {_=cancel.cancelled()=>break,_=tokio::time::sleep(Duration::from_millis(250))=>{}}
            let session = self
                .sessions
                .lock()
                .ok()
                .and_then(|s| s.get(&connection.id).cloned());
            if let Some(session) = session {
                if !session.lock().await.is_closed() {
                    failures = 0;
                    continue;
                }
            }
            if let Ok(mut sessions) = self.sessions.lock() {
                sessions.remove(&connection.id);
            }
            self.status(ConnectionStatus::new(&connection.id, "reconnecting"));
            if failures == 0 {
                self.diag
                    .record(&connection.id, "disconnect", 0, Some("E_DISCONNECTED"));
                if let Some(app) = &self.app {
                    let _ = app.emit(
                        "connection-lost",
                        serde_json::json!({"connectionId":connection.id}),
                    );
                }
            }
            let delay = reconnect_delay(failures);
            failures = failures.saturating_add(1);
            tokio::select! {_=cancel.cancelled()=>break,_=tokio::time::sleep(delay)=>{}}
            let attempt = match self.attempt(&connection.id) {
                Ok(a) => a,
                Err(_) => break,
            };
            let _guard = attempt.lock().await;
            let result = tokio::select! {biased;_=cancel.cancelled()=>break,r=self.establish(&connection,credential.as_deref().map(|s|s.as_str()))=>r};
            match result {
                Ok(status) => {
                    let connected = status.state == "connected";
                    self.status(status);
                    if !connected {
                        break;
                    }
                }
                Err(error) => {
                    if !retryable(&error.code) {
                        let mut status = ConnectionStatus::new(&connection.id, "failed");
                        status.error = Some(error.code);
                        self.status(status);
                        break;
                    }
                }
            }
        }
        if !cancel.is_cancelled() {
            if let Ok(mut controls) = self.controls.lock() {
                controls.remove(&connection.id);
            }
        }
    }
    pub fn trust(&self, target: &Target, fingerprint: &str) -> AppResult<()> {
        self.validate_target(target)?;
        let connection = self.db.connection(&target.connection_id)?;
        let mut pending = self.pending.lock().map_err(Self::lock_error)?;
        let key = pending.get(&target.connection_id).ok_or_else(|| {
            AppError::new(
                "E_HOST_KEY",
                "host-key",
                "No host key is awaiting confirmation",
            )
        })?;
        if key.host != connection.host
            || key.port != connection.port
            || key.key.fingerprint(HashAlg::Sha256).to_string() != fingerprint
        {
            return Err(AppError::new(
                "E_TARGET",
                "host-key",
                "Host key confirmation no longer matches the target",
            ));
        }
        self.known.trust(&key.host, key.port, &key.key)?;
        pending.remove(&target.connection_id);
        Ok(())
    }
    pub async fn disconnect(&self, target: &Target) -> AppResult<()> {
        self.validate_target(target)?;
        if let Some(cancel) = self
            .controls
            .lock()
            .map_err(Self::lock_error)?
            .remove(&target.connection_id)
        {
            cancel.cancel();
        }
        let attempt = self.attempt(&target.connection_id)?;
        let _guard = attempt.lock().await;
        self.disconnect_locked(&target.connection_id).await
    }
    async fn disconnect_locked(&self, id: &str) -> AppResult<()> {
        if let Some(cancel) = self.controls.lock().map_err(Self::lock_error)?.remove(id) {
            cancel.cancel();
        }
        self.pending.lock().map_err(Self::lock_error)?.remove(id);
        let session = self.sessions.lock().map_err(Self::lock_error)?.remove(id);
        if let Some(session) = session {
            let _ = session
                .lock()
                .await
                .disconnect(Disconnect::ByApplication, "Disconnected by user", "en")
                .await;
        }
        self.status(ConnectionStatus::new(id, "disconnected"));
        Ok(())
    }
    pub async fn open_channel(&self, target: &Target) -> AppResult<Channel<client::Msg>> {
        self.validate_target(target)?;
        let session = self
            .sessions
            .lock()
            .map_err(Self::lock_error)?
            .get(&target.connection_id)
            .cloned()
            .ok_or_else(|| {
                AppError::new(
                    "E_DISCONNECTED",
                    "channel",
                    "SSH connection is not available",
                )
            })?;
        let handle = session.lock().await;
        tokio::time::timeout(Duration::from_secs(15), handle.channel_open_session())
            .await
            .map_err(|_| AppError::new("E_TIMEOUT", "channel", "SSH channel open timed out"))?
            .map_err(|e| AppError::io("channel", e))
    }
    pub async fn exec(&self, target: &Target, command: &str) -> AppResult<Vec<u8>> {
        let mut channel = self.open_channel(target).await?;
        channel
            .exec(true, "exec /bin/sh -s")
            .await
            .map_err(|e| AppError::io("exec", e))?;
        channel
            .data_bytes([command.as_bytes(), b"\n"].concat())
            .await
            .map_err(|e| AppError::io("exec", e))?;
        channel.eof().await.map_err(|e| AppError::io("exec", e))?;
        let result = tokio::time::timeout(Duration::from_secs(60), async {
            let mut output = Vec::new();
            let mut status = None;
            while let Some(message) = channel.wait().await {
                match message {
                    ChannelMsg::Data { data } => {
                        if output.len() + data.len() > 4 * 1024 * 1024 {
                            return Err(AppError::new(
                                "E_LIMIT",
                                "exec",
                                "Command output exceeds limit",
                            ));
                        }
                        output.extend_from_slice(&data);
                    }
                    ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                    ChannelMsg::Failure => {
                        return Err(AppError::new(
                            "E_EXEC",
                            "exec",
                            "SSH exec request was rejected",
                        ))
                    }
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
            if status == Some(0) {
                Ok(output)
            } else {
                Err(AppError::new(
                    "E_EXEC",
                    "exec",
                    format!("Remote command failed (status {status:?})"),
                ))
            }
        })
        .await
        .map_err(|_| AppError::new("E_TIMEOUT", "exec", "Remote command timed out"));
        let _ = channel.close().await;
        result?
    }
}
fn secret_kind(connection: &Connection) -> &'static str {
    if connection.auth_method == "key" {
        "passphrase"
    } else {
        "password"
    }
}
pub fn reconnect_delay(attempt: u32) -> Duration {
    Duration::from_secs(1u64.checked_shl(attempt.min(5)).unwrap_or(30).min(30))
}
pub fn retryable(code: &str) -> bool {
    matches!(
        code,
        "E_IO" | "E_TIMEOUT" | "E_AUTH_TRANSPORT" | "E_DISCONNECTED"
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconnect_backoff_is_bounded() {
        assert_eq!(
            (0..7)
                .map(|n| reconnect_delay(n).as_secs())
                .collect::<Vec<_>>(),
            vec![1, 2, 4, 8, 16, 30, 30]
        );
    }
    #[test]
    fn credentials_and_host_keys_are_never_retried() {
        for code in [
            "E_AUTH",
            "E_KEY",
            "E_HOST_KEY_CHANGED",
            "E_CREDENTIAL_REQUIRED",
            "E_AGENT",
        ] {
            assert!(!retryable(code));
        }
    }
}
