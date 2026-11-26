//! Linux endpoint for an owned host transport. It never runs without an explicit launch frame.
//! Windows deployment is still gated until the helper artifact and bridge are qualified together.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.as_slice() == ["--version"] {
        println!(
            "auracode-host-supervisor {} protocol {}",
            env!("CARGO_PKG_VERSION"),
            ghost_process::supervisor::wire::PROTOCOL_VERSION
        );
        return ExitCode::SUCCESS;
    }
    #[cfg(target_os = "linux")]
    {
        if args.len() == 2 && args[0] == "--stdio" {
            let state = std::path::PathBuf::from(&args[1]);
            if state.is_absolute() {
                return match run(state) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(_) => {
                        // Never echo a malformed control frame, agent arguments or environment.
                        eprintln!("Ghost host supervisor stopped before confirming completion");
                        ExitCode::FAILURE
                    }
                };
            }
        }
        eprintln!("usage: ghost-host-supervisor --stdio <absolute-state-directory>");
    }
    #[cfg(not(target_os = "linux"))]
    eprintln!("the host supervisor endpoint requires Linux");
    ExitCode::FAILURE
}

#[cfg(target_os = "linux")]
fn run(state: std::path::PathBuf) -> std::io::Result<()> {
    use ghost_process::{
        EnvResolver, EnvResolverConfig, LeftoverRegistry, LogFactory, Redactor, Spawner,
    };
    use std::sync::Arc;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(async move {
        let rt = tokio::runtime::Handle::current();
        let run_dir = state.join("run");
        let registry = LeftoverRegistry::open(&run_dir, rt.clone())?;
        // Only registries whose recorded instance is dead are eligible. No distro-wide kill.
        let _ = ghost_process::sweep_stale(&run_dir, registry.own()).await;
        let redactor = Arc::new(Redactor::standard());
        let mut config = EnvResolverConfig::standard();
        config.redactor = Some(Arc::clone(&redactor));
        let resolver = EnvResolver::start(config, registry.clone(), rt.clone());
        let env = resolver.get().await;
        let logs = LogFactory::new(state.join("logs"), Arc::clone(&redactor), rt.clone());
        let id = uuid::Uuid::now_v7().to_string();
        let log = logs.open("supervised", &id);
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
        let (stop, shutdown) = tokio::sync::watch::channel(false);
        let signals = tokio::spawn(async move {
            tokio::select! { _ = term.recv() => {}, _ = interrupt.recv() => {}, _ = hangup.recv() => {} }
            stop.send_replace(true);
        });
        let result = ghost_process::supervisor::serve(
            tokio::io::stdin(),
            tokio::io::stdout(),
            ghost_process::supervisor::SupervisorContext {
                spawner: Spawner::new(registry.clone(), rt),
                env,
                redactor,
                log,
                shutdown: Some(shutdown),
            },
        )
        .await;
        signals.abort();
        // A failed serve still drops the process control and takes the owned tree down. Keep its
        // registry on failure so a later helper can recover escaped descendants after a crash.
        if result.is_ok() {
            registry.close()?;
        }
        result
    });
    // Tokio stdin uses a blocking read which cannot be cancelled while the parent leaves its
    // pipe open. The process tree is already stopped; never wait forever dropping that reader.
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    result
}
