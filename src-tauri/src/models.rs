use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub connection_id: String,
    pub project_id: Option<String>,
    pub session_id: Option<String>,
    pub request_seq: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Reply<T> {
    pub target: Target,
    pub data: T,
}
impl Target {
    pub fn reply<T>(&self, data: T) -> Reply<T> {
        Reply {
            target: self.clone(),
            data,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth_method: String,
    pub key_path: Option<String>,
    pub agent_kind: Option<String>,
    pub proxy_jump: Option<String>,
    pub source: Option<String>,
    pub last_used_at: Option<u64>,
}
impl Connection {
    pub fn validate(&self) -> AppResult<()> {
        if self.id.is_empty()
            || self.name.trim().is_empty()
            || self.host.trim().is_empty()
            || self.user.trim().is_empty()
            || self.port == 0
        {
            return Err(AppError::new(
                "E_INPUT",
                "connection",
                "Name, host, port and user are required",
            ));
        }
        if !["password", "key", "agent"].contains(&self.auth_method.as_str())
            || self.host.chars().any(char::is_whitespace)
        {
            return Err(AppError::new(
                "E_INPUT",
                "connection",
                "Invalid connection settings",
            ));
        }
        if self.auth_method == "key" && self.key_path.as_ref().is_none_or(|p| p.is_empty()) {
            return Err(AppError::new(
                "E_INPUT",
                "connection",
                "Private key path is required",
            ));
        }
        if self
            .agent_kind
            .as_deref()
            .is_some_and(|k| k != "openssh" && k != "pageant")
        {
            return Err(AppError::new("E_INPUT", "connection", "Unknown SSH agent"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    pub connection_id: String,
    pub state: String,
    pub fingerprint: Option<String>,
    pub previous_fingerprint: Option<String>,
    pub key_type: Option<String>,
    pub error: Option<String>,
}
impl ConnectionStatus {
    pub fn new(id: &str, state: &str) -> Self {
        Self {
            connection_id: id.into(),
            state: state.into(),
            fingerprint: None,
            previous_fingerprint: None,
            key_type: None,
            error: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub connection_id: String,
    pub name: String,
    pub path: String,
    pub description: String,
    pub default_agent: String,
    pub default_model: String,
    pub permission_mode: String,
    pub pinned: bool,
    pub archived: bool,
    pub last_opened_at: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub theme: String,
    pub language: String,
    pub close_action: String,
    pub scrollback: u32,
    pub font_size: u32,
    pub notifications: NotificationSettings,
    pub shortcuts: ShortcutSettings,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationSettings {
    pub disconnected: bool,
    pub completed: bool,
    pub approval: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ShortcutSettings {
    pub new_terminal: String,
    pub save_file: String,
    pub focus_dock: String,
    pub close_tab: String,
    pub next_tab: String,
    pub previous_tab: String,
    pub split: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            language: "zh-CN".into(),
            close_action: "ask".into(),
            scrollback: 10000,
            font_size: 14,
            notifications: NotificationSettings::default(),
            shortcuts: ShortcutSettings::default(),
        }
    }
}
impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            disconnected: true,
            completed: true,
            approval: true,
        }
    }
}
impl Default for ShortcutSettings {
    fn default() -> Self {
        Self {
            new_terminal: "Ctrl+Shift+T".into(),
            save_file: "Ctrl+S".into(),
            focus_dock: "Ctrl+Shift+F".into(),
            close_tab: "Ctrl+Shift+W".into(),
            next_tab: "Ctrl+PageDown".into(),
            previous_tab: "Ctrl+PageUp".into(),
            split: "Ctrl+Alt+S".into(),
        }
    }
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
