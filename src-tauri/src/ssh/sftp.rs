use super::manager::SshManager;
use crate::{
    error::{AppError, AppResult},
    models::Target,
};
use russh_sftp::{
    client::SftpSession,
    protocol::{FileAttributes, StatusCode},
};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const EDIT_LIMIT: u64 = 2 * 1024 * 1024;
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: u64,
    pub mtime: u32,
    pub permissions: u32,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDocument {
    pub path: String,
    pub content: String,
    pub mtime: u32,
    pub size: u64,
    pub permissions: u32,
    pub line_ending: String,
}
pub fn error(e: russh_sftp::client::error::Error) -> AppError {
    match &e {
        russh_sftp::client::error::Error::Status(s) if s.status_code == StatusCode::NoSuchFile => {
            AppError::new("E_NOT_FOUND", "sftp", e.to_string())
        }
        _ => AppError::io("sftp", e),
    }
}
pub async fn session(ssh: &SshManager, target: &Target) -> AppResult<SftpSession> {
    let channel = ssh.open_channel(target).await?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| AppError::io("sftp", e))?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(error)?;
    sftp.set_timeout(15);
    Ok(sftp)
}
pub fn entry(path: String, name: String, m: FileAttributes) -> FileEntry {
    let kind = if m.is_symlink() {
        "symlink"
    } else if m.is_dir() {
        "directory"
    } else if m.is_regular() {
        "file"
    } else {
        "other"
    };
    FileEntry {
        name,
        path,
        kind: kind.into(),
        size: m.size.unwrap_or(0),
        mtime: m.mtime.unwrap_or(0),
        permissions: m.permissions.unwrap_or(0),
    }
}
pub fn remote_path(path: &str) -> AppResult<String> {
    if !path.starts_with('/') || path.contains('\0') {
        return Err(AppError::new(
            "E_INPUT",
            "path",
            "An absolute remote path is required",
        ));
    }
    let mut parts = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(p),
        }
    }
    Ok(format!("/{}", parts.join("/")))
}
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
pub async fn list(sftp: &SftpSession, path: &str, hidden: bool) -> AppResult<Vec<FileEntry>> {
    let path = remote_path(path)?;
    let mut entries = Vec::new();
    for item in sftp.read_dir(&path).await.map_err(error)? {
        let name = item.file_name();
        if name == "." || name == ".." || (!hidden && name.starts_with('.')) {
            continue;
        }
        if name.contains('/') || name.contains('\0') {
            return Err(AppError::new("E_SFTP", "list", "Invalid directory entry"));
        }
        if entries.len() >= 20000 {
            return Err(AppError::new(
                "E_LIMIT",
                "list",
                "Directory exceeds 20000 entries",
            ));
        }
        entries.push(entry(
            format!("{}/{name}", path.trim_end_matches('/')),
            name,
            item.metadata(),
        ));
    }
    entries.sort_by(|a, b| {
        (a.kind != "directory", a.name.to_lowercase())
            .cmp(&(b.kind != "directory", b.name.to_lowercase()))
    });
    Ok(entries)
}
pub async fn read(sftp: &SftpSession, path: &str) -> AppResult<FileDocument> {
    let path = remote_path(path)?;
    let meta = sftp.metadata(&path).await.map_err(error)?;
    if !meta.is_regular() {
        return Err(AppError::new(
            "E_FILE_TYPE",
            "read",
            "Only regular files can be edited",
        ));
    }
    if meta.size.unwrap_or(EDIT_LIMIT + 1) > EDIT_LIMIT {
        return Err(AppError::new("E_LIMIT", "read", "Text file exceeds 2 MiB"));
    }
    let mut file = sftp.open(&path).await.map_err(error)?;
    let mut bytes = Vec::new();
    let result = (&mut file)
        .take(EDIT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .await;
    let close = file.close().await;
    result.map_err(|e| AppError::io("read", e))?;
    close.map_err(|e| AppError::io("read", e))?;
    if bytes.len() as u64 > EDIT_LIMIT {
        return Err(AppError::new("E_LIMIT", "read", "Text file exceeds 2 MiB"));
    }
    let after = sftp.metadata(&path).await.map_err(error)?;
    if meta.mtime != after.mtime || meta.size != after.size {
        return Err(AppError::new(
            "E_CONFLICT",
            "read",
            "File changed while reading; reload it",
        ));
    }
    document(path, bytes, after)
}
pub fn document(path: String, bytes: Vec<u8>, meta: FileAttributes) -> AppResult<FileDocument> {
    if bytes.contains(&0) {
        return Err(AppError::new(
            "E_ENCODING",
            "read",
            "Binary files cannot be edited",
        ));
    }
    let content = String::from_utf8(bytes)
        .map_err(|_| AppError::new("E_ENCODING", "read", "Only UTF-8 text is supported"))?;
    let crlf = content.matches("\r\n").count();
    let lf = content.matches('\n').count();
    let ending = if crlf == 0 {
        "lf"
    } else if crlf == lf {
        "crlf"
    } else {
        "mixed"
    };
    Ok(FileDocument {
        path,
        content,
        mtime: meta.mtime.unwrap_or(0),
        size: meta.size.unwrap_or(0),
        permissions: meta.permissions.unwrap_or(0),
        line_ending: ending.into(),
    })
}
pub async fn write_temp(
    sftp: &SftpSession,
    path: &str,
    content: &str,
    permissions: u32,
) -> AppResult<String> {
    if content.len() as u64 > EDIT_LIMIT || content.contains('\0') {
        return Err(AppError::new(
            "E_LIMIT",
            "save",
            "Content must be UTF-8 text within 2 MiB",
        ));
    }
    let (parent, _) = path.rsplit_once('/').unwrap();
    let tmp = format!("{parent}/.agentdesk-save-{}", uuid::Uuid::new_v4());
    let result = async {
        let mut file = sftp
            .open_with_flags_and_attributes(
                &tmp,
                russh_sftp::protocol::OpenFlags::WRITE
                    | russh_sftp::protocol::OpenFlags::CREATE
                    | russh_sftp::protocol::OpenFlags::EXCLUDE,
                FileAttributes {
                    permissions: Some(permissions & 0o7777),
                    ..Default::default()
                },
            )
            .await
            .map_err(error)?;
        let result = file.write_all(content.as_bytes()).await;
        let close = file.close().await;
        result.map_err(|e| AppError::io("save", e))?;
        close.map_err(|e| AppError::io("save", e))?;
        sftp.set_metadata(
            &tmp,
            FileAttributes {
                permissions: Some(permissions & 0o7777),
                ..Default::default()
            },
        )
        .await
        .map_err(error)?;
        Ok(())
    }
    .await;
    if let Err(e) = result {
        let _ = sftp.remove_file(&tmp).await;
        return Err(e);
    }
    Ok(tmp)
}
pub async fn finish<T>(sftp: SftpSession, result: AppResult<T>) -> AppResult<T> {
    let closed = sftp.close().await.map_err(error);
    let value = result?;
    closed?;
    Ok(value)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_paths_and_quotes_do_not_become_shell_code() {
        assert_eq!(remote_path("/a/../b//c").unwrap(), "/b/c");
        assert!(remote_path("relative").is_err());
        assert_eq!(quote("a'b;$(x)"), "'a'\\''b;$(x)'");
    }
    #[test]
    fn crlf_and_permissions_are_preserved() {
        let doc = document(
            "/file".into(),
            b"a\r\nb\r\n".to_vec(),
            FileAttributes {
                permissions: Some(0o100755),
                size: Some(6),
                mtime: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(doc.line_ending, "crlf");
        assert_eq!(doc.content, "a\r\nb\r\n");
        assert_eq!(doc.permissions, 0o100755);
    }
    #[test]
    fn binary_and_non_utf8_are_rejected() {
        for data in [vec![0], vec![255]] {
            assert!(document("/file".into(), data, FileAttributes::default()).is_err());
        }
    }
}
