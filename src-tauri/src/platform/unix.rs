use crate::error::{AppError, AppResult};
use russh::keys::agent::client::{AgentClient, AgentStream};
pub async fn agent(kind: &str) -> AppResult<AgentClient<Box<dyn AgentStream + Send + Unpin>>> {
    if kind == "pageant" {
        return Err(AppError::new(
            "E_AGENT",
            "authentication",
            "Pageant is only available on Windows",
        ));
    }
    AgentClient::connect_env()
        .await
        .map(|a| a.dynamic())
        .map_err(|_| AppError::new("E_AGENT", "authentication", "SSH_AUTH_SOCK is unavailable"))
}

pub fn path_within(path: &std::path::Path, root: &std::path::Path) -> bool {
    path.starts_with(root)
}
pub fn local_system_roots() -> Vec<std::path::PathBuf> {
    [
        "/bin", "/sbin", "/usr", "/etc", "/lib", "/boot", "/dev", "/proc", "/sys",
    ]
    .iter()
    .map(std::path::PathBuf::from)
    .collect()
}

pub fn display_local_path(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

pub fn validate_local_path(_path: &std::path::Path) -> crate::error::AppResult<()> {
    Ok(())
}
