use crate::error::{AppError, AppResult};
use zeroize::Zeroizing;

pub struct Secrets;
impl Secrets {
    fn entry(id: &str, kind: &str) -> AppResult<keyring::Entry> {
        keyring::Entry::new("dev.agentdesk.desktop", &format!("agentdesk:{id}:{kind}")).map_err(
            |_| {
                AppError::new(
                    "E_KEYRING",
                    "credentials",
                    "Operating system credential store unavailable",
                )
            },
        )
    }
    pub fn get(id: &str, kind: &str) -> AppResult<Option<Zeroizing<String>>> {
        match Self::entry(id, kind)?.get_password() {
            Ok(s) => Ok(Some(Zeroizing::new(s))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(AppError::new(
                "E_KEYRING",
                "credentials",
                "Saved credential cannot be read; enter it again",
            )),
        }
    }
    pub fn set(id: &str, kind: &str, value: &str) -> AppResult<()> {
        Self::entry(id, kind)?
            .set_password(value)
            .map_err(|_| AppError::new("E_KEYRING", "credentials", "Credential could not be saved"))
    }
    pub fn clear(id: &str, kind: &str) -> AppResult<()> {
        match Self::entry(id, kind)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(AppError::new(
                "E_KEYRING",
                "credentials",
                "Credential could not be removed",
            )),
        }
    }
}
