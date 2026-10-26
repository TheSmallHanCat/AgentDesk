use agentdesk_lib::{
    db::Db,
    models::{Connection, Project, Target},
};
use serde_json::json;

#[test]
fn host_session_ownership_and_identity_are_persistent_and_connection_scoped() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("desktop.sqlite");
    let db = Db::open(&path).unwrap();
    let connection = Connection {
        id: "one".into(),
        name: "One".into(),
        host: "localhost".into(),
        port: 22,
        user: "test".into(),
        auth_method: "password".into(),
        key_path: None,
        agent_kind: None,
        proxy_jump: None,
        source: None,
        last_used_at: None,
    };
    let mut other = connection.clone();
    other.id = "two".into();
    db.save_connection(&connection).unwrap();
    db.save_connection(&other).unwrap();
    let project = Project {
        id: "project".into(),
        connection_id: "one".into(),
        name: "Project".into(),
        path: "/project".into(),
        description: String::new(),
        default_agent: "extension-agent".into(),
        default_model: String::new(),
        permission_mode: "default".into(),
        pinned: false,
        archived: false,
        last_opened_at: None,
    };
    db.save_project(&project).unwrap();
    let target = Target {
        connection_id: "one".into(),
        project_id: Some("project".into()),
        session_id: None,
        request_seq: 1,
    };
    db.register_session(&target, &json!({"id":"session","projectId":"project"}))
        .unwrap();
    let session_target = Target {
        session_id: Some("session".into()),
        ..target.clone()
    };
    db.validate_target(&session_target).unwrap();
    db.save_draft(&session_target, "saved draft").unwrap();
    assert_eq!(db.draft(&session_target).unwrap(), "saved draft");
    assert_eq!(
        db.validate_target(&Target {
            connection_id: "two".into(),
            ..session_target.clone()
        })
        .unwrap_err()
        .code,
        "E_TARGET"
    );
    assert_eq!(
        db.validate_target(&Target {
            project_id: None,
            ..session_target
        })
        .unwrap_err()
        .code,
        "E_TARGET"
    );
    db.record_host("one", "host_1").unwrap();
    db.record_host("two", "host_1").unwrap();
    assert_eq!(db.same_host_connections("one").unwrap(), vec!["two"]);
    assert_eq!(
        db.record_host("one", "host_2").unwrap_err().code,
        "E_HOST_ID_CHANGED"
    );
    let device = db.device_id().unwrap();
    drop(db);
    let reopened = Db::open(&path).unwrap();
    assert_eq!(reopened.device_id().unwrap(), device);
    reopened
        .validate_target(&Target {
            session_id: Some("session".into()),
            ..target
        })
        .unwrap();
}
