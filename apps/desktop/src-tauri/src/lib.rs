//! The o7 desktop shell: the Tauri window, the plugins, and typed commands and channels over
//! o7-core and o7-drivers. Everything that is not UI stays in Rust; this crate is the thin layer
//! that exposes it to the React UI in `apps/desktop/src`.
//!
//! Startup ([`run`]): logging, launch switches from the environment, the plugins (single-instance
//! first, unless the launch runs on its own: [`LaunchOptions::single_instance`]), then `setup` runs
//! o7-core's startup sequence on the data root o7-core resolves (failures become an error
//! screen, never a crash), picks the window glass for this OS, creates the main window hidden,
//! starts the chat store and the session manager ([`sessions::start_chats`]), and arms the fallback
//! that shows the window if the page never reports its first paint. If no window can be created at
//! all, `setup` logs why and the app exits with [`STARTUP_FAILED`]. Once the window is on screen, a
//! session that did not close the database is followed by a background integrity check
//! ([`integrity`]).
//!
//! Quitting ([`sessions::run_quit`]): the first exit request of any kind (the last window closing,
//! Quit in the menu, a signal, a smoke run's verdict) is prevented while every agent stops, the
//! store commits and closes, and the registry file goes; then the app exits with the code it was
//! asked for. `RunEvent::Exit` is the synchronous last resort ([`state::AppState::at_exit`]).

pub mod agent_account_commands;
pub mod agent_installation_commands;
mod agent_tokens;
pub mod appearance;
pub mod bindings;
pub mod boot;
pub mod browser;
pub mod catalog_refresh;
pub mod changes_commands;
pub mod chat_export;
pub mod clone_commands;
pub mod commands;
pub mod composer_commands;
pub mod config_library;
pub mod config_vendor;
pub mod conversation_commands;
pub mod coordinator_commands;
pub mod data_cleanup_commands;
pub mod demo;
pub mod dictation;
pub mod error;
pub mod folder_resolution_commands;
pub mod general_preferences;
pub mod git_commands;
pub mod glass;
pub mod instructions_commands;
pub mod integrity;
pub mod launch;
pub mod launch_cache;
pub mod memory_commands;
pub mod mobile;
pub mod native_shell;
pub mod onboarding;
pub mod os;
pub mod plugin_library;
pub mod processes;
pub mod profile_commands;
pub mod project_tools;
pub mod remote;
pub mod repository_publication_commands;
pub mod reveal;
pub mod search_commands;
pub mod sessions;
pub mod shared_mcp;
pub mod smoke;
pub mod speech;
pub mod state;
mod sync;
pub mod terminal;
pub mod terminal_font;
pub mod uninstall;
pub mod updater;
pub mod usage_count;
pub mod window;
pub mod workspace_commands;
pub mod workspace_removal_commands;
pub mod worktree_setup_commands;

#[cfg(test)]
mod config_checks;
#[cfg(test)]
mod test_support;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use tauri::webview::PageLoadPayload;
use tauri::{Manager, RunEvent, Webview, WindowEvent};

use crate::glass::{GlassEffect, GlassPlan};
use crate::launch::LaunchOptions;
use crate::sessions::{AppQuitHost, ExitRequest};
use crate::state::AppState;

/// Log filter when `RUST_LOG` is unset.
const DEFAULT_LOG_FILTER: &str = "info,ptts=warn,xn=warn";

/// The exit code when o7 cannot start: Tauri failed to initialize, or no window could be
/// created.
pub const STARTUP_FAILED: i32 = 1;

/// Runs o7 until it exits. If the app cannot start at all (Tauri fails to initialize, or no
/// window can be created), the error is logged and the process exits with [`STARTUP_FAILED`].
pub fn run() {
    init_tracing();
    let launch = LaunchOptions::from_env();

    #[cfg(debug_assertions)]
    match bindings::export() {
        Ok(true) => tracing::info!(
            path = bindings::BINDINGS_PATH,
            "regenerated the TypeScript bindings"
        ),
        Ok(false) => {}
        Err(error) => tracing::warn!(%error, "could not regenerate the TypeScript bindings"),
    }

    let commands = bindings::builder();
    let mut builder = tauri::Builder::default();
    // First, so a second launch hands over to the running instance before anything else starts.
    // Smoke runs and launches with their own data folder skip it (see `single_instance`).
    if launch.single_instance() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // The data cleanup (an uninstall) asks this copy to quit: stop agents and terminals
            // without another question, since uninstalling is the answer.
            if uninstall::is_quit_request(&args) {
                tracing::info!("quitting for the data cleanup");
                if let Some(state) = app.try_state::<AppState>() {
                    state.quit.confirm_ahead();
                }
                app.exit(0);
                return;
            }
            window::focus_main(app);
        }));
    }
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .menu(macos::app_menu)
            .on_menu_event(macos::on_menu_event);
    }
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    {
        builder = builder.plugin(tauri_plugin_notification::init());
    }
    // The plugin's file is per bundle identifier, outside any data folder (see
    // `LaunchOptions::remembers_window_geometry`).
    if launch.remembers_window_geometry() {
        // A copy updated from auracode: its window opens where auracode's last was.
        o7_core::paths::carry_window_state();
        builder = builder.plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(window::restored_state())
                .build(),
        );
    }
    let app = builder
        .plugin(tauri_plugin_opener::init())
        // The folder picker and the quit question, called from Rust only: the page has no dialog
        // permission (capabilities/default.json).
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updater::Updates::default())
        .on_page_load(on_page_load)
        .invoke_handler(commands.invoke_handler())
        .setup(move |app| {
            setup(app, launch);
            // Never an error: see `setup`.
            Ok(())
        })
        .build(tauri::generate_context!());

    match app {
        Ok(app) => app.run(on_event),
        Err(error) => {
            tracing::error!(%error, "o7 could not start");
            std::process::exit(STARTUP_FAILED);
        }
    }
}

/// Runs once Tauri's event loop has started. Returns nothing on purpose: Tauri calls the setup hook
/// from inside its event loop and panics on an error (`Failed to setup app`), so the error would
/// never reach our log, the panic would unwind through the platform's event loop, and the exit
/// hooks would never run. So every failure is handled here: startup problems become the error
/// screen (`boot`), and a window that cannot be created at all is logged and ends the app with
/// [`STARTUP_FAILED`].
fn setup(app: &mut tauri::App, launch: LaunchOptions) {
    // A cleanup asked a running copy to quit and none was running (or this launch runs on its
    // own): there is nothing to quit, so end before any data folder or window opens.
    let args: Vec<String> = std::env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    if uninstall::is_quit_request(&args) {
        app.handle().exit(0);
        return;
    }
    // Follow the OS until the active profile is available, before the hidden window is revealed.
    app.set_theme(None);
    if let Some(Err(error)) = &launch.smoke {
        // Automation is waiting for a result: say why no scenario runs, and with
        // O7_SMOKE_EXIT end here, before any data opens or any agent starts.
        tracing::error!(%error, "the smoke switches cannot run a scenario");
        let report = smoke::setup_failed(error.scenario, &error.message);
        smoke::print(&report);
        if launch.smoke_exit {
            app.handle().exit(smoke::exit_code(&report));
            return;
        }
    }
    let mut booted = boot::startup();

    let os = os::OsInfo::detect();
    let plan = glass::plan(os.os, os.version, launch.no_glass);
    let webview_data = launch.webview_data_dir(booted.data_dir.as_deref());
    let glass = match create_window(app, &plan, launch.is_smoke(), webview_data.as_deref()) {
        Ok(glass) => glass,
        Err(error) => {
            tracing::error!(%error, "o7 could not create its window");
            if let Some(smoke) = launch.smoke_plan() {
                let report = smoke::no_window(smoke.scenario, &error.to_string());
                smoke::print(&smoke::finalize(report, smoke.scenario));
            }
            // Queued for the event loop, which handles it right after this hook returns; the exit
            // hooks run as for any other exit. The store closes as `booted` drops here.
            app.handle().exit(STARTUP_FAILED);
            return;
        }
    };
    tracing::info!(os = os.os, version = %os.display, ?glass, "main window created");

    let smoke = launch.smoke_plan().map(|plan| plan.scenario);
    let smoke_exit = launch.smoke_exit;
    let version = app.package_info().version.to_string();
    // Tauri's setup hook runs outside any Tokio context, so everything that spawns gets the handle.
    let rt = tauri::async_runtime::handle().inner().clone();
    let chats = sessions::start_chats(&mut booted, &version, &rt, sessions::Outside::real);
    let state = AppState::new(booted, chats, &os, glass, launch, version);
    let shown = Arc::clone(&state.window_shown);
    let reported = Arc::clone(&state.smoke_reported);
    let quit = Arc::clone(&state.quit);
    let clones = o7_core::cloning::Clones::new(state.data_dir().map(std::path::Path::to_path_buf));
    app.manage(state);
    #[cfg(windows)]
    app.manage(window::WebviewVisibility::default());
    app.manage(Arc::new(config_library::LibraryService::default()));
    app.manage(Arc::new(
        instructions_commands::InstructionsService::default(),
    ));
    app.manage(Arc::new(config_vendor::VendorService::default()));
    app.manage(Arc::new(clones));
    browser::setup(app.handle());
    mobile::setup(app.handle());
    native_shell::setup(app.handle());
    remote::setup(app.handle(), &rt);
    general_preferences::refresh(app.handle());
    // After the saved preferences: the first check waits a while, and honours the switch.
    if smoke.is_none() {
        updater::start_automatic_checks(app.handle().clone());
        dictation::commands::start_automatic(app.handle().clone());
    }
    appearance::refresh(app.handle());
    memory_commands::forward_changes(app.handle());
    coordinator_commands::forward_changes(app.handle());

    #[cfg(unix)]
    {
        let host = sessions::AppSignals {
            app: app.handle().clone(),
        };
        rt.spawn(async move {
            match sessions::SignalListener::new() {
                Ok(listener) => listener.run(quit, host).await,
                Err(error) => tracing::warn!(%error, "could not listen for quit signals"),
            }
        });
    }
    #[cfg(not(unix))]
    drop(quit);

    let handle = app.handle().clone();
    tauri::async_runtime::spawn(window::show_after(
        window::READY_FALLBACK,
        shown,
        move || {
            tracing::warn!(
                "the page did not report its first paint in time; showing the window anyway"
            );
            window::show_and_focus(&handle)
        },
    ));

    if let Some(scenario) = smoke {
        let handle = app.handle().clone();
        let after = smoke::watchdog(scenario);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(after).await;
            if !reported.swap(true, Ordering::AcqRel) {
                let report = smoke::finalize(smoke::timed_out(scenario, after), scenario);
                smoke::print(&report);
                if smoke_exit {
                    handle.exit(smoke::exit_code(&report));
                }
            }
        });
    }
}

/// Creates the main window with `plan`'s glass. If the platform refuses it, retries with an opaque
/// window when that could work ([`GlassPlan::opaque_fallback`]) rather than failing to start.
/// Returns the glass actually applied. A smoke run's page keeps running while the window is hidden
/// ([`window::create_main`]).
fn create_window(
    app: &tauri::App,
    plan: &GlassPlan,
    smoke: bool,
    webview_data: Option<&std::path::Path>,
) -> Result<GlassEffect, Box<dyn std::error::Error>> {
    let error = match window::create_main(app, plan, smoke, webview_data) {
        Ok(_) => return Ok(plan.effect),
        Err(error) => error,
    };
    let webview_runtime = tauri::webview_version();
    match plan.opaque_fallback(webview_runtime.is_ok()) {
        Some(opaque) => {
            tracing::error!(%error, "could not create the window with glass; retrying opaque");
            window::create_main(app, &opaque, smoke, webview_data)?;
            Ok(opaque.effect)
        }
        None => {
            if let Err(missing) = webview_runtime {
                tracing::error!(error = %missing, "no webview runtime is available");
            }
            Err(error)
        }
    }
}

/// A page that starts loading has new channel callbacks: every subscription of its webview goes,
/// or the frames for the old callbacks would be lost.
fn on_page_load(webview: &Webview, payload: &PageLoadPayload<'_>) {
    let Some(state) = webview.try_state::<AppState>() else {
        return;
    };
    let chats = state.chats();
    sessions::page_load(&state.pages, webview.label(), payload.event(), |owner| {
        state.dictation.drop_owner(owner);
        state.speech.drop_owner(owner);
        state.terminals.drop_owner(owner);
        // Its terminal chats end with its terminals: the chats get their sessions back.
        let ended = state.terminal_chats.drain_owner(owner);
        if let Some(chats) = chats.as_ref() {
            for (chat_id, generation) in ended {
                chats.manager.terminal_ended(&chat_id, generation);
            }
        }
        if let Some(vendor) = webview.try_state::<Arc<config_vendor::VendorService>>() {
            vendor.drop_owner(owner);
        }
        if let Some(clones) = webview.try_state::<Arc<o7_core::cloning::Clones>>() {
            clones.drop_owner(owner);
        }
        if let Some(chats) = chats {
            chats.manager.drop_owner(owner);
        }
    });
}

fn on_event(app: &tauri::AppHandle, event: RunEvent) {
    match event {
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::ThemeChanged(theme),
            ..
        } => {
            if let Some(window) = app.get_webview_window(&label) {
                appearance::repaint(app, &window, theme);
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } => window::focus_main(app),
        RunEvent::ExitRequested { code, api, .. } => on_exit_requested(app, code, &api),
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::Focused(true),
            ..
        } if label == window::MAIN_WINDOW => {
            window::sync_webview_visibility(app);
            catalog_refresh::on_focus(app);
        }
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::Resized(_) | WindowEvent::Focused(_),
            ..
        } if label == window::MAIN_WINDOW => window::sync_webview_visibility(app),
        // Closing retains sessions in the tray. If tray setup failed, use the safe quit path.
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::CloseRequested { api, .. },
            ..
        } if label == window::MAIN_WINDOW => {
            if let Some(state) = app.try_state::<AppState>() {
                state.dictation.cancel_all();
                state.speech.stop_all();
            }
            if native_shell::close_to_tray(app) {
                api.prevent_close();
                return;
            }
            if let Some(state) = app.try_state::<AppState>()
                && state.chats().is_some()
                && !state.quit.is_done()
            {
                api.prevent_close();
                app.exit(0);
            }
        }
        RunEvent::Exit => {
            // The browser's pages close before the window they live in.
            browser::shutdown(app);
            // Its emulators save their state and stop; none outlives the app.
            mobile::shutdown(app);
            remote::shutdown(app);
            if let Some(clones) = app.try_state::<Arc<o7_core::cloning::Clones>>() {
                clones.cancel_all();
            }
            if let Some(shell) = app.try_state::<Arc<native_shell::NativeShell>>() {
                shell.shutdown();
            }
            if let Some(state) = app.try_state::<AppState>() {
                state.at_exit();
            }
        }
        _ => {}
    }
}

/// Every first exit request, whatever its code or cause, runs the quit flow on a Tauri task; the
/// flow's own exit request passes.
fn on_exit_requested(app: &tauri::AppHandle, code: Option<i32>, api: &tauri::ExitRequestApi) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if state.chats().is_none() && !state.app_info().profile_switching {
        // Startup failed before the chats started: there is nothing to stop.
        return;
    }
    match state.quit.on_exit_requested() {
        ExitRequest::Allow => {}
        ExitRequest::Wait => api.prevent_exit(),
        ExitRequest::StartQuit => {
            api.prevent_exit();
            state.dictation.cancel_all();
            state.speech.stop_all();
            state.terminals.close_non_project();
            if let Some(clones) = app.try_state::<Arc<o7_core::cloning::Clones>>() {
                clones.cancel_all();
            }
            // The last window closing asks without a code; that is a normal quit.
            let code = code.unwrap_or(0);
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let flow = tauri::async_runtime::spawn(quit(app.clone(), code));
                if let Err(error) = flow.await {
                    // Without this the app could never exit: every later request would wait for
                    // a flow that is gone. `RunEvent::Exit` still kills and closes what is left.
                    tracing::error!(%error, "the quit flow failed; exiting anyway");
                    if let Some(state) = app.try_state::<AppState>() {
                        state.quit.abandon();
                    }
                    app.exit(code);
                }
            });
        }
    }
}

/// The quit flow with the app's window, dialog and chat runtime.
async fn quit(app: tauri::AppHandle, code: i32) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let _transition = state.transition.lock().await;
    if let Some(chats) = state.chats() {
        let host = AppQuitHost {
            app: &app,
            chats: &chats,
        };
        sessions::run_quit(&state.quit, &host, code).await;
    } else {
        // A pending unlock stopped before publishing a runtime. The transition is finished and
        // there is no owned store left to drain.
        state.quit.abandon();
        app.exit(code);
    }
}

/// macOS ends an app whose menu's Quit item is the predefined one without an exit request
/// (`terminate:` goes straight to `RunEvent::Exit`), which would skip the quit flow. The default
/// menu stays, except that Quit is our own item asking for the exit.
#[cfg(target_os = "macos")]
mod macos {
    use tauri::menu::{Menu, MenuEvent, MenuItem, MenuItemKind};
    use tauri::{AppHandle, Wry};

    pub const QUIT_MENU_ID: &str = "auracode-quit";

    pub fn app_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
        let menu = Menu::default(app)?;
        let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.into_iter().next() else {
            return Ok(menu);
        };
        let items = app_menu.items()?;
        let quit = items.iter().find_map(|item| {
            item.as_predefined_menuitem()
                .filter(|item| item.text().is_ok_and(|text| text.starts_with("Quit")))
        });
        if let Some(quit) = quit {
            app_menu.remove(quit)?;
            let name = &app.package_info().name;
            app_menu.append(&MenuItem::with_id(
                app,
                QUIT_MENU_ID,
                format!("Quit {name}"),
                true,
                Some("CmdOrCtrl+Q"),
            )?)?;
        }
        Ok(menu)
    }

    pub fn on_menu_event(app: &AppHandle, event: MenuEvent) {
        if event.id() == QUIT_MENU_ID {
            app.exit(0);
        }
    }
}

/// Logs to stderr, filtered by `RUST_LOG` (default `info`). Records from crates that use `log`
/// (Tauri, tao, wry) are bridged in by tracing-subscriber's `tracing-log` feature.
fn init_tracing() {
    use std::io::IsTerminal as _;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER));
    let installed = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .try_init();
    if let Err(error) = installed {
        eprintln!("o7: logging is unavailable: {error}");
    }
}
