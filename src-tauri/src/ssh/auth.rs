use crate::{
    error::{AppError, AppResult},
    models::Connection,
    platform,
};
use russh::{
    client::{Handle, Handler},
    keys::{load_secret_key, PrivateKeyWithHashAlg},
};
use std::sync::Arc;

pub async fn authenticate<H: Handler>(
    session: &mut Handle<H>,
    connection: &Connection,
    credential: Option<&str>,
) -> AppResult<()> {
    let result = match connection.auth_method.as_str() {
        "password" => session
            .authenticate_password(
                &connection.user,
                credential.ok_or_else(|| {
                    AppError::new(
                        "E_CREDENTIAL_REQUIRED",
                        "authentication",
                        "Enter the SSH password",
                    )
                })?,
            )
            .await
            .map_err(|_| {
                AppError::new(
                    "E_AUTH_TRANSPORT",
                    "authentication",
                    "SSH authentication exchange failed",
                )
            })?
            .success(),
        "key" => {
            let path = connection.key_path.as_ref().ok_or_else(|| {
                AppError::new("E_INPUT", "authentication", "Private key path is required")
            })?;
            let key =
                load_secret_key(path, credential.filter(|s| !s.is_empty())).map_err(|_| {
                    AppError::new(
                        "E_KEY",
                        "authentication",
                        "Private key cannot be loaded; check its format and passphrase",
                    )
                })?;
            let hash = session
                .best_supported_rsa_hash()
                .await
                .map_err(|e| AppError::io("authentication", e))?
                .flatten();
            session
                .authenticate_publickey(
                    &connection.user,
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await
                .map_err(|_| {
                    AppError::new(
                        "E_AUTH_TRANSPORT",
                        "authentication",
                        "SSH public key exchange failed",
                    )
                })?
                .success()
        }
        "agent" => {
            let mut agent =
                platform::agent(connection.agent_kind.as_deref().unwrap_or("openssh")).await?;
            let identities = agent.request_identities().await.map_err(|_| {
                AppError::new(
                    "E_AGENT",
                    "authentication",
                    "Could not read SSH agent identities",
                )
            })?;
            let hash = session
                .best_supported_rsa_hash()
                .await
                .map_err(|e| AppError::io("authentication", e))?
                .flatten();
            let mut accepted = false;
            for identity in identities.into_iter().take(32) {
                let key = match identity {
                    russh::keys::agent::AgentIdentity::PublicKey { key, .. } => key,
                    _ => continue,
                };
                if session
                    .authenticate_publickey_with(&connection.user, key, hash, &mut agent)
                    .await
                    .map_err(|_| {
                        AppError::new("E_AGENT", "authentication", "SSH agent signing failed")
                    })?
                    .success()
                {
                    accepted = true;
                    break;
                }
            }
            accepted
        }
        _ => {
            return Err(AppError::new(
                "E_INPUT",
                "authentication",
                "Unknown authentication method",
            ))
        }
    };
    if result {
        Ok(())
    } else {
        Err(AppError::new(
            "E_AUTH",
            "authentication",
            "SSH authentication was rejected",
        ))
    }
}
