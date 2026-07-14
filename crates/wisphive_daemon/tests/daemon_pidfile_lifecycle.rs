#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

struct DaemonChild(Child);

impl DaemonChild {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.0.try_wait()
    }
}

impl Drop for DaemonChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("daemon crate should live inside the workspace")
}

fn frontend_dist_is_available_for_cli_build() -> bool {
    let path = workspace_root().join("crates/wisphive_web/frontend/dist");
    if path.is_dir() {
        true
    } else {
        // rust-embed resolves this directory relative to the real web crate at
        // compile time, so it cannot be redirected into this test's tempdir.
        // Never fabricate it in the shared checkout: concurrent tests or a
        // SIGKILL could leave misleading state behind.
        eprintln!(
            "skipping daemon lifecycle test: {} is missing; run `just frontend-build` first",
            path.display()
        );
        false
    }
}

fn workspace_tempdir(prefix: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(workspace_root())
        .expect("create isolated temporary directory in the workspace")
}

fn short_tempdir(prefix: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in("/tmp")
        .expect("create isolated short-path temporary directory")
}

fn unix_sockets_are_available() -> bool {
    let temp = short_tempdir("wisphive-socket-probe-");
    match UnixListener::bind(temp.path().join("probe.sock")) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(error) => panic!("probe Unix socket support: {error}"),
    }
}

fn prepend_path(dir: &Path) -> OsString {
    let mut paths = vec![dir.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).expect("test PATH should contain valid paths")
}

fn build_wisphive_binaries(target_dir: &Path) -> (PathBuf, PathBuf) {
    let status = Command::new("cargo")
        .args([
            "build",
            "--quiet",
            "-p",
            "wisphive_cli",
            "-p",
            "wisphive_hook",
        ])
        .current_dir(workspace_root())
        // cargo test holds the workspace target-directory lock while this test
        // runs. An isolated target directory lets this test build the actual
        // CLI binary without waiting on itself.
        .env("CARGO_TARGET_DIR", target_dir)
        .status()
        .expect("cargo should build the real wisphive binaries for this integration test");
    assert!(
        status.success(),
        "building the real wisphive binaries failed"
    );

    (
        target_dir.join("debug/wisphive"),
        target_dir.join("debug/wisphive-hook"),
    )
}

fn wait_for_path(path: &Path, child: &mut DaemonChild) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        if let Some(status) = child.try_wait().expect("check daemon child status") {
            panic!("daemon exited before creating {}: {status}", path.display());
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("daemon did not create {} within 30 seconds", path.display());
}

fn wait_for_exit(child: &mut DaemonChild) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if child
            .try_wait()
            .expect("check daemon child status")
            .is_some()
        {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("daemon did not exit after wisphive daemon stop");
}

#[test]
fn daemon_start_stop_removes_pidfile_and_doctor_is_healthy_while_running() {
    if !unix_sockets_are_available() {
        // Some constrained CI sandboxes prohibit AF_UNIX outright. A daemon
        // cannot start there, so retain a passing suite without weakening the
        // real subprocess lifecycle coverage on normal Unix hosts.
        eprintln!("skipping daemon lifecycle test: AF_UNIX sockets are unavailable");
        return;
    }
    if !frontend_dist_is_available_for_cli_build() {
        return;
    }

    // Unix socket paths have a strict length cap, so HOME lives in a short
    // temporary directory rather than the (long) workspace checkout path.
    let home = short_tempdir("wisphive-test-home-");
    let wisphive_home = home.path().join(".wisphive");
    fs::create_dir(&wisphive_home).expect("create isolated Wisphive state directory");
    fs::set_permissions(&wisphive_home, fs::Permissions::from_mode(0o700))
        .expect("secure isolated Wisphive state directory");

    let build_target = tempfile::tempdir().expect("create isolated cargo target directory");
    let (wisphive, wisphive_hook) = build_wisphive_binaries(build_target.path());
    assert!(wisphive.is_file(), "wisphive CLI binary should exist");
    assert!(wisphive_hook.is_file(), "wisphive-hook binary should exist");

    // doctor checks both executables through PATH. Symlink the binaries into a
    // test-only bin directory so it never observes a developer installation.
    let bin_dir = home.path().join("bin");
    fs::create_dir(&bin_dir).expect("create isolated PATH directory");
    symlink(&wisphive, bin_dir.join("wisphive")).expect("link wisphive CLI");
    symlink(&wisphive_hook, bin_dir.join("wisphive-hook")).expect("link wisphive hook");
    let path = prepend_path(&bin_dir);

    let project = workspace_tempdir("wisphive-test-project-");
    let base = || {
        let mut command = Command::new(&wisphive);
        command
            .env("HOME", home.path())
            .env("PATH", &path)
            .current_dir(project.path());
        command
    };

    let hooks = base()
        .args(["hooks", "install", "--project"])
        .arg(project.path())
        .status()
        .expect("install hooks into isolated project");
    assert!(hooks.success(), "isolated hook installation should succeed");

    let enabled = base()
        .args(["hooks", "enable"])
        .status()
        .expect("enable isolated hook mode");
    assert!(enabled.success(), "isolated hook mode should enable");

    let mut daemon = DaemonChild(
        base()
            .args(["daemon", "start"])
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start the real wisphive daemon binary"),
    );
    let pid_path = wisphive_home.join("wisphive.pid");
    wait_for_path(&pid_path, &mut daemon);

    let doctor = base()
        .arg("doctor")
        .output()
        .expect("run doctor against the isolated HOME");
    assert!(
        doctor.status.success(),
        "doctor command should exit successfully"
    );
    let doctor_output = format!(
        "{}{}",
        String::from_utf8_lossy(&doctor.stdout),
        String::from_utf8_lossy(&doctor.stderr)
    );
    assert!(
        doctor_output.contains("All checks passed"),
        "doctor should report a healthy running daemon: {doctor_output}"
    );

    let stop = base()
        .args(["daemon", "stop"])
        .status()
        .expect("stop the real wisphive daemon binary");
    assert!(stop.success(), "daemon stop should succeed");
    wait_for_exit(&mut daemon);

    assert!(
        !pid_path.exists(),
        "a clean daemon stop must remove its pidfile instead of leaving stale state"
    );

    // After a deliberate stop, doctor correctly reports that no daemon is
    // running; the regression check here is specifically that it does not
    // misdiagnose a stale pidfile.
    let post_stop_doctor = base()
        .arg("doctor")
        .output()
        .expect("run doctor after the isolated daemon stops");
    let post_stop_output = format!(
        "{}{}",
        String::from_utf8_lossy(&post_stop_doctor.stdout),
        String::from_utf8_lossy(&post_stop_doctor.stderr)
    );
    assert!(
        !post_stop_output.contains("stale PID file"),
        "doctor must not report a stale pidfile after clean stop: {post_stop_output}"
    );
}
