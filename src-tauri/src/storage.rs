use crate::error::{AppError, AppResult};
use crate::models::Settings;
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub struct Storage {
    pub root: PathBuf,
    settings_lock: Mutex<()>,
}
impl Storage {
    pub fn new() -> AppResult<Self> {
        let exe = std::env::current_exe().map_err(|e| AppError::io("storage", e))?;
        let portable = exe
            .parent()
            .filter(|p| p.join("agentdesk.portable").is_file())
            .map(|p| p.join("data"));
        let standard = directories::BaseDirs::new()
            .ok_or_else(|| {
                AppError::new("E_STORAGE", "storage", "User data directory unavailable")
            })?
            .config_dir()
            .join("AgentDesk");
        let root = resolve_root(
            std::env::var_os("AGENTDESK_DATA_ROOT")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
            cfg!(debug_assertions),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(".agentdesk-data"),
            portable,
            standard,
        );
        std::fs::create_dir_all(&root).map_err(|e| AppError::io("storage", e))?;
        Ok(Self {
            root,
            settings_lock: Mutex::new(()),
        })
    }
    pub fn settings(&self) -> AppResult<Settings> {
        let _guard = self
            .settings_lock
            .lock()
            .map_err(|e| AppError::io("settings", e))?;
        let path = self.root.join("settings.json");
        if !path.exists() {
            return Ok(Settings::default());
        }
        serde_json::from_slice(&std::fs::read(path).map_err(|e| AppError::io("settings", e))?)
            .map_err(|e| AppError::new("E_SETTINGS", "settings", e.to_string()))
    }
    pub fn save_settings(&self, settings: &Settings) -> AppResult<()> {
        if !["system", "light", "dark"].contains(&settings.theme.as_str())
            || !["zh-CN", "en"].contains(&settings.language.as_str())
            || !["ask", "tray", "exit"].contains(&settings.close_action.as_str())
            || !(1000..=100000).contains(&settings.scrollback)
            || !(10..=32).contains(&settings.font_size)
        {
            return Err(AppError::new("E_INPUT", "settings", "Invalid settings"));
        }
        let _guard = self
            .settings_lock
            .lock()
            .map_err(|e| AppError::io("settings", e))?;
        write_json(&self.root.join("settings.json"), settings)
    }
}
fn resolve_root(
    env: Option<PathBuf>,
    dev: bool,
    dev_root: PathBuf,
    portable: Option<PathBuf>,
    standard: PathBuf,
) -> PathBuf {
    env.unwrap_or_else(|| {
        if dev {
            dev_root
        } else {
            portable.unwrap_or(standard)
        }
    })
}
pub fn write_json(path: &Path, value: &impl serde::Serialize) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new("E_INPUT", "storage", "Path requires a parent"))?;
    let mut temp =
        tempfile::NamedTempFile::new_in(parent).map_err(|e| AppError::io("storage", e))?;
    serde_json::to_writer_pretty(temp.as_file_mut(), value)
        .map_err(|e| AppError::io("storage", e))?;
    temp.as_file_mut()
        .flush()
        .map_err(|e| AppError::io("storage", e))?;
    temp.as_file()
        .sync_all()
        .map_err(|e| AppError::io("storage", e))?;
    temp.persist(path).map_err(|e| AppError::io("storage", e))?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn root_priority() {
        let p = PathBuf::from;
        assert_eq!(
            resolve_root(
                Some(p("env")),
                true,
                p("dev"),
                Some(p("portable")),
                p("standard")
            ),
            p("env")
        );
        assert_eq!(
            resolve_root(None, true, p("dev"), Some(p("portable")), p("standard")),
            p("dev")
        );
        assert_eq!(
            resolve_root(None, false, p("dev"), Some(p("portable")), p("standard")),
            p("portable")
        );
        assert_eq!(
            resolve_root(None, false, p("dev"), None, p("standard")),
            p("standard")
        );
    }
    #[test]
    fn atomic_json_replaces_without_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_json(&path, &1).unwrap();
        write_json(&path, &2).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "2");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
