//! ConPTY startup with Job assignment before the shell executes any instruction.
use super::executor::{quote_argument, Job};
use crate::error::{AppError, AppResult};
use portable_pty::{Child, ChildKiller, CommandBuilder, ExitStatus, PtySize};
use std::{
    fs::File,
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
    },
    ptr::{null, null_mut},
    sync::Arc,
};
use windows_sys::Win32::{
    Foundation::{INVALID_HANDLE_VALUE, STILL_ACTIVE},
    System::{
        Console::{ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON},
        Pipes::CreatePipe,
        Threading::{
            CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
            InitializeProcThreadAttributeList, ResumeThread, TerminateProcess,
            UpdateProcThreadAttribute, WaitForSingleObject, CREATE_SUSPENDED,
            CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTF_USESTDHANDLES, STARTUPINFOEXW,
        },
    },
};
fn failure(error: impl std::fmt::Display) -> AppError {
    AppError::io("local.pty", error)
}
fn coord(size: PtySize) -> COORD {
    COORD {
        X: size.cols as i16,
        Y: size.rows as i16,
    }
}
fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}
fn pipe() -> AppResult<(File, File)> {
    let (mut read, mut write) = (null_mut(), null_mut());
    if unsafe { CreatePipe(&mut read, &mut write, null(), 0) } == 0 {
        return Err(failure(io::Error::last_os_error()));
    }
    Ok(unsafe { (File::from_raw_handle(read), File::from_raw_handle(write)) })
}
pub struct Console {
    handle: HPCON,
}
unsafe impl Send for Console {}
impl Console {
    pub fn resize(&self, size: PtySize) -> AppResult<()> {
        let status = unsafe { ResizePseudoConsole(self.handle, coord(size)) };
        if status < 0 {
            return Err(failure(format!("ResizePseudoConsole failed ({status})")));
        }
        Ok(())
    }
}
impl Drop for Console {
    fn drop(&mut self) {
        unsafe { ClosePseudoConsole(self.handle) };
    }
}
struct Attributes {
    buffer: Vec<usize>,
    initialized: bool,
}
impl Attributes {
    fn new(console: HPCON) -> AppResult<Self> {
        let mut bytes = 0;
        unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes) };
        let mut this = Self {
            buffer: vec![0; bytes.div_ceil(std::mem::size_of::<usize>())],
            initialized: false,
        };
        if unsafe {
            InitializeProcThreadAttributeList(this.buffer.as_mut_ptr().cast(), 1, 0, &mut bytes)
        } == 0
        {
            return Err(failure(io::Error::last_os_error()));
        }
        this.initialized = true;
        if unsafe {
            UpdateProcThreadAttribute(
                this.buffer.as_mut_ptr().cast(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                console as *const _,
                std::mem::size_of::<HPCON>(),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err(failure(io::Error::last_os_error()));
        }
        Ok(this)
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { DeleteProcThreadAttributeList(self.buffer.as_mut_ptr().cast()) };
        }
    }
}
#[derive(Debug)]
struct Process {
    handle: OwnedHandle,
    id: u32,
}
impl Process {
    fn status(&self) -> io::Result<Option<ExitStatus>> {
        let mut code = 0;
        if unsafe { GetExitCodeProcess(self.handle.as_raw_handle(), &mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((code != STILL_ACTIVE as u32).then(|| ExitStatus::with_exit_code(code)))
    }
}
#[derive(Debug, Clone)]
struct ConsoleChild(Arc<Process>);
impl ChildKiller for ConsoleChild {
    fn kill(&mut self) -> io::Result<()> {
        if self.0.status()?.is_none()
            && unsafe { TerminateProcess(self.0.handle.as_raw_handle(), 1) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(self.clone())
    }
}
impl Child for ConsoleChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.0.status()
    }
    fn wait(&mut self) -> io::Result<ExitStatus> {
        unsafe { WaitForSingleObject(self.0.handle.as_raw_handle(), u32::MAX) };
        self.0
            .status()?
            .ok_or_else(|| io::Error::other("ConPTY process did not exit"))
    }
    fn process_id(&self) -> Option<u32> {
        Some(self.0.id)
    }
    fn as_raw_handle(&self) -> Option<RawHandle> {
        Some(self.0.handle.as_raw_handle())
    }
}
pub struct Spawned {
    pub console: Console,
    pub child: Box<dyn Child + Send + Sync>,
    pub job: Job,
    pub reader: File,
    pub writer: File,
}
pub fn spawn(command: CommandBuilder, size: PtySize) -> AppResult<Spawned> {
    let (input, writer) = pipe()?;
    let (reader, output) = pipe()?;
    let mut handle = 0;
    let status = unsafe {
        CreatePseudoConsole(
            coord(size),
            input.as_raw_handle(),
            output.as_raw_handle(),
            0,
            &mut handle,
        )
    };
    if status < 0 {
        return Err(failure(format!("CreatePseudoConsole failed ({status})")));
    }
    let console = Console { handle };
    drop((input, output));
    let mut attributes = Attributes::new(console.handle)?;
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
    startup.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
    startup.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
    startup.lpAttributeList = attributes.buffer.as_mut_ptr().cast();
    let executable = command
        .get_argv()
        .first()
        .ok_or_else(|| failure("Shell executable is missing"))?;
    let line = command
        .get_argv()
        .iter()
        .map(|v| quote_argument(&v.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");
    let mut environment = Vec::new();
    let mut variables = command.iter_full_env_as_str().collect::<Vec<_>>();
    variables.sort_by_key(|(k, _)| k.to_uppercase());
    for (key, value) in variables {
        environment.extend(wide(format!("{key}={value}")));
    }
    environment.push(0);
    let directory = command.get_cwd().map(wide);
    let job = Job::new()?;
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        CreateProcessW(
            wide(executable).as_ptr(),
            wide(line).as_mut_ptr(),
            null(),
            null(),
            0,
            CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            environment.as_ptr().cast(),
            directory.as_ref().map_or(null(), |p| p.as_ptr()),
            &startup.StartupInfo,
            &mut info,
        )
    } == 0
    {
        return Err(failure(io::Error::last_os_error()));
    }
    let process = Arc::new(Process {
        handle: unsafe { OwnedHandle::from_raw_handle(info.hProcess) },
        id: info.dwProcessId,
    });
    let thread = unsafe { OwnedHandle::from_raw_handle(info.hThread) };
    let mut child = ConsoleChild(process);
    // Both failures terminate the still-suspended process; it never reaches user code.
    use std::os::windows::io::AsHandle;
    if let Err(error) =
        job.assign_process(child.0.handle.as_handle(), std::path::Path::new(executable))
    {
        let _ = child.kill();
        return Err(error);
    }
    if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
        let error = failure(io::Error::last_os_error());
        let _ = child.kill();
        return Err(error);
    }
    Ok(Spawned {
        console,
        child: Box::new(child),
        job,
        reader,
        writer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Requires the fixed bundled PowerShell resources; starts an immediate child process"]
    fn first_instruction_child_is_owned_by_the_job() {
        use std::{
            path::Path,
            time::{Duration, Instant},
        };
        use windows_sys::Win32::{
            Foundation::WAIT_OBJECT_0,
            System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE},
        };
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("child.pid");
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let resources = crate::local::executor::ExecutorResources::discover(root);
        let executable = resources.powershell;
        let escaped = pidfile.to_string_lossy().replace('\'', "''");
        let source = format!("$child = Start-Process -FilePath '{}' -ArgumentList '-NoLogo','-NoProfile','-Command','Start-Sleep 60' -PassThru; Set-Content -LiteralPath '{escaped}' -Value $child.Id; Start-Sleep 60", executable.to_string_lossy().replace('\'', "''"));
        let mut command = CommandBuilder::new(&executable);
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &source,
        ]);
        command.cwd(dir.path());
        let opened = spawn(command, PtySize::default()).unwrap();
        // Drain the pseudo console until it closes, including any cursor query.
        let mut reader = opened.reader;
        let reader = std::thread::spawn(move || {
            let _ = io::copy(&mut reader, &mut io::sink());
        });
        let start = Instant::now();
        while !pidfile.exists() && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            pidfile.exists(),
            "The first shell command did not create its child"
        );
        let pid = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let child = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!child.is_null());
        let child = unsafe { OwnedHandle::from_raw_handle(child) };
        opened.job.terminate();
        assert_eq!(
            unsafe { WaitForSingleObject(child.as_raw_handle(), 5000) },
            WAIT_OBJECT_0
        );
        drop(opened.writer);
        drop(opened.console);
        reader.join().unwrap();
    }
}
