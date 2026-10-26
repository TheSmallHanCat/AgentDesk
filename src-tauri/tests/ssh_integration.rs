use agentdesk_lib::{
    db::Db,
    models::{Connection, Target},
    session_log::Diagnostics,
    ssh::{
        auth, forward,
        known_hosts::KnownHosts,
        manager::SshManager,
        pty::{PtyManager, TerminalFrame},
        sftp,
    },
};
use russh::{
    client,
    keys::{Algorithm, PrivateKey, PublicKey},
    server::{self, Auth, Session, VerifiedPublicKey},
    Channel, ChannelId,
};
use russh_sftp::protocol::{self, Attrs, Data, FileAttributes, OpenFlags, StatusCode};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::io::AsyncReadExt;

#[derive(Default)]
struct Observed {
    auth: AtomicUsize,
    pty: AtomicUsize,
    resizes: AtomicUsize,
    reverse: AtomicUsize,
}
struct ServerHandler {
    observed: Arc<Observed>,
    public: PublicKey,
    channels: HashMap<ChannelId, Channel<server::Msg>>,
}
impl server::Handler for ServerHandler {
    type Error = russh::Error;
    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        self.observed.auth.fetch_add(1, Ordering::SeqCst);
        Ok(if user == "fixture" && password == "fixture-password" {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }
    async fn auth_publickey(
        &mut self,
        user: &str,
        key: VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        self.observed.auth.fetch_add(1, Ordering::SeqCst);
        Ok(if user == "fixture" && key.public_key == &self.public {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }
    async fn channel_open_session(
        &mut self,
        channel: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }
    async fn pty_request(
        &mut self,
        id: ChannelId,
        _term: &str,
        _cols: u32,
        _rows: u32,
        _w: u32,
        _h: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.observed.pty.fetch_add(1, Ordering::SeqCst);
        session.channel_success(id)?;
        Ok(())
    }
    async fn exec_request(
        &mut self,
        id: ChannelId,
        _command: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(id)?;
        session.data(id, "ready 中文\r\n".as_bytes().to_vec())?;
        Ok(())
    }
    async fn data(
        &mut self,
        id: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.channels.contains_key(&id) {
            session.data(id, data.to_vec())?;
        }
        Ok(())
    }
    async fn window_change_request(
        &mut self,
        _id: ChannelId,
        _cols: u32,
        _rows: u32,
        _w: u32,
        _h: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.observed.resizes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name == "sftp" {
            session.channel_success(id)?;
            let channel = self.channels.remove(&id).unwrap();
            tokio::spawn(async move {
                russh_sftp::server::run(channel.into_stream(), MemorySftp::new()).await;
            });
        } else {
            session.channel_failure(id)?;
        }
        Ok(())
    }
    async fn channel_open_direct_streamlocal(
        &mut self,
        channel: Channel<server::Msg>,
        path: &str,
        reply: server::ChannelOpenHandle,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        assert_eq!(path, "/tmp/host.sock");
        reply.accept().await;
        let _ = session;
        tokio::spawn(async move {
            let _ = channel.data_bytes(b"streamlocal".to_vec()).await;
            let _ = channel.eof().await;
            let _ = channel.close().await;
        });
        Ok(())
    }
    async fn streamlocal_forward(
        &mut self,
        _path: &str,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        self.observed.reverse.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }
    async fn cancel_streamlocal_forward(
        &mut self,
        _path: &str,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        self.observed.reverse.fetch_sub(1, Ordering::SeqCst);
        Ok(true)
    }
}
#[derive(Clone)]
struct MemoryFile {
    bytes: Vec<u8>,
    mode: u32,
}
struct MemorySftp {
    files: HashMap<String, MemoryFile>,
    read_dirs: HashSet<String>,
}
impl MemorySftp {
    fn new() -> Self {
        Self {
            files: HashMap::from([(
                "/file.txt".into(),
                MemoryFile {
                    bytes: b"first\r\nsecond\r\n".to_vec(),
                    mode: 0o100755,
                },
            )]),
            read_dirs: HashSet::new(),
        }
    }
    fn attrs(&self, path: &str) -> Result<FileAttributes, StatusCode> {
        if path == "/" {
            return Ok(FileAttributes {
                permissions: Some(0o40755),
                ..Default::default()
            });
        }
        let f = self.files.get(path).ok_or(StatusCode::NoSuchFile)?;
        Ok(FileAttributes {
            size: Some(f.bytes.len() as u64),
            permissions: Some(f.mode),
            mtime: Some(42),
            ..Default::default()
        })
    }
}
fn status(id: u32) -> protocol::Status {
    protocol::Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "OK".into(),
        language_tag: "en".into(),
    }
}
impl russh_sftp::server::Handler for MemorySftp {
    type Error = StatusCode;
    fn unimplemented(&self) -> StatusCode {
        StatusCode::OpUnsupported
    }
    async fn realpath(&mut self, id: u32, path: String) -> Result<protocol::Name, StatusCode> {
        self.attrs(&path)?;
        Ok(protocol::Name {
            id,
            files: vec![protocol::File::dummy(path)],
        })
    }
    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        Ok(Attrs {
            id,
            attrs: self.attrs(&path)?,
        })
    }
    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        self.stat(id, path).await
    }
    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, StatusCode> {
        self.stat(id, handle).await
    }
    async fn open(
        &mut self,
        id: u32,
        filename: String,
        flags: OpenFlags,
        attrs: FileAttributes,
    ) -> Result<protocol::Handle, StatusCode> {
        if flags.contains(OpenFlags::EXCLUDE) && self.files.contains_key(&filename) {
            return Err(StatusCode::Failure);
        }
        if flags.contains(OpenFlags::CREATE) {
            self.files.entry(filename.clone()).or_insert(MemoryFile {
                bytes: Vec::new(),
                mode: attrs.permissions.unwrap_or(0o600) | 0o100000,
            });
        }
        self.attrs(&filename)?;
        Ok(protocol::Handle {
            id,
            handle: filename,
        })
    }
    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, StatusCode> {
        let bytes = &self.files.get(&handle).ok_or(StatusCode::NoSuchFile)?.bytes;
        let offset = offset as usize;
        if offset >= bytes.len() {
            return Err(StatusCode::Eof);
        }
        Ok(Data {
            id,
            data: bytes[offset..bytes.len().min(offset + len as usize)].to_vec(),
        })
    }
    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<protocol::Status, StatusCode> {
        let file = self.files.get_mut(&handle).ok_or(StatusCode::NoSuchFile)?;
        let offset = offset as usize;
        if offset + data.len() > file.bytes.len() {
            file.bytes.resize(offset + data.len(), 0);
        }
        file.bytes[offset..offset + data.len()].copy_from_slice(&data);
        Ok(status(id))
    }
    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<protocol::Status, StatusCode> {
        let file = self.files.get_mut(&path).ok_or(StatusCode::NoSuchFile)?;
        if let Some(mode) = attrs.permissions {
            file.mode = mode | 0o100000;
        }
        Ok(status(id))
    }
    async fn opendir(&mut self, id: u32, path: String) -> Result<protocol::Handle, StatusCode> {
        if path != "/" {
            return Err(StatusCode::NoSuchFile);
        }
        self.read_dirs.remove(&path);
        Ok(protocol::Handle { id, handle: path })
    }
    async fn readdir(&mut self, id: u32, handle: String) -> Result<protocol::Name, StatusCode> {
        if !self.read_dirs.insert(handle) {
            return Err(StatusCode::Eof);
        }
        Ok(protocol::Name {
            id,
            files: self
                .files
                .keys()
                .map(|path| {
                    protocol::File::new(path.trim_start_matches('/'), self.attrs(path).unwrap())
                })
                .collect(),
        })
    }
    async fn close(&mut self, id: u32, _handle: String) -> Result<protocol::Status, StatusCode> {
        Ok(status(id))
    }
}
struct Fixture {
    port: u16,
    key: PrivateKey,
    user_key: PrivateKey,
    observed: Arc<Observed>,
    task: tokio::task::JoinHandle<()>,
    dir: tempfile::TempDir,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn start() -> Self {
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let user_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let observed = Arc::new(Observed::default());
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let config = Arc::new(server::Config {
            keys: vec![key.clone()],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let obs = observed.clone();
        let public = user_key.public_key().clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let handler = ServerHandler {
                    observed: obs.clone(),
                    public: public.clone(),
                    channels: HashMap::new(),
                };
                let config = config.clone();
                tokio::spawn(async move {
                    if let Ok(running) = server::run_stream(config, stream, handler).await {
                        let _ = running.await;
                    }
                });
            }
        });
        Self {
            port,
            key,
            user_key,
            observed,
            task,
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn connection(&self) -> Connection {
        Connection {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Loopback".into(),
            host: "127.0.0.1".into(),
            port: self.port,
            user: "fixture".into(),
            auth_method: "password".into(),
            key_path: None,
            agent_kind: None,
            proxy_jump: None,
            source: None,
            last_used_at: None,
        }
    }
    fn manager(&self, connection: &Connection) -> Arc<SshManager> {
        let db = Arc::new(Db::open(&self.dir.path().join("test.sqlite")).unwrap());
        db.save_connection(connection).unwrap();
        Arc::new(SshManager::new(
            db,
            Arc::new(KnownHosts::new(self.dir.path().join("known_hosts"), None)),
            Arc::new(Diagnostics::new(self.dir.path())),
            None,
        ))
    }
    async fn raw(&self) -> client::Handle<PinnedClient> {
        client::connect(
            Arc::new(client::Config::default()),
            ("127.0.0.1", self.port),
            PinnedClient(self.key.public_key().clone()),
        )
        .await
        .unwrap()
    }
}
struct PinnedClient(PublicKey);
impl client::Handler for PinnedClient {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(key.public_key() == self.0)
    }
}
fn target(c: &Connection) -> Target {
    Target {
        connection_id: c.id.clone(),
        project_id: None,
        session_id: None,
        request_seq: 1,
    }
}

#[tokio::test]
async fn fingerprint_confirmation_precedes_authentication_and_changed_key_is_rejected() {
    let f = Fixture::start().await;
    let c = f.connection();
    let m = f.manager(&c);
    let t = target(&c);
    let first = m
        .connect(&t, Some("fixture-password".into()), false)
        .await
        .unwrap();
    assert_eq!(first.state, "awaitingTrust");
    assert_eq!(f.observed.auth.load(Ordering::SeqCst), 0);
    m.trust(&t, first.fingerprint.as_ref().unwrap()).unwrap();
    let connected = m
        .connect(&t, Some("fixture-password".into()), false)
        .await
        .unwrap();
    assert_eq!(connected.state, "connected");
    assert_eq!(f.observed.auth.load(Ordering::SeqCst), 1);
    m.disconnect(&t).await.unwrap();
    let wrong = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    std::fs::write(
        f.dir.path().join("known_hosts"),
        format!(
            "[127.0.0.1]:{} {}\n",
            f.port,
            wrong.public_key().to_openssh().unwrap()
        ),
    )
    .unwrap();
    let changed = m
        .connect(&t, Some("fixture-password".into()), false)
        .await
        .unwrap();
    assert_eq!(changed.state, "failed");
    assert!(changed.previous_fingerprint.is_some());
    assert_eq!(f.observed.auth.load(Ordering::SeqCst), 1);
    assert!(m.open_channel(&t).await.is_err());
}
#[tokio::test]
async fn password_rejection_and_encrypted_private_key_authentication() {
    let f = Fixture::start().await;
    let mut handle = f.raw().await;
    let mut c = f.connection();
    assert_eq!(
        auth::authenticate(&mut handle, &c, Some("wrong"))
            .await
            .unwrap_err()
            .code,
        "E_AUTH"
    );
    let encrypted = f
        .user_key
        .encrypt(&mut rand::rng(), "fixture-passphrase")
        .unwrap();
    let path = f.dir.path().join("encrypted-key");
    std::fs::write(
        &path,
        encrypted
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    c.auth_method = "key".into();
    c.key_path = Some(path.to_string_lossy().into_owned());
    auth::authenticate(&mut handle, &c, Some("fixture-passphrase"))
        .await
        .unwrap();
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "done", "en")
        .await;
}
#[tokio::test]
async fn pty_channel_echoes_unicode_resizes_and_checks_target_ownership() {
    let f = Fixture::start().await;
    let c = f.connection();
    let m = f.manager(&c);
    let t = target(&c);
    let first = m
        .connect(&t, Some("fixture-password".into()), false)
        .await
        .unwrap();
    m.trust(&t, first.fingerprint.as_ref().unwrap()).unwrap();
    m.connect(&t, Some("fixture-password".into()), false)
        .await
        .unwrap();
    let pty = Arc::new(PtyManager::default());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let channel = tauri::ipc::Channel::<TerminalFrame>::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(body) = body {
            let value: serde_json::Value = serde_json::from_str(&body).unwrap();
            tx.send(value).unwrap();
        }
        Ok(())
    });
    let id = pty
        .open(&m, t.clone(), "/", "shell", 80, 24, channel)
        .await
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let bytes: Vec<u8> = serde_json::from_value(frame["data"].clone()).unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("中文"));
    pty.ack(&t, &id, frame["seq"].as_u64().unwrap()).unwrap();
    let mut wrong = t.clone();
    wrong.connection_id = "another".into();
    assert_eq!(
        pty.write(&wrong, &id, "bad".into()).await.unwrap_err().code,
        "E_TARGET"
    );
    pty.resize(&t, &id, 100, 40).await.unwrap();
    pty.write(&t, &id, "输入\r".into()).await.unwrap();
    let echoed = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let bytes: Vec<u8> = serde_json::from_value(echoed["data"].clone()).unwrap();
    assert_eq!(bytes, "输入\r".as_bytes());
    assert_eq!(f.observed.pty.load(Ordering::SeqCst), 1);
    assert_eq!(f.observed.resizes.load(Ordering::SeqCst), 1);
    pty.close(&t, &id).unwrap();
    m.disconnect(&t).await.unwrap();
}
#[tokio::test]
async fn sftp_lists_reads_and_preserves_crlf_and_permissions_on_temp_write() {
    let f = Fixture::start().await;
    let c = f.connection();
    let m = f.manager(&c);
    let t = target(&c);
    let first = m
        .connect(&t, Some("fixture-password".into()), false)
        .await
        .unwrap();
    m.trust(&t, first.fingerprint.as_ref().unwrap()).unwrap();
    m.connect(&t, Some("fixture-password".into()), false)
        .await
        .unwrap();
    let s = sftp::session(&m, &t).await.unwrap();
    let entries = sftp::list(&s, "/", true).await.unwrap();
    assert_eq!(entries[0].name, "file.txt");
    let document = sftp::read(&s, "/file.txt").await.unwrap();
    assert_eq!(document.line_ending, "crlf");
    assert_eq!(document.permissions & 0o777, 0o755);
    let path = sftp::write_temp(&s, "/file.txt", "changed\r\n", document.permissions)
        .await
        .unwrap();
    let updated = sftp::read(&s, &path).await.unwrap();
    assert_eq!(updated.content, "changed\r\n");
    assert_eq!(updated.permissions & 0o777, 0o755);
    s.close().await.unwrap();
    m.disconnect(&t).await.unwrap();
}
#[tokio::test]
async fn streamlocal_direct_and_reverse_requests_are_supported() {
    let f = Fixture::start().await;
    let mut h = f.raw().await;
    auth::authenticate(&mut h, &f.connection(), Some("fixture-password"))
        .await
        .unwrap();
    let channel = tokio::time::timeout(
        Duration::from_secs(5),
        forward::direct(&h, "/tmp/host.sock"),
    )
    .await
    .expect("direct request timeout")
    .unwrap();
    let mut stream = channel.into_stream();
    let mut bytes = [0u8; 11];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut bytes))
        .await
        .expect("direct data timeout")
        .unwrap();
    assert_eq!(&bytes, b"streamlocal");
    tokio::time::timeout(
        Duration::from_secs(5),
        forward::reverse(&h, "/tmp/device.sock"),
    )
    .await
    .expect("reverse request timeout")
    .unwrap();
    assert_eq!(f.observed.reverse.load(Ordering::SeqCst), 1);
    tokio::time::timeout(
        Duration::from_secs(5),
        forward::cancel_reverse(&h, "/tmp/device.sock"),
    )
    .await
    .expect("cancel reverse timeout")
    .unwrap();
    assert_eq!(f.observed.reverse.load(Ordering::SeqCst), 0);
    let _ = h
        .disconnect(russh::Disconnect::ByApplication, "done", "en")
        .await;
}
