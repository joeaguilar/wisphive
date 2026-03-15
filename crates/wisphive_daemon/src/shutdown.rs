use anyhow::Result;
use tokio::sync::watch;
use tracing::info;

/// Create a shutdown signal pair.
/// Returns (sender, receiver). Drop or send on the sender to trigger shutdown.
pub fn shutdown_channel() -> (watch::Sender<bool>, watch::Receiver<bool>) {
    watch::channel(false)
}

/// Install OS signal handlers (SIGTERM, SIGINT) that trigger the shutdown sender.
pub async fn wait_for_signal(shutdown_tx: watch::Sender<bool>) {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");

        tokio::select! {
            _ = ctrl_c => {
                info!("received SIGINT");
            }
            _ = sigterm.recv() => {
                info!("received SIGTERM");
            }
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.expect("failed to listen for ctrl-c");
        info!("received SIGINT");
    }

    let _ = shutdown_tx.send(true);
    info!("shutdown signal sent");
}

/// Write a PID file. Returns a guard that removes the file on drop.
pub fn write_pid_file(path: &std::path::Path) -> Result<PidGuard> {
    let pid = std::process::id();
    std::fs::write(path, pid.to_string())?;
    info!("PID file written: {} (pid: {})", path.display(), pid);
    Ok(PidGuard {
        path: path.to_path_buf(),
    })
}

/// Check if another daemon is already running by reading the PID file.
pub fn check_existing_daemon(path: &std::path::Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let contents = std::fs::read_to_string(path)?;
    let pid: u32 = contents.trim().parse()?;

    // Check if process is alive
    #[cfg(unix)]
    {
        // kill -0 checks if process exists without sending a signal
        let result = unsafe { libc::kill(pid as i32, 0) };
        if result == 0 {
            anyhow::bail!(
                "another daemon is already running (pid: {}). \
                 If this is stale, remove {}",
                pid,
                path.display()
            );
        }
    }

    // Stale PID file — remove it
    info!("removing stale PID file (pid: {})", pid);
    std::fs::remove_file(path)?;
    Ok(())
}

/// Guard that removes the PID file when dropped.
pub struct PidGuard {
    path: std::path::PathBuf,
}

impl Drop for PidGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        // Can't use tracing here (may be shut down), so best-effort silent cleanup.
    }
}
