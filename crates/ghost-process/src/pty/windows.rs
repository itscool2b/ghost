//! ConPTY with an owned Job Object. CreateProcess remains suspended until assignment succeeds.
//! All native handles have one RAII owner; no shell command string or inherited handles are used.
#![allow(
    unsafe_code,
    reason = "ConPTY and atomic Job Object process creation require Win32 bindings"
)]

use super::{Prepared, PtySize, Terminal};
use crate::EnvMap;
use process_wrap::tokio::ChildWrapper;
use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::future::Future;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::ExitStatusExt;
use std::path::Path;
use std::pin::Pin;
use std::process::ExitStatus;
use std::sync::{Arc, Mutex};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::watch;
use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION,
    ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

fn win(error: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(error.code().0 & 0xffff)
}
fn handle(value: &impl AsRawHandle) -> HANDLE {
    HANDLE(value.as_raw_handle())
}
fn coord(size: PtySize) -> COORD {
    COORD {
        X: size.cols as i16,
        Y: size.rows as i16,
    }
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

struct Console {
    handle: Mutex<Option<HPCON>>,
    close: std::sync::mpsc::SyncSender<HPCON>,
}
impl Terminal for Console {
    fn resize(&self, size: PtySize) -> io::Result<()> {
        let guard = self
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let console =
            guard.ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "terminal is closed"))?;
        // SAFETY: this lock keeps the owned pseudo console open throughout resize.
        unsafe { ResizePseudoConsole(console, coord(size)) }.map_err(win)
    }
    fn close(&self) {
        let console = self
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(console) = console {
            // Older Windows versions can wait for the output pipe while closing. Its reader must
            // remain independently runnable, including when the view has gone away.
            let _ = self.close.try_send(console);
        }
    }
}
impl Drop for Console {
    fn drop(&mut self) {
        self.close();
    }
}

struct Attributes {
    storage: Vec<usize>,
    initialized: bool,
}
impl Attributes {
    fn pointer(&self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        LPPROC_THREAD_ATTRIBUTE_LIST(self.storage.as_ptr().cast_mut().cast())
    }
    fn new(console: HPCON) -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: the initial null call asks Windows for storage size, without writing a list.
        let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &mut bytes) };
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut list = Self {
            storage: vec![0; bytes.div_ceil(size_of::<usize>())],
            initialized: false,
        };
        // SAFETY: storage is aligned and sized for this opaque Windows list.
        unsafe { InitializeProcThreadAttributeList(Some(list.pointer()), 1, None, &mut bytes) }
            .map_err(win)?;
        list.initialized = true;
        // SAFETY: the pseudo console is alive until creation finishes. This special attribute
        // takes the pseudo-console handle value, not a pointer to that value.
        unsafe {
            UpdateProcThreadAttribute(
                list.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                Some(console.0 as *const _),
                size_of::<HPCON>(),
                None,
                None,
            )
        }
        .map_err(win)?;
        Ok(list)
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: the initialized list remains in its original allocation.
        if self.initialized {
            unsafe {
                DeleteProcThreadAttributeList(self.pointer());
            }
        }
    }
}

#[derive(Debug)]
struct NativeChild {
    process: Arc<OwnedHandle>,
    job: OwnedHandle,
    pid: u32,
    exit: watch::Receiver<Option<Result<u32, i32>>>,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
}
impl Drop for NativeChild {
    fn drop(&mut self) {
        // SAFETY: only this terminal's handles are used. The direct kill also covers an early
        // assignment failure, when the suspended child has not entered the job yet.
        unsafe {
            let _ = TerminateJobObject(handle(&self.job), 1);
            let _ = TerminateProcess(handle(self.process.as_ref()), 1);
        }
    }
}
impl ChildWrapper for NativeChild {
    fn inner(&self) -> &dyn ChildWrapper {
        self
    }
    fn inner_mut(&mut self) -> &mut dyn ChildWrapper {
        self
    }
    fn into_inner(self: Box<Self>) -> Box<dyn ChildWrapper> {
        self
    }
    fn process_handle(&self) -> Option<BorrowedHandle<'_>> {
        Some(self.process.as_handle())
    }
    fn stdin(&mut self) -> &mut Option<ChildStdin> {
        &mut self.stdin
    }
    fn stdout(&mut self) -> &mut Option<ChildStdout> {
        &mut self.stdout
    }
    fn stderr(&mut self) -> &mut Option<ChildStderr> {
        &mut self.stderr
    }
    fn id(&self) -> Option<u32> {
        Some(self.pid)
    }
    fn start_kill(&mut self) -> io::Result<()> {
        // SAFETY: the owned job contains only this terminal and its descendants.
        unsafe { TerminateJobObject(handle(&self.job), 1) }.map_err(win)
    }
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match *self.exit.borrow() {
            None => Ok(None),
            Some(Ok(code)) => Ok(Some(ExitStatus::from_raw(code))),
            Some(Err(code)) => Err(io::Error::from_raw_os_error(code)),
        }
    }
    fn wait(&mut self) -> Pin<Box<dyn Future<Output = io::Result<ExitStatus>> + Send + '_>> {
        Box::pin(async move {
            self.exit
                .wait_for(Option::is_some)
                .await
                .map_err(|_| io::Error::other("terminal waiter ended"))?;
            self.try_wait()?
                .ok_or_else(|| io::Error::other("terminal exit status missing"))
        })
    }
}

async fn pipe(server_reads: bool) -> io::Result<(NamedPipeServer, File)> {
    let name = format!(r"\\.\pipe\auracode-pty-{}", uuid::Uuid::now_v7());
    let server = ServerOptions::new()
        .first_pipe_instance(true)
        .max_instances(1)
        .access_inbound(server_reads)
        .access_outbound(!server_reads)
        .reject_remote_clients(true)
        .in_buffer_size(65536)
        .out_buffer_size(65536)
        .create(&name)?;
    let client = OpenOptions::new()
        .read(!server_reads)
        .write(server_reads)
        .open(name)?;
    server.connect().await?;
    Ok((server, client))
}

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut wide: Vec<_> = value.encode_wide().collect();
    if wide.contains(&0) {
        return Err(invalid("terminal argument contains a null character"));
    }
    wide.push(0);
    Ok(wide)
}
fn quote(value: &OsStr, output: &mut Vec<u16>) -> io::Result<()> {
    output.push(u16::from(b'"'));
    let mut slashes = 0;
    for c in value.encode_wide() {
        if c == 0 {
            return Err(invalid("terminal argument contains a null character"));
        }
        if c == u16::from(b'\\') {
            slashes += 1;
            continue;
        }
        output.extend(std::iter::repeat_n(
            u16::from(b'\\'),
            if c == u16::from(b'"') {
                slashes * 2 + 1
            } else {
                slashes
            },
        ));
        output.push(c);
        slashes = 0;
    }
    output.extend(std::iter::repeat_n(u16::from(b'\\'), slashes * 2));
    output.push(u16::from(b'"'));
    Ok(())
}

pub(super) async fn prepare(
    program: &Path,
    args: &[OsString],
    cwd: &Path,
    vars: &EnvMap,
    size: PtySize,
) -> io::Result<Prepared> {
    if !program
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Err(invalid(
            "Windows terminals require a native executable, not a shell shim",
        ));
    }
    let application = wide(program.as_os_str())?;
    let cwd = wide(cwd.as_os_str())?;
    let mut command = Vec::new();
    quote(program.as_os_str(), &mut command)?;
    for arg in args {
        command.push(u16::from(b' '));
        quote(arg, &mut command)?;
    }
    command.push(0);
    if command.len() > 32767 {
        return Err(invalid("terminal command line is too long"));
    }
    let mut environment = Vec::new();
    for (name, value) in vars {
        let key = wide(name.as_os_str())?;
        environment.extend_from_slice(&key[..key.len() - 1]);
        environment.push(u16::from(b'='));
        environment.extend(wide(value)?);
    }
    environment.push(0);
    if environment.len() == 1 {
        environment.push(0);
    }
    let (input, input_read) = pipe(false).await?;
    let (output, output_write) = pipe(true).await?;
    // Reserve the closer before creating the console. Thread exhaustion is a startup error,
    // never a panic inside Drop or a synchronous close that can stall the output reader.
    let (close, closing) = std::sync::mpsc::sync_channel::<HPCON>(1);
    std::thread::Builder::new()
        .name("auracode-conpty-close".into())
        .spawn(move || {
            if let Ok(console) = closing.recv() {
                // SAFETY: the sender transfers the sole owned console handle exactly once.
                unsafe { ClosePseudoConsole(console) };
            }
        })?;
    // SAFETY: both synchronous pipe endpoints are live and the size was checked by the caller.
    let console =
        unsafe { CreatePseudoConsole(coord(size), handle(&input_read), handle(&output_write), 0) }
            .map_err(win)?;
    let terminal = Arc::new(Console {
        handle: Mutex::new(Some(console)),
        close,
    });
    let attributes = Attributes::new(console)?;
    // SAFETY: a successful CreateJobObject returns a new owned kernel handle.
    let job = unsafe { OwnedHandle::from_raw_handle(CreateJobObjectW(None, None).map_err(win)?.0) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: the info struct and its size match the declared information class.
    unsafe {
        SetInformationJobObject(
            handle(&job),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    }
    .map_err(win)?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    // ConPTY supplies console handles. Explicit invalid standard handles prevent Windows from
    // retaining the parent's redirected handles instead of attaching the child to this console.
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
    startup.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
    startup.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
    startup.lpAttributeList = attributes.pointer();
    let mut process = PROCESS_INFORMATION::default();
    // SAFETY: all buffers are terminated and remain live. No inherited handles, shell, or
    // breakaway flag is used. Suspension prevents descendants before Job assignment.
    unsafe {
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            Some(environment.as_ptr().cast()),
            PCWSTR(cwd.as_ptr()),
            &startup.StartupInfo,
            &mut process,
        )
    }
    .map_err(win)?;
    // SAFETY: CreateProcess succeeded and transferred these two owned handles.
    let process_handle = Arc::new(unsafe { OwnedHandle::from_raw_handle(process.hProcess.0) });
    let thread = unsafe { OwnedHandle::from_raw_handle(process.hThread.0) };
    let (exit_tx, exit) = watch::channel(None);
    let child = NativeChild {
        process: Arc::clone(&process_handle),
        job,
        pid: process.dwProcessId,
        exit,
        stdin: None,
        stdout: None,
        stderr: None,
    };
    // SAFETY: the child is still suspended and all handles are owned above. Dropping child kills
    // the suspended process if either assignment or resumption fails.
    unsafe { AssignProcessToJobObject(handle(&child.job), handle(process_handle.as_ref())) }
        .map_err(win)?;
    if unsafe { ResumeThread(handle(&thread)) } == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    tokio::task::spawn_blocking(move || {
        // SAFETY: this Arc keeps the process handle alive while waiting. Job cleanup can run
        // independently and wakes this wait, including during runtime shutdown.
        let result = unsafe {
            if WaitForSingleObject(handle(process_handle.as_ref()), INFINITE) != WAIT_OBJECT_0 {
                Err(io::Error::last_os_error().raw_os_error().unwrap_or(1))
            } else {
                let mut code = 0;
                GetExitCodeProcess(handle(process_handle.as_ref()), &mut code)
                    .map(|()| code)
                    .map_err(|error| error.code().0 & 0xffff)
            }
        };
        exit_tx.send_replace(Some(result));
    });
    drop(input_read);
    drop(output_write);
    Ok(Prepared {
        child: Box::new(child),
        input: Box::new(input),
        output: Box::new(output),
        terminal,
    })
}
