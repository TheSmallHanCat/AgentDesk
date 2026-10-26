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
