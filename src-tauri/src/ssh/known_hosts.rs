use crate::error::{AppError, AppResult};
use russh::keys::{
    known_hosts::{known_host_keys_path, learn_known_hosts_path},
    Algorithm, HashAlg, PublicKey,
};
use std::{
    io::Write,
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
        self.check_locked(host, port, key)
    }
    fn keys_locked(&self, host: &str, port: u16) -> AppResult<Vec<PublicKey>> {
        let mut known = Vec::new();
        for file in std::iter::once(&self.path).chain(self.system.iter()) {
            if !file.exists() {
                continue;
            }
            let text = std::fs::read_to_string(file).map_err(|e| AppError::io("host-key", e))?;
            // russh 0.65 splits on literal spaces and retains CRLF. Normalize only
            // ordinary entries; keep its hostname hashing and matching semantics.
            let mut normalized =
                tempfile::NamedTempFile::new().map_err(|e| AppError::io("host-key", e))?;
            for line in text.lines() {
                let fields: Vec<_> = line.split_whitespace().collect();
                if fields.len() < 3 || fields[0].starts_with(['#', '@']) {
                    continue;
                }
                writeln!(normalized, "{} {} {}", fields[0], fields[1], fields[2])
                    .map_err(|e| AppError::io("host-key", e))?;
            }
            normalized
                .flush()
                .map_err(|e| AppError::io("host-key", e))?;
            known.extend(
                known_host_keys_path(host, port, normalized.path())
                    .map_err(|e| AppError::io("host-key", e))?
                    .into_iter()
                    .map(|(_, key)| key),
            );
        }
        Ok(known)
    }
    fn check_locked(&self, host: &str, port: u16, key: &PublicKey) -> AppResult<KeyDecision> {
        for file in std::iter::once(&self.path).chain(self.system.iter()) {
            reject_revoked_key(file, key)?;
        }
        let known: Vec<_> = self
            .keys_locked(host, port)?
            .into_iter()
            .filter(|k| same_algorithm(&k.algorithm(), &key.algorithm()))
            .collect();
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
    pub fn preferred(&self, host: &str, port: u16) -> AppResult<russh::Preferred> {
        let _guard = self.lock.lock().map_err(|e| AppError::io("host-key", e))?;
        let known = self.keys_locked(host, port)?;
        let mut preferred = russh::Preferred::default();
        let mut algorithms = preferred.key.to_vec();
        algorithms.sort_by_key(|a| !known.iter().any(|k| same_algorithm(&k.algorithm(), a)));
        preferred.key = algorithms.into();
        Ok(preferred)
    }
    pub fn trust(&self, host: &str, port: u16, key: &PublicKey) -> AppResult<()> {
        let _guard = self.lock.lock().map_err(|e| AppError::io("host-key", e))?;
        match self.check_locked(host, port, key)? {
            KeyDecision::Trusted => Ok(()),
            KeyDecision::Unknown => learn_known_hosts_path(host, port, key, &self.path)
                .map_err(|e| AppError::io("host-key", e)),
            KeyDecision::Changed(_) => Err(AppError::new(
                "E_HOST_KEY",
                "host-key",
                "Only a previously unknown host key may be trusted",
            )),
        }
    }
}
fn same_algorithm(a: &Algorithm, b: &Algorithm) -> bool {
    a == b || (matches!(a, Algorithm::Rsa { .. }) && matches!(b, Algorithm::Rsa { .. }))
}
fn reject_revoked_key(path: &Path, key: &PublicKey) -> AppResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let text = std::fs::read_to_string(path).map_err(|e| AppError::io("host-key", e))?;
    for line in text
        .lines()
        .filter(|l| l.split_whitespace().next() == Some("@revoked"))
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
    #[test]
    fn review_crlf_and_tabs_are_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let public = key();
        let openssh = public.to_openssh().unwrap();
        std::fs::write(
            &path,
            format!("example\t{}\r\n", openssh.replace(' ', "\t")),
        )
        .unwrap();
        let hosts = KnownHosts::new(path, None);
        assert!(matches!(
            hosts.check("example", 22, &public).unwrap(),
            KeyDecision::Trusted
        ));
    }
    #[test]
    fn review_another_algorithm_requires_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let hosts = KnownHosts::new(dir.path().join("known_hosts"), None);
        let ecdsa = russh::keys::PrivateKey::random(
            &mut rand::rng(),
            russh::keys::Algorithm::Ecdsa {
                curve: russh::keys::ssh_key::EcdsaCurve::NistP256,
            },
        )
        .unwrap();
        hosts.trust("example", 22, ecdsa.public_key()).unwrap();
        assert!(matches!(
            hosts.check("example", 22, &key()).unwrap(),
            KeyDecision::Unknown
        ));
    }
    #[test]
    fn review_concurrent_trust_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let hosts = std::sync::Arc::new(KnownHosts::new(path.clone(), None));
        let public = key();
        std::thread::scope(|scope| {
            for _ in 0..16 {
                let hosts = hosts.clone();
                let public = &public;
                scope.spawn(move || hosts.trust("example", 22, public).unwrap());
            }
        });
        assert_eq!(
            std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .filter(|l| !l.is_empty())
                .count(),
            1
        );
    }
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
