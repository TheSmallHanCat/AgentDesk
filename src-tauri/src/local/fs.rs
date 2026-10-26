use crate::{
    db::Db,
    error::{AppError, AppResult},
    models::Target,
    ssh::sftp::{FileDocument, FileEntry, EDIT_LIMIT},
};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

fn io(error: std::io::Error) -> AppError {
    AppError::new(
        if error.kind() == std::io::ErrorKind::NotFound {
            "E_NOT_FOUND"
        } else {
            "E_IO"
        },
        "local.file",
        error.to_string(),
    )
}
pub fn local(db: &Db, target: &Target) -> AppResult<bool> {
    db.validate_target(target)?;
    Ok(target
        .project_id
        .as_ref()
        .map(|id| db.project(id))
        .transpose()?
        .is_some_and(|p| p.device_local))
}
fn normalized(path: &Path) -> AppResult<PathBuf> {
    fs::canonicalize(path).map_err(io)
}

fn protected_root(path: &Path) -> AppResult<PathBuf> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or_else(|| io(error))?;
            Ok(protected_root(parent)?.join(
                path.file_name()
                    .ok_or_else(|| AppError::new("E_INPUT", "local.file", "File name required"))?,
            ))
        }
        Err(error) => Err(io(error)),
    }
}

pub fn checked(
    db: &Db,
    target: &Target,
    path: &str,
    modifying: bool,
    follow: bool,
) -> AppResult<PathBuf> {
    db.validate_target(target)?;
    let project = db.project(
        target
            .project_id
            .as_deref()
            .ok_or_else(|| AppError::new("E_TARGET", "local.file", "Project target required"))?,
    )?;
    if !project.device_local {
        return Err(AppError::new(
            "E_TARGET",
            "local.file",
            "Local project target required",
        ));
    }
    let root = normalized(Path::new(&project.path))?;
    let input = Path::new(path);
    if !input.is_absolute()
        || input
            .components()
            .any(|p| matches!(p, std::path::Component::ParentDir))
    {
        return Err(AppError::new(
            "E_INPUT",
            "local.file",
            "Normalized absolute local path required",
        ));
    }
    let resolved = if follow && input.exists() {
        normalized(input)?
    } else {
        normalized(input.parent().ok_or_else(|| {
            AppError::new("E_PROTECTED", "local.file", "Root cannot be modified")
        })?)?
        .join(
            input
                .file_name()
                .ok_or_else(|| AppError::new("E_INPUT", "local.file", "File name required"))?,
        )
    };
    if !crate::platform::path_within(&resolved, &root) {
        return Err(AppError::new(
            "E_PROTECTED",
            "local.file",
            "Path is outside the local project",
        ));
    }
    if modifying {
        let home =
            directories::BaseDirs::new().and_then(|base| fs::canonicalize(base.home_dir()).ok());
        let mut protected: Vec<PathBuf> = db
            .projects()?
            .iter()
            .filter(|p| p.device_local)
            .map(|p| protected_root(Path::new(&p.path)))
            .collect::<AppResult<_>>()?;
        protected.extend(home);
        if protected
            .iter()
            .any(|path| crate::platform::path_within(path, &resolved))
            || crate::platform::local_system_roots().iter().any(|path| {
                crate::platform::path_within(path, &resolved)
                    || crate::platform::path_within(&resolved, path)
            })
            || resolved.components().count() <= 2
        {
            return Err(AppError::new(
                "E_PROTECTED",
                "local.file",
                "Project roots, home, system directories and their ancestors cannot be modified",
            ));
        }
    }
    Ok(resolved)
}
fn entry(path: &Path) -> AppResult<FileEntry> {
    let meta = fs::symlink_metadata(path).map_err(io)?;
    Ok(FileEntry {
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
        path: crate::platform::display_local_path(path),
        kind: if meta.is_symlink() {
            "symlink"
        } else if meta.is_dir() {
            "directory"
        } else if meta.is_file() {
            "file"
        } else {
            "other"
        }
        .into(),
        size: meta.len(),
        mtime: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|t| t.as_secs() as u32)
            .unwrap_or(0),
        permissions: if meta.permissions().readonly() {
            0o444
        } else {
            0o644
        },
    })
}
pub fn list(db: &Db, target: &Target, path: &str, hidden: bool) -> AppResult<Vec<FileEntry>> {
    let path = checked(db, target, path, false, true)?;
    let mut result = Vec::new();
    for item in fs::read_dir(path).map_err(io)? {
        let item = item.map_err(io)?;
        if !hidden && item.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if result.len() >= 20000 {
            return Err(AppError::new(
                "E_LIMIT",
                "local.list",
                "Directory exceeds 20000 entries",
            ));
        }
        result.push(entry(&item.path())?);
    }
    result.sort_by_key(|entry| (entry.kind != "directory", entry.name.to_lowercase()));
    Ok(result)
}
pub fn stat(db: &Db, target: &Target, path: &str) -> AppResult<FileEntry> {
    entry(&checked(db, target, path, false, false)?)
}
pub fn read(db: &Db, target: &Target, path: &str) -> AppResult<FileDocument> {
    let path = checked(db, target, path, false, true)?;
    let meta = entry(&path)?;
    if meta.kind != "file" {
        return Err(AppError::new(
            "E_FILE_TYPE",
            "local.read",
            "Only regular files can be edited",
        ));
    }
    if meta.size > EDIT_LIMIT {
        return Err(AppError::new(
            "E_LIMIT",
            "local.read",
            "Text file exceeds 2 MiB",
        ));
    }
    let bytes = fs::read(&path).map_err(io)?;
    if bytes.len() as u64 > EDIT_LIMIT || bytes.contains(&0) {
        return Err(AppError::new(
            "E_FILE_TYPE",
            "local.read",
            "File is not supported text",
        ));
    }
    let content = String::from_utf8(bytes)
        .map_err(|_| AppError::new("E_ENCODING", "local.read", "UTF-8 text required"))?;
    let crlf = content.matches("\r\n").count();
    let lf = content.matches('\n').count();
    let line_ending = if crlf == 0 {
        "lf"
    } else if crlf == lf {
        "crlf"
    } else {
        "mixed"
    }
    .into();
    Ok(FileDocument {
        path: meta.path,
        content,
        mtime: meta.mtime,
        size: meta.size,
        permissions: meta.permissions,
        line_ending,
        save_mode: None,
    })
}
#[allow(clippy::too_many_arguments)]
pub fn save(
    db: &Db,
    target: &Target,
    path: &str,
    content: &str,
    mtime: u32,
    size: u64,
    force: bool,
) -> AppResult<FileDocument> {
    let path = checked(db, target, path, true, true)?;
    let before = fs::metadata(&path).map_err(io)?;
    if content.len() as u64 > EDIT_LIMIT {
        return Err(AppError::new(
            "E_LIMIT",
            "local.save",
            "Text file exceeds 2 MiB",
        ));
    }
    if content.contains('\0') {
        return Err(AppError::new(
            "E_FILE_TYPE",
            "local.save",
            "Binary content cannot be saved as text",
        ));
    }
    let current = entry(&path)?;
    if !force && (current.mtime != mtime || current.size != size) {
        return Err(AppError::new(
            "E_CONFLICT",
            "local.save",
            "File mtime or size changed",
        ));
    }
    let mut file = fs::OpenOptions::new().write(true).open(&path).map_err(io)?;
    let opened = file.metadata().map_err(io)?;
    if opened.modified().ok() != before.modified().ok() || opened.len() != before.len() {
        return Err(AppError::new(
            "E_CONFLICT",
            "local.save",
            "File changed before saving",
        ));
    }
    file.set_len(0).map_err(io)?;
    file.write_all(content.as_bytes()).map_err(io)?;
    file.sync_all().map_err(io)?;
    drop(file);
    let mut result = read(
        db,
        target,
        path.to_str()
            .ok_or_else(|| AppError::new("E_ENCODING", "local.save", "Unicode path required"))?,
    )?;
    if result.content != content {
        return Err(AppError::new(
            "E_CONFLICT",
            "local.save",
            "File content changed while saving; reload before retrying",
        ));
    }
    result.save_mode = Some("inPlace".into());
    Ok(result)
}
pub fn mutate(
    db: &Db,
    target: &Target,
    action: &str,
    path: &str,
    destination: Option<&str>,
) -> AppResult<()> {
    let path = checked(db, target, path, true, false)?;
    match action {
        "create" => {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(io)?;
        }
        "mkdir" => fs::create_dir(path).map_err(io)?,
        "rename" => {
            let destination = checked(db, target, destination.unwrap_or_default(), true, false)?;
            if destination.exists() {
                return Err(AppError::new(
                    "E_EXISTS",
                    "local.rename",
                    "Destination exists",
                ));
            }
            fs::rename(path, destination).map_err(io)?;
        }
        "delete" => {
            let metadata = fs::symlink_metadata(&path).map_err(io)?;
            if metadata.is_dir() && !metadata.is_symlink() {
                fs::remove_dir_all(path).map_err(io)?;
            } else {
                fs::remove_file(path).map_err(io)?;
            }
        }
        _ => {
            return Err(AppError::new(
                "E_INPUT",
                "local.file",
                "Unknown file operation",
            ))
        }
    }
    Ok(())
}
