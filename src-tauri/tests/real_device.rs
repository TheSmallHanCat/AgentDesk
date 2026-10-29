//! Opt-in M3 acceptance through the product SSH, Host and bundled Windows endpoint.
#![cfg(windows)]

use agentdesk_lib::{
    db::Db,
    error::{AppError, AppResult},
    host::{
        client::{remote_home, HostClient},
        installer::{start_script, Installer, STOP_SCRIPT},
    },
    local::executor::{ExecutorResources, LocalExecutor},
    models::{Connection, Project, Target},
    session_log::Diagnostics,
    ssh::{known_hosts::KnownHosts, manager::SshManager, sftp::quote},
};
use base64::Engine;
use russh::ChannelMsg;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

fn must<T>(result: AppResult<T>) -> T {
    result.unwrap_or_else(|error| panic!("M3 acceptance failed: {} at {}", error.code, error.stage))
}

async fn connect_host(
    ssh: &SshManager,
    target: &Target,
    device_id: &str,
) -> AppResult<Arc<HostClient>> {
    if let Ok(home) = std::env::var("AGENTDESK_E2E_DEVICE_HOME") {
        assert!(home.starts_with("/home/agentdesk-test/agentdesk-e2e/") && !home.contains(".."));
        HostClient::connect_at(ssh, target, device_id, &format!("{home}/run/host.sock")).await
    } else {
        HostClient::connect(ssh, target, device_id).await
    }
}

async fn event(
    receiver: &mut tokio::sync::broadcast::Receiver<Value>,
    session: &str,
    kind: &str,
    seconds: u64,
) -> Value {
    tokio::time::timeout(Duration::from_secs(seconds), async {
        loop {
            match receiver.recv().await {
                Ok(value) if value["sessionId"] == session && value["type"] == kind => {
                    return value
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => (),
                Err(_) => panic!("M3 Host stream ended before expected event"),
            }
        }
    })
    .await
    .expect("M3 expected event timed out")
}

async fn execute(
    ssh: &SshManager,
    target: &Target,
    script: &str,
    seconds: u64,
) -> AppResult<Vec<u8>> {
    let mut channel = ssh.open_channel(target).await?;
    channel
        .exec(true, "exec /bin/sh -s")
        .await
        .map_err(|error| AppError::io("e2e.exec", error))?;
    channel
        .data_bytes(format!("{script}\n").into_bytes())
        .await
        .map_err(|error| AppError::io("e2e.exec", error))?;
    channel
        .eof()
        .await
        .map_err(|error| AppError::io("e2e.exec", error))?;
    let result = tokio::time::timeout(Duration::from_secs(seconds), async {
        let mut bytes = Vec::new();
        let mut status = None;
        while let Some(message) = channel.wait().await {
            match message {
                ChannelMsg::Data { data } => {
                    if bytes.len() + data.len() > 1024 * 1024 {
                        return Err(AppError::new(
                            "E_LIMIT",
                            "e2e.exec",
                            "Acceptance output limit exceeded",
                        ));
                    }
                    bytes.extend_from_slice(&data);
                }
                ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                ChannelMsg::Close => break,
                _ => (),
            }
        }
        if status == Some(0) {
            Ok(bytes)
        } else {
            Err(AppError::new(
                "E_EXEC",
                "e2e.exec",
                "Acceptance command failed",
            ))
        }
    })
    .await
    .map_err(|_| AppError::new("E_TIMEOUT", "e2e.exec", "Acceptance command timed out"))?;
    let _ = channel.close().await;
    result
}

async fn python(
    ssh: &SshManager,
    target: &Target,
    parameters: Value,
    source: &str,
    seconds: u64,
) -> Value {
    let encoded =
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&parameters).unwrap());
    let command = format!("{}\"$HOME/.agentdesk/host/current/venv/bin/python\" - <<'AGENTDESK_E2E_PY'\nimport base64,json,os,pathlib,subprocess,time\np=json.loads(base64.b64decode('{encoded}'))\n{source}\nAGENTDESK_E2E_PY\n", test_client_path());
    serde_json::from_slice(&must(execute(ssh, target, &command, seconds).await))
        .expect("M3 acceptance returned invalid metadata")
}

fn test_client_path() -> &'static str {
    if std::env::var("AGENTDESK_E2E_SSH_CLIENT").as_deref() == Ok("1") {
        "export PATH=\"$HOME/.agentdesk/acceptance/ssh-client/bin:$PATH\"\n"
    } else {
        ""
    }
}

async fn tunnel(
    ssh: &SshManager,
    target: &Target,
    client: &HostClient,
    resources: &ExecutorResources,
) -> (LocalExecutor, String) {
    let registration = must(client.request("device.register", json!({})).await);
    must(
        client
            .request(
                "device.project.enable",
                json!({"projectId":target.project_id}),
            )
            .await,
    );
    let socket = registration["tunnelSocket"]
        .as_str()
        .expect("Device socket is required")
        .to_owned();
    let mut executor = must(LocalExecutor::start(resources.clone()).await);
    let result = async {
        ssh.reverse_local(target, &socket, executor.port(), CancellationToken::new())
            .await?;
        let mut parameters = executor.take_credentials();
        parameters["generation"] = registration["generation"].clone();
        let ready = client.request("device.tunnel.ready", parameters).await?;
        if ready["state"] != "ready" {
            return Err(AppError::new(
                "E_DEVICE_PROBE",
                "e2e.tunnel",
                "Host did not confirm device probe",
            ));
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        let _ = ssh.cancel_reverse_local(target, &socket).await;
        let _ = client.request("device.offline", json!({})).await;
        executor.stop().await;
        must::<()>(Err(error));
    }
    (executor, socket)
}

async fn reconnect_while_old_control_is_blackholed(
    db: Arc<Db>,
    directory: &Path,
    private: &Path,
    client: &HostClient,
    device_id: &str,
) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    async fn forward<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
        mut read: R,
        mut write: W,
        blackhole: Arc<AtomicBool>,
        stop: CancellationToken,
    ) {
        let mut bytes = vec![0; 32768];
        loop {
            let result = tokio::select! { _ = stop.cancelled() => break, result = read.read(&mut bytes) => result };
            let Ok(count) = result else { break };
            if count == 0 {
                break;
            }
            if !blackhole.load(Ordering::SeqCst) && write.write_all(&bytes[..count]).await.is_err()
            {
                break;
            }
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let blackhole = Arc::new(AtomicBool::new(false));
    let stop = CancellationToken::new();
    let gate = blackhole.clone();
    let cancel = stop.clone();
    let remote_host = std::env::var("AGENTDESK_E2E_HOST").unwrap();
    let remote_port = std::env::var("AGENTDESK_E2E_PORT")
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let proxy = tokio::spawn(async move {
        let (incoming, _) = listener.accept().await.unwrap();
        let remote = tokio::net::TcpStream::connect((remote_host.as_str(), remote_port))
            .await
            .unwrap();
        let (ir, iw) = incoming.into_split();
        let (rr, rw) = remote.into_split();
        tokio::join!(
            forward(ir, rw, gate.clone(), cancel.clone()),
            forward(rr, iw, gate, cancel)
        );
    });
    let old = Connection {
        id: "m3-blackhole".into(),
        name: "Review".into(),
        host: "127.0.0.1".into(),
        port,
        user: "agentdesk-test".into(),
        auth_method: "key".into(),
        key_path: Some(private.join("id_ed25519").to_string_lossy().into()),
        agent_kind: None,
        proxy_jump: None,
        source: None,
        last_used_at: None,
    };
    must(db.save_connection(&old));
    let pinned = russh::keys::PublicKey::from_openssh(
        &std::fs::read_to_string(private.join("server_key.pub")).unwrap(),
    )
    .unwrap();
    let known = Arc::new(KnownHosts::new(
        directory.join("blackhole-known-hosts"),
        None,
    ));
    must(known.trust("127.0.0.1", port, &pinned));
    let old_ssh = Arc::new(SshManager::new(
        db,
        known,
        Arc::new(Diagnostics::new(directory)),
        None,
    ));
    let target = Target {
        connection_id: old.id,
        device_local: false,
        project_id: None,
        session_id: None,
        request_seq: 1,
    };
    assert_eq!(
        must(old_ssh.connect(&target, None, None).await).state,
        "connected"
    );
    let old_client = must(connect_host(&old_ssh, &target, device_id).await);
    let previous = must(old_client.request("device.register", json!({})).await);
    blackhole.store(true, Ordering::SeqCst);
    let started = Instant::now();
    let next = must(client.request("device.register", json!({})).await);
    assert_ne!(previous["generation"], next["generation"]);
    assert!(started.elapsed() < Duration::from_secs(10));
    old_client.cancel.cancel();
    stop.cancel();
    proxy.await.unwrap();
    must(old_ssh.disconnect(&target).await);
    println!("PASS: m3_blackholed_control_immediate_reconnect_registration_takeover");
}

const PROBE: &str = r#"
command=['ssh','-F',p['config'],'device-script']
if p.get('sandbox'):
    command=[str(pathlib.Path.home()/'.agentdesk/host/current/bin/codex'),'-c','sandbox_mode="workspace-write"','-c','sandbox_workspace_write.network_access=true','-c','sandbox_workspace_write.writable_roots='+json.dumps([p['deviceRoot']]),'sandbox','--']+command
if p.get('readOnly'):
    command=[str(pathlib.Path.home()/'.agentdesk/host/current/bin/codex'),'-c','sandbox_mode="read-only"','sandbox','--','ssh','-F',p['config'],'device-script']
started=time.monotonic()
result=subprocess.run(command,input=p['script'].encode('utf-8'),stdout=subprocess.PIPE,stderr=subprocess.PIPE,cwd=p['cwd'],timeout=80)
error=result.stderr.decode('utf-8',errors='replace')
category='device-offline' if 'E_DEVICE_OFFLINE' in error else next((name for name in ['Operation not permitted','Permission denied','user namespace','bwrap','Landlock','No such file'] if name.lower() in error.lower()),'other')
print(json.dumps({'code':result.returncode,'utf8':'中文执行成功' in result.stdout.decode('utf-8',errors='replace'),'elapsed':time.monotonic()-started,'errorCategory':category}))
"#;

#[tokio::test]
#[ignore = "Requires dedicated E2E server, prepared Host and Windows resources; exercises real 60s wait and 300s grace"]
async fn real_device_reverse_native_reconnect_sandbox_wait_grace_cleanup() {
    for name in ["AGENTDESK_E2E_HOST", "AGENTDESK_E2E_PORT"] {
        if std::env::var(name)
            .ok()
            .is_none_or(|value| value.is_empty())
        {
            println!("SKIP: missing {name}");
            return;
        }
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let private = root.join(".tmp/e2e");
    assert!(
        private.join("id_ed25519").is_file(),
        "Provision the dedicated E2E key before running acceptance"
    );
    let directory = tempfile::tempdir().unwrap();
    let db = Arc::new(must(Db::open(Path::new(":memory:"))));
    let ssh = Arc::new(SshManager::new(
        db.clone(),
        Arc::new(KnownHosts::new(directory.path().join("known_hosts"), None)),
        Arc::new(Diagnostics::new(directory.path())),
        None,
    ));
    let connection = Connection {
        id: "m3-e2e".into(),
        name: "M3".into(),
        host: std::env::var("AGENTDESK_E2E_HOST").unwrap(),
        port: std::env::var("AGENTDESK_E2E_PORT")
            .unwrap()
            .parse()
            .unwrap(),
        user: "agentdesk-test".into(),
        auth_method: "key".into(),
        key_path: Some(private.join("id_ed25519").to_string_lossy().into_owned()),
        agent_kind: None,
        proxy_jump: None,
        source: None,
        last_used_at: None,
    };
    must(db.save_connection(&connection));
    let mut target = Target {
        device_local: false,
        connection_id: connection.id,
        project_id: None,
        session_id: None,
        request_seq: 1,
    };
    let status = must(ssh.connect(&target, None, None).await);
    if status.state == "awaitingTrust" {
        must(ssh.trust(&target, status.fingerprint.as_deref().unwrap()));
        assert_eq!(
            must(ssh.connect(&target, None, None).await).state,
            "connected"
        );
    }
    let home = std::env::var("AGENTDESK_E2E_DEVICE_HOME")
        .unwrap_or(must(remote_home(&ssh, &target).await));
    let device_id = "m3-e2e-device";
    let reuse = std::env::var("AGENTDESK_E2E_REUSE_HOST").as_deref() == Ok("1");
    if !reuse {
        if let Ok(existing) = connect_host(&ssh, &target, device_id).await {
            let environment = must(existing.request("env.probe", json!({})).await);
            assert_eq!(
                environment["host"]["activeRuns"].as_u64().unwrap_or(0),
                0,
                "Dedicated Host has an active run; avoid deployment during another acceptance"
            );
            existing.cancel.cancel();
            must(ssh.exec(&target, STOP_SCRIPT).await);
        }
        let installer = Installer::default();
        let plan = must(installer.plan(&target, &home, &root.join("resources/host"), true));
        let plan = must(installer.take(&target, plan["planId"].as_str().unwrap()));
        must(
            installer
                .execute(&ssh, &target, plan, tauri::ipc::Channel::new(|_| Ok(())))
                .await,
        );
        must(
            ssh.exec(
                &target,
                &format!("{}{}", test_client_path(), start_script(true)),
            )
            .await,
        );
        println!("PASS: m3_production_host_deployed");
    }
    let client = must(connect_host(&ssh, &target, device_id).await);
    reconnect_while_old_control_is_blackholed(
        db.clone(),
        directory.path(),
        &private,
        &client,
        device_id,
    )
    .await;
    let local_path = root.join(".tmp/e2e/local-project/reverse-channel");
    std::fs::create_dir_all(&local_path).unwrap();
    let project = Project {
        runtime_mode: "direct".into(),
        device_local: true,
        id: format!("m3_{}", uuid::Uuid::new_v4().simple()),
        connection_id: target.connection_id.clone(),
        name: "M3".into(),
        path: local_path.to_string_lossy().into_owned(),
        description: String::new(),
        default_agent: "fake".into(),
        default_model: "fake-default".into(),
        permission_mode: "default".into(),
        pinned: false,
        archived: false,
        last_opened_at: None,
    };
    must(db.save_project(&project));
    target.project_id = Some(project.id.clone());
    target.device_local = true;
    let registered = must(
        client
            .request(
                "project.upsert",
                json!({"id":project.id,"path":project.path,"name":project.name,"kind":"local","deviceId":device_id}),
            )
            .await,
    );
    let native_root = registered["nativeRoot"]
        .as_str()
        .expect("Native root required")
        .to_owned();
    let session = must(client.request("session.create", json!({"projectId":project.id,"agent":"fake","model":"fake-default","permissionMode":"default"})).await);
    for agent in ["claude", "codex"] {
        let error = client.request("session.create", json!({"projectId":project.id,"agent":agent,"model":"unused","permissionMode":"plan"})).await.unwrap_err();
        assert_eq!(error.code, "E_LOCAL_PLAN_UNSUPPORTED");
    }
    println!("PASS: m3_local_plan_explicitly_rejected_before_native_execution");
    must(db.register_session(&target, &session));
    let sid = session["id"].as_str().unwrap().to_owned();
    target.session_id = Some(sid.clone());
    let resources = ExecutorResources::discover(&root);
    let (mut executor, socket) = tunnel(&ssh, &target, &client, &resources).await;
    let device_root = socket.rsplit_once('/').unwrap().0.to_owned();
    println!("PASS: m3_streamlocal_forward_host_device_script_probe");
    let mut events = client.events.subscribe();
    must(
        client
            .request("sync.subscribe", json!({"sessionId":sid}))
            .await,
    );
    let file = local_path
        .join("result.txt")
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "''");
    let script = format!("$lines = @(\n  '第一行',\n  '第二行'\n)\nif ($lines.Count -eq 2) {{\n  Set-Content -LiteralPath '{file}' -Value ($lines -join '|') -Encoding utf8NoBOM\n}}\nAdd-Content -LiteralPath '{file}' -Value '已修改' -Encoding utf8NoBOM\nWrite-Output '中文执行成功'\nexit 7\n");
    must(client.request("run.send", json!({"sessionId":sid,"clientMsgId":uuid::Uuid::new_v4().to_string(),"text":"Execute the local project operation","script":[{"type":"device","script":script}]})).await);
    let result = loop {
        let next = event(&mut events, &sid, "item.completed", 30).await;
        if next["payload"]["kind"] == "tool_result" {
            break next;
        }
    };
    assert_eq!(result["payload"]["exitCode"], 7);
    assert!(result["payload"]["output"]
        .as_str()
        .unwrap()
        .contains("中文执行成功"));
    assert_eq!(
        event(&mut events, &sid, "run.finished", 30).await["payload"]["state"],
        "completed"
    );
    assert!(std::fs::read_to_string(local_path.join("result.txt"))
        .unwrap()
        .contains("已修改"));
    println!("PASS: m3_fake_graphical_protocol_local_multiline_utf8_exit7");
    let session_config = format!(
        "{}/session-ssh/{sid}.conf",
        if std::env::var("AGENTDESK_E2E_DEVICE_HOME").is_ok() {
            home.clone()
        } else {
            format!("{home}/.agentdesk")
        }
    );
    let initial_config = must(
        ssh.exec(&target, &format!("cat {}", quote(&session_config)))
            .await,
    );
    assert_eq!(
        initial_config.iter().filter(|byte| **byte == b'\n').count(),
        1
    );
    let mut probe =
        json!({"config":session_config,"script":script,"cwd":native_root,"deviceRoot":device_root});
    let manual = python(&ssh, &target, probe.clone(), PROBE, 90).await;
    assert_eq!(manual["code"], 7);
    assert_eq!(manual["utf8"], true);
    probe["sandbox"] = true.into();
    probe["script"] = "Write-Output '中文执行成功'\n".into();
    let sandbox = python(&ssh, &target, probe.clone(), PROBE, 90).await;
    if sandbox["code"] == 0 && sandbox["utf8"] == true {
        println!("PASS: m3_codex_workspace_write_network_device_root");
    } else {
        println!(
            "RESULT: m3_codex_first_tier_blocked={}",
            sandbox["errorCategory"].as_str().unwrap_or("other")
        );
    }

    let read_only_file = local_path.join(format!("readonly-{}.txt", uuid::Uuid::new_v4().simple()));
    let mut read_only = probe.clone();
    read_only["readOnly"] = true.into();
    read_only["script"] = format!(
        "Set-Content -LiteralPath '{}' -Value 'readonly-probe'\n",
        read_only_file
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "''")
    )
    .into();
    let readonly_result = python(&ssh, &target, read_only, PROBE, 90).await;
    println!(
        "OBSERVE: m3_raw_codex_read_only_socket_code={} local_write={} category={}",
        readonly_result["code"],
        read_only_file.exists(),
        readonly_result["errorCategory"]
    );
    if read_only_file.exists() {
        std::fs::remove_file(read_only_file).unwrap();
    }

    // Independent project grants share a device key, but Host normal paths and
    // session entries must stop working immediately when their grant is revoked.
    let second_id = format!("m3_second_{}", uuid::Uuid::new_v4().simple());
    let second_path = local_path.join("second-project");
    std::fs::create_dir_all(&second_path).unwrap();
    let mut second_project = project.clone();
    second_project.id = second_id.clone();
    second_project.path = second_path.to_string_lossy().into_owned();
    must(db.save_project(&second_project));
    must(client.request("project.upsert", json!({"id":second_id,"path":second_path.to_string_lossy(),"name":"Second","kind":"local"})).await);
    must(
        client
            .request("device.project.enable", json!({"projectId":second_id}))
            .await,
    );
    let second = must(client.request("session.create", json!({"projectId":second_id,"agent":"fake","model":"fake-default","permissionMode":"default"})).await);
    let second_sid = second["id"].as_str().unwrap();
    must(
        client
            .request("sync.subscribe", json!({"sessionId":second_sid}))
            .await,
    );
    let second_run = must(client.request("run.send", json!({"sessionId":second_sid,"clientMsgId":uuid::Uuid::new_v4().to_string(),"text":"Inspect","script":[{"type":"sleep","seconds":600},{"type":"wait_input"}]})).await);
    must(client.request("run.send", json!({"sessionId":sid,"clientMsgId":uuid::Uuid::new_v4().to_string(),"text":"Inspect","script":[{"type":"sleep","seconds":600}]})).await);
    let mut revocation_events = client.events.subscribe();
    must(
        client
            .request("device.project.revoke", json!({"projectId":project.id}))
            .await,
    );
    let stopped = event(&mut revocation_events, &sid, "run.finished", 15).await;
    assert_eq!(stopped["payload"]["state"], "interrupted");
    assert_eq!(stopped["payload"]["reason"], "本机授权已撤销");
    let remaining = must(client.request("env.probe", json!({})).await);
    assert_eq!(remaining["host"]["activeRuns"], 1);
    let mut second_probe = probe.clone();
    second_probe["config"] = format!(
        "{}/session-ssh/{second_sid}.conf",
        device_root.rsplit_once("/devices/").unwrap().0
    )
    .into();
    second_probe["script"] = "Write-Output '中文执行成功'\n".into();
    second_probe["sandbox"] = false.into();
    assert_eq!(
        python(&ssh, &target, second_probe.clone(), PROBE, 30).await["code"],
        0
    );
    println!("PASS: m3_revoke_stops_only_running_project");
    let denied = client.request("run.send", json!({"sessionId":sid,"clientMsgId":uuid::Uuid::new_v4().to_string(),"text":"Inspect","script":[]})).await.unwrap_err();
    assert_eq!(denied.code, "E_DEVICE_UNAUTHORIZED");
    let revoked = python(&ssh, &target, json!({"root":device_root,"config":session_config}), "print(json.dumps({'key':(pathlib.Path(p['root'])/'keys/id').is_file(),'session':pathlib.Path(p['config']).exists()}))", 15).await;
    assert_eq!(revoked, json!({"key":true,"session":false}));
    assert_ne!(
        python(&ssh, &target, probe.clone(), PROBE, 15).await["code"],
        0
    );
    must(
        client
            .request("device.project.enable", json!({"projectId":project.id}))
            .await,
    );
    must(client.request("run.send", json!({"sessionId":sid,"clientMsgId":uuid::Uuid::new_v4().to_string(),"text":"Inspect","script":[{"type":"sleep","seconds":600}]})).await);
    must(client.request("device.offline", json!({})).await);
    let absent = python(
        &ssh,
        &target,
        json!({"config":session_config}),
        "print(json.dumps({'exists':pathlib.Path(p['config']).exists()}))",
        15,
    )
    .await;
    assert_eq!(absent["exists"], false);
    must(ssh.cancel_reverse_local(&target, &socket).await);
    executor.stop().await;
    let mut second_target = target.clone();
    second_target.project_id = Some(second_id.clone());
    second_target.session_id = None;
    must(db.register_session(&second_target, &second));
    second_target.session_id = Some(second_sid.to_owned());
    let mut reconnect_events = client.events.subscribe();
    let (next_executor, next_socket) = tunnel(&ssh, &second_target, &client, &resources).await;
    assert_eq!(next_socket, socket);
    executor = next_executor;
    let stopped = event(&mut reconnect_events, &sid, "run.finished", 15).await;
    assert_eq!(stopped["payload"]["state"], "interrupted");
    let remaining = must(client.request("env.probe", json!({})).await);
    assert_eq!(remaining["host"]["activeRuns"], 1);
    let absent = python(
        &ssh,
        &target,
        json!({"config":session_config}),
        "print(json.dumps({'exists':pathlib.Path(p['config']).exists()}))",
        15,
    )
    .await;
    assert_eq!(absent["exists"], false);
    println!("PASS: m3_reconnect_only_b_interrupts_a_without_restoring_a_config");
    assert_eq!(
        python(&ssh, &target, second_probe, PROBE, 30).await["code"],
        0
    );
    println!("PASS: m3_other_project_channel_remains_usable_after_revoke_and_reconnect");
    // Append is acknowledged before stop, but the scripted task never consumes it.
    let queued_id = uuid::Uuid::new_v4().to_string();
    let second_run_id = second_run["runId"].as_str().unwrap();
    must(client.request("run.append", json!({"sessionId":second_sid,"runId":second_run_id,"clientMsgId":queued_id,"text":"must remain undelivered"})).await);
    let mut stop_events = client.events.subscribe();
    must(
        client
            .request(
                "run.stop",
                json!({"sessionId":second_sid,"runId":second_run_id}),
            )
            .await,
    );
    let notice = event(&mut stop_events, second_sid, "item.completed", 15).await;
    assert_eq!(notice["payload"]["delivery"], "undelivered");
    assert_eq!(notice["payload"]["clientMsgId"], queued_id);
    event(&mut stop_events, second_sid, "run.finished", 15).await;
    let history = must(
        client
            .request(
                "session.history",
                json!({"sessionId":second_sid,"limit":100}),
            )
            .await,
    );
    assert!(!history["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["payload"]["kind"] == "text"
            && event["payload"]["text"] == "must remain undelivered"));
    println!("PASS: m3_fake_append_immediate_stop_never_executes_input");
    must(
        client
            .request("device.project.revoke", json!({"projectId":second_id}))
            .await,
    );
    let revoked = python(&ssh, &target, json!({"root":device_root}), "print(json.dumps({'keys':(pathlib.Path(p['root'])/'keys').exists(),'ready':(pathlib.Path(p['root'])/'ready').exists()}))", 15).await;
    assert_eq!(revoked, json!({"keys":false,"ready":false}));
    must(ssh.cancel_reverse_local(&target, &socket).await);
    executor.stop().await;
    let (next_executor, next_socket) = tunnel(&ssh, &target, &client, &resources).await;
    assert_eq!(next_socket, socket);
    executor = next_executor;
    let mut resumed_events = client.events.subscribe();
    must(client.request("run.send", json!({"sessionId":sid,"clientMsgId":uuid::Uuid::new_v4().to_string(),"text":"Inspect","script":[]})).await);
    event(&mut resumed_events, &sid, "run.finished", 10).await;
    println!("PASS: m3_project_revoke_blocks_session_last_revoke_deletes_keys");

    let old_runtime = executor.runtime_directory().unwrap().to_path_buf();
    let old_key = executor.credentials().host_public_key.clone();
    must(client.request("device.offline", json!({})).await);
    must(ssh.cancel_reverse_local(&target, &socket).await);
    executor.stop().await;
    assert!(!old_runtime.exists());
    client.cancel.cancel();
    must(ssh.disconnect(&target).await);
    assert_eq!(
        must(ssh.connect(&target, None, None).await).state,
        "connected"
    );
    let client = must(connect_host(&ssh, &target, device_id).await);
    let (mut executor, reconnected_socket) = tunnel(&ssh, &target, &client, &resources).await;
    assert_eq!(reconnected_socket, socket);
    assert_ne!(executor.credentials().host_public_key, old_key);
    assert_eq!(
        must(
            ssh.exec(&target, &format!("cat {}", quote(&session_config)))
                .await
        ),
        initial_config
    );
    probe["sandbox"] = false.into();
    assert_eq!(
        python(&ssh, &target, probe.clone(), PROBE, 90).await["code"],
        0
    );
    println!("PASS: m3_transport_reconnect_stable_session_config_rotated_keys");

    // Keep control SSH alive while a command waits at the device entry, then reconnect it.
    must(client.request("device.offline", json!({})).await);
    must(ssh.cancel_reverse_local(&target, &socket).await);
    executor.stop().await;
    let waiting_ssh = ssh.clone();
    let waiting_target = target.clone();
    let mut waiting_probe = probe.clone();
    waiting_probe["config"] = format!("{device_root}/ssh.conf").into();
    let waiter = tokio::spawn(async move {
        python(&waiting_ssh, &waiting_target, waiting_probe, PROBE, 90).await
    });
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!waiter.is_finished());
    let (mut executor, _) = tunnel(&ssh, &target, &client, &resources).await;
    let resumed = waiter.await.unwrap();
    assert_eq!(resumed["code"], 0);
    assert!(resumed["elapsed"].as_f64().unwrap() >= 2.0);
    println!("PASS: m3_offline_entry_wait_reconnect_resumes_same_command");

    let mut events = client.events.subscribe();
    must(
        client
            .request("sync.subscribe", json!({"sessionId":sid}))
            .await,
    );
    must(client.request("run.send", json!({"sessionId":sid,"clientMsgId":uuid::Uuid::new_v4().to_string(),"text":"Wait for device reconnection","script":[{"type":"sleep","seconds":600}]})).await);
    let grace_started = Instant::now();
    must(client.request("device.offline", json!({})).await);
    must(ssh.cancel_reverse_local(&target, &socket).await);
    let port = executor.port();
    let runtime = executor.runtime_directory().unwrap().to_path_buf();
    executor.stop().await;
    assert!(!runtime.exists());
    assert!(
        tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .is_err()
    );
    let closed = event(&mut events, &sid, "device.state", 10).await;
    assert_eq!(closed["payload"]["state"], "closed");
    assert_eq!(closed["payload"]["grace"], 300);
    let mut offline_probe = probe.clone();
    offline_probe["config"] = format!("{device_root}/ssh.conf").into();
    let timeout = python(&ssh, &target, offline_probe, PROBE, 90).await;
    assert_ne!(timeout["code"], 0);
    assert_eq!(timeout["errorCategory"], "device-offline");
    assert!((59.0..75.0).contains(&timeout["elapsed"].as_f64().unwrap()));
    println!("PASS: m3_default_device_wait_timeout_60_seconds");
    let finished = event(&mut events, &sid, "run.finished", 270).await;
    assert_eq!(finished["payload"]["state"], "interrupted");
    assert!(grace_started.elapsed() >= Duration::from_secs(299));
    println!("PASS: m3_default_device_grace_300_seconds_run_interrupted");
    let cleanup = python(&ssh, &target, json!({"root":device_root,"native":native_root,"config":session_config}), "root=pathlib.Path(p['root'])\nprint(json.dumps({'keys':(root/'keys').exists(),'socket':(root/'tunnel.sock').exists(),'ready':(root/'ready').exists(),'native':pathlib.Path(p['native']).is_dir(),'sessionConfig':pathlib.Path(p['config']).is_file()}))", 15).await;
    assert_eq!(
        cleanup,
        json!({"keys":false,"socket":false,"ready":false,"native":true,"sessionConfig":false})
    );
    println!("PASS: m3_local_process_keys_socket_cleanup_native_preserved");
    must(
        client
            .request("sync.unsubscribe", json!({"sessionId":sid}))
            .await,
    );
    client.cancel.cancel();
    must(ssh.disconnect(&target).await);
}
