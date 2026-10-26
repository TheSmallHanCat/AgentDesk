use crate::{
    db::Db,
    error::{AppError, AppResult},
    models::Target,
    ssh::manager::SshManager,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{broadcast, oneshot, Mutex as AsyncMutex},
};
use tokio_util::sync::CancellationToken;

pub const MAX_FRAME: usize = 4 * 1024 * 1024;
type Writer = Box<dyn AsyncWrite + Unpin + Send>;
type Pending = HashMap<u64, oneshot::Sender<AppResult<Value>>>;

pub async fn read_frame<R: AsyncBufRead + Unpin>(reader: &mut R) -> std::io::Result<Vec<u8>> {
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(frame)
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Incomplete Host frame",
                ))
            };
        }
        let end = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|p| p + 1);
        let count = end.unwrap_or(available.len());
        if frame.len() + count > MAX_FRAME {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Host frame exceeds 4 MiB",
            ));
        }
        frame.extend_from_slice(&available[..count]);
        reader.consume(count);
        if end.is_some() {
            return Ok(frame);
        }
    }
}

pub struct HostClient {
    writer: AsyncMutex<Option<Writer>>,
    pending: Mutex<Pending>,
    next: AtomicU64,
    pub cancel: CancellationToken,
    pub events: broadcast::Sender<Value>,
    pub hello: Mutex<Value>,
}
impl HostClient {
    pub async fn connect(
        ssh: &SshManager,
        target: &Target,
        device_id: &str,
    ) -> AppResult<Arc<Self>> {
        let home = remote_home(ssh, target).await?;
        let socket = format!("{home}/.agentdesk/run/host.sock");
        let stream = ssh.open_streamlocal(target, &socket).await?.into_stream();
        let (reader, writer) = tokio::io::split(stream);
        let (events, _) = broadcast::channel(512);
        let client = Arc::new(Self {
            writer: AsyncMutex::new(Some(Box::new(writer))),
            pending: Mutex::default(),
            next: AtomicU64::new(1),
            cancel: CancellationToken::new(),
            events,
            hello: Mutex::new(Value::Null),
        });
        let weak = Arc::downgrade(&client);
        let cancel = client.cancel.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            loop {
                let frame = tokio::select! { _ = cancel.cancelled() => break, result = read_frame(&mut reader) => result };
                let Ok(frame) = frame else { break };
                if frame.is_empty() {
                    break;
                }
                let Ok(message) = serde_json::from_slice::<Value>(&frame) else {
                    break;
                };
                let Some(client) = weak.upgrade() else { break };
                if let Some(method) = message.get("method").and_then(Value::as_str) {
                    if method == "event" || method == "approval.request" {
                        let _ = client.events.send(message["params"].clone());
                    } else if message.get("id").is_some() {
                        // Host approvals are persisted events answered through
                        // approval.respond. Unknown server requests fail closed.
                        let _ = client.write(&json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32601,"message":"Use approval.respond"}})).await;
                    }
                } else if let Some(id) = message["id"].as_u64() {
                    let sender = client
                        .pending
                        .lock()
                        .ok()
                        .and_then(|mut map| map.remove(&id));
                    if let Some(sender) = sender {
                        let response = if message.get("error").is_some() {
                            let error = &message["error"];
                            let data = &error["data"];
                            Err(AppError::new(
                                data["code"].as_str().unwrap_or("E_HOST"),
                                data["stage"].as_str().unwrap_or("host.rpc"),
                                error["message"].as_str().unwrap_or("Host request failed"),
                            )
                            .with_details(data.clone()))
                        } else {
                            Ok(message["result"].clone())
                        };
                        let _ = sender.send(response);
                    }
                }
            }
            cancel.cancel();
            if let Some(client) = weak.upgrade() {
                if let Some(mut writer) = client.writer.lock().await.take() {
                    let _ = tokio::time::timeout(Duration::from_secs(1), writer.shutdown()).await;
                }
                if let Ok(mut pending) = client.pending.lock() {
                    pending.clear();
                }
            }
        });
        let hello = client.request("hello", json!({"protocolVersion":"1.0","client":"agentdesk-desktop/0.2.0","deviceId":device_id,"deviceName":"AgentDesk","os":std::env::consts::OS})).await?;
        if hello["protocolVersion"]
            .as_str()
            .and_then(|v| v.split('.').next())
            != Some("1")
        {
            client.cancel.cancel();
            return Err(AppError::new(
                "E_PROTOCOL_VERSION",
                "host.hello",
                "Incompatible Host protocol",
            ));
        }
        *client
            .hello
            .lock()
            .map_err(|e| AppError::io("host.hello", e))? = hello;
        Ok(client)
    }
    async fn write(&self, message: &Value) -> AppResult<()> {
        let mut data = serde_json::to_vec(message).map_err(|e| AppError::io("host.frame", e))?;
        data.push(b'\n');
        if data.len() > MAX_FRAME {
            return Err(AppError::new(
                "E_LIMIT",
                "host.frame",
                "Request exceeds 4 MiB",
            ));
        }
        let mut writer = self.writer.lock().await;
        let writer = writer.as_mut().ok_or_else(|| {
            AppError::new("E_DISCONNECTED", "host.write", "Host stream is closed")
        })?;
        writer
            .write_all(&data)
            .await
            .map_err(|e| AppError::io("host.write", e))?;
        writer
            .flush()
            .await
            .map_err(|e| AppError::io("host.write", e))
    }
    pub async fn request(&self, method: &str, params: Value) -> AppResult<Value> {
        if self.cancel.is_cancelled() {
            return Err(AppError::new(
                "E_DISCONNECTED",
                "host.rpc",
                "Host is disconnected",
            ));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self
                .pending
                .lock()
                .map_err(|e| AppError::io("host.rpc", e))?;
            if pending.len() >= 64 {
                return Err(AppError::new(
                    "E_BUSY",
                    "host.rpc",
                    "Too many pending Host requests",
                ));
            }
            pending.insert(id, sender);
        }
        let result = async {
            tokio::time::timeout(
                Duration::from_secs(20),
                self.write(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})),
            )
            .await
            .map_err(|_| AppError::new("E_TIMEOUT", "host.write", "Host write timed out"))??;
            tokio::time::timeout(Duration::from_secs(90), receiver)
                .await
                .map_err(|_| AppError::new("E_TIMEOUT", "host.rpc", "Host response timed out"))?
                .map_err(|_| {
                    AppError::new(
                        "E_DISCONNECTED",
                        "host.rpc",
                        "Host disconnected before acknowledgement",
                    )
                })?
        }
        .await;
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&id);
        }
        if result.as_ref().is_err_and(|error| {
            matches!(error.code.as_str(), "E_TIMEOUT" | "E_IO" | "E_DISCONNECTED")
        }) {
            self.cancel.cancel();
        }
        if matches!(method, "run.send" | "run.append")
            && result
                .as_ref()
                .is_err_and(|e| matches!(e.code.as_str(), "E_TIMEOUT" | "E_DISCONNECTED" | "E_IO"))
        {
            return Err(AppError::new(
                "E_DELIVERY_UNKNOWN",
                "run.send",
                "Message delivery is unknown; inspect session history before resending",
            ));
        }
        result
    }
}
impl Drop for HostClient {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub async fn remote_home(ssh: &SshManager, target: &Target) -> AppResult<String> {
    let bytes = ssh.exec(target, "printf '%s' \"$HOME\"").await?;
    let home = String::from_utf8(bytes).map_err(|e| AppError::io("host.home", e))?;
    crate::ssh::forward::validate_socket(&format!("{home}/.agentdesk/run/host.sock"))?;
    Ok(home)
}

pub struct HostPool {
    clients: AsyncMutex<HashMap<String, Arc<HostClient>>>,
    connect_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    pub subscriptions: Mutex<HashMap<String, (Target, CancellationToken)>>,
    device_id: String,
}
impl HostPool {
    pub fn new(device_id: String) -> Self {
        Self {
            clients: AsyncMutex::default(),
            connect_locks: Mutex::default(),
            subscriptions: Mutex::default(),
            device_id,
        }
    }
    pub async fn client(
        &self,
        ssh: &SshManager,
        db: &Db,
        target: &Target,
    ) -> AppResult<Arc<HostClient>> {
        ssh.validate_target(target)?;
        let connect_lock = self
            .connect_locks
            .lock()
            .map_err(|error| AppError::io("host.connect", error))?
            .entry(target.connection_id.clone())
            .or_default()
            .clone();
        let _guard = connect_lock.lock().await;
        if let Some(client) = self
            .clients
            .lock()
            .await
            .get(&target.connection_id)
            .filter(|c| !c.cancel.is_cancelled())
        {
            return Ok(client.clone());
        }
        let client = HostClient::connect(ssh, target, &self.device_id).await?;
        let host_id = client
            .hello
            .lock()
            .map_err(|e| AppError::io("host.identity", e))?["hostId"]
            .as_str()
            .ok_or_else(|| {
                AppError::new("E_PROTOCOL", "host.identity", "Host identity is missing")
            })?
            .to_owned();
        db.record_host(&target.connection_id, &host_id)?;
        self.clients
            .lock()
            .await
            .insert(target.connection_id.clone(), client.clone());
        Ok(client)
    }
    pub async fn disconnect(&self, connection_id: &str) {
        if let Some(client) = self.clients.lock().await.remove(connection_id) {
            client.cancel.cancel();
        }
        if let Ok(mut entries) = self.subscriptions.lock() {
            entries.retain(|_, (target, token)| {
                if target.connection_id == connection_id {
                    token.cancel();
                    false
                } else {
                    true
                }
            });
        }
    }
    pub fn unsubscribe(&self, target: &Target, id: &str) -> AppResult<()> {
        let mut entries = self
            .subscriptions
            .lock()
            .map_err(|e| AppError::io("host.unsubscribe", e))?;
        if let Some((owner, _)) = entries.get(id) {
            if !crate::ssh::pty::same_resource(owner, target) {
                return Err(AppError::new(
                    "E_TARGET",
                    "host.unsubscribe",
                    "Subscription belongs to another target",
                ));
            }
        }
        if let Some((_, token)) = entries.remove(id) {
            token.cancel();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };
    struct PartialFailure {
        written: Arc<Mutex<Vec<u8>>>,
    }
    impl AsyncWrite for PartialFailure {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            data: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            let mut written = self.written.lock().unwrap();
            if written.is_empty() {
                written.extend_from_slice(&data[..3]);
                Poll::Ready(Ok(3))
            } else {
                Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "write failed",
                )))
            }
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
    #[tokio::test]
    async fn partial_write_failure_invalidates_transport_without_resending() {
        let written = Arc::new(Mutex::new(Vec::new()));
        let (events, _) = broadcast::channel(1);
        let client = HostClient {
            writer: AsyncMutex::new(Some(Box::new(PartialFailure {
                written: written.clone(),
            }))),
            pending: Mutex::default(),
            next: AtomicU64::new(1),
            cancel: CancellationToken::new(),
            events,
            hello: Mutex::new(Value::Null),
        };
        assert_eq!(
            client
                .request("run.send", json!({"text":"test"}))
                .await
                .unwrap_err()
                .code,
            "E_DELIVERY_UNKNOWN"
        );
        assert!(client.cancel.is_cancelled());
        assert_eq!(
            client
                .request("session.list", json!({}))
                .await
                .unwrap_err()
                .code,
            "E_DISCONNECTED"
        );
        assert_eq!(written.lock().unwrap().len(), 3);
    }
    #[tokio::test]
    async fn frame_limits_and_partial_eof() {
        let mut reader = BufReader::new(&b"{\"id\":1}\n{\"id\":2}\n"[..]);
        assert_eq!(read_frame(&mut reader).await.unwrap(), b"{\"id\":1}\n");
        assert_eq!(read_frame(&mut reader).await.unwrap(), b"{\"id\":2}\n");
        assert!(read_frame(&mut BufReader::new(&b"{}"[..])).await.is_err());
        let oversized = vec![b'x'; MAX_FRAME + 1];
        assert!(read_frame(&mut BufReader::new(&oversized[..]))
            .await
            .is_err());
    }
}
