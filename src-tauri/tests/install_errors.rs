use agentdesk_lib::{error::AppError, models::Target};
use serde_json::json;
use std::sync::Arc;
#[test]
fn probe_failure_preserves_failed_recovery_details() {
    let directory = tempfile::tempdir().unwrap();
    let db = Arc::new(agentdesk_lib::db::Db::open(std::path::Path::new(":memory:")).unwrap());
    let ssh = agentdesk_lib::ssh::manager::SshManager::new(
        db,
        Arc::new(agentdesk_lib::ssh::known_hosts::KnownHosts::new(
            directory.path().join("known_hosts"),
            None,
        )),
        Arc::new(agentdesk_lib::session_log::Diagnostics::new(
            directory.path(),
        )),
        None,
    );
    let target = Target {
        connection_id: "test".into(),
        project_id: None,
        session_id: None,
        request_seq: 1,
        device_local: false,
    };
    let error = AppError::new("E_PROTOCOL", "install.probe", "Probe failed").with_details(json!({"recoveryCode":"E_IO","recoveryStage":"install.recover","hint":"Host recovery failed"}));
    let error = ssh.install_error(&target, error);
    let details = error.details.unwrap();
    assert_eq!(details["installStage"], "install.probe");
    assert_eq!(details["recoveryCode"], "E_IO");
    assert_eq!(details["recoveryStage"], "install.recover");
    assert!(details["hint"]
        .as_str()
        .unwrap()
        .contains("recovery failed"));
}
