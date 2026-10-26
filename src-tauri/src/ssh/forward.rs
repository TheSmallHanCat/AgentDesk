use crate::error::{AppError, AppResult};
use russh::{
    client::{Handle, Handler, Msg},
    Channel,
};

pub fn validate_socket(path: &str) -> AppResult<()> {
    if !path.starts_with('/')
        || path.contains('\0')
        || path.len() > 100
        || path.split('/').any(|p| p == "..")
    {
        return Err(AppError::new(
            "E_INPUT",
            "streamlocal",
            "A normalized absolute Unix socket path of at most 100 bytes is required",
        ));
    }
    Ok(())
}
pub async fn direct<H: Handler>(handle: &Handle<H>, socket: &str) -> AppResult<Channel<Msg>> {
    validate_socket(socket)?;
    handle
        .channel_open_direct_streamlocal(socket)
        .await
        .map_err(|e| AppError::io("direct-streamlocal", e))
}
pub async fn reverse<H: Handler>(handle: &Handle<H>, socket: &str) -> AppResult<()> {
    validate_socket(socket)?;
    handle
        .streamlocal_forward(socket)
        .await
        .map_err(|e| AppError::io("streamlocal-forward", e))
}
pub async fn cancel_reverse<H: Handler>(handle: &Handle<H>, socket: &str) -> AppResult<()> {
    validate_socket(socket)?;
    handle
        .cancel_streamlocal_forward(socket)
        .await
        .map_err(|e| AppError::io("cancel-streamlocal-forward", e))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_socket_paths() {
        for path in ["host.sock", "/a/../socket", "/a\0b"] {
            assert!(validate_socket(path).is_err());
        }
        assert!(validate_socket("/tmp/agentdesk/host.sock").is_ok());
        assert!(validate_socket(&format!("/{}", "a".repeat(100))).is_err());
    }
}
