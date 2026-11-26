//! The Ghost executable. Everything lives in the library (`ghost_desktop`), where tests and
//! the bindings export can reach it.

// No console window behind the app in Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The session-scoped memory helper uses stdio without creating a window or taking the
    // profile's writer lock. It talks to the already-running app through a scoped grant.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| helper(&arg, "memory"))
    {
        ghost_core::memory_service::run_helper();
        return;
    }
    // The browser's MCP helper (the Browser plugin's tools), started by an agent the same way.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| helper(&arg, "browser"))
    {
        ghost_core::browser_service::run_helper();
        return;
    }
    // The devices' MCP helper (the Mobile plugin's tools), started by an agent the same way.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| helper(&arg, "mobile"))
    {
        ghost_core::mobile_service::run_helper();
        return;
    }
    // Ghost's bot tools for an assistant on this computer (Hermes, OpenClaw): through the running
    // app's door on loopback, never the relay (bot_helper.rs).
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| helper(&arg, "bot"))
    {
        ghost_desktop::bot_helper::run();
        return;
    }
    // A coordinated project's MCP helper, started by its coordinator's or a worker's agent.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| helper(&arg, "project"))
    {
        ghost_core::coordinator_service::run_helper();
        return;
    }
    // `ghost --uninstall-cleanup` deletes Ghost's data and exits; nothing else starts.
    if let Some(code) = ghost_desktop::uninstall::run_from_args() {
        std::process::exit(code);
    }
    #[cfg(target_os = "linux")]
    relaunch_with_launch_env();
    ghost_desktop::run();
}

/// Whether `arg` asks for the helper mode `name`: `--ghost-<name>-helper`, or the same under the
/// app's earlier names (`--o7-`, `--aura-`), which an agent's saved configuration may still pass.
fn helper(arg: &std::ffi::OsStr, name: &str) -> bool {
    ["ghost", "o7", "aura"]
        .iter()
        .any(|app| arg == std::ffi::OsStr::new(&format!("--{app}-{name}-helper")))
}

/// The NVIDIA driver's own switch for its explicit sync under Wayland.
#[cfg(target_os = "linux")]
const NVIDIA_SWITCH: &str = "__NV_DISABLE_EXPLICIT_SYNC";

/// What Ghost sets for itself on Linux before anything starts, which only a new process can
/// take (changing its own env is unsafe once anything could read it). The program runs itself
/// again with them (same process, same arguments), once: `GHOST_LAUNCH_SET` names them, so the
/// new process does not run again and agents do not inherit them
/// (`ghost_process::strip_launch_context`). When the restart fails, the app goes on without them.
///
/// - **NVIDIA under Wayland.** The window closes as soon as it opens ("Error 71 (Protocol error)
///   dispatching to Wayland display"). The driver's EGL layer puts the window's surface under
///   explicit sync (`wp_linux_drm_syncobj_surface_v1`) once WebKitGTK's GPU renderer draws to it,
///   and GTK 3 then commits a frame of its own without an acquire point: "Missing acquire
///   timeline" (driver 615, WebKitGTK 2.50 and 2.54, Hyprland, 16 November 2026). The driver's own
///   switch, `__NV_DISABLE_EXPLICIT_SYNC=1`, avoids it and keeps the GPU renderer. The other known
///   way out, `WEBKIT_DISABLE_DMABUF_RENDERER=1`, makes WebKitGTK paint on the CPU and copy every
///   frame: on a 4K screen at 2x a chat scrolled at 21 frames a second against 60, and the stress
///   view drew 12 to 25 against 60. So the app sets the driver's switch, unless the person set
///   either variable themselves.
/// - **GStreamer in an AppImage** (`ghost_desktop::browser::media::launch_env`): a registry of
///   Ghost's own, and an image without plugins of its own uses the system's.
#[cfg(target_os = "linux")]
fn relaunch_with_launch_env() {
    use std::os::unix::process::CommandExt;
    let set = launch_changes(
        &|name| std::env::var_os(name),
        std::path::Path::new("/proc/driver/nvidia/version").exists(),
        std::path::Path::new("/"),
    );
    if set.is_empty() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut args = std::env::args_os();
    let mut command = std::process::Command::new(exe);
    if let Some(arg0) = args.next() {
        command.arg0(arg0);
    }
    let names: Vec<&str> = set.iter().map(|(name, _)| *name).collect();
    let error = command
        .args(args)
        .envs(set.iter().map(|(name, value)| (*name, value)))
        .env(ghost_process::LAUNCH_SET_ENV, names.join(":"))
        .exec();
    eprintln!(
        "Ghost: could not restart with {}: {error}",
        names.join(", ")
    );
}

/// What [`relaunch_with_launch_env`] sets: nothing when this process is already the re-run (or
/// something set `GHOST_LAUNCH_SET` for it). `nvidia` says whether NVIDIA's driver is loaded;
/// `root` is where the system's folders are.
#[cfg(target_os = "linux")]
fn launch_changes(
    var: &dyn Fn(&str) -> Option<std::ffi::OsString>,
    nvidia: bool,
    root: &std::path::Path,
) -> Vec<(&'static str, std::ffi::OsString)> {
    if var(ghost_process::LAUNCH_SET_ENV).is_some() {
        return Vec::new();
    }
    let mut set = Vec::new();
    if nvidia
        && var("WAYLAND_DISPLAY").is_some()
        && var(NVIDIA_SWITCH).is_none()
        && var("WEBKIT_DISABLE_DMABUF_RENDERER").is_none()
    {
        set.push((NVIDIA_SWITCH, "1".into()));
    }
    set.extend(ghost_desktop::browser::media::launch_env(var, root));
    set
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let pairs: Vec<(String, OsString)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), OsString::from(value)))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn nvidia_under_wayland_gets_the_drivers_switch_unless_the_person_chose() {
        let root = tempfile::tempdir().unwrap();
        let wayland = env(&[("WAYLAND_DISPLAY", "wayland-1")]);
        assert_eq!(
            launch_changes(&wayland, true, root.path()),
            [(NVIDIA_SWITCH, OsString::from("1"))]
        );
        assert!(launch_changes(&wayland, false, root.path()).is_empty());
        assert!(
            launch_changes(&env(&[]), true, root.path()).is_empty(),
            "X11"
        );
        for chosen in [NVIDIA_SWITCH, "WEBKIT_DISABLE_DMABUF_RENDERER"] {
            let env = env(&[("WAYLAND_DISPLAY", "wayland-1"), (chosen, "0")]);
            assert!(
                launch_changes(&env, true, root.path()).is_empty(),
                "{chosen}"
            );
        }
    }

    #[test]
    fn an_appimage_without_plugins_runs_once_more_with_the_systems() {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("usr/lib/gstreamer-1.0");
        std::fs::create_dir_all(&system).unwrap();
        let image = tempfile::tempdir().unwrap();
        let appdir = image.path().display().to_string();
        let launcher = format!("{appdir}/usr/lib/gstreamer-1.0:");
        let first = env(&[
            ("APPDIR", &appdir),
            ("HOME", "/home/someone"),
            ("WAYLAND_DISPLAY", "wayland-1"),
            ("GST_PLUGIN_SYSTEM_PATH_1_0", &launcher),
        ]);
        let set = launch_changes(&first, true, root.path());
        let names: Vec<&str> = set.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            [
                NVIDIA_SWITCH,
                "GST_REGISTRY_1_0",
                "GST_PLUGIN_SYSTEM_PATH_1_0"
            ],
            "one re-run carries every change"
        );
        let path = &set[2].1;
        assert!(
            std::env::split_paths(path).any(|dir| dir == system),
            "{path:?}"
        );

        // The re-run itself changes nothing more.
        let again = env(&[
            ("APPDIR", &appdir),
            ("WAYLAND_DISPLAY", "wayland-1"),
            (
                ghost_process::LAUNCH_SET_ENV,
                "__NV_DISABLE_EXPLICIT_SYNC:GST_REGISTRY_1_0",
            ),
        ]);
        assert!(launch_changes(&again, true, root.path()).is_empty());
    }
}
