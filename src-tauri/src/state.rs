use crate::{
    db::Db,
    error::AppResult,
    session_log::Diagnostics,
    ssh::{known_hosts::KnownHosts, manager::SshManager},
    storage::Storage,
};
use std::sync::Arc;
pub struct AppState {
    pub db: Arc<Db>,
    pub storage: Storage,
    pub ssh: Arc<SshManager>,
    pub diagnostics: Arc<Diagnostics>,
    pub local_terminals: Arc<crate::local::pty::PtyManager>,
    pub terminals: Arc<crate::ssh::pty::PtyManager>,
    pub transfers: Arc<crate::ssh::transfer::TransferManager>,
    pub hosts: Arc<crate::host::client::HostPool>,
    pub local_channels: Arc<crate::local::channels::LocalChannels>,
    pub installer: Arc<crate::host::installer::Installer>,
}
impl AppState {
    pub fn new(app: tauri::AppHandle) -> AppResult<Self> {
        let storage = Storage::new()?;
        let db = Arc::new(Db::open(&storage.root.join("desktop.sqlite"))?);
        let diagnostics = Arc::new(Diagnostics::new(&storage.root));
        let system = directories::BaseDirs::new().map(|b| b.home_dir().join(".ssh/known_hosts"));
        let known = Arc::new(KnownHosts::new(storage.root.join("known_hosts"), system));
        let ssh = Arc::new(SshManager::new(
            db.clone(),
            known,
            diagnostics.clone(),
            Some(app),
        ));
        let hosts = Arc::new(crate::host::client::HostPool::new(db.device_id()?));
        Ok(Self {
            db,
            storage,
            ssh,
            diagnostics,
            terminals: Arc::default(),
            local_terminals: Arc::default(),
            transfers: Arc::default(),
            hosts,
            installer: Arc::default(),
            local_channels: Arc::default(),
        })
    }
}
