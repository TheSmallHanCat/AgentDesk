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
    let policy = format!(
        r#"set -eu
p={}
d={}
[ -w "$p" ] || exit 13
links=$(stat -Lc %h -- "$p")
owner=$(stat -Lc %u -- "$p")
if [ "$links" = 1 ] && [ "$owner" = "$(id -u)" ] && [ -w "$d" ] && command -v getfacl >/dev/null 2>&1; then
  acl=$(getfacl -cp -- "$p")
  if printf '%s\n' "$acl" | grep -Eq '^(user|group):[^:]|^default:|^mask:'; then printf inPlace; else printf atomic; fi
else
  printf inPlace
fi
"#,
        sftp::quote(&path),
        sftp::quote(parent)
    );
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
pub async fn protect(
    db: &crate::db::Db,
    target: &Target,
    s: &SftpSession,
    path: &str,
) -> AppResult<()> {
    db.validate_target(target)?;
    let lexical = sftp::remote_path(path)?;
    let roots: Vec<_> = db
        .projects()?
        .into_iter()
        .filter(|p| p.connection_id == target.connection_id)
        .map(|p| p.path)
        .collect();
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
