use crate::{
    error::{AppError, AppResult},
    models::now,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub connection_id: String,
    pub stage: String,
    pub elapsed_ms: u64,
    pub success: bool,
    pub code: Option<String>,
    pub timestamp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr_tail: Option<String>,
}
pub struct Diagnostics {
    records: Mutex<VecDeque<Diagnostic>>,
    path: PathBuf,
}
impl Diagnostics {
    pub fn new(root: &Path) -> Self {
        Self {
            records: Mutex::new(VecDeque::new()),
            path: root.join("diagnostics.jsonl"),
        }
    }
    pub fn record(&self, connection_id: &str, stage: &str, elapsed_ms: u64, code: Option<&str>) {
        let Ok(mut records) = self.records.lock() else {
            return;
        };
        let value = Diagnostic {
            connection_id: connection_id.into(),
            stage: stage.into(),
            elapsed_ms,
            success: code.is_none(),
            code: code.map(str::to_owned),
            timestamp: now(),
            exit_code: None,
            stderr_tail: None,
        };
        if records.len() >= 500 {
            records.pop_front();
        }
        records.push_back(value.clone());
        // The schema is an allowlist: no free-form errors, paths, credentials or payloads.
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() > 1024 * 1024) {
            let _ = std::fs::rename(&self.path, self.path.with_extension("previous.jsonl"));
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            if let Ok(line) = serde_json::to_string(&value) {
                let _ = writeln!(file, "{line}");
            }
        }
    }
    pub fn install_failure(
        &self,
        connection_id: &str,
        stage: &str,
        elapsed_ms: u64,
        code: &str,
        exit_code: Option<u32>,
        stderr: &[u8],
    ) -> serde_json::Value {
        let tail = sanitized_install_tail(stderr);
        let value = Diagnostic {
            connection_id: connection_id.into(),
            stage: stage.into(),
            elapsed_ms,
            success: false,
            code: Some(code.into()),
            timestamp: now(),
            exit_code,
            stderr_tail: Some(tail.clone()),
        };
        if let Ok(mut records) = self.records.lock() {
            if records.len() >= 500 {
                records.pop_front();
            }
            records.push_back(value.clone());
        }
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() > 1024 * 1024) {
            let _ = std::fs::rename(&self.path, self.path.with_extension("previous.jsonl"));
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            if let Ok(line) = serde_json::to_string(&value) {
                let _ = writeln!(file, "{line}");
            }
        }
        serde_json::json!({"installStage":stage,"exitCode":exit_code,"stderrTail":tail})
    }
    pub fn list(&self) -> AppResult<Vec<Diagnostic>> {
        self.records
            .lock()
            .map(|r| r.iter().cloned().collect())
            .map_err(|e| AppError::io("diagnostics", e))
    }
    pub fn export(&self, path: &Path) -> AppResult<String> {
        let records = self.list()?;
        let (actual, mut file) = create_unique(path)?;
        serde_json::to_writer_pretty(
            &mut file,
            &serde_json::json!({"version":1,"application":"AgentDesk","records":records}),
        )
        .map_err(|e| AppError::io("diagnostics", e))?;
        file.sync_all()
            .map_err(|e| AppError::io("diagnostics", e))?;
        Ok(actual.to_string_lossy().into_owned())
    }
}
/// Keep only recognized diagnostics from the tail. Unknown text may contain an
/// endpoint, signed URL, account or credential; never persist it verbatim.
pub fn sanitized_install_tail(stderr: &[u8]) -> String {
    let text =
        String::from_utf8_lossy(&stderr[stderr.len().saturating_sub(2048)..]).to_ascii_lowercase();
    let known = [
        "text file busy",
        "permission denied",
        "no such file or directory",
        "file exists",
        "resource temporarily unavailable",
        "operation not permitted",
        "connection refused",
        "could not resolve host",
        "connection timed out",
        "not found",
        "invalid argument",
        "hash mismatch",
        "checksum",
        "failed to connect",
        "http 403",
        "http 429",
        "http 502",
        "http 503",
        "http 504",
        "database is locked",
        "e_database_schema",
        "exit 75",
        "traceback",
        "syntaxerror",
    ];
    let lines: Vec<_> = text
        .lines()
        .rev()
        .take(8)
        .map(|line| {
            let matches: Vec<_> = known
                .iter()
                .filter(|value| line.contains(**value))
                .copied()
                .collect();
            if matches.is_empty() {
                "[redacted]".into()
            } else {
                matches.join("; ")
            }
        })
        .collect();
    lines.into_iter().rev().collect::<Vec<String>>().join("\n")
}
pub fn create_unique(path: &Path) -> AppResult<(PathBuf, std::fs::File)> {
    let parent = path.parent().filter(|p| p.is_dir()).ok_or_else(|| {
        AppError::new(
            "E_INPUT",
            "local-file",
            "Destination directory does not exist",
        )
    })?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| AppError::new("E_INPUT", "local-file", "Invalid file name"))?;
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| format!(".{s}"))
        .unwrap_or_default();
    for n in 0..10000 {
        let candidate = if n == 0 {
            path.to_owned()
        } else {
            parent.join(format!("{stem} ({n}){ext}"))
        };
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(AppError::io("local-file", e)),
        }
    }
    Err(AppError::new(
        "E_DESTINATION",
        "local-file",
        "No available destination name",
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installation_failures_keep_stage_exit_and_only_safe_stderr_categories() {
        let directory = tempfile::tempdir().unwrap();
        let diagnostics = Diagnostics::new(directory.path());
        let details = diagnostics.install_failure("c", "install.start", 3, "E_EXEC", Some(75), b"private-fixture password=secret-fixture https://private-fixture/?token=secret-fixture\nPermission denied: /private-fixture\n");
        assert_eq!(details["exitCode"], 75);
        let text = std::fs::read_to_string(directory.path().join("diagnostics.jsonl")).unwrap();
        assert!(text.contains("permission denied"));
        assert!(!text.contains("private-fixture"));
        assert!(!text.contains("secret-fixture"));
    }
    #[test]
    fn exports_only_allowed_fields_and_never_overwrites() {
        let d = tempfile::tempdir().unwrap();
        let diag = Diagnostics::new(d.path());
        diag.record("conn1", "authenticate", 5, Some("E_AUTH"));
        let p = d.path().join("report.json");
        std::fs::write(&p, "keep").unwrap();
        let exported = diag.export(&p).unwrap();
        assert_ne!(exported, p.to_string_lossy());
        assert_eq!(std::fs::read_to_string(p).unwrap(), "keep");
        let content = std::fs::read_to_string(exported).unwrap();
        assert!(content.contains("E_AUTH"));
        assert!(!content.contains("password"));
    }
}
