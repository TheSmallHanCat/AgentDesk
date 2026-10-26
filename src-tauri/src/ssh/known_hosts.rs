use crate::error::{AppError, AppResult};
use russh::keys::{
    known_hosts::{known_host_keys_path, learn_known_hosts_path},
    HashAlg, PublicKey,
};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

pub enum KeyDecision {
    Trusted,
    Unknown,
    Changed(String),
}
pub struct KnownHosts {
    path: PathBuf,
    system: Option<PathBuf>,
    lock: Mutex<()>,
}
impl KnownHosts {
    pub fn new(path: PathBuf, system: Option<PathBuf>) -> Self {
        Self {
            path,
            system,
            lock: Mutex::new(()),
        }
    }
    pub fn check(&self, host: &str, port: u16, key: &PublicKey) -> AppResult<KeyDecision> {
        let _guard = self.lock.lock().map_err(|e| AppError::io("host-key", e))?;
        let mut known = Vec::new();
        for file in std::iter::once(&self.path).chain(self.system.iter()) {
            reject_revoked_key(file, key)?;
            known.extend(
                known_host_keys_path(host, port, file)
                    .map_err(|e| AppError::io("host-key", e))?
                    .into_iter()
                    .map(|(_, k)| k),
            );
        }
        if known.iter().any(|k| k == key) {
            return Ok(KeyDecision::Trusted);
        }
        if known.is_empty() {
            return Ok(KeyDecision::Unknown);
        }
        Ok(KeyDecision::Changed(
            known
                .iter()
                .map(|k| k.fingerprint(HashAlg::Sha256).to_string())
                .collect::<Vec<_>>()
                .join(", "),
        ))
    }
    pub fn trust(&self, host: &str, port: u16, key: &PublicKey) -> AppResult<()> {
        if !matches!(self.check(host, port, key)?, KeyDecision::Unknown) {
            return Err(AppError::new(
                "E_HOST_KEY",
                "host-key",
                "Only a previously unknown host key may be trusted",
            ));
        }
        let _guard = self.lock.lock().map_err(|e| AppError::io("host-key", e))?;
        learn_known_hosts_path(host, port, key, &self.path).map_err(|e| AppError::io("host-key", e))
    }
}
fn reject_revoked_key(path: &Path, key: &PublicKey) -> AppResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let text = std::fs::read_to_string(path).map_err(|e| AppError::io("host-key", e))?;
    for line in text
        .lines()
        .filter(|l| l.trim_start().starts_with("@revoked "))
    {
        let mut fields = line.split_whitespace();
        fields.next();
        fields.next();
        if let (Some(algorithm), Some(data)) = (fields.next(), fields.next()) {
            if PublicKey::from_openssh(&format!("{algorithm} {data}")).is_ok_and(|k| k == *key) {
                return Err(AppError::new(
                    "E_HOST_KEY_REVOKED",
                    "host-key",
                    "Server key is revoked in known_hosts",
                ));
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn key() -> PublicKey {
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
            .unwrap()
            .public_key()
            .clone()
    }
    #[test]
    fn unknown_trust_and_change_are_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let hosts = KnownHosts::new(dir.path().join("known_hosts"), None);
        let a = key();
        let b = key();
        assert!(matches!(
            hosts.check("example", 22, &a).unwrap(),
            KeyDecision::Unknown
        ));
        hosts.trust("example", 22, &a).unwrap();
        assert!(matches!(
            hosts.check("example", 22, &a).unwrap(),
            KeyDecision::Trusted
        ));
        assert!(matches!(
            hosts.check("example", 22, &b).unwrap(),
            KeyDecision::Changed(_)
        ));
        assert!(hosts.trust("example", 22, &b).is_err());
    }
}
