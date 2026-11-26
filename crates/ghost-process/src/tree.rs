//! The programs an agent started, as they run now: what the Processes page lists.
//!
//! Windows: right after it starts, every agent also joins a job of its own ([`TreeJob`]), nested
//! in the job process-wrap ends it with. What it starts from then on stays in that job after its
//! parent exits (a `Start-Process` or `cmd /c start` child has no living parent), so it still
//! counts as the agent's. The parent chain from the agent adds anything started before it joined.
//! Unix: the agent's process group and its descendants, from one process-table snapshot.
//!
//! Command lines are for the person looking: masked ([`mask_command`]), cut short, never logged.

use std::collections::HashSet;

use serde::Serialize;

use crate::control::ProcessControl;

/// Processes listed per agent at most.
pub const MAX_TREE: usize = 256;
/// Characters of a command line kept.
pub const MAX_COMMAND: usize = 400;

/// One process in an agent's tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TreeProcess {
    pub pid: u32,
    /// Its parent, when that parent is in the same tree.
    pub parent: Option<u32>,
    /// The executable's file name.
    pub name: String,
    /// The command line, masked and cut to [`MAX_COMMAND`] characters; `None` when the system
    /// does not say.
    pub command: Option<String>,
    /// When it started, in milliseconds since the Unix epoch.
    pub started_ms: Option<u64>,
    /// Processor time used so far (Windows).
    pub cpu_ms: Option<u64>,
    /// Working set (Windows).
    pub memory_bytes: Option<u64>,
}

/// Why [`stop`] did nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StopError {
    #[error("That process is the agent itself. Stop the chat instead.")]
    Agent,
    #[error("That process no longer runs under this agent.")]
    NotInTree,
    #[error("The process could not be stopped: {0}")]
    Failed(String),
}

/// Every process on the system, read once for one listing.
pub struct Snapshot {
    #[cfg(windows)]
    entries: Vec<win::Entry>,
    #[cfg(unix)]
    table: Vec<crate::ptable::ProcInfo>,
}

impl Snapshot {
    /// Reads the process table (blocking).
    pub fn take() -> std::io::Result<Snapshot> {
        #[cfg(windows)]
        {
            Ok(Snapshot {
                entries: win::entries()?,
            })
        }
        #[cfg(unix)]
        {
            Ok(Snapshot {
                table: crate::ptable::snapshot()?,
            })
        }
    }

    /// The pids in `control`'s tree that still run, the agent first.
    fn members(&self, control: &ProcessControl) -> Vec<u32> {
        let root = control.pid();
        let mut pids = vec![root];
        let mut seen = HashSet::from([root]);
        #[cfg(windows)]
        {
            let running: HashSet<u32> = self.entries.iter().map(|entry| entry.pid).collect();
            if !running.contains(&root) {
                return Vec::new();
            }
            let joined = control.job_members().unwrap_or_default();
            let chained = win::descendants(&self.entries, root);
            for pid in joined.into_iter().chain(chained) {
                if running.contains(&pid) && seen.insert(pid) {
                    pids.push(pid);
                }
            }
        }
        #[cfg(unix)]
        {
            if !self.table.iter().any(|process| process.pid == root) {
                return Vec::new();
            }
            let grouped = self
                .table
                .iter()
                .filter(|process| process.pgid == root)
                .map(|process| process.pid);
            let chained = crate::ptable::descendants(&self.table, root)
                .into_iter()
                .map(|process| process.pid);
            for pid in chained.chain(grouped) {
                if seen.insert(pid) {
                    pids.push(pid);
                }
            }
        }
        pids.truncate(MAX_TREE);
        pids
    }

    /// `control`'s tree as it runs now: the agent first, then what it started, nearest first.
    /// Empty once the agent has exited.
    pub fn tree(&self, control: &ProcessControl) -> Vec<TreeProcess> {
        let pids = self.members(control);
        let members: HashSet<u32> = pids.iter().copied().collect();
        let mut tree: Vec<TreeProcess> = pids
            .iter()
            .filter_map(|&pid| self.describe(pid, &members))
            .collect();
        // A recycled id can make an older process look like a child: a child never starts before
        // its parent.
        let started: std::collections::HashMap<u32, Option<u64>> = tree
            .iter()
            .map(|process| (process.pid, process.started_ms))
            .collect();
        for process in &mut tree {
            if let Some(parent) = process.parent
                && let (Some(Some(parent_start)), Some(start)) =
                    (started.get(&parent), process.started_ms)
                && start < *parent_start
            {
                process.parent = None;
            }
        }
        tree
    }

    #[cfg(windows)]
    fn describe(&self, pid: u32, members: &HashSet<u32>) -> Option<TreeProcess> {
        let entry = self.entries.iter().find(|entry| entry.pid == pid)?;
        let facts = win::facts(pid);
        Some(TreeProcess {
            pid,
            parent: members.contains(&entry.ppid).then_some(entry.ppid),
            name: entry.name.clone(),
            command: facts.command.as_deref().map(mask_command),
            started_ms: facts.started_ms,
            cpu_ms: facts.cpu_ms,
            memory_bytes: facts.memory_bytes,
        })
    }

    #[cfg(unix)]
    fn describe(&self, pid: u32, members: &HashSet<u32>) -> Option<TreeProcess> {
        let process = self.table.iter().find(|process| process.pid == pid)?;
        let program = process.argv.first().cloned().unwrap_or_default();
        let name = program.rsplit('/').next().unwrap_or(&program).to_owned();
        Some(TreeProcess {
            pid,
            parent: members.contains(&process.ppid).then_some(process.ppid),
            name,
            command: (!process.argv.is_empty()).then(|| mask_command(&process.argv.join(" "))),
            started_ms: None,
            cpu_ms: None,
            memory_bytes: None,
        })
    }
}

/// Ends `pid` and everything under it in `control`'s tree, deepest first. Refuses the agent
/// itself (its chat's stop keeps the session consistent) and anything outside its tree.
pub fn stop(control: &ProcessControl, pid: u32) -> Result<usize, StopError> {
    if pid == control.pid() {
        return Err(StopError::Agent);
    }
    let snapshot = Snapshot::take().map_err(|error| StopError::Failed(error.to_string()))?;
    let tree = snapshot.tree(control);
    if !tree.iter().any(|process| process.pid == pid) {
        return Err(StopError::NotInTree);
    }
    let mut targets = vec![pid];
    let mut index = 0;
    while index < targets.len() {
        let parent = targets[index];
        let children: Vec<u32> = tree
            .iter()
            .filter(|process| process.parent == Some(parent) && !targets.contains(&process.pid))
            .map(|process| process.pid)
            .collect();
        targets.extend(children);
        index += 1;
    }
    let mut stopped = 0;
    let mut failure = None;
    for target in targets.iter().rev() {
        match kill(*target) {
            Ok(()) => stopped += 1,
            Err(error) => failure = Some(error),
        }
    }
    match (stopped, failure) {
        (0, Some(error)) => Err(StopError::Failed(error)),
        _ => Ok(stopped),
    }
}

#[cfg(windows)]
fn kill(pid: u32) -> Result<(), String> {
    win::terminate(pid)
}

#[cfg(unix)]
fn kill(pid: u32) -> Result<(), String> {
    let raw = i32::try_from(pid).map_err(|_| "invalid process id".to_owned())?;
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(raw),
        nix::sys::signal::Signal::SIGKILL,
    )
    .map_err(|error| error.to_string())
}

/// Masks what could be a secret, then cuts to [`MAX_COMMAND`] characters: the value after a flag
/// or variable named like one (`--token X`, `API_KEY=X`), and any run of 24 or more token
/// characters with both letters and digits (keys, tokens, JWT parts). A UUID stays readable.
pub fn mask_command(command: &str) -> String {
    const MASK: &str = "•••";
    let secret_name = |word: &str| {
        let word = word.trim_start_matches('-').to_ascii_lowercase();
        [
            "token",
            "secret",
            "password",
            "passwd",
            "apikey",
            "api-key",
            "api_key",
            "auth",
            "credential",
            "bearer",
        ]
        .iter()
        .any(|name| word.contains(name))
    };
    let token_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '=');
    let uuid = |run: &str| {
        run.len() == 36
            && run.char_indices().all(|(index, c)| match index {
                8 | 13 | 18 | 23 => c == '-',
                _ => c.is_ascii_hexdigit(),
            })
    };
    let mask_runs = |word: &str| {
        let mut out = String::with_capacity(word.len());
        let mut run = String::new();
        let flush = |run: &mut String, out: &mut String| {
            let letters = run.chars().any(|c| c.is_ascii_alphabetic());
            let digits = run.chars().any(|c| c.is_ascii_digit());
            if run.len() >= 24 && letters && digits && !uuid(run) {
                out.push_str(MASK);
            } else {
                out.push_str(run);
            }
            run.clear();
        };
        for c in word.chars() {
            if token_char(c) {
                run.push(c);
            } else {
                flush(&mut run, &mut out);
                out.push(c);
            }
        }
        flush(&mut run, &mut out);
        out
    };
    let mut words = Vec::new();
    let mut mask_next = false;
    for word in command.split(' ') {
        if mask_next && !word.is_empty() {
            words.push(MASK.to_owned());
            mask_next = false;
            continue;
        }
        if let Some((name, _)) = word.split_once('=')
            && secret_name(name)
        {
            words.push(format!("{name}={MASK}"));
            continue;
        }
        if word.starts_with('-') && secret_name(word) {
            mask_next = true;
        }
        words.push(mask_runs(word));
    }
    let masked = words.join(" ");
    if masked.chars().count() > MAX_COMMAND {
        let mut cut: String = masked.chars().take(MAX_COMMAND - 1).collect();
        cut.push('…');
        cut
    } else {
        masked
    }
}

#[cfg(windows)]
pub(crate) use win::TreeJob;

#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "process listing, job membership and termination need Win32 bindings"
)]
mod win {
    use std::io;
    use std::mem::size_of;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    use windows::Wdk::System::Threading::{
        NtQueryInformationProcess, ProcessCommandLineInformation,
    };
    use windows::Win32::Foundation::{ERROR_MORE_DATA, FILETIME, HANDLE, UNICODE_STRING};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOBOBJECT_BASIC_PROCESS_ID_LIST,
        JobObjectBasicProcessIdList, QueryInformationJobObject,
    };
    use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
        PROCESS_TERMINATE, TerminateProcess,
    };
    use windows::core::PCWSTR;

    fn owned(handle: HANDLE) -> OwnedHandle {
        // SAFETY: every caller passes a handle the call just opened and nothing else owns.
        unsafe { OwnedHandle::from_raw_handle(handle.0) }
    }

    fn raw(handle: &OwnedHandle) -> HANDLE {
        HANDLE(handle.as_raw_handle())
    }

    pub(crate) struct Entry {
        pub(crate) pid: u32,
        pub(crate) ppid: u32,
        pub(crate) name: String,
    }

    /// One ToolHelp snapshot of every process: id, parent id and executable name.
    pub(crate) fn entries() -> io::Result<Vec<Entry>> {
        // SAFETY: plain Win32 call; the handle is owned at once.
        let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }?);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = Vec::new();
        // SAFETY: `entry` is a properly sized PROCESSENTRY32W the calls fill in.
        let mut more = unsafe { Process32FirstW(raw(&snapshot), &mut entry) }.is_ok();
        while more {
            let name = &entry.szExeFile;
            let end = name
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(name.len());
            found.push(Entry {
                pid: entry.th32ProcessID,
                ppid: entry.th32ParentProcessID,
                name: String::from_utf16_lossy(&name[..end]),
            });
            // SAFETY: as above.
            more = unsafe { Process32NextW(raw(&snapshot), &mut entry) }.is_ok();
        }
        Ok(found)
    }

    /// Every process whose parent chain in `entries` leads to `root`, nearest first.
    pub(crate) fn descendants(entries: &[Entry], root: u32) -> Vec<u32> {
        let mut found = Vec::new();
        let mut frontier = vec![root];
        let mut seen = std::collections::HashSet::from([root]);
        while let Some(parent) = frontier.pop() {
            for entry in entries.iter().filter(|entry| entry.ppid == parent) {
                if entry.pid != 0 && seen.insert(entry.pid) {
                    found.push(entry.pid);
                    frontier.insert(0, entry.pid);
                }
            }
        }
        found
    }

    /// A job an agent joins right after it starts, nested in process-wrap's: it only lists.
    pub(crate) struct TreeJob(OwnedHandle);

    impl TreeJob {
        /// `None` when the job cannot be made or joined; the parent chain still lists the tree.
        pub(crate) fn attach(pid: u32) -> Option<TreeJob> {
            // SAFETY: plain Win32 calls; both handles are owned at once.
            let job = owned(unsafe { CreateJobObjectW(None, PCWSTR::null()) }.ok()?);
            let process = owned(
                unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) }.ok()?,
            );
            // SAFETY: both handles are open and owned for the call's duration.
            unsafe { AssignProcessToJobObject(raw(&job), raw(&process)) }.ok()?;
            Some(TreeJob(job))
        }

        /// The processes in the job now.
        pub(crate) fn members(&self) -> Vec<u32> {
            const ROOM: usize = 1024;
            // The list's two counts take one `usize` on 64-bit Windows, then `ROOM` ids.
            let mut buffer = vec![0usize; 2 + ROOM];
            let bytes = u32::try_from(buffer.len() * size_of::<usize>()).unwrap_or(u32::MAX);
            // SAFETY: `buffer` is `bytes` long and aligned for the list it receives.
            let result = unsafe {
                QueryInformationJobObject(
                    Some(raw(&self.0)),
                    JobObjectBasicProcessIdList,
                    buffer.as_mut_ptr().cast(),
                    bytes,
                    None,
                )
            };
            if let Err(error) = result
                && error.code() != ERROR_MORE_DATA.to_hresult()
            {
                return Vec::new();
            }
            // SAFETY: the call wrote the list's header at the start of `buffer`.
            let list = unsafe { &*buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() };
            let count = (list.NumberOfProcessIdsInList as usize).min(ROOM);
            // SAFETY: `count` ids follow the header inside `buffer`.
            let ids = unsafe { std::slice::from_raw_parts(list.ProcessIdList.as_ptr(), count) };
            ids.iter()
                .filter_map(|&id| u32::try_from(id).ok())
                .collect()
        }
    }

    #[derive(Default)]
    pub(crate) struct Facts {
        pub(crate) started_ms: Option<u64>,
        pub(crate) cpu_ms: Option<u64>,
        pub(crate) memory_bytes: Option<u64>,
        pub(crate) command: Option<String>,
    }

    fn ticks(time: FILETIME) -> u64 {
        (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
    }

    /// Start time, processor time, working set and command line; what the system refuses
    /// (a protected or elevated process) stays `None`.
    pub(crate) fn facts(pid: u32) -> Facts {
        // SAFETY: plain Win32 call; the handle is owned at once.
        let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
        else {
            return Facts::default();
        };
        let process = owned(handle);
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: four valid FILETIME out-pointers.
        let times = unsafe {
            GetProcessTimes(
                raw(&process),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        }
        .is_ok();
        let mut memory = PROCESS_MEMORY_COUNTERS {
            cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        // SAFETY: `memory` is a properly sized PROCESS_MEMORY_COUNTERS.
        let measured =
            unsafe { K32GetProcessMemoryInfo(raw(&process), &mut memory, memory.cb) }.as_bool();
        Facts {
            // FILETIME counts 100 ns since 1601.
            started_ms: times
                .then(|| ticks(created).checked_sub(116_444_736_000_000_000))
                .flatten()
                .map(|since_1970| since_1970 / 10_000),
            cpu_ms: times.then(|| (ticks(kernel) + ticks(user)) / 10_000),
            memory_bytes: measured.then_some(memory.WorkingSetSize as u64),
            command: command_line(&process),
        }
    }

    fn command_line(process: &OwnedHandle) -> Option<String> {
        let mut buffer = vec![0u64; 512];
        let mut needed = 0u32;
        for _ in 0..2 {
            let bytes = u32::try_from(buffer.len() * 8).ok()?;
            // SAFETY: `buffer` is `bytes` long and 8-byte aligned for the UNICODE_STRING header.
            let status = unsafe {
                NtQueryInformationProcess(
                    raw(process),
                    ProcessCommandLineInformation,
                    buffer.as_mut_ptr().cast(),
                    bytes,
                    &mut needed,
                )
            };
            if status.is_ok() {
                let start = buffer.as_ptr() as usize;
                let end = start + buffer.len() * 8;
                // SAFETY: the call wrote a UNICODE_STRING header at the start of `buffer`.
                let text = unsafe { &*buffer.as_ptr().cast::<UNICODE_STRING>() };
                let at = text.Buffer.0 as usize;
                let length = usize::from(text.Length);
                // The string must lie inside the buffer the call filled.
                if text.Buffer.is_null() || at < start || at + length > end {
                    return None;
                }
                // SAFETY: bounds checked above; UTF-16 units, `Length` in bytes.
                let units = unsafe { std::slice::from_raw_parts(text.Buffer.0, length / 2) };
                return Some(String::from_utf16_lossy(units));
            }
            let wanted = needed as usize;
            if wanted <= buffer.len() * 8 || wanted > 70_000 {
                return None;
            }
            buffer = vec![0u64; wanted.div_ceil(8)];
        }
        None
    }

    pub(crate) fn terminate(pid: u32) -> Result<(), String> {
        // SAFETY: plain Win32 calls; the handle is owned at once.
        let process = owned(
            unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) }
                .map_err(|error| error.message())?,
        );
        // SAFETY: the handle is open with PROCESS_TERMINATE.
        unsafe { TerminateProcess(raw(&process), 1) }.map_err(|error| error.message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_in_command_lines_are_masked_and_the_rest_stays_readable() {
        assert_eq!(
            // Joined by the compiler: written whole, a secret scanner takes it for a real one.
            mask_command(concat!("node cli.js --tok", "en abc123 --model opus")),
            "node cli.js --token ••• --model opus"
        );
        assert_eq!(
            mask_command("env API_KEY=sk-live-42 OTHER=1 run"),
            "env API_KEY=••• OTHER=1 run"
        );
        assert_eq!(
            mask_command("curl -H Bearer ghp_1234567890abcdefghijABCDEFGHIJ"),
            "curl -H Bearer •••"
        );
        assert_eq!(
            mask_command(r#"claude --mcp-config {"key":"x7Kq2mZp9Lr4Tv8Wn1Bc5Yd3"}"#),
            r#"claude --mcp-config {"key":"•••"}"#
        );
        // Paths, versions, UUIDs and short words stay as they are.
        let plain = r"C:\Users\me\AppData\Local\Programs\node.exe C:\tools\run-1.2.3.js --session 01a1a3bf-db51-7797-8d68-b1680ec18319";
        assert_eq!(mask_command(plain), plain);
        let long = "x".repeat(MAX_COMMAND + 50);
        assert_eq!(mask_command(&long).chars().count(), MAX_COMMAND);
        assert!(mask_command(&long).ends_with('…'));
    }

    #[cfg(windows)]
    #[test]
    fn the_parent_chain_lists_descendants_nearest_first_and_ignores_cycles() {
        let entry = |pid, ppid| win::Entry {
            pid,
            ppid,
            name: format!("p{pid}.exe"),
        };
        // 10 -> 11 -> 13, 10 -> 12; 20 is unrelated; 30 and 31 claim each other as parent.
        let entries = [
            entry(10, 4),
            entry(11, 10),
            entry(12, 10),
            entry(13, 11),
            entry(20, 4),
            entry(30, 31),
            entry(31, 30),
        ];
        assert_eq!(win::descendants(&entries, 10), vec![11, 12, 13]);
        assert_eq!(win::descendants(&entries, 30), vec![31]);
        assert!(win::descendants(&entries, 20).is_empty());
    }
}
