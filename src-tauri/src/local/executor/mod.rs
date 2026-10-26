//! The local endpoint owns only connection-scoped resources; it never installs a service.

use crate::error::{AppError, AppResult};
use base64::Engine;
use serde::Serialize;
use std::path::{Path, PathBuf};
use zeroize::Zeroize;

#[cfg(windows)]
use crate::platform::executor::{self as windows, Job, LocalProcess};
#[cfg(windows)]
use std::{ffi::OsStr, time::Duration};

#[derive(Clone)]
pub struct ExecutorResources {
    pub openssh_directory: PathBuf,
    pub powershell: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentCheck {
    pub name: String,
    pub path: String,
    pub available: bool,
    pub error: Option<String>,
}

impl ExecutorResources {
    pub fn discover(resource_directory: &Path) -> Self {
        let candidates = [
            resource_directory.join("resources/windows"),
            resource_directory.join("windows"),
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../resources/windows"),
        ];
        let root = candidates
            .iter()
            .find(|path| path.is_dir())
            .unwrap_or(&candidates[0]);
        Self {
            openssh_directory: root.join("openssh"),
            powershell: root.join("powershell/pwsh.exe"),
        }
    }

    pub fn check(&self) -> Vec<ComponentCheck> {
        [
            ("OpenSSH server", self.openssh_directory.join("sshd.exe")),
            (
                "OpenSSH key generator",
                self.openssh_directory.join("ssh-keygen.exe"),
            ),
            (
                "OpenSSH SFTP",
                self.openssh_directory.join("sftp-server.exe"),
            ),
            ("PowerShell 7", self.powershell.clone()),
        ]
        .into_iter()
        .map(|(name, path)| {
            let error = match std::fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => None,
                Ok(_) => Some("Expected a regular executable file".into()),
                Err(error) => Some(format!(
                    "{}; prepare bundled components or inspect security software",
                    error
                )),
            };
            ComponentCheck {
                name: name.into(),
                path: path.to_string_lossy().into_owned(),
                available: error.is_none(),
                error,
            }
        })
        .collect()
    }

    pub async fn probe(&self) -> Vec<ComponentCheck> {
        let mut checks = self.check();
        #[cfg(windows)]
        {
            let sshd = self.openssh_directory.join("sshd.exe");
            let sshd_arguments = ["-V".into()];
            let powershell_arguments = [
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "$PSVersionTable.PSVersion.ToString()".into(),
            ];
            let inspect = async {
                if checks[0].available {
                    run_component_bounded(&sshd, &sshd_arguments, Duration::from_secs(6)).await
                } else {
                    Ok(())
                }
            };
            let inspect_powershell = async {
                if checks[3].available {
                    run_component_bounded(
                        &self.powershell,
                        &powershell_arguments,
                        Duration::from_secs(6),
                    )
                    .await
                } else {
                    Ok(())
                }
            };
            let (sshd, powershell) = tokio::join!(inspect, inspect_powershell);
            for (index, result) in [(0, sshd), (3, powershell)] {
                if let Err(error) = result {
                    checks[index].available = false;
                    checks[index].error = Some(error.message);
                }
            }
        }
        #[cfg(not(windows))]
        for component in &mut checks {
            component.available = false;
            component.error = Some("The bundled local executor requires Windows".into());
        }
        checks
    }

    fn require(&self) -> AppResult<()> {
        if let Some(component) = self
            .check()
            .into_iter()
            .find(|component| !component.available)
        {
            return Err(AppError::new(
                "E_LOCAL_COMPONENT",
                "local.executor",
                format!(
                    "{}: {}",
                    component.path,
                    component.error.unwrap_or_default()
                ),
            ));
        }
        Ok(())
    }
}

/// This value must only cross the authenticated control channel; never log or emit it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorCredentials {
    pub user: String,
    pub private_key: String,
    pub host_public_key: String,
    pub script_command: String,
}

impl Drop for ExecutorCredentials {
    fn drop(&mut self) {
        self.private_key.zeroize();
    }
}

pub struct LocalExecutor {
    port: u16,
    credentials: ExecutorCredentials,
    #[cfg(windows)]
    job: Option<Job>,
    #[cfg(windows)]
    process: Option<LocalProcess>,
    runtime: Option<tempfile::TempDir>,
}

pub fn script_command() -> String {
    let bytes: Vec<u8> = include_str!("runner.ps1")
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    // RemoteCommand expands percent tokens before Windows cmd expands environment variables.
    format!(
        "\"%%AGENTDESK_EXECUTOR_PWSH%%\" -NoLogo -NoProfile -NonInteractive -EncodedCommand {}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

impl LocalExecutor {
    #[cfg(windows)]
    pub async fn start(resources: ExecutorResources) -> AppResult<Self> {
        resources.require()?;
        let runtime = tempfile::Builder::new()
            .prefix("agentdesk-executor-")
            .tempdir()
            .map_err(|error| AppError::io("local.executor", error))?;
        windows::protect_directory(runtime.path())?;
        let account = windows::current_account()?;
        if account.contains(['\r', '\n', '\0', '"']) {
            return Err(AppError::new(
                "E_LOCAL_USER",
                "local.executor",
                "Current Windows account cannot be represented safely in sshd configuration",
            ));
        }
        let user = account.rsplit('\\').next().unwrap_or(&account).to_owned();
        let keygen = resources.openssh_directory.join("ssh-keygen.exe");
        for name in ["host", "identity"] {
            let path = runtime.path().join(name);
            run_component(
                &keygen,
                &[
                    "-q".into(),
                    "-t".into(),
                    "ed25519".into(),
                    "-N".into(),
                    "".into(),
                    "-f".into(),
                    path.to_string_lossy().into_owned(),
                ],
            )
            .await?;
        }
        let public_key = std::fs::read_to_string(runtime.path().join("identity.pub"))
            .map_err(|error| AppError::io("local.executor", error))?;
        std::fs::write(runtime.path().join("authorized_keys"), public_key.trim())
            .map_err(|error| AppError::io("local.executor", error))?;
        let private_key = std::fs::read_to_string(runtime.path().join("identity"))
            .map_err(|error| AppError::io("local.executor", error))?;
        let host_public_key = std::fs::read_to_string(runtime.path().join("host.pub"))
            .map_err(|error| AppError::io("local.executor", error))?
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        let listener =
            std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(|error| {
                AppError::new(
                    "E_LOCAL_PORT",
                    "local.executor",
                    format!("Cannot allocate 127.0.0.1 loopback port: {}", error),
                )
            })?;
        let port = listener
            .local_addr()
            .map_err(|error| AppError::io("local.executor", error))?
            .port();
        let runtime_path = config_path(runtime.path())?;
        let openssh_path = config_path(&resources.openssh_directory)?;
        let allowed = if account == user
            || account.starts_with(&format!(
                "{}\\",
                std::env::var("COMPUTERNAME")
                    .unwrap_or_default()
                    .to_lowercase()
            )) {
            format!("\"{}\"", user)
        } else {
            format!("\"{}\" \"{}\"", account, user)
        };
        let configuration = format!(
            "Port {port}\nListenAddress 127.0.0.1\nAddressFamily inet\nHostKey \"{runtime_path}/host\"\nAuthorizedKeysFile \"{runtime_path}/authorized_keys\"\nPidFile \"{runtime_path}/sshd.pid\"\nPubkeyAuthentication yes\nAuthenticationMethods publickey\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nAllowUsers {allowed}\nAllowTcpForwarding no\nAllowAgentForwarding no\nPermitTunnel no\nX11Forwarding no\nPermitUserEnvironment no\nMaxAuthTries 3\nMaxStartups 64\nMaxSessions 128\nSubsystem sftp \"{openssh_path}/sftp-server.exe\"\nLogLevel ERROR\n"
        );
        let configuration_path = runtime.path().join("sshd_config");
        std::fs::write(&configuration_path, configuration)
            .map_err(|error| AppError::io("local.executor", error))?;
        let sshd = resources.openssh_directory.join("sshd.exe");
        run_component(
            &sshd,
            &[
                "-t".into(),
                "-f".into(),
                configuration_path.to_string_lossy().into_owned(),
            ],
        )
        .await?;
        // Validate PowerShell itself before offering a server entry point to the Host.
        run_component(
            &resources.powershell,
            &[
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "exit 0".into(),
            ],
        )
        .await?;
        let job = Job::new()?;
        drop(listener);
        let process = job.spawn(
            &sshd,
            &[
                "-D".into(),
                "-E".into(),
                runtime
                    .path()
                    .join("sshd.errors")
                    .to_string_lossy()
                    .into_owned(),
                "-f".into(),
                configuration_path.to_string_lossy().into_owned(),
            ],
            &[
                ("SSH_TEST_ENVIRONMENT", OsStr::new("1")),
                ("AGENTDESK_EXECUTOR_PWSH", resources.powershell.as_os_str()),
            ],
        )?;
        let mut executor = Self {
            port,
            credentials: ExecutorCredentials {
                user,
                private_key,
                host_public_key,
                script_command: script_command(),
            },
            job: Some(job),
            process: Some(process),
            runtime: Some(runtime),
        };
        if let Err(error) = executor.wait_listening(&sshd).await {
            executor.shutdown();
            return Err(error);
        }
        Ok(executor)
    }

    #[cfg(not(windows))]
    pub async fn start(resources: ExecutorResources) -> AppResult<Self> {
        resources.require()?;
        Err(AppError::new(
            "E_PLATFORM",
            "local.executor",
            "The bundled local execution endpoint requires Windows",
        ))
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn credentials(&self) -> &ExecutorCredentials {
        &self.credentials
    }

    pub fn is_running(&self) -> bool {
        #[cfg(windows)]
        {
            self.process
                .as_ref()
                .is_some_and(|process| matches!(process.exit_code(), Ok(None)))
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    pub fn runtime_directory(&self) -> Option<&Path> {
        self.runtime.as_ref().map(tempfile::TempDir::path)
    }

    #[cfg(windows)]
    pub fn process_id(&self) -> Option<u32> {
        self.process.as_ref().map(|process| process.id)
    }

    #[cfg(windows)]
    async fn wait_listening(&self, sshd: &Path) -> AppResult<()> {
        use tokio::io::AsyncReadExt;
        for _ in 0..100 {
            if let Some(code) = self
                .process
                .as_ref()
                .expect("started executor has a process")
                .exit_code()?
            {
                let diagnostic = self
                    .runtime
                    .as_ref()
                    .and_then(|runtime| std::fs::read(runtime.path().join("sshd.errors")).ok())
                    .unwrap_or_default();
                let tail = &diagnostic[diagnostic.len().saturating_sub(2048)..];
                return Err(AppError::new(
                    "E_LOCAL_SSHD",
                    "local.executor",
                    format!(
                        "{} exited ({}), loopback port {}: {}",
                        sshd.display(),
                        code,
                        self.port,
                        String::from_utf8_lossy(tail).trim()
                    ),
                ));
            }
            let probe = async {
                let mut socket =
                    tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, self.port))
                        .await?;
                let mut banner = [0; 8];
                socket.read_exact(&mut banner).await?;
                Ok::<bool, std::io::Error>(banner.starts_with(b"SSH-2.0-"))
            };
            if matches!(
                tokio::time::timeout(Duration::from_millis(200), probe).await,
                Ok(Ok(true))
            ) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err(AppError::new("E_LOCAL_SSHD", "local.executor", format!("{} did not open 127.0.0.1:{}; inspect security software and loopback port availability", sshd.display(), self.port)))
    }

    pub async fn stop(&mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        #[cfg(windows)]
        {
            if let Some(job) = self.job.take() {
                job.terminate();
                drop(job);
            }
            if let Some(process) = self.process.take() {
                process.wait(Duration::from_secs(5));
            }
        }
        self.credentials.private_key.zeroize();
        if let Some(runtime) = self.runtime.take() {
            let _ = runtime.close();
        }
    }
}

impl Drop for LocalExecutor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(windows)]
fn config_path(path: &Path) -> AppResult<String> {
    let canonical = path.canonicalize().map_err(|error| {
        AppError::new(
            "E_LOCAL_PATH",
            "local.executor",
            format!("{}: {}", path.display(), error),
        )
    })?;
    let text = canonical.to_string_lossy();
    let text = if let Some(network_path) = text.strip_prefix(r"\\?\UNC\") {
        format!("//{}", network_path.replace('\\', "/"))
    } else {
        text.strip_prefix(r"\\?\")
            .unwrap_or(&text)
            .replace('\\', "/")
    };
    if text.contains(['\r', '\n', '\0', '"']) {
        return Err(AppError::new(
            "E_LOCAL_PATH",
            "local.executor",
            "Component path contains unsupported configuration characters",
        ));
    }
    Ok(text)
}

#[cfg(windows)]
async fn run_component(path: &Path, arguments: &[String]) -> AppResult<()> {
    run_component_bounded(path, arguments, Duration::from_secs(20)).await
}

#[cfg(windows)]
async fn run_component_bounded(
    path: &Path,
    arguments: &[String],
    limit: Duration,
) -> AppResult<()> {
    let mut command = windows::hidden_command(path);
    command.args(arguments);
    let mut command = tokio::process::Command::from(command);
    command.kill_on_drop(true);
    let output = tokio::time::timeout(limit, command.output())
        .await
        .map_err(|_| {
            AppError::new(
                "E_LOCAL_COMPONENT",
                "local.executor",
                format!("{} timed out; inspect security software", path.display()),
            )
        })?
        .map_err(|error| {
            AppError::new(
                "E_LOCAL_COMPONENT",
                "local.executor",
                format!(
                    "{}: {}; inspect component files and security software",
                    path.display(),
                    error
                ),
            )
        })?;
    if !output.status.success() {
        let error = &output.stderr[output.stderr.len().saturating_sub(2048)..];
        return Err(AppError::new(
            "E_LOCAL_COMPONENT",
            "local.executor",
            format!(
                "{} exited {}: {}",
                path.display(),
                output.status,
                String::from_utf8_lossy(error).trim()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_reads_a_complete_script_and_uses_plain_utf8() {
        let command = script_command();
        let encoded = command.split_whitespace().last().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let source = String::from_utf16(
            &bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(source.contains("[Console]::In.ReadToEnd()"));
        assert!(source.contains("[ScriptBlock]::Create($source)"));
        assert!(source.contains("exit $global:LASTEXITCODE"));
        assert!(source.contains("OutputRendering = 'PlainText'"));
        assert!(!command.contains("-Command -"));
        assert!(!command.contains("-File -"));
    }

    #[test]
    fn missing_component_reports_the_actual_file() {
        let temporary = tempfile::tempdir().unwrap();
        let resources = ExecutorResources::discover(temporary.path());
        let resources = ExecutorResources {
            openssh_directory: temporary.path().join("openssh"),
            powershell: resources.powershell,
        };
        let result = resources.require().unwrap_err();
        assert_eq!(result.code, "E_LOCAL_COMPONENT");
        assert!(result.message.contains("sshd.exe"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn probe_rejects_an_existing_nonexecutable_component() {
        let directory = tempfile::tempdir().unwrap();
        let openssh_directory = directory.path().join("openssh");
        std::fs::create_dir(&openssh_directory).unwrap();
        for name in ["sshd.exe", "ssh-keygen.exe", "sftp-server.exe"] {
            std::fs::write(openssh_directory.join(name), b"invalid executable").unwrap();
        }
        let powershell = directory.path().join("pwsh.exe");
        std::fs::write(&powershell, b"invalid executable").unwrap();
        let resources = ExecutorResources {
            openssh_directory,
            powershell,
        };
        assert!(resources
            .check()
            .iter()
            .all(|component| component.available));
        let checked = resources.probe().await;
        assert!(!checked[0].available);
        assert!(!checked[3].available);
        assert!(checked[0].error.as_deref().unwrap().contains("sshd.exe"));
        assert!(checked[3].error.as_deref().unwrap().contains("pwsh.exe"));
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "Requires python scripts/build_windows_resources.py; starts an isolated loopback SSH server"]
    async fn executor_real_multiline_utf8_exit_and_cleanup() {
        use std::os::windows::process::CommandExt;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let resources = ExecutorResources::discover(root);
        assert!(resources
            .probe()
            .await
            .iter()
            .all(|component| component.available));
        let mut executor = LocalExecutor::start(resources.clone()).await.unwrap();
        let runtime = executor.runtime_directory().unwrap().to_path_buf();
        let project = root.join(".tmp/e2e/local-project/executor-smoke");
        std::fs::create_dir_all(&project).unwrap();
        let known_hosts = runtime.join("known_hosts");
        std::fs::write(
            &known_hosts,
            format!(
                "agentdesk-local {}\n",
                executor.credentials().host_public_key
            ),
        )
        .unwrap();
        let config = runtime.join("client.conf");
        std::fs::write(&config, format!(
            "Host device-script\n HostName 127.0.0.1\n Port {}\n User \"{}\"\n HostKeyAlias agentdesk-local\n IdentityFile \"{}\"\n UserKnownHostsFile \"{}\"\n StrictHostKeyChecking yes\n IdentitiesOnly yes\n BatchMode yes\n RequestTTY no\n RemoteCommand {}\n",
            executor.port(), executor.credentials().user,
            runtime.join("identity").to_string_lossy().replace('\\', "/"),
            known_hosts.to_string_lossy().replace('\\', "/"),
            executor.credentials().script_command,
        )).unwrap();
        let client = resources.openssh_directory.join("ssh.exe");
        let mut command = std::process::Command::new(&client);
        command
            .creation_flags(0x0800_0000)
            .args(["-F", config.to_str().unwrap(), "device-script"]);
        let mut command = tokio::process::Command::from(command);
        command
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().unwrap();
        let file = project
            .join("result.txt")
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "''");
        let source = format!("$values = @(\n  '第一行',\n  '第二行'\n)\nif ($values.Count -eq 2) {{\n  Set-Content -LiteralPath '{file}' -Value ($values -join '|') -Encoding utf8NoBOM\n}}\nAdd-Content -LiteralPath '{file}' -Value '已修改' -Encoding utf8NoBOM\nWrite-Output '中文执行成功'\nexit 7\n");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(source.as_bytes())
            .await
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(7),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("中文执行成功"));
        assert_eq!(
            std::fs::read_to_string(project.join("result.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "第一行|第二行\n已修改\n"
        );

        // Keep PowerShell alive, then verify closing the owner also closes its SSH session.
        let mut sleeper = command.spawn().unwrap();
        sleeper
            .stdin
            .take()
            .unwrap()
            .write_all(b"Write-Output $PID\nStart-Sleep 60\n")
            .await
            .unwrap();
        let mut output = BufReader::new(sleeper.stdout.take().unwrap());
        let mut pid = String::new();
        tokio::time::timeout(Duration::from_secs(10), output.read_line(&mut pid))
            .await
            .unwrap()
            .unwrap();
        assert!(pid.trim().parse::<u32>().is_ok());
        let port = executor.port();
        executor.stop().await;
        assert!(
            !runtime.exists(),
            "Connection-scoped keys and runtime files must be removed"
        );
        assert!(
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .is_err()
        );
        let stopped = tokio::time::timeout(Duration::from_secs(10), sleeper.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(!stopped.success());

        let mut failed = LocalExecutor::start(resources).await.unwrap();
        assert!(failed.is_running());
        failed.job.as_ref().unwrap().terminate();
        for _ in 0..100 {
            if !failed.is_running() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            !failed.is_running(),
            "Unexpected sshd exit must revoke readiness"
        );
        failed.stop().await;
    }
}
