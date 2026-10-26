//! Opt-in acceptance against the dedicated user. Never print server errors or credentials.
use agentdesk_lib::{
    commands::file::{protect, save},
    db::Db,
    error::AppResult,
    models::{Connection, Project, Target},
    session_log::Diagnostics,
    ssh::{
        auth, forward,
        known_hosts::KnownHosts,
        manager::SshManager,
        pty::{PtyManager, TerminalFrame},
        sftp,
        transfer::TransferManager,
    },
};
use russh::{client, keys::PublicKey, Channel};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BASE: &str = "/home/agentdesk-test/agentdesk-e2e";

fn must<T>(result: AppResult<T>) -> T {
    result.unwrap_or_else(|error| panic!("Operation failed: {} at {}", error.code, error.stage))
}

struct Environment {
    host: String,
    port: u16,
    root_user: String,
    root_password: String,
    private: PathBuf,
}
impl Environment {
    fn load() -> Option<Self> {
        let variables = [
            "AGENTDESK_E2E_HOST",
            "AGENTDESK_E2E_PORT",
            "AGENTDESK_E2E_ROOT_USER",
            "AGENTDESK_E2E_ROOT_PASSWORD",
        ];
        for variable in variables {
            if std::env::var(variable).ok().is_none_or(|v| v.is_empty()) {
                println!("SKIP: missing {variable}");
                return None;
            }
        }
        Some(Self {
            host: std::env::var(variables[0]).unwrap(),
            port: std::env::var(variables[1])
                .unwrap()
                .parse()
                .expect("Invalid test port"),
            root_user: std::env::var(variables[2]).unwrap(),
            root_password: std::env::var(variables[3]).unwrap(),
            private: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.tmp/e2e"),
        })
    }
    fn host_pattern(&self) -> String {
        if self.port == 22 {
            self.host.clone()
        } else {
            format!("[{}]:{}", self.host, self.port)
        }
    }
    fn key_auth_enabled(&self) -> bool {
        !std::fs::read_to_string(self.private.join("auth-server-policy.json"))
            .unwrap_or_default()
            .contains("pubkeyauthentication=no")
    }
    fn ordinary(&self) -> Connection {
        self.connection(false, self.key_auth_enabled().then_some("id_ed25519"))
    }
    fn credential(&self, connection: &Connection) -> Option<String> {
        (connection.auth_method == "password").then(|| {
            if connection.user == self.root_user {
                self.root_password.clone()
            } else {
                self.secret("user_password")
            }
        })
    }
    fn secret(&self, name: &str) -> String {
        std::fs::read_to_string(self.private.join(name))
            .expect("Run scripts/e2e_prepare.py provision first")
            .trim()
            .to_string()
    }
    fn connection(&self, root: bool, key: Option<&str>) -> Connection {
        Connection {
            id: uuid::Uuid::new_v4().to_string(),
            name: "E2E".into(),
            host: self.host.clone(),
            port: self.port,
            user: if root {
                self.root_user.clone()
            } else {
                "agentdesk-test".into()
            },
            auth_method: if key.is_some() { "key" } else { "password" }.into(),
            key_path: key.map(|name| self.private.join(name).to_string_lossy().into_owned()),
            agent_kind: Some("openssh".into()),
            proxy_jump: None,
            source: None,
            last_used_at: None,
        }
    }
}
struct Context {
    _dir: tempfile::TempDir,
    db: Arc<Db>,
    manager: Arc<SshManager>,
    known_path: PathBuf,
}
impl Context {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(must(Db::open(std::path::Path::new(":memory:"))));
        let known_path = dir.path().join("known_hosts");
        let manager = Arc::new(SshManager::new(
            db.clone(),
            Arc::new(KnownHosts::new(known_path.clone(), None)),
            Arc::new(Diagnostics::new(dir.path())),
            None,
        ));
        Self {
            _dir: dir,
            db,
            manager,
            known_path,
        }
    }
    async fn connect(&self, c: &Connection, credential: Option<String>) -> Target {
        must(self.db.save_connection(c));
        let target = Target {
            connection_id: c.id.clone(),
            project_id: None,
            session_id: None,
            request_seq: 1,
        };
        let initial = must(
            self.manager
                .connect(&target, credential.clone(), None)
                .await,
        );
        if initial.state == "awaitingTrust" {
            must(
                self.manager
                    .trust(&target, initial.fingerprint.as_deref().unwrap()),
            );
            assert_eq!(
                must(self.manager.connect(&target, credential, None).await).state,
                "connected"
            );
        } else {
            assert_eq!(initial.state, "connected");
        }
        target
    }
}

#[tokio::test]
#[ignore = "requires dedicated test server environment and scripts/e2e_prepare.py"]
async fn real_authentication_and_fingerprints() {
    let Some(env) = Environment::load() else {
        return;
    };
    let context = Context::new();
    let cases = [
        (true, None, Some(env.root_password.clone()), "root_password"),
        (
            false,
            None,
            Some(env.secret("user_password")),
            "user_password",
        ),
        (false, Some("id_ed25519"), None, "ed25519"),
        (
            false,
            Some("id_ed25519_encrypted"),
            Some(env.secret("key_passphrase")),
            "encrypted_ed25519",
        ),
        (false, Some("id_rsa"), None, "rsa_pem"),
    ];
    for (root, key, secret, label) in cases {
        if key.is_some() && !env.key_auth_enabled() {
            println!("SKIP: auth_{label}: server disables public key authentication");
            continue;
        }
        let c = env.connection(root, key);
        let t = context.connect(&c, secret).await;
        must(context.manager.disconnect(&t).await);
        println!("PASS: auth_{label}");
    }
    for (key, secret, label) in [
        (
            None,
            Some("agentdesk-invalid-password".to_string()),
            "wrong_password",
        ),
        (Some("id_wrong"), None, "wrong_key"),
    ] {
        if key.is_some() && !env.key_auth_enabled() {
            println!("SKIP: auth_reject_{label}: server disables public key authentication");
            continue;
        }
        let c = env.connection(false, key);
        must(context.db.save_connection(&c));
        let t = Target {
            connection_id: c.id,
            project_id: None,
            session_id: None,
            request_seq: 2,
        };
        let error = context.manager.connect(&t, secret, None).await.unwrap_err();
        assert_eq!(error.code, "E_AUTH");
        println!("PASS: auth_reject_{label}");
    }
    if env.key_auth_enabled() && std::env::var("AGENTDESK_E2E_AGENT").as_deref() == Ok("1") {
        let mut c = env.connection(false, None);
        c.auth_method = "agent".into();
        let t = context.connect(&c, None).await;
        must(context.manager.disconnect(&t).await);
        println!("PASS: auth_windows_agent_signature");
    } else {
        println!(
            "SKIP: Windows OpenSSH agent service unavailable or public key authentication disabled"
        );
    }
    let c = env.ordinary();
    let separate = Context::new();
    let t = separate.connect(&c, env.credential(&c)).await;
    must(separate.manager.disconnect(&t).await);
    let wrong =
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    std::fs::write(
        &separate.known_path,
        format!(
            "{} {}\n",
            env.host_pattern(),
            wrong.public_key().to_openssh().unwrap()
        ),
    )
    .unwrap();
    let changed = must(separate.manager.connect(&t, None, None).await);
    assert_eq!(changed.error.as_deref(), Some("E_HOST_KEY_CHANGED"));
    assert!(separate.manager.open_channel(&t).await.is_err());
    println!("PASS: fingerprint_first_confirmation_and_change_rejection");
    let ecdsa = env.private.join("server_ecdsa.pub");
    if ecdsa.exists() {
        let context = Context::new();
        let public = PublicKey::from_openssh(&std::fs::read_to_string(ecdsa).unwrap()).unwrap();
        std::fs::write(
            &context.known_path,
            format!(
                "{}\t{}\r\n",
                env.host_pattern(),
                public.to_openssh().unwrap().replace(' ', "\t")
            ),
        )
        .unwrap();
        let t = context.connect(&c, env.credential(&c)).await;
        assert!(must(context.manager.statuses())
            .iter()
            .any(|status| status.key_type.as_deref() == Some("ecdsa-sha2-nistp256")));
        must(context.manager.disconnect(&t).await);
        println!("PASS: fingerprint_ecdsa_only_crlf_tab");
    } else {
        println!("SKIP: test server has no ECDSA host key");
    }
}

#[tokio::test]
#[ignore = "requires dedicated test server environment and prepared scoped fixtures"]
async fn real_files_terminals_and_downloads() {
    let Some(env) = Environment::load() else {
        return;
    };
    let context = Context::new();
    let c = env.ordinary();
    let mut t = context.connect(&c, env.credential(&c)).await;
    let root = Project {
        id: "e2e-project".into(),
        connection_id: c.id.clone(),
        name: "E2E".into(),
        path: format!("{BASE}/files"),
        description: String::new(),
        default_agent: "claude".into(),
        default_model: String::new(),
        permission_mode: "default".into(),
        pinned: false,
        archived: false,
        last_opened_at: None,
    };
    must(context.db.save_project(&root));
    must(context.db.save_project(&Project {
        id: "second-project".into(),
        path: format!("{BASE}/another-project"),
        ..root.clone()
    }));
    t.project_id = Some(root.id);
    let s = must(sftp::session(&context.manager, &t).await);
    let visible = must(sftp::list(&s, &format!("{BASE}/files"), false).await);
    let hidden = must(sftp::list(&s, &format!("{BASE}/files"), true).await);
    assert!(!visible.iter().any(|entry| entry.name == ".hidden"));
    assert!(hidden.iter().any(|entry| entry.name == ".hidden"));
    let entries = must(sftp::list(&s, BASE, true).await);
    for link in ["link", "dangling"] {
        assert!(entries
            .iter()
            .any(|entry| entry.name == link && entry.kind == "symlink"));
    }
    assert!(sftp::list(&s, &format!("{BASE}/no-access"), true)
        .await
        .is_err());
    println!("PASS: sftp_hidden_symlink_dangling_permissions");
    for protected in [
        "/",
        "/etc",
        "/home",
        "/home/agentdesk-test",
        BASE,
        &format!("{BASE}/files"),
        &format!("{BASE}/another-project"),
    ] {
        assert_eq!(
            protect(&context.db, &t, &s, protected)
                .await
                .unwrap_err()
                .code,
            "E_PROTECTED"
        );
    }
    must(protect(&context.db, &t, &s, &format!("{BASE}/files/new.txt")).await);
    let path = format!("{BASE}/files/new.txt");
    let file = s
        .open_with_flags(
            &path,
            russh_sftp::protocol::OpenFlags::WRITE
                | russh_sftp::protocol::OpenFlags::CREATE
                | russh_sftp::protocol::OpenFlags::EXCLUDE,
        )
        .await
        .expect("Create failed");
    file.close().await.unwrap();
    s.rename(&path, format!("{BASE}/files/renamed.txt"))
        .await
        .unwrap();
    s.remove_file(format!("{BASE}/files/renamed.txt"))
        .await
        .unwrap();
    s.create_dir(format!("{BASE}/files/new-dir")).await.unwrap();
    s.remove_dir(format!("{BASE}/files/new-dir")).await.unwrap();
    println!("PASS: file_create_rename_delete_protection");
    for (name, ending) in [
        ("lf.txt", "lf"),
        ("crlf.txt", "crlf"),
        ("mixed.txt", "mixed"),
    ] {
        let path = format!("{BASE}/files/{name}");
        let original = must(sftp::read(&s, &path).await);
        assert_eq!(original.line_ending, ending);
        let saved = must(
            save(
                &context.manager,
                &t,
                &s,
                &path,
                &original.content,
                original.mtime,
                original.size,
                false,
            )
            .await,
        );
        assert_eq!(saved.content, original.content);
        assert_eq!(saved.permissions, original.permissions);
    }
    assert_eq!(
        must(sftp::read(&s, &format!("{BASE}/files/limit.txt")).await).size,
        sftp::EDIT_LIMIT
    );
    assert_eq!(
        sftp::read(&s, &format!("{BASE}/files/over-limit.txt"))
            .await
            .unwrap_err()
            .code,
        "E_LIMIT"
    );
    assert_eq!(
        sftp::read(&s, &format!("{BASE}/files/binary.bin"))
            .await
            .unwrap_err()
            .code,
        "E_ENCODING"
    );
    let readonly = format!("{BASE}/files/readonly.txt");
    let original = must(sftp::read(&s, &readonly).await);
    assert!(save(
        &context.manager,
        &t,
        &s,
        &readonly,
        "changed",
        original.mtime,
        original.size,
        false
    )
    .await
    .is_err());
    let conflict = format!("{BASE}/files/conflict.txt");
    let original = must(sftp::read(&s, &conflict).await);
    must(
        context
            .manager
            .exec(
                &t,
                &format!("printf changed-outside >> {}", sftp::quote(&conflict)),
            )
            .await,
    );
    assert_eq!(
        save(
            &context.manager,
            &t,
            &s,
            &conflict,
            "conflict",
            original.mtime,
            original.size,
            false
        )
        .await
        .unwrap_err()
        .code,
        "E_CONFLICT"
    );
    println!("PASS: editor_line_endings_limit_utf8_readonly_conflict");
    for (path, alias) in [
        (
            format!("{BASE}/files/hardlink.txt"),
            Some(format!("{BASE}/files/hardlink-alias.txt")),
        ),
        (format!("{BASE}/nonwritable/file.txt"), None),
    ] {
        let before = must(
            context
                .manager
                .exec(
                    &t,
                    &format!("stat -Lc '%i %h %u %g %a' -- {}", sftp::quote(&path)),
                )
                .await,
        );
        let original = must(sftp::read(&s, &path).await);
        let saved = must(
            save(
                &context.manager,
                &t,
                &s,
                &path,
                "verified in place\n",
                original.mtime,
                original.size,
                false,
            )
            .await,
        );
        assert_eq!(saved.save_mode.as_deref(), Some("inPlace"));
        assert_eq!(
            before,
            must(
                context
                    .manager
                    .exec(
                        &t,
                        &format!("stat -Lc '%i %h %u %g %a' -- {}", sftp::quote(&path))
                    )
                    .await
            )
        );
        if let Some(alias) = alias {
            assert_eq!(must(sftp::read(&s, &alias).await).content, saved.content);
        }
    }
    println!("PASS: save_preserves_hardlinks_inode_metadata_and_nonwritable_parent");
    let inherited = format!("{BASE}/default-acl/file.txt");
    let original = must(sftp::read(&s, &inherited).await);
    let before = must(
        context
            .manager
            .exec(&t, &format!("getfacl -cp -- {}", sftp::quote(&inherited)))
            .await,
    );
    let saved = must(
        save(
            &context.manager,
            &t,
            &s,
            &inherited,
            "private content\n",
            original.mtime,
            original.size,
            false,
        )
        .await,
    );
    assert_eq!(saved.save_mode.as_deref(), Some("inPlace"));
    assert_eq!(
        before,
        must(
            context
                .manager
                .exec(&t, &format!("getfacl -cp -- {}", sftp::quote(&inherited)))
                .await
        )
    );
    let special = format!("{BASE}/files/special-mode.txt");
    let original = must(sftp::read(&s, &special).await);
    assert_eq!(
        save(
            &context.manager,
            &t,
            &s,
            &special,
            "changed",
            original.mtime,
            original.size,
            false
        )
        .await
        .unwrap_err()
        .code,
        "E_FILE_MODE"
    );
    println!("PASS: save_default_acl_does_not_expand_access_and_special_mode_rejected");
    s.close().await.unwrap();
    // Every terminal action is sent through the same product manager and IPC frames.
    let pty = Arc::new(PtyManager::default());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TerminalFrame>();
    let output = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(body) = body {
            if let Ok(frame) = serde_json::from_str::<serde_json::Value>(&body) {
                // TerminalFrame intentionally has Serialize only in the product.
                tx.send(TerminalFrame {
                    target: serde_json::from_value(frame["target"].clone()).unwrap(),
                    terminal_id: frame["terminalId"].as_str().unwrap().into(),
                    data: serde_json::from_value(frame["data"].clone()).unwrap(),
                    seq: frame["seq"].as_u64().unwrap(),
                    exited: frame["exited"].as_bool().unwrap(),
                    exit_code: frame["exitCode"].as_u64().map(|n| n as u32),
                })
                .unwrap();
            }
        }
        Ok(())
    });
    let directory = format!("{BASE}/空 格'中文");
    let id = must(
        pty.open(
            &context.manager,
            t.clone(),
            &directory,
            "shell",
            80,
            24,
            output,
        )
        .await,
    );
    must(pty.resize(&t, &id, 123, 37).await);
    must(pty.write(&t, &id, "printf '\\nPWD=%s\\n' \"$PWD\"; agentdesk-e2e-path; printf '\\n中文回显\\n'; stty size; exit 23\n".into()).await);
    let mut bytes = Vec::new();
    let exit = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let frame = rx.recv().await.expect("PTY output ended early");
            assert_eq!(frame.target, t);
            bytes.extend_from_slice(&frame.data);
            must(pty.ack(&t, &id, frame.seq));
            if frame.exited {
                break frame.exit_code;
            }
        }
    })
    .await
    .expect("PTY timed out");
    assert_eq!(exit, Some(23));
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains("shell=${SHELL"),
        "Terminal bootstrap was echoed"
    );
    for expected in [directory.as_str(), "path-ready", "中文回显", "37 123"] {
        assert!(text.contains(expected), "Missing PTY observation");
    }
    println!("PASS: pty_quoted_unicode_directory_bashrc_resize_echo_exit");
    must(context.manager.disconnect(&t).await);
}

#[tokio::test]
#[ignore = "requires scoped 210 MiB download fixtures"]
async fn real_downloads_and_cancellation() {
    let Some(env) = Environment::load() else {
        return;
    };
    let context = Context::new();
    let c = env.ordinary();
    let t = context.connect(&c, env.credential(&c)).await;
    let transfers = Arc::new(TransferManager::default());
    let destination = tempfile::tempdir().unwrap();
    for path in [
        format!("{BASE}/files/lf.txt"),
        format!("{BASE}/files/lf.txt"),
        format!("{BASE}/大目录 CON"),
    ] {
        let id = must(transfers.download(
            None,
            context.manager.clone(),
            t.clone(),
            path,
            destination.path().to_string_lossy().into_owned(),
        ));
        let result = tokio::time::timeout(Duration::from_secs(600), async {
            loop {
                let transfer = must(transfers.list())
                    .into_iter()
                    .find(|j| j.id == id)
                    .unwrap();
                if !matches!(transfer.state.as_str(), "queued" | "running") {
                    break transfer;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("Download test exceeded its bound");
        if result.state != "completed" {
            panic!(
                "Product download failed: state={}, code={}, directory={}",
                result.state,
                result
                    .error
                    .as_deref()
                    .unwrap_or("unknown")
                    .split(':')
                    .next()
                    .unwrap_or("unknown"),
                result.path.ends_with("大目录 CON")
            );
        }
        if result.path.ends_with("大目录 CON") {
            assert!(result.transferred >= 200 * 1024 * 1024);
            let listing = std::process::Command::new("tar")
                .args(["-tzf", result.local_path.as_deref().unwrap()])
                .output()
                .unwrap();
            assert!(listing.status.success());
            assert!(String::from_utf8_lossy(&listing.stdout).contains("CON"));
        }
    }
    assert!(destination.path().join("lf (1).txt").is_file());
    println!("PASS: download_file_collision_and_210mib_streamed_tar");
    let id = must(transfers.download(
        None,
        context.manager.clone(),
        t.clone(),
        format!("{BASE}/大目录 CON"),
        destination.path().to_string_lossy().into_owned(),
    ));
    tokio::time::sleep(Duration::from_millis(500)).await;
    must(transfers.cancel(&t, &id));
    must(transfers.shutdown().await);
    assert_eq!(
        must(transfers.list())
            .into_iter()
            .find(|j| j.id == id)
            .unwrap()
            .state,
        "cancelled"
    );
    assert!(!std::fs::read_dir(destination.path())
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .path()
            .extension()
            .is_some_and(|s| s == "part")));
    let remaining = must(
        context
            .manager
            .exec(
                &t,
                &format!(
                    "ps -u agentdesk-test -o args= | grep '[t]ar -czf -' | grep -F {} || true",
                    sftp::quote(BASE)
                ),
            )
            .await,
    );
    assert!(
        remaining.is_empty(),
        "Archive process survived cancellation"
    );
    let leftovers = must(context.manager.exec(&t, "find /tmp -maxdepth 1 -user agentdesk-test -name 'agentdesk-download.*' -print 2>/dev/null || true").await);
    assert!(
        leftovers.is_empty(),
        "Remote archive temporary files remain"
    );
    println!("PASS: download_cancel_remote_process_and_local_part_cleanup");
    must(context.manager.disconnect(&t).await);
}

struct ForwardClient {
    key: PublicKey,
    local_port: u16,
}
impl client::Handler for ForwardClient {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(key.public_key() == self.key)
    }
    async fn server_channel_open_forwarded_streamlocal(
        &mut self,
        channel: Channel<client::Msg>,
        _path: &str,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        let port = self.local_port;
        tokio::spawn(async move {
            if let Ok(mut tcp) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
                let mut stream = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await;
            }
        });
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires test server Unix socket fixtures"]
async fn real_streamlocal_and_two_connection_isolation() {
    let Some(env) = Environment::load() else {
        return;
    };
    let context = Context::new();
    let ordinary = env.ordinary();
    let a = context.connect(&ordinary, env.credential(&ordinary)).await;
    let root = env.connection(true, None);
    let b = context
        .connect(&root, Some(env.root_password.clone()))
        .await;
    for _ in 0..8 {
        let (ordinary, root) = tokio::join!(
            context.manager.exec(&a, "id -un"),
            context.manager.exec(&b, "id -un")
        );
        assert_eq!(must(ordinary), b"agentdesk-test\n");
        assert_eq!(
            must(root),
            env.root_user
                .as_bytes()
                .iter()
                .copied()
                .chain([b'\n'])
                .collect::<Vec<_>>()
        );
    }
    println!("PASS: simultaneous_root_user_connection_isolation");
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let local_port = listener.local_addr().unwrap().port();
    let echo = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut read, mut write) = stream.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
            });
        }
    });
    let key = PublicKey::from_openssh(&env.secret("server_key.pub")).unwrap();
    let mut handle = client::connect(
        Arc::new(client::Config::default()),
        (env.host.as_str(), env.port),
        ForwardClient { key, local_port },
    )
    .await
    .expect("Pinned SSH handshake failed");
    must(auth::authenticate(&mut handle, &ordinary, env.credential(&ordinary).as_deref()).await);
    let socket = format!("{BASE}/s1/echo.sock");
    let script = format!(
        r#"set -eu
mkdir -p {base}/s1
chmod 700 {base}/s1
cat > {base}/s1/echo.py <<'PY'
from __future__ import print_function
import os, socket, signal, sys
base = sys.argv[1]
path = base + '/echo.sock'
s = socket.socket(socket.AF_UNIX)
s.bind(path)
os.chmod(path, 0o600)
def close(signum, frame):
    s.close()
    os.unlink(path)
    sys.exit(0)
signal.signal(signal.SIGTERM, close)
with open(base + '/echo.pid', 'w') as f: f.write(str(os.getpid()))
s.listen(4)
while True:
    c, address = s.accept()
    data = c.recv(256)
    c.sendall(data)
    c.close()
PY
setsid nohup python {base}/s1/echo.py {base}/s1 </dev/null >{base}/s1/stdout 2>{base}/s1/stderr &
"#,
        base = BASE
    );
    must(context.manager.exec(&a, &script).await);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let channel = must(forward::direct(&handle, &socket).await);
    let mut stream = channel.into_stream();
    stream.write_all(b"s1-forward").await.unwrap();
    let mut response = [0; 10];
    tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&response, b"s1-forward");
    drop(stream);
    assert_eq!(
        must(
            context
                .manager
                .exec(&a, &format!("stat -Lc %a -- {}", sftp::quote(&socket)))
                .await
        ),
        b"600\n"
    );
    println!("PASS: russh_direct_streamlocal_socket_0600");
    let reverse = format!("{BASE}/s1/reverse.sock");
    must(forward::reverse(&handle, &reverse).await);
    assert_eq!(
        must(
            context
                .manager
                .exec(&a, &format!("stat -Lc %a -- {}", sftp::quote(&reverse)))
                .await
        ),
        b"600\n"
    );
    let echo_command = format!("python -c {}", sftp::quote(&format!("import socket; s=socket.socket(socket.AF_UNIX); s.connect({reverse:?}); s.sendall(b's1-reverse'); assert s.recv(64)==b's1-reverse'; s.close()")));
    must(context.manager.exec(&a, &echo_command).await);
    must(forward::cancel_reverse(&handle, &reverse).await);
    let exists = must(
        context
            .manager
            .exec(
                &a,
                &format!(
                    "if test -S {}; then printf present; else printf absent; fi",
                    sftp::quote(&reverse)
                ),
            )
            .await,
    );
    // OpenSSH 7.4 leaves the socket inode after cancelling the listener.
    // Record the actual server behavior, and remove only this test-owned inode.
    if exists == b"present" {
        println!("OBSERVE: OpenSSH leaves reverse socket inode after cancel; listener closed");
        must(context.manager.exec(&a, &format!("python -c {}", sftp::quote(&format!("import socket; s=socket.socket(socket.AF_UNIX); s.settimeout(2)\ntry:\n s.connect({reverse:?})\nexcept socket.error:\n pass\nelse:\n raise SystemExit(1)\ns.close()")))).await);
        must(
            context
                .manager
                .exec(&a, &format!("rm -- {}", sftp::quote(&reverse)))
                .await,
        );
    }
    println!("PASS: russh_reverse_streamlocal_to_local_tcp_and_cancel_cleanup");
    must(forward::reverse(&handle, &reverse).await);
    handle
        .disconnect(russh::Disconnect::ByApplication, "E2E complete", "en")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    must(
        context
            .manager
            .exec(
                &a,
                &format!(
                    "rm -f -- {}\nkill -TERM \"$(cat {BASE}/s1/echo.pid)\"",
                    sftp::quote(&reverse)
                ),
            )
            .await,
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    must(
        context
            .manager
            .exec(
                &a,
                &format!(
                    "test ! -S {} && test ! -S {}",
                    sftp::quote(&socket),
                    sftp::quote(&reverse)
                ),
            )
            .await,
    );
    println!("PASS: streamlocal_disconnect_test_owned_socket_cleanup");
    echo.abort();
    must(context.manager.disconnect(&a).await);
    must(context.manager.disconnect(&b).await);
}

struct BlackholeProxy {
    port: u16,
    blocked: Arc<std::sync::atomic::AtomicBool>,
    cancel: tokio_util::sync::CancellationToken,
    connections: Arc<std::sync::Mutex<Vec<tokio_util::sync::CancellationToken>>>,
    accept: tokio::task::JoinHandle<()>,
}
impl Drop for BlackholeProxy {
    fn drop(&mut self) {
        self.blocked
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.cancel.cancel();
        self.accept.abort();
    }
}
impl BlackholeProxy {
    fn cut_current(&self) {
        for connection in self.connections.lock().unwrap().drain(..) {
            connection.cancel();
        }
    }
    async fn start(host: String, port: u16) -> Self {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let proxy_port = listener.local_addr().unwrap().port();
        let blocked = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel = tokio_util::sync::CancellationToken::new();
        let b = blocked.clone();
        let c = cancel.clone();
        let connections = Arc::new(std::sync::Mutex::new(Vec::new()));
        let active = connections.clone();
        let accept = tokio::spawn(async move {
            loop {
                let incoming = tokio::select! { _ = c.cancelled() => break, incoming = listener.accept() => incoming };
                let Ok((local, _)) = incoming else {
                    break;
                };
                let Ok(remote) = tokio::net::TcpStream::connect((host.as_str(), port)).await else {
                    continue;
                };
                let (local_read, local_write) = local.into_split();
                let (remote_read, remote_write) = remote.into_split();
                let b = b.clone();
                let c = c.child_token();
                active.lock().unwrap().push(c.clone());
                tokio::spawn(async move {
                    tokio::join!(
                        drop_or_forward(local_read, remote_write, b.clone(), c.clone()),
                        drop_or_forward(remote_read, local_write, b, c)
                    );
                });
            }
        });
        Self {
            port: proxy_port,
            blocked,
            cancel,
            connections,
            accept,
        }
    }
}
async fn drop_or_forward(
    mut read: tokio::net::tcp::OwnedReadHalf,
    mut write: tokio::net::tcp::OwnedWriteHalf,
    blocked: Arc<std::sync::atomic::AtomicBool>,
    cancel: tokio_util::sync::CancellationToken,
) {
    let mut bytes = vec![0; 65536];
    loop {
        let amount = tokio::select! { _ = cancel.cancelled() => break, result = read.read(&mut bytes) => match result { Ok(n) if n > 0 => n, _ => break } };
        if !blocked.load(std::sync::atomic::Ordering::SeqCst) {
            let written = tokio::select! { _ = cancel.cancelled() => break, result = write.write_all(&bytes[..amount]) => result };
            if written.is_err() {
                break;
            }
        }
    }
    let _ = write.shutdown().await;
}

#[tokio::test]
#[ignore = "approximately two minutes, uses a local scoped blackhole proxy"]
async fn real_blackhole_detection_reconnect_stop_and_no_replay() {
    let Some(env) = Environment::load() else {
        return;
    };
    let proxy = BlackholeProxy::start(env.host.clone(), env.port).await;
    let context = Context::new();
    let mut c = env.ordinary();
    c.host = "127.0.0.1".into();
    c.port = proxy.port;
    let t = context.connect(&c, env.credential(&c)).await;
    let marker = format!("{BASE}/network-no-replay");
    must(
        context
            .manager
            .exec(&t, &format!(": > {}", sftp::quote(&marker)))
            .await,
    );
    must(
        context
            .manager
            .exec(&t, &format!("printf once >> {}", sftp::quote(&marker)))
            .await,
    );
    let pty = Arc::new(PtyManager::default());
    let ack = pty.clone();
    let output = tauri::ipc::Channel::<TerminalFrame>::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(text) = body {
            let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
            let target = serde_json::from_value(frame["target"].clone()).unwrap();
            let _ = ack.ack(
                &target,
                frame["terminalId"].as_str().unwrap(),
                frame["seq"].as_u64().unwrap(),
            );
        }
        Ok(())
    });
    let terminal = must(
        pty.open(&context.manager, t.clone(), BASE, "shell", 80, 24, output)
            .await,
    );
    proxy
        .blocked
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let started = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(65), async {
        loop {
            if must(context.manager.statuses())
                .iter()
                .any(|s| s.state == "reconnecting")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("Offline detection exceeded 65 seconds");
    let elapsed = started.elapsed().as_secs();
    assert!(
        (30..=60).contains(&elapsed),
        "Offline detection outside expected keepalive range"
    );
    println!("PASS: blackhole_offline_detection_seconds={elapsed}");
    // Force one failed reconnect handshake, then release only this local proxy.
    tokio::time::sleep(Duration::from_secs(18)).await;
    assert!(must(context.manager.statuses())
        .iter()
        .any(|s| s.state == "reconnecting"));
    proxy
        .blocked
        .store(false, std::sync::atomic::Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            if must(context.manager.statuses())
                .iter()
                .any(|s| s.state == "connected")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("Reconnect did not recover");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(pty
        .write(&t, &terminal, "printf replay\n".into())
        .await
        .is_err());
    assert_eq!(
        must(
            context
                .manager
                .exec(&t, &format!("cat -- {}", sftp::quote(&marker)))
                .await
        ),
        b"once"
    );
    println!("PASS: reconnect_backoff_old_pty_closed_write_not_replayed");
    // A second loss verifies stop while a reconnect is pending.
    proxy
        .blocked
        .store(true, std::sync::atomic::Ordering::SeqCst);
    proxy.cut_current();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if must(context.manager.statuses())
                .iter()
                .any(|s| s.state == "reconnecting")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("Second loss did not enter reconnecting state");
    must(context.manager.disconnect(&t).await);
    proxy
        .blocked
        .store(false, std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!context.manager.is_active(&t.connection_id));
    assert!(must(context.manager.statuses())
        .iter()
        .any(|s| s.state == "disconnected"));
    println!("PASS: stop_reconnect_local_proxy_restored_no_firewall_changes");
}
