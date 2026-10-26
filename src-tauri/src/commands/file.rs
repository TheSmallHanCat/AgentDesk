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
    let result = async {
        let path = s.canonicalize(&path).await.map_err(sftp::error)?;
        let meta = s.metadata(&path).await.map_err(sftp::error)?;
        check_version(&meta, expected_mtime, expected_size, force)?;
        let tmp = sftp::write_temp(&s, &path, &content, meta.permissions.unwrap_or(0o600)).await?;
        let saved = async {
            let latest = s.metadata(&path).await.map_err(sftp::error)?;
            check_version(&latest, expected_mtime, expected_size, force)?;
            state
                .ssh
                .exec(
                    &target,
                    &format!("mv -f -- {} {}", sftp::quote(&tmp), sftp::quote(&path)),
                )
                .await?;
            sftp::read(&s, &path).await
        }
        .await;
        let _ = s.remove_file(&tmp).await;
        saved
    }
    .await;
    sftp::finish(s, result).await.map(|v| target.reply(v))
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
async fn protect(state: &AppState, target: &Target, s: &SftpSession, path: &str) -> AppResult<()> {
    if path == "/" {
        return Err(AppError::new(
            "E_PROTECTED",
            "file",
            "Root directory cannot be modified",
        ));
    }
    if let Some(id) = &target.project_id {
        let root = state.db.project(id)?.path;
        if path == root || s.canonicalize(path).await.ok().as_deref() == Some(root.as_str()) {
            return Err(AppError::new(
                "E_PROTECTED",
                "file",
                "Project root cannot be deleted or renamed",
            ));
        }
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
        protect(&state, &target, &s, &path).await?;
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
        protect(&state, &target, &s, &path).await?;
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
