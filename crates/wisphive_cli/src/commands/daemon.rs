use anyhow::Result;
use tracing::info;
use tracing_subscriber::EnvFilter;
use wisphive_daemon::DaemonConfig;
use wisphive_daemon::server::Server;
use wisphive_daemon::shutdown;

/// Start the daemon in the foreground (for now).
pub async fn start() -> Result<()> {
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

    // Run the server
    let server = Server::new(config).await?;
    server.run(shutdown_rx).await?;

    info!("wisphive daemon stopped cleanly");
    Ok(())
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
