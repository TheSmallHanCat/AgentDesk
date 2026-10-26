use crate::error::{AppError, AppResult};
use russh::keys::agent::client::{AgentClient, AgentStream};

pub mod executor;

pub async fn agent(kind: &str) -> AppResult<AgentClient<Box<dyn AgentStream + Send + Unpin>>> {
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        if kind == "pageant" {
            AgentClient::connect_pageant().await.map(|a| a.dynamic())
        } else {
            AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")
                .await
                .map(|a| a.dynamic())
        }
    })
    .await;
    match result {
        Ok(Ok(a)) => Ok(a),
        _ => Err(AppError::new(
            "E_AGENT",
            "authentication",
            "SSH agent is unavailable; start the selected agent and load a key",
        )),
    }
}

pub mod paths;
