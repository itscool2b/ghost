//! `auracode --uninstall-cleanup`: deleting all of o7's data from this device, run before
//! anything else starts (no window, no Tauri, no data folder opened).
//!
//! Three callers share it, so the decisions stay in one tested piece of Rust
//! (`o7_core::uninstall`):
//!
//! - the Windows uninstaller's hook (`windows/hooks.nsh`), with `--interactive` unless it runs
//!   quietly: it asks about worktrees with unsaved work and says what could not be deleted;
//! - Settings' "Delete all o7 data" ([`crate::data_cleanup_commands`]), which starts this
//!   mode with `--after-pid` and the choices made in its review, then quits;
//! - a person in a terminal, following `docs/uninstall.md` (macOS has no uninstaller).
//!
//! Before deleting anything it waits for o7 to be gone: a running copy is asked to quit
//! through the single-instance hand-over ([`QUIT_FLAG`]), which stops only o7's own agents
//! and terminals. If it is still running after that, nothing is deleted and the exit code says so.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use o7_core::uninstall::{
    self, GitTool, Os, Plan, PlatformDirs, Report, Retry, Targets, WorktreeChoice,
};

/// The first argument that selects this mode.
pub const CLEANUP_FLAG: &str = "--uninstall-cleanup";
/// The argument a cleanup launches o7 with to ask a running copy to quit. The running copy
/// receives it through the single-instance plugin; a copy that finds none exits at once.
pub const QUIT_FLAG: &str = "--quit-for-uninstall";

/// Exit codes the uninstaller hook reads.
pub mod exit {
    /// Everything was deleted (worktrees the person kept are not a failure).
    pub const DONE: i32 = 0;
    /// The arguments were not understood; nothing was deleted.
    pub const USAGE: i32 = 2;
    /// o7 is still running; nothing was deleted.
    pub const RUNNING: i32 = 3;
    /// Some paths could not be deleted; they are listed.
    pub const INCOMPLETE: i32 = 4;
}

const USAGE: &str =
    "Usage: auracode --uninstall-cleanup [--dry-run] [--interactive | --report-failures]
       [--data-dir <folder>] [--remove-unsaved-worktrees | --remove-worktree <folder>...]
       [--after-pid <pid>] [--trash-app <o7.app>]

Deletes everything o7 keeps on this device. Your agents, their settings and your
repositories are not touched. Worktrees with unsaved work are kept unless you choose otherwise.";

/// What the command line asked for.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Options {
    /// Only this data folder (one chosen with `O7_DATA_DIR`), nothing of the installed app's.
    pub data_dir: Option<PathBuf>,
    /// Wait for this process (the app that started the cleanup) to exit first.
    pub after_pid: Option<u32>,
    /// Ask about worktrees with unsaved work, and say what could not be deleted.
    pub interactive: bool,
    /// Only say what could not be deleted.
    pub report_failures: bool,
    /// Delete every worktree with unsaved work too.
    pub remove_unsaved: bool,
    /// Delete these worktrees with unsaved work too (the ones a review showed).
    pub remove_worktrees: Vec<PathBuf>,
    /// macOS: move this app bundle to the Trash afterwards.
    pub trash_app: Option<PathBuf>,
    /// Print what would go and change nothing.
    pub dry_run: bool,
}

/// Reads the arguments after [`CLEANUP_FLAG`].
pub fn parse(args: &[OsString]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .filter(|value| !value.is_empty())
                .cloned()
                .ok_or_else(|| format!("{name} needs a value."))
        };
        match arg.to_str() {
            Some("--dry-run") => options.dry_run = true,
            Some("--interactive") => options.interactive = true,
            Some("--report-failures") => options.report_failures = true,
            Some("--remove-unsaved-worktrees") => options.remove_unsaved = true,
            Some("--keep-unsaved-worktrees") => options.remove_unsaved = false,
            Some("--data-dir") => options.data_dir = Some(value("--data-dir")?.into()),
            Some("--remove-worktree") => {
                options
                    .remove_worktrees
                    .push(value("--remove-worktree")?.into());
            }
            Some("--trash-app") => options.trash_app = Some(value("--trash-app")?.into()),
            Some("--after-pid") => {
                let pid = value("--after-pid")?;
                options.after_pid = Some(
                    pid.to_str()
                        .and_then(|pid| pid.parse().ok())
                        .ok_or("--after-pid needs a process id.")?,
                );
            }
            _ => return Err(format!("Unknown argument {}.", arg.to_string_lossy())),
        }
    }
    if options.remove_unsaved && !options.remove_worktrees.is_empty() {
        return Err("Use --remove-unsaved-worktrees or --remove-worktree, not both.".to_owned());
    }
    Ok(options)
}

/// Runs the cleanup when the process was started for it, and returns its exit code; `None` for
/// any other launch, which goes on to start the app.
pub fn run_from_args() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new(CLEANUP_FLAG)) {
        return None;
    }
    attach_console();
    let rest: Vec<OsString> = args.collect();
    let options = parse(&rest).and_then(|options| {
        scoped_for_build(options, o7_core::paths::DEVELOPMENT_BUILD, || {
            o7_core::paths::DataRoot::resolve()
                .map(|root| root.root().to_path_buf())
                .map_err(|error| error.to_string())
        })
    });
    Some(match options {
        Ok(options) => run(&options),
        Err(message) => {
            eprintln!("{message}\n\n{USAGE}");
            exit::USAGE
        }
    })
}

/// A development build cleans up only its own data folder, as if `--data-dir` named it
/// (`resolve`: the development folder, or `O7_DATA_DIR`'s). The machine scope is the
/// installed app's: it deletes the installed app's data folder, and asks the installed app to quit
/// first, which ends every chat it runs.
fn scoped_for_build(
    mut options: Options,
    development: bool,
    resolve: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<Options, String> {
    if development && options.data_dir.is_none() {
        let root = resolve().map_err(|error| {
            format!("A development build cleans up its own data folder, and {error}")
        })?;
        options.data_dir = Some(root);
    }
    Ok(options)
}

/// Whether a second launch's arguments (the single-instance plugin's, the executable first) ask
/// this copy to quit for a cleanup. Only exactly that one argument counts: a link (earlier
/// versions registered `auracode://` links with the system) arrives as one argument and can never
/// add another.
pub fn is_quit_request(args: &[String]) -> bool {
    matches!(args, [_, flag] if flag == QUIT_FLAG)
}

fn run(options: &Options) -> i32 {
    let targets = match &options.data_dir {
        Some(dir) => Targets::data_folder(std::path::absolute(dir).unwrap_or_else(|_| dir.clone())),
        None => Targets::machine(Os::current(), &PlatformDirs::current(), environment_root()),
    };
    dispatch(
        options,
        || preview(&targets),
        || execute_cleanup(options, &targets),
    )
}

/// A dry-run never enters the effectful path, including process waiting and quit requests.
fn dispatch(
    options: &Options,
    preview: impl FnOnce() -> i32,
    execute: impl FnOnce() -> i32,
) -> i32 {
    if options.dry_run {
        preview()
    } else {
        execute()
    }
}

fn preview(targets: &Targets) -> i32 {
    let git = GitTool::from_path();
    let plan = uninstall::plan(targets, &git);
    print!("{}", describe_plan(&plan));
    exit::DONE
}

fn execute_cleanup(options: &Options, targets: &Targets) -> i32 {
    if let Some(pid) = options.after_pid {
        wait_for_exit(pid, Duration::from_secs(120));
    }
    // Only the installed app answers the hand-over; a data folder of its own runs alone.
    let ask = options.after_pid.is_none() && options.data_dir.is_none();
    if !wait_until_free(targets, ask) {
        let message = "o7 is still running, so nothing was deleted. Quit o7 and try again.";
        println!("{message}");
        if options.interactive || options.report_failures {
            dialog::alert(message);
        }
        return exit::RUNNING;
    }
    let git = GitTool::from_path();
    let plan = uninstall::plan(targets, &git);
    let choice = choose(&plan, options);
    // Added agent accounts' tokens go with the profiles that list them, read before those go.
    let kept_tokens = remove_agent_tokens(&uninstall::agent_account_ids(&plan))
        // A bot link's call-back key goes with the data folder that lists the link.
        + crate::remote_callback::remove_keys(&remote_bot_ids(&targets.data_roots));
    let mut report = uninstall::execute(&plan, &choice, &git, &Retry::PATIENT);
    if kept_tokens > 0 {
        report.notes.push(format!(
            "{kept_tokens} agent account token(s) could not be removed from secure storage. Remove the entries named auracode.agents.v1 in Credential Manager or Keychain Access."
        ));
    }
    let mut trash_error = None;
    if let Some(bundle) = &options.trash_app {
        trash_error = move_to_trash(bundle).err();
    }
    print!("{}", describe_report(&report));
    if let Some(error) = &trash_error {
        println!(
            "Could not move {} to the Trash: {error}",
            options
                .trash_app
                .as_deref()
                .unwrap_or(Path::new(""))
                .display()
        );
    }
    let complete = report.complete() && trash_error.is_none();
    if !complete && (options.interactive || options.report_failures) {
        dialog::alert(&failure_text(&report, trash_error.as_deref()));
    }
    if complete {
        exit::DONE
    } else {
        exit::INCOMPLETE
    }
}

/// A data folder from `O7_DATA_DIR`, by the rule o7-core resolves it with.
/// Removes each account's token from secure storage; answers how many stayed. With no secure
/// storage there were never any tokens to remove.
fn remove_agent_tokens(ids: &[String]) -> usize {
    use crate::agent_tokens::{NativeTokens, TokenEntry};
    if ids.is_empty() {
        return 0;
    }
    let Ok(store) = NativeTokens::new() else {
        return 0;
    };
    ids.iter()
        .filter(|id| {
            TokenEntry::new(id)
                .and_then(|entry| store.remove(&entry))
                .is_err()
        })
        .count()
}

/// The ids of the bot links each data folder's `remote.json` lists, read before the folders go:
/// a link may have a call-back key in secure storage under its id.
fn remote_bot_ids(roots: &[PathBuf]) -> Vec<String> {
    roots
        .iter()
        .filter_map(|root| std::fs::read(root.join("remote.json")).ok())
        .filter_map(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .flat_map(|settings| {
            settings["bots"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|bot| bot["id"].as_str().map(str::to_owned))
                .filter(|id| !id.is_empty() && id.len() <= 64)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn environment_root() -> Option<PathBuf> {
    let named = |name| {
        std::env::var_os(name).filter(|value: &std::ffi::OsString| {
            !value.is_empty() && !value.to_str().is_some_and(|text| text.trim().is_empty())
        })
    };
    // The override under the app's earlier name counts too, as o7_core::paths reads it.
    named(o7_core::paths::DATA_DIR_ENV).or_else(|| named(o7_core::paths::LEGACY_DATA_DIR_ENV))?;
    o7_core::paths::DataRoot::resolve()
        .ok()
        .map(|root| root.root().to_path_buf())
}

/// Which unsaved worktrees go: those named, all of them when asked, or (interactive) what the
/// person answers. Without a choice they stay.
pub fn choose(plan: &Plan, options: &Options) -> WorktreeChoice {
    if !options.remove_worktrees.is_empty() {
        return WorktreeChoice::Remove(options.remove_worktrees.clone());
    }
    let unsaved: Vec<PathBuf> = plan
        .unsaved()
        .map(|worktree| worktree.path.clone())
        .collect();
    if unsaved.is_empty() {
        return WorktreeChoice::KeepUnsaved;
    }
    if options.remove_unsaved
        || (options.interactive && !dialog::ask_keep(&worktree_question(plan)))
    {
        return WorktreeChoice::Remove(unsaved);
    }
    WorktreeChoice::KeepUnsaved
}

/// At most this many worktrees or failures are named in a dialog.
const LISTED: usize = 6;

/// The uninstaller's question about worktrees with unsaved work.
pub fn worktree_question(plan: &Plan) -> String {
    let unsaved: Vec<_> = plan.unsaved().collect();
    let mut text = String::from(
        "o7 is deleting its data. These worktrees have work that is not saved anywhere else:\n\n",
    );
    for worktree in unsaved.iter().take(LISTED) {
        let name = worktree
            .branch
            .clone()
            .unwrap_or_else(|| worktree.path.display().to_string());
        match &worktree.repository {
            Some(repository) => text.push_str(&format!(
                "{name} in {}: {}\n",
                repository.display(),
                worktree.summary()
            )),
            None => text.push_str(&format!("{name}: {}\n", worktree.summary())),
        }
    }
    if unsaved.len() > LISTED {
        text.push_str(&format!("and {} more\n", unsaved.len() - LISTED));
    }
    text.push_str(
        "\nKeep them? Yes leaves these folders where they are, with their branches, so nothing is lost. No deletes them with everything else.",
    );
    text
}

/// What could not be deleted, for the dialog.
pub fn failure_text(report: &Report, trash: Option<&str>) -> String {
    let mut text = String::from("o7 could not delete everything. Still on this computer:\n\n");
    for failure in report.failed.iter().take(LISTED) {
        text.push_str(&format!("{}: {}\n", failure.path.display(), failure.error));
    }
    if report.failed.len() > LISTED {
        text.push_str(&format!("and {} more\n", report.failed.len() - LISTED));
    }
    if let Some(error) = trash {
        text.push_str(&format!(
            "The app itself was not moved to the Trash: {error}\n"
        ));
    }
    text.push_str(
        "\nClose any program that is using them, then run the cleanup again or delete them by hand. docs/uninstall.md lists every location.",
    );
    text
}

fn describe_plan(plan: &Plan) -> String {
    let mut text = String::new();
    if plan.is_empty() {
        text.push_str("Nothing of o7's is on this device.\n");
    }
    for item in &plan.items {
        text.push_str(&format!("Would remove {}\n", item.path.display()));
    }
    for worktree in &plan.worktrees {
        let verb = if worktree.unsaved() {
            "Would keep"
        } else {
            "Would remove"
        };
        text.push_str(&format!(
            "{verb} worktree {} ({})\n",
            worktree.path.display(),
            worktree.summary()
        ));
    }
    text
}

fn describe_report(report: &Report) -> String {
    let mut text = String::new();
    for path in &report.removed {
        text.push_str(&format!("Removed {}\n", path.display()));
    }
    for worktree in &report.kept {
        text.push_str(&format!(
            "Kept worktree {} ({})\n",
            worktree.path.display(),
            worktree.summary()
        ));
    }
    for failure in &report.failed {
        text.push_str(&format!(
            "Could not remove {} after {} tries: {}\n",
            failure.path.display(),
            failure.attempts,
            failure.error
        ));
    }
    for note in &report.notes {
        text.push_str(&format!("{note}\n"));
    }
    text
}

/// Waits until no o7 process holds a profile in the data folders. When `ask`, a running
/// copy is first asked to quit the way a second launch would ask it to show its window.
fn wait_until_free(targets: &Targets, ask: bool) -> bool {
    let busy = || !uninstall::in_use(&targets.data_roots).is_empty();
    if !busy() {
        return true;
    }
    if ask {
        request_quit();
    }
    let deadline = Instant::now() + Duration::from_secs(if ask { 45 } else { 60 });
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        if !busy() {
            // The lock goes as the process ends; let its last writes land first.
            std::thread::sleep(Duration::from_millis(500));
            return true;
        }
    }
    false
}

/// Launches o7 with [`QUIT_FLAG`]: the single-instance plugin hands it to the running copy
/// and exits; with no running copy it exits before opening anything.
fn request_quit() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut command = Command::new(exe);
    command
        .arg(QUIT_FLAG)
        // The hand-over only reaches the installed app's instance.
        .env_remove(o7_core::paths::DATA_DIR_ENV)
        .env_remove(o7_core::paths::LEGACY_DATA_DIR_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let Ok(mut child) = command.spawn() else {
        return;
    };
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(15) {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Waits for process `pid` to exit, at most `limit`.
fn wait_for_exit(pid: u32, limit: Duration) {
    #[cfg(windows)]
    {
        windows::wait_for_exit(pid, limit);
    }
    #[cfg(unix)]
    {
        let started = Instant::now();
        while started.elapsed() < limit {
            let alive = Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if !alive {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

/// A release build has no console of its own (it is a GUI program); when started from a
/// terminal, print there.
fn attach_console() {
    #[cfg(windows)]
    windows::attach_console();
}

#[cfg(target_os = "macos")]
fn move_to_trash(bundle: &Path) -> Result<(), String> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    if bundle.extension() != Some(OsStr::new("app")) || !bundle.join("Contents").is_dir() {
        return Err("It is not an app bundle.".to_owned());
    }
    let path = NSString::from_str(&bundle.to_string_lossy());
    let url = NSURL::fileURLWithPath(&path);
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, None)
        .map_err(|error| error.localizedDescription().to_string())
}

#[cfg(not(target_os = "macos"))]
fn move_to_trash(_bundle: &Path) -> Result<(), String> {
    Err(
        "Only macOS apps are moved to the Trash; on Windows, uninstall o7 from Settings."
            .to_owned(),
    )
}

/// The app bundle Settings can move to the Trash: the `.app` this executable runs from, unless it
/// runs from a disk image or a translocated copy, which cannot be trashed in place.
pub fn app_bundle() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let bundle = exe.parent()?.parent()?.parent()?.to_path_buf();
    let text = bundle.to_string_lossy();
    (bundle.extension() == Some(OsStr::new("app"))
        && bundle.join("Contents").join("Info.plist").is_file()
        && !text.contains("/AppTranslocation/")
        && !text.starts_with("/Volumes/"))
    .then_some(bundle)
}

/// Native dialogs for a run with no window of its own.
mod dialog {
    /// Asks the uninstaller's worktree question; `true` keeps the worktrees (also the answer when
    /// no dialog can be shown).
    pub fn ask_keep(text: &str) -> bool {
        #[cfg(windows)]
        return super::windows::ask_keep(text);
        #[cfg(target_os = "macos")]
        return super::macos::ask_keep(text);
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = text;
            true
        }
    }

    pub fn alert(text: &str) {
        #[cfg(windows)]
        super::windows::alert(text);
        #[cfg(target_os = "macos")]
        super::macos::alert(text);
        #[cfg(not(any(windows, target_os = "macos")))]
        let _ = text;
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::process::{Command, Stdio};

    /// `osascript` shows the dialog; the text goes in as an argument, never as script.
    fn osascript(script: &[&str], text: &str) -> Option<String> {
        let mut command = Command::new("/usr/bin/osascript");
        for line in script {
            command.args(["-e", line]);
        }
        let output = command
            .arg("--")
            .arg(text)
            .stdin(Stdio::null())
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub fn ask_keep(text: &str) -> bool {
        let answer = osascript(
            &[
                "on run argv",
                "display dialog (item 1 of argv) with title \"auracode\" buttons {\"Delete them\", \"Keep them\"} default button \"Keep them\" with icon caution",
                "end run",
            ],
            text,
        );
        !answer.is_some_and(|answer| answer.contains("Delete them"))
    }

    pub fn alert(text: &str) {
        let _ = osascript(
            &[
                "on run argv",
                "display alert \"auracode\" message (item 1 of argv) as warning",
                "end run",
            ],
            text,
        );
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows {
    //! The Win32 calls this mode needs, confined here.
    use std::time::Duration;

    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDNO, MB_DEFBUTTON1, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO,
        MessageBoxW,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn attach_console() {
        // SAFETY: no pointers; failing (no parent console) leaves the process as it was.
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }

    fn message(text: &str, style: u32) -> i32 {
        let text = wide(text);
        let title = wide("o7");
        // SAFETY: both strings are NUL-terminated and outlive the call; no owner window.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                style | MB_SETFOREGROUND | MB_TOPMOST,
            )
        }
    }

    pub fn ask_keep(text: &str) -> bool {
        message(text, MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON1) != IDNO
    }

    pub fn alert(text: &str) {
        message(text, MB_OK | MB_ICONWARNING);
    }

    pub fn wait_for_exit(pid: u32, limit: Duration) {
        // SAFETY: plain handle request; a null handle (process gone, no access) is checked.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return;
        }
        let millis = u32::try_from(limit.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: a valid handle, closed right after the wait.
        unsafe {
            WaitForSingleObject(handle, millis);
            CloseHandle(handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use o7_core::uninstall::Worktree;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn dry_run_never_enters_process_wait_quit_delete_or_trash_path() {
        let options = Options {
            dry_run: true,
            interactive: true,
            after_pid: Some(123),
            trash_app: Some("/Applications/o7.app".into()),
            remove_unsaved: true,
            ..Options::default()
        };
        let result = dispatch(
            &options,
            || {
                let text = describe_plan(&plan(vec![worktree("kept", 2, 0)]));
                assert!(text.contains("Would keep worktree kept"), "{text}");
                exit::DONE
            },
            || {
                panic!(
                    "dry-run entered the path that waits, requests quit, deletes or moves to Trash"
                )
            },
        );
        assert_eq!(result, exit::DONE);
    }

    #[test]
    fn parses_every_option() {
        let options = parse(&args(&[
            "--interactive",
            "--data-dir",
            "/data",
            "--remove-worktree",
            "/data/profiles/local/worktrees/a",
            "--remove-worktree",
            "/data/profiles/local/worktrees/b",
            "--after-pid",
            "4242",
            "--trash-app",
            "/Applications/o7.app",
            "--dry-run",
        ]))
        .unwrap();
        assert_eq!(
            options,
            Options {
                data_dir: Some("/data".into()),
                after_pid: Some(4242),
                interactive: true,
                report_failures: false,
                remove_unsaved: false,
                remove_worktrees: vec![
                    "/data/profiles/local/worktrees/a".into(),
                    "/data/profiles/local/worktrees/b".into()
                ],
                trash_app: Some("/Applications/o7.app".into()),
                dry_run: true,
            }
        );
        assert_eq!(parse(&[]).unwrap(), Options::default());
    }

    #[test]
    fn refuses_what_it_does_not_understand() {
        assert!(parse(&args(&["--force"])).is_err());
        assert!(parse(&args(&["--data-dir"])).is_err());
        assert!(parse(&args(&["--data-dir", ""])).is_err());
        assert!(parse(&args(&["--after-pid", "me"])).is_err());
        assert!(
            parse(&args(&[
                "--remove-unsaved-worktrees",
                "--remove-worktree",
                "/x"
            ]))
            .is_err()
        );
    }

    /// A cleanup from a development build without `--data-dir` planned the installed app's
    /// whole data folder, and its quit request reached the installed app.
    #[test]
    fn a_development_build_cleans_up_only_its_own_folder() {
        let own = PathBuf::from("/data/codes.o7.desktop.dev");
        let development = scoped_for_build(Options::default(), true, || Ok(own.clone())).unwrap();
        assert_eq!(development.data_dir.as_deref(), Some(own.as_path()));

        // A folder named on the command line stays the one cleaned up.
        let named = parse(&args(&["--data-dir", "/scratch"])).unwrap();
        let kept = scoped_for_build(named.clone(), true, || panic!("not resolved")).unwrap();
        assert_eq!(kept, named);

        // No folder to name refuses, rather than falling back to the machine scope.
        let refused = scoped_for_build(Options::default(), true, || Err("no data folder".into()))
            .unwrap_err();
        assert!(refused.contains("no data folder"), "{refused}");

        // An installed build keeps the machine scope.
        let installed =
            scoped_for_build(Options::default(), false, || panic!("not resolved")).unwrap();
        assert_eq!(installed.data_dir, None);
    }

    #[test]
    fn only_the_exact_quit_argument_asks_a_running_copy_to_quit() {
        let exe = "C:\\Program Files\\o7\\o7.exe".to_owned();
        assert!(is_quit_request(&[exe.clone(), QUIT_FLAG.into()]));
        assert!(!is_quit_request(std::slice::from_ref(&exe)));
        assert!(!is_quit_request(&[
            exe.clone(),
            "auracode://signin?x=--quit-for-uninstall".into()
        ]));
        assert!(!is_quit_request(&[
            exe.clone(),
            QUIT_FLAG.into(),
            "x".into()
        ]));
        assert!(!is_quit_request(&[exe, CLEANUP_FLAG.into()]));
    }

    fn worktree(name: &str, changed: u32, commits: u32) -> Worktree {
        let mut worktree = GitTool::missing().inspect(Path::new(name));
        worktree.problem = None;
        worktree.changed_files = changed;
        worktree.unique_commits = commits;
        worktree.branch = Some(format!("aura/{name}"));
        worktree.repository = Some(PathBuf::from("C:\\code\\shop"));
        worktree
    }

    fn plan(worktrees: Vec<Worktree>) -> Plan {
        Plan {
            scope: uninstall::Scope::Machine,
            items: Vec::new(),
            worktrees,
            data_roots: Vec::new(),
        }
    }

    #[test]
    fn unsaved_worktrees_stay_unless_chosen() {
        let plan = plan(vec![
            worktree("clean", 0, 0),
            worktree("dirty", 2, 0),
            worktree("ahead", 0, 1),
        ]);
        let quiet = Options::default();
        assert_eq!(choose(&plan, &quiet), WorktreeChoice::KeepUnsaved);
        let all = Options {
            remove_unsaved: true,
            ..Options::default()
        };
        assert_eq!(
            choose(&plan, &all),
            WorktreeChoice::Remove(vec!["dirty".into(), "ahead".into()])
        );
        let listed = Options {
            remove_worktrees: vec!["ahead".into()],
            ..Options::default()
        };
        assert_eq!(
            choose(&plan, &listed),
            WorktreeChoice::Remove(vec!["ahead".into()])
        );
    }

    #[test]
    fn the_question_names_each_worktree_and_what_it_would_lose() {
        let many: Vec<_> = (0..8)
            .map(|index| worktree(&format!("w{index}"), 1, 0))
            .collect();
        let text = worktree_question(&plan(many));
        assert!(text.contains("aura/w0 in C:\\code\\shop: 1 uncommitted file"));
        assert!(text.contains("and 2 more"));
        assert!(text.contains("Yes leaves these folders where they are"));
        assert!(!text.contains('\u{2014}'), "no em dashes");
    }

    #[test]
    fn the_failure_text_lists_what_is_left() {
        let report = Report {
            failed: vec![uninstall::Failure {
                path: PathBuf::from("C:\\x\\EBWebView\\Cookies"),
                error: "Another program is using it.".into(),
                attempts: 8,
            }],
            ..Report::default()
        };
        let text = failure_text(&report, None);
        assert!(text.contains("C:\\x\\EBWebView\\Cookies: Another program is using it."));
        assert!(!text.contains('\u{2014}'));
    }
}
