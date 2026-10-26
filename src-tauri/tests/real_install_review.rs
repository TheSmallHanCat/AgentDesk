//! Opt-in installation/recovery stress against the dedicated test user.
use agentdesk_lib::{
    commands::env::{execute_host_install, install_host, rollback_host},
    db::Db,
    error::AppResult,
    host::{
        client::{remote_home, HostPool},
        installer::{snapshot_script, start_script, HostLinks, Installer},
    },
    models::{Connection, Target},
    session_log::Diagnostics,
    ssh::{known_hosts::KnownHosts, manager::SshManager},
    state::AppState,
    storage::Storage,
};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
fn must<T>(result: AppResult<T>) -> T {
    result.unwrap_or_else(|e| {
        panic!(
            "Installation review failed: {} at {} ({})",
            e.code,
            e.stage,
            e.details.unwrap_or(Value::Null)
        )
    })
}
fn progress() -> tauri::ipc::Channel<Value> {
    tauri::ipc::Channel::new(|_| Ok(()))
}
async fn links(state: &AppState, target: &Target) -> HostLinks {
    must(HostLinks::parse(&must(
        state
            .ssh
            .exec_stage(target, "review.snapshot", &snapshot_script(false))
            .await,
    )))
}
async fn check_running(state: &AppState, target: &Target) {
    let client = must(state.hosts.client(&state.ssh, &state.db, target).await);
    let value = must(client.request("env.probe", json!({})).await);
    assert_eq!(value["host"]["running"], true);
    assert_eq!(value["host"]["activeRuns"], 0);
}
#[tokio::test]
#[ignore = "Requires dedicated E2E server; runs ten install/upgrade cycles and twenty rollbacks"]
async fn real_install_ten_cycles_and_failure_recovery() {
    if std::env::var("AGENTDESK_E2E_HOST").is_err() || std::env::var("AGENTDESK_E2E_PORT").is_err()
    {
        println!("SKIP: missing E2E environment");
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let private = root.join(".tmp/e2e");
    let directory = tempfile::tempdir().unwrap();
    std::env::set_var("AGENTDESK_DATA_ROOT", directory.path());
    let db = Arc::new(must(Db::open(Path::new(":memory:"))));
    let diagnostics = Arc::new(Diagnostics::new(directory.path()));
    let ssh = Arc::new(SshManager::new(
        db.clone(),
        Arc::new(KnownHosts::new(directory.path().join("known_hosts"), None)),
        diagnostics.clone(),
        None,
    ));
    let connection = Connection {
        id: "install-review".into(),
        name: "Review".into(),
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
    must(db.save_connection(&connection));
    let target = Target {
        device_local: false,
        connection_id: connection.id,
        project_id: None,
        session_id: None,
        request_seq: 1,
    };
    let status = must(ssh.connect(&target, None, None).await);
    if status.state == "awaitingTrust" {
        let pin = russh::keys::PublicKey::from_openssh(
            &std::fs::read_to_string(private.join("server_key.pub")).unwrap(),
        )
        .unwrap()
        .fingerprint(russh::keys::HashAlg::Sha256)
        .to_string();
        assert_eq!(status.fingerprint.as_deref(), Some(pin.as_str()));
        must(ssh.trust(&target, &pin));
        assert_eq!(
            must(ssh.connect(&target, None, None).await).state,
            "connected"
        );
    }
    let state = AppState {
        db: db.clone(),
        storage: must(Storage::new()),
        ssh: ssh.clone(),
        diagnostics: diagnostics.clone(),
        local_terminals: Arc::default(),
        terminals: Arc::default(),
        transfers: Arc::default(),
        hosts: Arc::new(HostPool::new("installer-review-device".into())),
        local_channels: Arc::default(),
        installer: Arc::new(Installer::default()),
    };
    let home = must(remote_home(&ssh, &target).await);
    let resources = root.join("resources/host");
    must(
        ssh.exec_stage(&target, "review.start", &start_script(true))
            .await,
    );
    check_running(&state, &target).await;
    // Fail once before activation and once after activation. Both execute the
    // actual maintenance/stop/recovery orchestration used by env_install_run.
    for activated in [false, true] {
        let before = links(&state, &target).await;
        let planned = must(state.installer.plan(&target, &home, &resources, true));
        let mut plan = must(
            state
                .installer
                .take(&target, planned["planId"].as_str().unwrap()),
        );
        if activated {
            // Replace expensive preparation with the retained verified version;
            // keep the production commit barrier, symlink activation and recovery.
            let from = plan
                .script
                .find(r"printf '%s\n' 'Preparing verified uv runtime'")
                .unwrap();
            let until = plan
                .script
                .find(r"printf '%s\n' 'Waiting for installation commit'")
                .unwrap();
            plan.script.replace_range(
                from..until,
                &format!(
                    "mkdir -p \"$version\"\ncp -as {}/. \"$version/\"\n",
                    agentdesk_lib::ssh::sftp::quote(before.current.as_deref().unwrap())
                ),
            );
            plan.script.push_str("\nphase activate.links\nprintf 'secret-fixture Permission denied\n' >&2\nexit 72\n");
        } else {
            plan.script = plan.script.replace(
                "phase uv.download",
                "phase uv.download\nprintf 'secret-fixture Permission denied\n' >&2\nexit 72",
            );
        }
        let error = execute_host_install(&state, target.clone(), plan, progress())
            .await
            .unwrap_err();
        println!(
            "OBSERVE: injected activated={activated} code={} stage={} details={}",
            error.code,
            error.stage,
            error.details.as_ref().unwrap_or(&Value::Null)
        );
        assert_eq!(error.details.as_ref().unwrap()["exitCode"], 72);
        assert!(error.stage.starts_with("install."));
        assert!(
            error
                .details
                .as_ref()
                .unwrap()
                .get("recoveryCode")
                .is_none(),
            "Recovery failed"
        );
        assert_eq!(links(&state, &target).await, before);
        check_running(&state, &target).await;
        println!("PASS: install_failure_recovers_running_version activated={activated}");
    }
    let saved = std::fs::read_to_string(directory.path().join("diagnostics.jsonl")).unwrap();
    assert!(!saved.contains("secret-fixture"));
    assert!(saved.contains("permission denied"));
    for index in 0..10 {
        let before = links(&state, &target).await;
        let planned = must(state.installer.plan(&target, &home, &resources, true));
        must(
            install_host(
                &state,
                target.clone(),
                planned["planId"].as_str().unwrap().into(),
                progress(),
            )
            .await,
        );
        check_running(&state, &target).await;
        let installed = links(&state, &target).await;
        assert_ne!(installed.current, before.current);
        assert_eq!(installed.previous, before.current);
        must(rollback_host(&state, target.clone(), Some(true)).await);
        assert_eq!(links(&state, &target).await.current, before.current);
        check_running(&state, &target).await;
        must(rollback_host(&state, target.clone(), Some(true)).await);
        assert_eq!(links(&state, &target).await.current, installed.current);
        check_running(&state, &target).await;
        println!("PASS: install_upgrade_rollback_cycle_{}", index + 1);
    }
    println!("PASS: ten_installs_twenty_rollbacks_running_verified");
    state.hosts.disconnect(&target.connection_id).await;
    must(ssh.disconnect(&target).await);
}
