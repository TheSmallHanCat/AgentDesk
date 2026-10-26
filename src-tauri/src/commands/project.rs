use crate::{
    error::{AppError, AppResult},
    models::{Project, Reply, Target},
    ssh::sftp,
    state::AppState,
};
#[tauri::command]
pub fn project_list(state: tauri::State<'_, AppState>) -> AppResult<Vec<Project>> {
    state.db.projects()
}
#[tauri::command]
pub async fn project_save(
    state: tauri::State<'_, AppState>,
    mut project: Project,
    create_dir: bool,
) -> AppResult<Project> {
    if project.id.is_empty() {
        project.id = uuid::Uuid::new_v4().to_string();
    }
    let existing = state.db.project(&project.id).ok();
    if let Some(old) = existing {
        if old.path != project.path
            || old.connection_id != project.connection_id
            || old.device_local != project.device_local
        {
            return Err(AppError::new(
                "E_TARGET",
                "project",
                "Create a new project to change its location",
            ));
        }
        state.db.save_project(&project)?;
        return Ok(project);
    }
    if project.device_local {
        let path =
            std::fs::canonicalize(&project.path).map_err(|e| AppError::io("local.project", e))?;
        if !path.is_dir() {
            return Err(AppError::new(
                "E_NOT_DIRECTORY",
                "local.project",
                "Project path is not a directory",
            ));
        }
        project.path = crate::platform::display_local_path(&path);
        state.db.save_project(&project)?;
        return Ok(project);
    }
    let target = Target {
        device_local: false,
        connection_id: project.connection_id.clone(),
        project_id: None,
        session_id: None,
        request_seq: 0,
    };
    let path = sftp::remote_path(&project.path)?;
    let s = sftp::session(&state.ssh, &target).await?;
    let result = async {
        if create_dir {
            state
                .ssh
                .exec(&target, &format!("mkdir -p -- {}", sftp::quote(&path)))
                .await?;
        }
        let canonical = s.canonicalize(&path).await.map_err(sftp::error)?;
        if !s.metadata(&canonical).await.map_err(sftp::error)?.is_dir() {
            return Err(AppError::new(
                "E_NOT_DIRECTORY",
                "project",
                "Project path is not a directory",
            ));
        }
        Ok(canonical)
    }
    .await;
    project.path = sftp::finish(s, result).await?;
    state.db.save_project(&project)?;
    let connection = state.db.connection(&project.connection_id)?;
    state
        .db
        .record_dir(&project.connection_id, &connection.user, &project.path)?;
    Ok(project)
}
#[tauri::command]
pub fn project_archive(
    state: tauri::State<'_, AppState>,
    id: String,
    archived: bool,
) -> AppResult<()> {
    let mut p = state.db.project(&id)?;
    p.archived = archived;
    state.db.save_project(&p)
}
#[tauri::command]
pub fn dir_recent(
    state: tauri::State<'_, AppState>,
    target: Target,
) -> AppResult<Reply<Vec<String>>> {
    state.ssh.validate_target(&target)?;
    let c = state.db.connection(&target.connection_id)?;
    state.db.recent(&c.id, &c.user).map(|v| target.reply(v))
}
#[tauri::command]
pub async fn dir_complete(
    state: tauri::State<'_, AppState>,
    target: Target,
    path: String,
) -> AppResult<Reply<Vec<String>>> {
    let trailing = path.is_empty() || path.ends_with('/');
    let path = if path.is_empty() {
        "/".into()
    } else {
        sftp::remote_path(&path)?
    };
    let (directory, prefix) = if trailing {
        (path.clone(), String::new())
    } else {
        let (d, p) = path.rsplit_once('/').unwrap();
        (
            if d.is_empty() { "/".into() } else { d.into() },
            p.to_string(),
        )
    };
    let s = sftp::session(&state.ssh, &target).await?;
    let result = async {
        let mut dirs = Vec::new();
        for e in sftp::list(&s, &directory, true).await? {
            if e.name.starts_with(&prefix)
                && (e.kind == "directory"
                    || (e.kind == "symlink" && s.metadata(&e.path).await.is_ok_and(|m| m.is_dir())))
            {
                dirs.push(e.path);
                if dirs.len() >= 100 {
                    break;
                }
            }
        }
        Ok(dirs)
    }
    .await;
    sftp::finish(s, result).await.map(|v| target.reply(v))
}

#[tauri::command]
pub async fn project_pick_local(app: tauri::AppHandle) -> AppResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let path = tokio::task::spawn_blocking(move || app.dialog().file().blocking_pick_folder())
        .await
        .map_err(|e| AppError::io("local.pick", e))?;
    path.map(|p| {
        p.into_path()
            .map(|p| p.to_string_lossy().replace(char::from(92), "/"))
            .map_err(|e| AppError::io("local.pick", e))
    })
    .transpose()
}
