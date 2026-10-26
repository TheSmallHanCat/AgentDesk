use crate::{
    error::{AppError, AppResult},
    models::{Reply, Target},
    ssh::sftp::{self, FileDocument, FileEntry},
    state::AppState,
};
use russh_sftp::{
    client::SftpSession,
    protocol::{FileAttributes, OpenFlags},
};
#[tauri::command]
pub async fn fs_list(
    state: tauri::State<'_, AppState>,
    target: Target,
    path: String,
    show_hidden: bool,
) -> AppResult<Reply<Vec<FileEntry>>> {
    if crate::local::fs::local(&state.db, &target)? {
        return crate::local::fs::list(&state.db, &target, &path, show_hidden)
            .map(|data| target.reply(data));
    }
    let s = sftp::session(&state.ssh, &target).await?;
    let result = sftp::list(&s, &path, show_hidden).await;
    sftp::finish(s, result).await.map(|v| target.reply(v))
}
#[tauri::command]
pub async fn fs_read(
    state: tauri::State<'_, AppState>,
    target: Target,
    path: String,
) -> AppResult<Reply<FileDocument>> {
    if crate::local::fs::local(&state.db, &target)? {
        return crate::local::fs::read(&state.db, &target, &path).map(|data| target.reply(data));
    }
    let s = sftp::session(&state.ssh, &target).await?;
    let result = sftp::read(&s, &path).await;
    sftp::finish(s, result).await.map(|v| target.reply(v))
}
#[tauri::command]
pub async fn fs_stat(
    state: tauri::State<'_, AppState>,
    target: Target,
    path: String,
) -> AppResult<Reply<FileEntry>> {
    if crate::local::fs::local(&state.db, &target)? {
        return crate::local::fs::stat(&state.db, &target, &path).map(|data| target.reply(data));
    }
    let path = sftp::remote_path(&path)?;
    let s = sftp::session(&state.ssh, &target).await?;
    let result = s
        .symlink_metadata(&path)
        .await
        .map_err(sftp::error)
        .map(|m| {
            sftp::entry(
                path.clone(),
                path.rsplit('/').next().unwrap_or("").into(),
                m,
            )
        });
    sftp::finish(s, result).await.map(|v| target.reply(v))
}
#[tauri::command]
pub async fn fs_create(
    state: tauri::State<'_, AppState>,
    target: Target,
    path: String,
) -> AppResult<Reply<()>> {
    if crate::local::fs::local(&state.db, &target)? {
        return crate::local::fs::mutate(&state.db, &target, "create", &path, None)
            .map(|data| target.reply(data));
    }
    let path = sftp::remote_path(&path)?;
    let s = sftp::session(&state.ssh, &target).await?;
    let result = async {
        let f = s
            .open_with_flags_and_attributes(
                &path,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                FileAttributes {
                    permissions: Some(0o600),
                    ..Default::default()
                },
            )
            .await
            .map_err(sftp::error)?;
        f.close().await.map_err(|e| AppError::io("create", e))
    }
    .await;
    sftp::finish(s, result).await.map(|v| target.reply(v))
}
#[tauri::command]
pub async fn fs_mkdir(
    state: tauri::State<'_, AppState>,
    target: Target,
    path: String,
) -> AppResult<Reply<()>> {
    if crate::local::fs::local(&state.db, &target)? {
        return crate::local::fs::mutate(&state.db, &target, "mkdir", &path, None)
            .map(|data| target.reply(data));
    }
    let path = sftp::remote_path(&path)?;
    let s = sftp::session(&state.ssh, &target).await?;
    let result = s.create_dir(path).await.map_err(sftp::error);
    sftp::finish(s, result).await.map(|v| target.reply(v))
}
#[tauri::command]
pub async fn fs_write(
    state: tauri::State<'_, AppState>,
    target: Target,
    path: String,
    content: String,
    expected_mtime: u32,
    expected_size: u64,
    force: bool,
) -> AppResult<Reply<FileDocument>> {
    if crate::local::fs::local(&state.db, &target)? {
        return crate::local::fs::save(
            &state.db,
            &target,
            &path,
            &content,
            expected_mtime,
            expected_size,
            force,
        )
        .map(|data| target.reply(data));
    }
    let path = sftp::remote_path(&path)?;
    let s = sftp::session(&state.ssh, &target).await?;
    let result = save(
        &state.ssh,
        &target,
        &s,
        &path,
        &content,
        expected_mtime,
        expected_size,
        force,
    )
    .await;
    sftp::finish(s, result).await.map(|v| target.reply(v))
}
#[allow(clippy::too_many_arguments)]
pub async fn save(
    ssh: &crate::ssh::manager::SshManager,
    target: &Target,
    s: &SftpSession,
    path: &str,
    content: &str,
    expected_mtime: u32,
    expected_size: u64,
    force: bool,
) -> AppResult<FileDocument> {
    if content.len() as u64 > sftp::EDIT_LIMIT || content.contains('\0') {
        return Err(AppError::new(
            "E_LIMIT",
            "save",
            "Content must be UTF-8 text within 2 MiB",
        ));
    }
    let path = s.canonicalize(path).await.map_err(sftp::error)?;
    let meta = s.metadata(&path).await.map_err(sftp::error)?;
    check_version(&meta, expected_mtime, expected_size, force)?;
    let (parent, _) = path.rsplit_once('/').unwrap();
    let parent = if parent.is_empty() { "/" } else { parent };
    let policy = save_policy(&path, parent);
    let atomic = ssh.exec(target, &policy).await? == b"atomic";
    if atomic {
        let tmp = sftp::write_temp(s, &path, content, meta.permissions.unwrap_or(0o600)).await?;
        let result = async {
            s.set_metadata(
                &tmp,
                FileAttributes {
                    uid: meta.uid,
                    gid: meta.gid,
                    ..Default::default()
                },
            )
            .await
            .map_err(sftp::error)?;
            check_version(
                &s.metadata(&path).await.map_err(sftp::error)?,
                expected_mtime,
                expected_size,
                force,
            )?;
            ssh.exec(
                target,
                &format!("mv -f -- {} {}", sftp::quote(&tmp), sftp::quote(&path)),
            )
            .await?;
            verify_saved(s, &path, content, "atomic").await
        }
        .await;
        let _ = s.remove_file(&tmp).await;
        result
    } else {
        // Fully read the original before opening with TRUNCATE. Keep inode metadata
        // and links, accepting that a transport failure can leave partial content.
        let _original = sftp::read(s, &path).await?;
        check_version(
            &s.metadata(&path).await.map_err(sftp::error)?,
            expected_mtime,
            expected_size,
            force,
        )?;
        use tokio::io::AsyncWriteExt;
        let mut file = s
            .open_with_flags(&path, OpenFlags::WRITE | OpenFlags::TRUNCATE)
            .await
            .map_err(sftp::error)?;
        let result = file
            .write_all(content.as_bytes())
            .await
            .map_err(|e| AppError::io("save", e));
        let closed = file.close().await.map_err(|e| AppError::io("save", e));
        result?;
        closed?;
        verify_saved(s, &path, content, "inPlace").await
    }
}
fn save_policy(path: &str, parent: &str) -> String {
    format!(
        r#"set -eu
p={}
d={}
[ -w "$p" ] || exit 13
links=$(stat -Lc %h -- "$p")
owner=$(stat -Lc %u -- "$p")
group=$(stat -Lc %g -- "$p")
case " $(id -G) " in *" $group "*) group_ok=1;; *) group_ok=0;; esac
if [ "$links" = 1 ] && [ "$owner" = "$(id -u)" ] && [ "$group_ok" = 1 ] && [ -w "$d" ] && command -v getfacl >/dev/null 2>&1; then
  acl=$(getfacl -cp -- "$p")
  parent_acl=$(getfacl -cp -- "$d")
  if printf '%s\n%s\n' "$acl" "$parent_acl" | grep -Eq '^(user|group):[^:]|^default:|^mask:'; then printf inPlace; else printf atomic; fi
else
  printf inPlace
fi
"#,
        sftp::quote(path),
        sftp::quote(parent)
    )
}
async fn verify_saved(
    s: &SftpSession,
    path: &str,
    content: &str,
    mode: &str,
) -> AppResult<FileDocument> {
    let mut document = sftp::read(s, path).await?;
    if document.content != content {
        return Err(AppError::new(
            "E_VERIFY",
            "save",
            "Saved content could not be verified",
        ));
    }
    document.save_mode = Some(mode.into());
    Ok(document)
}

fn check_version(meta: &FileAttributes, mtime: u32, size: u64, force: bool) -> AppResult<()> {
    if !meta.is_regular() {
        return Err(AppError::new(
            "E_FILE_TYPE",
            "save",
            "Only regular files may be saved",
        ));
    }
    if meta.permissions.unwrap_or(0) & 0o7000 != 0 {
        return Err(AppError::new(
            "E_FILE_MODE",
            "save",
            "Files with special permission bits cannot be edited",
        ));
    }
    if !force && (meta.mtime != Some(mtime) || meta.size != Some(size)) {
        return Err(AppError::new(
            "E_CONFLICT",
            "save",
            "Remote mtime or size changed",
        ));
    }
    Ok(())
}
pub fn protected_path(path: &str, home: &str, roots: &[String]) -> bool {
    fn ancestor(path: &str, root: &str) -> bool {
        path == root
            || root
                .strip_prefix(path)
                .is_some_and(|suffix| suffix.starts_with('/'))
    }
    path == "/"
        || (!path.is_empty() && path.starts_with('/') && path[1..].split('/').count() == 1)
        || ancestor(path, home)
        || roots.iter().any(|root| ancestor(path, root))
}
pub fn protected_roots(db: &crate::db::Db, target: &Target) -> AppResult<Vec<String>> {
    let connection = db.connection(&target.connection_id)?;
    let connections = db.connections()?;
    Ok(db
        .projects()?
        .into_iter()
        .filter(|p| {
            connections.iter().any(|c| {
                c.id == p.connection_id
                    && c.host.eq_ignore_ascii_case(&connection.host)
                    && c.port == connection.port
            })
        })
        .map(|p| p.path)
        .collect())
}
pub async fn protect(
    db: &crate::db::Db,
    target: &Target,
    s: &SftpSession,
    path: &str,
) -> AppResult<()> {
    db.validate_target(target)?;
    let lexical = sftp::remote_path(path)?;
    let roots = protected_roots(db, target)?;
    let home = s.canonicalize(".").await.map_err(sftp::error)?;
    if protected_path(&lexical, &home, &roots) {
        return Err(AppError::new(
            "E_PROTECTED",
            "file",
            "Registered project roots, home and their ancestors cannot be modified",
        ));
    }
    // Resolve the parent even for a dangling symlink; never follow the final
    // symlink during removal. Deleting a link does not delete its destination.
    let (parent, name) = lexical.rsplit_once('/').unwrap();
    let parent = s
        .canonicalize(if parent.is_empty() { "/" } else { parent })
        .await
        .map_err(sftp::error)?;
    let resolved = format!("{}/{name}", parent.trim_end_matches('/'));
    if protected_path(&resolved, &home, &roots) {
        return Err(AppError::new(
            "E_PROTECTED",
            "file",
            "The resolved path contains a protected root",
        ));
    }
    Ok(())
}
#[tauri::command]
pub async fn fs_rename(
    state: tauri::State<'_, AppState>,
    target: Target,
    path: String,
    new_path: String,
) -> AppResult<Reply<()>> {
    if crate::local::fs::local(&state.db, &target)? {
        return crate::local::fs::mutate(&state.db, &target, "rename", &path, Some(&new_path))
            .map(|data| target.reply(data));
    }
    let path = sftp::remote_path(&path)?;
    let new_path = sftp::remote_path(&new_path)?;
    let s = sftp::session(&state.ssh, &target).await?;
    let result = async {
        protect(&state.db, &target, &s, &path).await?;
        match s.symlink_metadata(&new_path).await {
            Ok(_) => return Err(AppError::new("E_EXISTS", "rename", "Destination exists")),
            Err(e) => {
                let error = sftp::error(e);
                if error.code != "E_NOT_FOUND" {
                    return Err(error);
                }
            }
        }
        s.rename(path, new_path).await.map_err(sftp::error)
    }
    .await;
    sftp::finish(s, result).await.map(|v| target.reply(v))
}
#[tauri::command]
pub async fn fs_delete(
    state: tauri::State<'_, AppState>,
    target: Target,
    path: String,
) -> AppResult<Reply<()>> {
    if crate::local::fs::local(&state.db, &target)? {
        return crate::local::fs::mutate(&state.db, &target, "delete", &path, None)
            .map(|data| target.reply(data));
    }
    let path = sftp::remote_path(&path)?;
    let s = sftp::session(&state.ssh, &target).await?;
    let result = async {
        protect(&state.db, &target, &s, &path).await?;
        let mut stack = vec![(path, false, 0)];
        let mut visited = 0;
        while let Some((p, after, depth)) = stack.pop() {
            visited += 1;
            if visited > 50000 || depth > 64 {
                return Err(AppError::new(
                    "E_LIMIT",
                    "delete",
                    "Directory exceeds deletion limits",
                ));
            }
            if after {
                s.remove_dir(p).await.map_err(sftp::error)?;
                continue;
            }
            let meta = s.symlink_metadata(&p).await.map_err(sftp::error)?;
            if meta.is_dir() && !meta.is_symlink() {
                stack.push((p.clone(), true, depth));
                for e in sftp::list(&s, &p, true).await? {
                    stack.push((e.path, false, depth + 1));
                }
            } else {
                s.remove_file(p).await.map_err(sftp::error)?;
            }
        }
        Ok(())
    }
    .await;
    sftp::finish(s, result).await.map(|v| target.reply(v))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::Db,
        models::{Connection, Project},
    };

    fn connection(id: &str, host: &str, port: u16, user: &str) -> Connection {
        Connection {
            id: id.into(),
            name: id.into(),
            host: host.into(),
            port,
            user: user.into(),
            auth_method: "password".into(),
            key_path: None,
            agent_kind: None,
            proxy_jump: None,
            source: None,
            last_used_at: None,
        }
    }

    fn project(id: &str, connection_id: &str, path: &str) -> Project {
        Project {
            device_local: false,
            id: id.into(),
            connection_id: connection_id.into(),
            name: id.into(),
            path: path.into(),
            description: String::new(),
            default_agent: "claude".into(),
            default_model: String::new(),
            permission_mode: "default".into(),
            pinned: false,
            archived: false,
            last_opened_at: None,
        }
    }

    #[test]
    fn protected_roots_include_other_connections_and_users_on_the_same_endpoint() {
        let directory = tempfile::tempdir().unwrap();
        let db = Db::open(&directory.path().join("projects.sqlite")).unwrap();
        for connection in [
            connection("admin", "work.example", 22, "root"),
            connection("ordinary", "WORK.EXAMPLE", 22, "ordinary"),
        ] {
            db.save_connection(&connection).unwrap();
        }
        db.save_project(&project("own", "admin", "/work/admin/root"))
            .unwrap();
        let mut other = project("other", "ordinary", "/work/ordinary/root");
        other.archived = true;
        db.save_project(&other).unwrap();
        let target = Target {
            device_local: false,
            connection_id: "admin".into(),
            project_id: None,
            session_id: None,
            request_seq: 1,
        };
        let roots = protected_roots(&db, &target).unwrap();
        assert_eq!(roots.len(), 2);
        assert!(protected_path("/work/ordinary/root", "/root", &roots));
        assert!(protected_path("/work/ordinary", "/root", &roots));
        assert!(!protected_path("/work/ordinary/root/file", "/root", &roots));
        let target = Target {
            device_local: false,
            connection_id: "ordinary".into(),
            ..target
        };
        let roots = protected_roots(&db, &target).unwrap();
        assert!(protected_path("/work/admin/root", "/home/ordinary", &roots));
    }

    #[test]
    fn protected_roots_do_not_mix_other_hosts_or_ports() {
        let directory = tempfile::tempdir().unwrap();
        let db = Db::open(&directory.path().join("projects.sqlite")).unwrap();
        for connection in [
            connection("selected", "work.example", 22, "ordinary"),
            connection("other-host", "another.example", 22, "ordinary"),
            connection("other-port", "work.example", 2200, "ordinary"),
        ] {
            db.save_connection(&connection).unwrap();
        }
        db.save_project(&project("own", "selected", "/work/current/root"))
            .unwrap();
        db.save_project(&project("elsewhere", "other-host", "/work/elsewhere/root"))
            .unwrap();
        db.save_project(&project(
            "other-service",
            "other-port",
            "/work/service/root",
        ))
        .unwrap();
        let target = Target {
            device_local: false,
            connection_id: "selected".into(),
            project_id: None,
            session_id: None,
            request_seq: 1,
        };
        let roots = protected_roots(&db, &target).unwrap();
        assert_eq!(roots, ["/work/current/root"]);
        assert!(!protected_path("/work/elsewhere", "/home/ordinary", &roots));
        assert!(!protected_path(
            "/work/service/root",
            "/home/ordinary",
            &roots
        ));
    }

    #[test]
    fn special_file_modes_are_rejected_even_when_force_is_requested() {
        let mut meta = FileAttributes {
            permissions: Some(0o100755),
            mtime: Some(1),
            size: Some(10),
            ..Default::default()
        };
        assert!(check_version(&meta, 1, 10, false).is_ok());
        for special in [0o4000, 0o2000, 0o1000, 0o7000] {
            meta.permissions = Some(0o100755 | special);
            for force in [false, true] {
                assert_eq!(
                    check_version(&meta, 1, 10, force).unwrap_err().code,
                    "E_FILE_MODE"
                );
            }
        }
    }

    fn policy_result(
        links: &str,
        owner: &str,
        group: &str,
        groups: &str,
        acl: &str,
        parent_acl: &str,
    ) -> String {
        use std::{
            io::Write,
            process::{Command, Stdio},
        };
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let file = directory.path().join("file");
        std::fs::write(&file, "original").unwrap();
        let scripts = [
            ("stat", "case \"$2\" in %h) printf '%s' \"$TEST_LINKS\";; %u) printf '%s' \"$TEST_OWNER\";; %g) printf '%s' \"$TEST_GROUP\";; *) exit 1;; esac"),
            ("id", "case \"$1\" in -G) printf '%s' \"$TEST_GROUPS\";; -u) printf 1000;; *) exit 1;; esac"),
            ("getfacl", "if test \"$3\" = \"$TEST_FILE\"; then printf '%s\\n' \"$TEST_ACL\"; else printf '%s\\n' \"$TEST_PARENT_ACL\"; fi"),
        ];
        for (name, script) in scripts {
            let path = bin.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let shell_path = |path: &std::path::Path| {
            let path = path.to_string_lossy().replace('\\', "/");
            if cfg!(windows) && path.as_bytes().get(1) == Some(&b':') {
                format!("/{}{}", path[..1].to_ascii_lowercase(), &path[2..])
            } else {
                path
            }
        };
        let shell = if cfg!(windows) {
            // GitHub's PowerShell jobs expose git.exe without necessarily adding
            // sh.exe to PATH. Resolve the shell from that same Git installation.
            let output = Command::new("git").arg("--exec-path").output().unwrap();
            assert!(output.status.success());
            let git_path =
                std::path::PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
            git_path
                .ancestors()
                .flat_map(|path| [path.join("bin/sh.exe"), path.join("usr/bin/sh.exe")])
                .find(|path| path.is_file())
                .expect("Windows save-policy regression requires Git Bash")
        } else {
            std::path::PathBuf::from("sh")
        };
        let file_path = shell_path(&file);
        let policy = format!(
            "PATH={}:\"$PATH\"\nexport PATH\n{}",
            sftp::quote(&shell_path(&bin)),
            save_policy(&file_path, &shell_path(directory.path()))
        );
        let mut process = Command::new(shell)
            .arg("-s")
            .env("TEST_LINKS", links)
            .env("TEST_OWNER", owner)
            .env("TEST_GROUP", group)
            .env("TEST_GROUPS", groups)
            .env("TEST_ACL", acl)
            .env("TEST_PARENT_ACL", parent_acl)
            .env("TEST_FILE", &file_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect(
                "Save-policy regression requires the standard POSIX shell (Git Bash on Windows)",
            );
        process
            .stdin
            .take()
            .unwrap()
            .write_all(policy.as_bytes())
            .unwrap();
        let output = process.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "Save-policy regression failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn save_policy_executes_acl_and_group_checks_before_selecting_atomic_mode() {
        let plain = "user::rw-\ngroup::r--\nother::---";
        assert_eq!(
            policy_result("1", "1000", "2000", "1000 2000", plain, plain),
            "atomic"
        );
        let inherited = format!("{plain}\ndefault:user:2001:r--\ndefault:mask::r--");
        assert_eq!(
            policy_result("1", "1000", "2000", "1000 2000", plain, &inherited),
            "inPlace"
        );
        let extended = format!("{plain}\nuser:2001:r--\nmask::r--");
        assert_eq!(
            policy_result("1", "1000", "2000", "1000 2000", &extended, plain),
            "inPlace"
        );
        assert_eq!(
            policy_result("1", "1000", "2000", "1000", plain, plain),
            "inPlace"
        );
        assert_eq!(
            policy_result("1", "1000", "100", "1000 2000", plain, plain),
            "inPlace"
        );
        assert_eq!(
            policy_result("2", "1000", "2000", "1000 2000", plain, plain),
            "inPlace"
        );
        assert_eq!(
            policy_result("1", "2001", "2000", "1000 2000", plain, plain),
            "inPlace"
        );
    }
    #[test]
    fn review_protects_all_registered_roots_and_ancestors() {
        let roots = vec!["/work/a".into(), "/work/nested/b".into()];
        for path in [
            "/",
            "/etc",
            "/work",
            "/work/a",
            "/work/nested",
            "/work/nested/b",
            "/home",
            "/home/test",
        ] {
            assert!(protected_path(path, "/home/test", &roots), "{path}");
        }
        for path in [
            "/work/another",
            "/work/a/file",
            "/home/testing/file",
            "/home/test/data",
        ] {
            assert!(!protected_path(path, "/home/test", &roots), "{path}");
        }
    }
    #[test]
    fn external_modification_requires_explicit_override() {
        let mut meta = FileAttributes {
            permissions: Some(0o100644),
            mtime: Some(1),
            size: Some(10),
            ..Default::default()
        };
        assert!(check_version(&meta, 1, 10, false).is_ok());
        meta.size = Some(11);
        assert_eq!(
            check_version(&meta, 1, 10, false).unwrap_err().code,
            "E_CONFLICT"
        );
        assert!(check_version(&meta, 1, 10, true).is_ok());
    }
}
