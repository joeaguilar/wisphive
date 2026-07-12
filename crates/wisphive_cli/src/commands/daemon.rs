use anyhow::Result;
use tracing::{Level, info, warn};
use wisphive_daemon::DaemonConfig;
use wisphive_daemon::logging::{self, LogStore};
use wisphive_daemon::server::Server;
use wisphive_daemon::shutdown;

/// Options for launching an embedded web UI alongside the daemon.
pub struct WebOptions {
    pub host: [u8; 4],
    pub port: u16,
    pub dev: bool,
    /// itr#267: if `false` and the web admin password has never been set,
    /// auto-open the default browser onto the onboarding URL once the
    /// server is listening. CLI flag: `--no-open`.
    pub no_open: bool,
    /// itr#310: auth/security profile (LocalLAN default; Enterprise
    /// requires `--auth-rp-id` plus user-provided TLS cert once itr#270
    /// lands). Resolved + validated by the CLI before this struct is
    /// built.
    pub auth_profile: wisphive_web::AuthProfile,
}

/// Start the daemon in the foreground. Optionally also serve the web UI in
/// the same process so a single `wisphive daemon start --web` gets you both.
pub async fn start(web: Option<WebOptions>) -> Result<()> {
    let config = DaemonConfig::default_location();
    config.ensure_dirs()?;

    // Initialize logging: stderr stays quiet (WARN by default) while a daily
    // JSON log file under ~/.wisphive/logs and an in-memory ring buffer
    // capture full INFO+ traffic for forensics and live tailing. The
    // StoreLayer registered inside `init` already owns its own `Arc<LogStore>`
    // clone; the `log_store` binding here is the hand-off point for the
    // follow-up issue that threads live logs into the embedded web server.
    let log_store = LogStore::new(4096);
    let log_guards = logging::init(&config.log_dir, log_store.clone(), Level::WARN)?;
    if let Err(e) = logging::prune_old_files(&config.log_dir, config.log_retention_days) {
        // The subscriber is already installed, so this `warn!` reaches
        // the file/store sinks; surface it instead of swallowing because
        // anything other than NotFound (which the pruner already absorbs)
        // means the operator has a broken `~/.wisphive/logs` they should
        // see — e.g. EACCES on a hand-edited dir.
        warn!(error = %e, "log pruning failed at startup");
    }

    // Check for existing daemon
    shutdown::check_existing_daemon(&config.pid_path)?;

    // Write PID file (guard removes it on drop)
    let pid_guard = shutdown::write_pid_file(&config.pid_path)?;

    info!("starting wisphive daemon");

    let (shutdown_tx, shutdown_rx) = shutdown::shutdown_channel();

    // Spawn signal handler
    tokio::spawn(shutdown::wait_for_signal(shutdown_tx));

    // Optionally spawn the embedded web UI server. It connects to the same
    // Unix socket the daemon owns, so we must wait until the daemon has
    // actually bound the listener — Server::run does that early, but we're
    // racing it here. A short retry loop inside serve() (via its socket
    // connect on each upgrade) handles that gracefully since each WebSocket
    // client opens a fresh connection per upgrade.
    let (web_handle, browser_handle) = if let Some(opts) = web {
        let socket_path = config.socket_path.clone();
        let addr = std::net::SocketAddr::from((opts.host, opts.port));
        info!(%addr, dev = opts.dev, "starting embedded web server");
        if opts.host == [0, 0, 0, 0] {
            warn!("web UI is listening on all interfaces (0.0.0.0). Ensure this is intentional.");
        }

        // itr#267: auto-open the default browser on first-run. Fire-and-
        // forget in its own task so a missing browser (CI, headless) can't
        // block — or crash — daemon startup.
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let browser = if opts.no_open {
            None
        } else {
            let db_path = config.db_path.clone();
            Some(tokio::spawn(crate::maybe_open_browser(
                db_path, opts.host, opts.port, opts.dev, ready_rx,
            )))
        };

        let host = opts.host;
        let port = opts.port;
        let dev = opts.dev;
        let profile = opts.auth_profile;
        let web_log_store = log_store.clone();
        let serve = tokio::spawn(async move {
            if let Err(e) = wisphive_web::serve_with_readiness(
                socket_path,
                port,
                dev,
                host,
                profile,
                Some(web_log_store),
                Some(ready_tx),
            )
            .await
            {
                tracing::error!("embedded web server exited: {e}");
            }
        });
        (Some(serve), browser)
    } else {
        (None, None)
    };

    // Run the server (blocks until shutdown)
    let server = Server::new(config).await?;
    server.run(shutdown_rx).await?;

    // Stop the web task if it's still running.
    if let Some(handle) = web_handle {
        handle.abort();
    }
    // And the first-run browser task, in case the user set the password
    // between the check and the server exiting.
    if let Some(handle) = browser_handle {
        handle.abort();
    }

    info!("wisphive daemon stopped cleanly");

    // `process::exit` below skips Rust destructors, so remove the PID file
    // explicitly while this clean shutdown path still owns the guard.
    drop(pid_guard);

    // Flush any buffered tracing output before force-exiting so the last
    // log line actually reaches the terminal.
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    // Drop the non-blocking file appender's worker guard so its background
    // thread flushes pending log records to disk before we force-exit.
    drop(log_guards);

    // Force-exit to guarantee the shell regains control. Without this, any
    // detached std::thread PTY readers or stuck spawn_blocking tasks can
    // keep the process alive indefinitely after main returns, leaving the
    // user's terminal frozen until they `kill -9` the daemon.
    std::process::exit(0);
}

/// Stop the running daemon by sending SIGTERM.
pub async fn stop() -> Result<()> {
    let config = DaemonConfig::default_location();

    if !config.pid_path.exists() {
        eprintln!("Daemon is not running (no PID file found)");
        return Ok(());
    }

    let pid_str = std::fs::read_to_string(&config.pid_path)?;
    let pid: i32 = pid_str.trim().parse()?;

    #[cfg(unix)]
    {
        let result = unsafe { libc::kill(pid, libc::SIGTERM) };
        if result == 0 {
            eprintln!("Sent SIGTERM to daemon (pid: {})", pid);
        } else {
            eprintln!(
                "Failed to send signal to pid {}. Daemon may not be running.",
                pid
            );
            // Clean up stale PID file
            let _ = std::fs::remove_file(&config.pid_path);
        }
    }

    #[cfg(not(unix))]
    {
        eprintln!("Stop not supported on this platform");
    }

    Ok(())
}

/// Show daemon status.
pub async fn status() -> Result<()> {
    let config = DaemonConfig::default_location();

    if !config.pid_path.exists() {
        eprintln!("Daemon: not running");
        return Ok(());
    }

    let pid_str = std::fs::read_to_string(&config.pid_path)?;
    let pid: i32 = pid_str.trim().parse()?;

    #[cfg(unix)]
    {
        if process_exists(pid) {
            eprintln!("Daemon: running (pid: {})", pid);
        } else {
            eprintln!("Daemon: not running (stale PID file)");
        }
    }

    let socket_exists = config.socket_path.exists();
    eprintln!(
        "Socket: {}",
        if socket_exists {
            "present"
        } else {
            "not found"
        }
    );
    eprintln!("Database: {}", config.db_path.display());

    Ok(())
}

#[cfg(unix)]
fn process_exists(pid: i32) -> bool {
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }

    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
