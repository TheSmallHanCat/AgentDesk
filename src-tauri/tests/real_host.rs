//! Opt-in M2 acceptance through the production russh, installer and Host client.
use agentdesk_lib::{
    db::Db,
    error::AppResult,
    host::{
        client::{remote_home, HostClient},
        installer::{start_script, Installer, STOP_SCRIPT},
    },
    models::{Connection, Project, Target},
    session_log::Diagnostics,
    ssh::{known_hosts::KnownHosts, manager::SshManager},
};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};

fn must<T>(result: AppResult<T>) -> T {
    result.unwrap_or_else(|error| panic!("M2 failed: {} at {}", error.code, error.stage))
}
async fn until(receiver: &mut tokio::sync::broadcast::Receiver<Value>, kind: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let event = receiver.recv().await.expect("Host event stream closed");
            if event["type"] == kind {
                return event;
            }
        }
    })
    .await
    .expect("Expected Host event timed out")
}

#[tokio::test]
#[ignore = "requires dedicated E2E server and generated Host release resources"]
async fn real_host_deploy_stream_approval_stop_reconnect() {
    for name in [
        "AGENTDESK_E2E_HOST",
        "AGENTDESK_E2E_PORT",
        "AGENTDESK_E2E_ROOT_USER",
        "AGENTDESK_E2E_ROOT_PASSWORD",
    ] {
        if std::env::var(name)
            .ok()
            .is_none_or(|value| value.is_empty())
        {
            println!("SKIP: missing {name}");
            return;
        }
    }
    let private = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.tmp/e2e");
    if !private.join("id_ed25519").exists() {
        println!("SKIP: run E2E provisioning first");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let db = Arc::new(must(Db::open(std::path::Path::new(":memory:"))));
    let ssh = Arc::new(SshManager::new(
        db.clone(),
        Arc::new(KnownHosts::new(directory.path().join("known_hosts"), None)),
        Arc::new(Diagnostics::new(directory.path())),
        None,
    ));
    let c = Connection {
        id: "m2-e2e".into(),
        name: "M2".into(),
        host: std::env::var("AGENTDESK_E2E_HOST").unwrap(),
        port: std::env::var("AGENTDESK_E2E_PORT")
            .unwrap()
            .parse()
            .unwrap(),
        user: "agentdesk-test".into(),
        auth_method: "key".into(),
        key_path: Some(private.join("id_ed25519").to_string_lossy().into()),
        agent_kind: None,
        proxy_jump: None,
        source: None,
        last_used_at: None,
    };
    must(db.save_connection(&c));
    let mut target = Target {
        connection_id: c.id,
        project_id: None,
        session_id: None,
        request_seq: 1,
    };
    let initial = must(ssh.connect(&target, None, None).await);
    if initial.state == "awaitingTrust" {
        must(ssh.trust(&target, initial.fingerprint.as_ref().unwrap()));
        assert_eq!(
            must(ssh.connect(&target, None, None).await).state,
            "connected"
        );
    }
    let home = must(remote_home(&ssh, &target).await);
    if let Ok(existing) = HostClient::connect(&ssh, &target, "m2-e2e-device").await {
        let status = must(existing.request("env.probe", json!({})).await);
        assert_eq!(
            status["host"]["activeRuns"].as_u64().unwrap_or(0),
            0,
            "Dedicated Host still has an active run"
        );
        existing.cancel.cancel();
        must(ssh.exec(&target, STOP_SCRIPT).await);
    }
    let installer = Installer::default();
    let resources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../resources/host");
    let plan = must(installer.plan(&target, &home, &resources, true));
    let plan = must(installer.take(&target, plan["planId"].as_str().unwrap()));
    let progress = tauri::ipc::Channel::new(|_| Ok(()));
    must(installer.execute(&ssh, &target, plan, progress).await);
    println!("PASS: m2_production_installer_verified_artifacts");
    let before_cancel = must(
        ssh.exec(&target, "readlink \"$HOME/.agentdesk/host/current\"")
            .await,
    );
    let cancelling = Arc::new(Installer::default());
    let cancel_plan = must(cancelling.plan(&target, &home, &resources, true));
    let cancel_id = cancel_plan["planId"].as_str().unwrap().to_owned();
    let cancel_plan = must(cancelling.take(&target, &cancel_id));
    let cancel_owner = target.clone();
    let cancel_manager = cancelling.clone();
    let progress = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(text) = body {
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["message"]
                .as_str()
                .is_some_and(|text| text.contains("Preparing verified uv"))
            {
                must(cancel_manager.cancel(&cancel_owner, &cancel_id));
            }
        }
        Ok(())
    });
    assert_eq!(
        cancelling
            .execute(&ssh, &target, cancel_plan, progress)
            .await
            .unwrap_err()
            .code,
        "E_CANCELLED"
    );
    assert_eq!(
        must(
            ssh.exec(&target, "readlink \"$HOME/.agentdesk/host/current\"")
                .await
        ),
        before_cancel
    );
    println!("PASS: m2_installer_cancel_preserves_current");
    must(ssh.exec(&target, &start_script(true)).await);
    let client = must(HostClient::connect(&ssh, &target, "m2-e2e-device").await);
    let capabilities = must(client.request("agent.status", json!({})).await);
    assert_eq!(capabilities["fake"]["available"], true);
    for kind in ["claude", "codex"] {
        assert_eq!(capabilities[kind]["installed"], true);
        assert_eq!(capabilities[kind]["loggedIn"], false);
        assert_eq!(capabilities[kind]["handshake"], true);
    }
    println!("PASS: m2_native_unlogged_handshake_models_capabilities");
    let project = Project {
        id: format!("m2_{}", uuid::Uuid::new_v4().simple()),
        connection_id: target.connection_id.clone(),
        name: "M2".into(),
        path: format!("{home}/agentdesk-e2e/files"),
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
    must(
        client
            .request(
                "project.upsert",
                json!({"id":project.id,"path":project.path,"name":project.name}),
            )
            .await,
    );
    let session=must(client.request("session.create",json!({"projectId":project.id,"agent":"fake","model":"fake-default","permissionMode":"default"})).await);
    must(db.register_session(&target, &session));
    let sid = session["id"].as_str().unwrap().to_owned();
    target.session_id = Some(sid.clone());
    let mut events = client.events.subscribe();
    must(
        client
            .request("sync.subscribe", json!({"sessionId":sid}))
            .await,
    );
    let message_id = uuid::Uuid::new_v4().to_string();
    let input = json!({"sessionId":sid,"clientMsgId":message_id,"text":"Process the project","script":[{"type":"text","text":"streaming result","chunks":5,"delay":0.03},{"type":"tool","name":"Read","input":{"path":"README.md"},"output":"done"},{"type":"approval","tool":"Write","input":{"path":"result.txt"}},{"type":"text","text":"approved"}]});
    let run = must(client.request("run.send", input.clone()).await);
    assert_eq!(must(client.request("run.send", input).await), run);
    until(&mut events, "item.delta").await;
    let approval = until(&mut events, "approval.request").await;
    let aid = &approval["payload"]["approvalId"];
    must(
        client
            .request(
                "approval.respond",
                json!({"sessionId":sid,"approvalId":aid,"decision":{"behavior":"allow"}}),
            )
            .await,
    );
    assert_eq!(
        client
            .request(
                "approval.respond",
                json!({"sessionId":sid,"approvalId":aid,"decision":{"behavior":"deny"}})
            )
            .await
            .unwrap_err()
            .code,
        "E_ALREADY_RESOLVED"
    );
    let finished = until(&mut events, "run.finished").await;
    assert_eq!(finished["payload"]["state"], "completed");
    println!("PASS: m2_fake_stream_tool_approval_idempotency");
    let last_seq = finished["seq"].as_u64().unwrap();
    let run=must(client.request("run.send",json!({"sessionId":sid,"clientMsgId":uuid::Uuid::new_v4().to_string(),"text":"Continue","script":[{"type":"text","text":"recovery","chunks":8,"delay":0.1},{"type":"sleep","seconds":60}]})).await);
    until(&mut events, "item.delta").await;
    let host_id = client.hello.lock().unwrap()["hostId"].clone();
    client.cancel.cancel();
    must(ssh.disconnect(&target).await);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        must(ssh.connect(&target, None, None).await).state,
        "connected"
    );
    let reconnected = must(HostClient::connect(&ssh, &target, "m2-e2e-device").await);
    assert_eq!(reconnected.hello.lock().unwrap()["hostId"], host_id);
    let mut events = reconnected.events.subscribe();
    must(
        reconnected
            .request("sync.subscribe", json!({"sessionId":sid}))
            .await,
    );
    let recovered = must(
        reconnected
            .request("sync.since", json!({"sessionId":sid,"lastSeq":last_seq}))
            .await,
    );
    assert!(recovered["events"]
        .as_array()
        .unwrap()
        .iter()
        .all(|event| event["seq"].as_u64().unwrap() > last_seq));
    must(
        reconnected
            .request("run.stop", json!({"sessionId":sid,"runId":run["runId"]}))
            .await,
    );
    assert_eq!(
        until(&mut events, "run.finished").await["payload"]["state"],
        "interrupted"
    );
    let history = must(
        reconnected
            .request("session.history", json!({"sessionId":sid,"limit":200}))
            .await,
    );
    let history = history["events"].as_array().unwrap();
    let replayed: Vec<u64> = recovered["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect();
    let watermark = recovered["currentSeq"].as_u64().unwrap();
    let expected: Vec<u64> = history
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .filter(|seq| *seq > last_seq && *seq <= watermark)
        .collect();
    assert_eq!(
        replayed, expected,
        "Reconnect replay must include every durable event in its cursor interval"
    );
    let user_messages = history
        .iter()
        .filter(|event| event["type"] == "user.message")
        .count();
    assert_eq!(user_messages, 2);
    let seqs: std::collections::BTreeSet<u64> = history
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs.len(), history.len());
    println!("PASS: m2_ssh_disconnect_host_survival_stop_replay_no_duplicates");
    for kind in ["claude", "codex"] {
        // Session creation must fail before any prompt or account login.
        let error = reconnected
            .request(
                "session.create",
                json!({"projectId":project.id,"agent":kind,"model":"","permissionMode":"default"}),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "E_NOT_LOGGED_IN");
    }
    println!("PASS: m2_native_not_logged_in_errors_without_prompt");
    must(
        reconnected
            .request("session.delete", json!({"sessionId":sid}))
            .await,
    );
    reconnected.cancel.cancel();
    // The maintenance lease is set atomically with the active-run check and
    // survives the stream closing while SSH performs the service operation.
    let maintenance = must(HostClient::connect(&ssh, &target, "m2-e2e-device").await);
    assert_eq!(
        must(maintenance.request("host.prepareShutdown", json!({})).await)["prepared"],
        true
    );
    maintenance.cancel.cancel();
    must(ssh.exec(&target, STOP_SCRIPT).await);
    let current = must(
        ssh.exec(&target, "readlink \"$HOME/.agentdesk/host/current\"")
            .await,
    );
    must(
        ssh.exec(&target, &agentdesk_lib::host::installer::rollback_script())
            .await,
    );
    must(ssh.exec(&target, &start_script(true)).await);
    let previous = must(HostClient::connect(&ssh, &target, "m2-e2e-device").await);
    assert_eq!(previous.hello.lock().unwrap()["hostId"], host_id);
    previous.cancel.cancel();
    must(ssh.exec(&target, STOP_SCRIPT).await);
    must(
        ssh.exec(&target, &agentdesk_lib::host::installer::rollback_script())
            .await,
    );
    assert_eq!(
        must(
            ssh.exec(&target, "readlink \"$HOME/.agentdesk/host/current\"")
                .await
        ),
        current
    );
    must(ssh.exec(&target, &start_script(true)).await);
    println!("PASS: m2_host_stop_start_and_version_rollback");
    must(ssh.disconnect(&target).await);
}
