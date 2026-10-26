//! Windows-only ownership and access control for the connection's local SSH server.

use crate::error::{AppError, AppResult};
use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::ptr::{null, null_mut};
use std::time::Duration;
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, STILL_ACTIVE};
use windows_sys::Win32::Security::Authentication::Identity::{GetUserNameExW, NameSamCompatible};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, SetFileSecurityW, TokenUser, DACL_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, ResumeThread,
    TerminateProcess, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED,
    CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTUPINFOW,
};

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

/// Win32-OpenSSH reads the 64-bit machine key even in a private sshd instance.
/// Read it only; never change the system SSH server's shell configuration.
pub fn openssh_default_shell() -> AppResult<String> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY,
    };
    let mut buffer = vec![0u16; 32768];
    let mut bytes = (buffer.len() * 2) as u32;
    let code = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            wide(r"SOFTWARE\OpenSSH").as_ptr(),
            wide("DefaultShell").as_ptr(),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok("cmd.exe".into());
    }
    if code != ERROR_SUCCESS {
        return Err(AppError::new(
            "E_LOCAL_SHELL",
            "local.executor",
            format!("Cannot inspect OpenSSH DefaultShell (Windows error {code})"),
        ));
    }
    let count = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    Ok(String::from_utf16_lossy(&buffer[..count]))
}

pub fn force_command(powershell: &str, default_shell: &str, arguments: &str) -> AppResult<String> {
    if powershell.contains(['%', '"', '\r', '\n', '\0']) {
        return Err(AppError::new(
            "E_LOCAL_PATH",
            "local.executor",
            "PowerShell path contains unsupported command characters",
        ));
    }
    let shell = Path::new(default_shell)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    match shell.as_str() {
        "cmd.exe" | "" => Ok(format!("\"{powershell}\" {arguments}")),
        "pwsh.exe" | "powershell.exe" => Ok(format!(
            "& '{}' {arguments}; exit $LASTEXITCODE",
            powershell.replace('\'', "''")
        )),
        _ => Err(AppError::new(
            "E_LOCAL_SHELL",
            "local.executor",
            "OpenSSH DefaultShell must be cmd.exe, powershell.exe or pwsh.exe for local execution",
        )),
    }
}

pub fn hidden_command(executable: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(executable);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

pub fn component_exit_reason(code: u32) -> &'static str {
    use windows_sys::Win32::Foundation::{
        STATUS_ACCESS_DISABLED_BY_POLICY_DEFAULT, STATUS_ACCESS_DISABLED_BY_POLICY_OTHER,
        STATUS_ACCESS_DISABLED_BY_POLICY_PATH, STATUS_ACCESS_DISABLED_BY_POLICY_PUBLISHER,
        STATUS_DLL_INIT_FAILED, STATUS_DLL_NOT_FOUND, STATUS_INVALID_IMAGE_FORMAT,
    };
    match code as i32 {
        STATUS_DLL_NOT_FOUND => "A required component DLL is missing",
        STATUS_DLL_INIT_FAILED => "A component DLL failed to initialize",
        STATUS_INVALID_IMAGE_FORMAT => {
            "The executable or one of its DLLs has an incompatible format"
        }
        STATUS_ACCESS_DISABLED_BY_POLICY_DEFAULT
        | STATUS_ACCESS_DISABLED_BY_POLICY_OTHER
        | STATUS_ACCESS_DISABLED_BY_POLICY_PATH
        | STATUS_ACCESS_DISABLED_BY_POLICY_PUBLISHER => {
            "Windows application control policy blocked this component"
        }
        _ => "The component exited unsuccessfully",
    }
}

fn failure(operation: &str, path: &Path) -> AppError {
    AppError::new(
        "E_LOCAL_EXECUTOR",
        "local.executor",
        format!(
            "{}: {}: {}",
            operation,
            path.display(),
            std::io::Error::last_os_error()
        ),
    )
}

/// Quote one Windows process argument using the CommandLineToArgvW convention.
pub fn quote_argument(value: &str) -> String {
    let mut result = String::from("\"");
    let mut backslashes = 0;
    for character in value.chars() {
        if character == '\\' {
            backslashes += 1;
        } else {
            if character == '"' {
                result.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
            } else {
                result.extend(std::iter::repeat_n('\\', backslashes));
            }
            backslashes = 0;
            result.push(character);
        }
    }
    result.extend(std::iter::repeat_n('\\', backslashes * 2));
    result.push('"');
    result
}

pub fn current_account() -> AppResult<String> {
    let mut size = 0;
    // The first call reports the buffer size and does not read account credentials.
    unsafe { GetUserNameExW(NameSamCompatible, null_mut(), &mut size) };
    let mut buffer = vec![0u16; size as usize];
    if !unsafe { GetUserNameExW(NameSamCompatible, buffer.as_mut_ptr(), &mut size) } {
        return Err(failure(
            "Cannot identify the current Windows user",
            Path::new("Windows token"),
        ));
    }
    Ok(String::from_utf16_lossy(&buffer[..size as usize]).to_lowercase())
}

/// Remove inherited access before writing any key. Children inherit this private DACL.
pub fn protect_directory(path: &Path) -> AppResult<()> {
    unsafe {
        let mut raw_token = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token) == 0 {
            return Err(failure("Cannot inspect Windows token", path));
        }
        let token = OwnedHandle::from_raw_handle(raw_token);
        let mut size = 0;
        GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut size);
        // TOKEN_USER requires pointer alignment, so use usize rather than a byte buffer.
        let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        ) == 0
        {
            return Err(failure("Cannot inspect Windows user SID", path));
        }
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut sid = null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut sid) == 0 {
            return Err(failure("Cannot format Windows user SID", path));
        }
        let mut count = 0;
        while *sid.add(count) != 0 {
            count += 1;
        }
        let sid_text = String::from_utf16_lossy(std::slice::from_raw_parts(sid, count));
        LocalFree(sid.cast());
        let descriptor_text = wide(format!("D:P(A;OICI;FA;;;{})", sid_text));
        let mut descriptor = null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor_text.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        ) == 0
        {
            return Err(failure("Cannot create a private key directory ACL", path));
        }
        let success = SetFileSecurityW(
            wide(path).as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        );
        let error = if success == 0 {
            Some(failure("Cannot protect local key directory", path))
        } else {
            None
        };
        LocalFree(descriptor);
        error.map_or(Ok(()), Err)
    }
}

pub struct Job {
    handle: OwnedHandle,
}

pub struct LocalProcess {
    handle: OwnedHandle,
    pub id: u32,
}

impl Job {
    pub fn new() -> AppResult<Self> {
        unsafe {
            let raw = CreateJobObjectW(null(), null());
            if raw.is_null() {
                return Err(failure(
                    "Cannot create Windows Job Object",
                    Path::new("Job Object"),
                ));
            }
            let handle = OwnedHandle::from_raw_handle(raw);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            ) == 0
            {
                return Err(failure(
                    "Cannot configure Windows Job Object",
                    Path::new("Job Object"),
                ));
            }
            Ok(Self { handle })
        }
    }

    pub fn spawn(
        &self,
        executable: &Path,
        arguments: &[String],
        extra_environment: &[(&str, &OsStr)],
    ) -> AppResult<LocalProcess> {
        let mut command = quote_argument(&executable.to_string_lossy());
        for argument in arguments {
            command.push(' ');
            command.push_str(&quote_argument(argument));
        }
        let mut environment: Vec<(OsString, OsString)> = std::env::vars_os()
            .filter(|(key, _)| {
                let name = key.to_string_lossy().to_ascii_uppercase();
                !name.ends_with("_POSIX_FD_STATE")
                    && !name.starts_with("AGENTDESK_E2E_")
                    && !extra_environment
                        .iter()
                        .any(|(key, _)| name.eq_ignore_ascii_case(key))
            })
            .collect();
        environment.extend(
            extra_environment
                .iter()
                .map(|(key, value)| (OsString::from(key), value.to_os_string())),
        );
        environment.sort_by_key(|(key, _)| key.to_string_lossy().to_uppercase());
        let mut environment_block = Vec::new();
        for (key, value) in environment {
            environment_block.extend(key.encode_wide());
            environment_block.push('=' as u16);
            environment_block.extend(value.encode_wide());
            environment_block.push(0);
        }
        environment_block.push(0);
        let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        unsafe {
            if CreateProcessW(
                wide(executable).as_ptr(),
                wide(command).as_mut_ptr(),
                null(),
                null(),
                0,
                CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
                environment_block.as_ptr().cast(),
                null(),
                &startup,
                &mut process,
            ) == 0
            {
                return Err(failure(
                    "Cannot start bundled component (check security software)",
                    executable,
                ));
            }
            let handle = OwnedHandle::from_raw_handle(process.hProcess);
            // Assign before the first instruction, so descendants cannot escape during startup.
            if AssignProcessToJobObject(self.handle.as_raw_handle(), handle.as_raw_handle()) == 0 {
                let error = failure(
                    "Cannot assign bundled component to Windows Job Object",
                    executable,
                );
                TerminateProcess(handle.as_raw_handle(), 1);
                CloseHandle(process.hThread);
                return Err(error);
            }
            if ResumeThread(process.hThread) == u32::MAX {
                let error = failure("Cannot resume bundled component", executable);
                TerminateProcess(handle.as_raw_handle(), 1);
                CloseHandle(process.hThread);
                return Err(error);
            }
            CloseHandle(process.hThread);
            Ok(LocalProcess {
                handle,
                id: process.dwProcessId,
            })
        }
    }

    pub fn terminate(&self) {
        unsafe {
            TerminateJobObject(self.handle.as_raw_handle(), 1);
        }
    }

    pub fn assign_process(&self, handle: BorrowedHandle<'_>, path: &Path) -> AppResult<()> {
        if unsafe { AssignProcessToJobObject(self.handle.as_raw_handle(), handle.as_raw_handle()) }
            == 0
        {
            return Err(failure(
                "Cannot assign local terminal to Windows Job Object",
                path,
            ));
        }
        Ok(())
    }
}

impl LocalProcess {
    pub fn exit_code(&self) -> AppResult<Option<u32>> {
        let mut code = 0;
        if unsafe { GetExitCodeProcess(self.handle.as_raw_handle(), &mut code) } == 0 {
            return Err(failure(
                "Cannot inspect local sshd process",
                Path::new("sshd.exe"),
            ));
        }
        Ok((code != STILL_ACTIVE as u32).then_some(code))
    }

    pub fn wait(&self, timeout: Duration) {
        unsafe {
            WaitForSingleObject(
                self.handle.as_raw_handle() as HANDLE,
                timeout.as_millis().min(u32::MAX as u128) as u32,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_arguments_preserve_quotes_and_trailing_backslashes() {
        assert_eq!(
            quote_argument("C:\\some folder\\"),
            "\"C:\\some folder\\\\\""
        );
        assert_eq!(quote_argument("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote_argument(""), "\"\"");
    }
}
