use anyhow::Result;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use wisphive_daemon::DaemonConfig;
use wisphive_daemon::server::Server;
use wisphive_daemon::shutdown;

/// Options for launching an embedded web UI alongside the daemon.
pub struct WebOptions {
    pub host: [u8; 4],
    pub port: u16,
    pub dev: bool,
}

/// Start the daemon in the foreground. Optionally also serve the web UI in
/// the same process so a single `wisphive daemon start --web` gets you both.
pub async fn start(web: Option<WebOptions>) -> Result<()> {
    let config = DaemonConfig::default_location();
    config.ensure_dirs()?;

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // Check for existing daemon
    shutdown::check_existing_daemon(&config.pid_path)?;

    // Write PID file (guard removes it on drop)
    let _pid_guard = shutdown::write_pid_file(&config.pid_path)?;

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
    let web_handle = if let Some(opts) = web {
        let socket_path = config.socket_path.clone();
        let addr = std::net::SocketAddr::from((opts.host, opts.port));
        info!(%addr, dev = opts.dev, "starting embedded web server");
        if opts.host == [0, 0, 0, 0] {
            warn!("web UI is listening on all interfaces (0.0.0.0). Ensure this is intentional.");
        }
        Some(tokio::spawn(async move {
            if let Err(e) =
                wisphive_web::serve(socket_path, opts.port, opts.dev, opts.host).await
            {
                tracing::error!("embedded web server exited: {e}");
            }
        }))
    } else {
        None
    };

    // Run the server (blocks until shutdown)
    let server = Server::new(config).await?;
    server.run(shutdown_rx).await?;

    // Stop the web task if it's still running.
    if let Some(handle) = web_handle {
        handle.abort();
    }

    info!("wisphive daemon stopped cleanly");

    // Flush any buffered tracing output before force-exiting so the last
    // log line actually reaches the terminal.
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

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
        let result = unsafe { libc::kill(pid, 0) };
        if result == 0 {
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
