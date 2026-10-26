use agentdesk_lib::{
    db::Db,
    local::fs,
    models::{Connection, Project, Target},
};

#[test]
fn local_files_keep_ownership_roots_conflicts_and_native_content() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(std::path::Path::new(":memory:")).unwrap();
    db.save_connection(&Connection {
        id: "c".into(),
        name: "c".into(),
        host: "localhost".into(),
        port: 22,
        user: "test".into(),
        auth_method: "password".into(),
        key_path: None,
        agent_kind: None,
        proxy_jump: None,
        source: None,
        last_used_at: None,
    })
    .unwrap();
    let project = Project {
        device_local: true,
        id: "p".into(),
        connection_id: "c".into(),
        name: "Local".into(),
        path: dir.path().to_string_lossy().into(),
        description: String::new(),
        default_agent: "fake".into(),
        default_model: "fake-default".into(),
        permission_mode: "default".into(),
        pinned: false,
        archived: false,
        last_opened_at: None,
    };
    db.save_project(&project).unwrap();
    let owner = Target {
        device_local: true,
        connection_id: "c".into(),
        project_id: Some("p".into()),
        session_id: None,
        request_seq: 1,
    };
    let path = dir
        .path()
        .join("content.txt")
        .to_string_lossy()
        .into_owned();
    fs::mutate(&db, &owner, "create", &path, None).unwrap();
    let before = fs::read(&db, &owner, &path).unwrap();
    let saved = fs::save(
        &db,
        &owner,
        &path,
        "中文\r\n",
        before.mtime,
        before.size,
        false,
    )
    .unwrap();
    assert_eq!(saved.content, "中文\r\n");
    assert_eq!(saved.line_ending, "crlf");
    assert_eq!(
        fs::save(
            &db,
            &owner,
            &path,
            "binary\0content",
            saved.mtime,
            saved.size,
            false
        )
        .unwrap_err()
        .code,
        "E_FILE_TYPE"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), saved.content);
    std::fs::write(&path, "external modification").unwrap();
    assert_eq!(
        fs::save(
            &db,
            &owner,
            &path,
            "overwrite",
            saved.mtime,
            saved.size,
            false
        )
        .unwrap_err()
        .code,
        "E_CONFLICT"
    );
    assert_eq!(
        fs::mutate(&db, &owner, "delete", &project.path, None)
            .unwrap_err()
            .code,
        "E_PROTECTED"
    );
    let outside = dir.path().parent().unwrap().join("outside.txt");
    assert_eq!(
        fs::checked(&db, &owner, &outside.to_string_lossy(), true, false)
            .unwrap_err()
            .code,
        "E_PROTECTED"
    );
    let mut wrong = owner.clone();
    wrong.connection_id = "wrong".into();
    assert!(fs::read(&db, &wrong, &path).is_err());
    assert_eq!(fs::list(&db, &owner, &project.path, true).unwrap().len(), 1);
    fs::mutate(&db, &owner, "delete", &path, None).unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let directory = dir.path().join("directory");
        let link = dir.path().join("junction");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("preserved.txt"), "preserved").unwrap();
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&directory)
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Cannot create the isolated junction"
        );
        fs::mutate(&db, &owner, "delete", &link.to_string_lossy(), None).unwrap();
        assert!(!link.exists());
        assert_eq!(
            std::fs::read_to_string(directory.join("preserved.txt")).unwrap(),
            "preserved"
        );
        let symlink = dir.path().join("directory-symlink");
        if std::os::windows::fs::symlink_dir(&directory, &symlink).is_ok() {
            fs::mutate(&db, &owner, "delete", &symlink.to_string_lossy(), None).unwrap();
            assert!(directory.join("preserved.txt").exists());
        }
    }
    assert!(dir.path().exists());
}

#[cfg(windows)]
#[test]
fn wide_local_projects_cannot_modify_canonical_home_or_system_paths() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(std::path::Path::new(":memory:")).unwrap();
    db.save_connection(&Connection {
        id: "c".into(),
        name: "c".into(),
        host: "localhost".into(),
        port: 22,
        user: "test".into(),
        auth_method: "password".into(),
        key_path: None,
        agent_kind: None,
        proxy_jump: None,
        source: None,
        last_used_at: None,
    })
    .unwrap();
    let home = directories::BaseDirs::new()
        .unwrap()
        .home_dir()
        .to_path_buf();
    let root = home.ancestors().last().unwrap();
    let mut project = Project {
        device_local: true,
        id: "wide".into(),
        connection_id: "c".into(),
        name: "Local".into(),
        path: root.to_string_lossy().into(),
        description: String::new(),
        default_agent: "fake".into(),
        default_model: "fake-default".into(),
        permission_mode: "default".into(),
        pinned: false,
        archived: false,
        last_opened_at: None,
    };
    db.save_project(&project).unwrap();
    let owner = Target {
        device_local: true,
        connection_id: "c".into(),
        project_id: Some("wide".into()),
        session_id: None,
        request_seq: 1,
    };
    assert_eq!(
        fs::checked(&db, &owner, &home.to_string_lossy(), true, true)
            .unwrap_err()
            .code,
        "E_PROTECTED"
    );
    for name in ["WINDIR", "ProgramFiles", "ProgramData"] {
        if let Some(path) = std::env::var_os(name) {
            assert_eq!(
                fs::checked(&db, &owner, &path.to_string_lossy(), true, true)
                    .unwrap_err()
                    .code,
                "E_PROTECTED"
            );
        }
    }
    let home_alias = format!("{}.", home.display());
    assert!(fs::checked(&db, &owner, &home_alias, true, false).is_err());
    let alternate_stream = format!("{}:stream", home.display());
    assert!(fs::checked(&db, &owner, &alternate_stream, true, false).is_err());
    let short_system = root.join("PROGRA~1");
    if short_system.is_dir() {
        assert_eq!(
            fs::checked(&db, &owner, &short_system.to_string_lossy(), true, false)
                .unwrap_err()
                .code,
            "E_PROTECTED"
        );
    }
    project.id = "missing".into();
    project.path = dir.path().join("missing").to_string_lossy().into();
    db.save_project(&project).unwrap();
    let allowed = dir.path().join("valid.txt");
    assert!(fs::checked(&db, &owner, &allowed.to_string_lossy(), true, false).is_ok());
}
