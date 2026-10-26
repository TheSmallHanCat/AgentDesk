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
