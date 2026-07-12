//! Shared trusted-file reads for policy inputs.
//!
//! A trusted read checks the opened descriptor rather than path metadata, so a
//! rename or symlink swap cannot create a check-then-open window. The policy
//! rule intentionally permits readable files such as `0644`; configurations do
//! not contain secrets, but another local account must not be able to write one.

use std::path::Path;

/// Why a file could not be trusted as an operator-owned policy input.
#[derive(Debug)]
pub enum TrustError {
    /// The file belongs to a different effective user.
    NotOwned,
    /// The file is writable by its group or by other users.
    LooseMode(u32),
    /// The opened descriptor was not a regular file.
    NotARegularFile,
    /// Opening or reading the file failed.
    Io(std::io::Error),
}

impl std::fmt::Display for TrustError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotOwned => f.write_str("file is not owned by the effective user"),
            Self::LooseMode(mode) => write!(
                f,
                "file permissions are {mode:#06o}; group or other users may write it"
            ),
            Self::NotARegularFile => f.write_str("file is not a regular file"),
            Self::Io(error) => write!(f, "file I/O failed: {error}"),
        }
    }
}

impl std::error::Error for TrustError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::NotOwned | Self::LooseMode(_) | Self::NotARegularFile => None,
        }
    }
}

/// Read a regular, effective-user-owned file that group and other users cannot
/// write. On Unix this inspects metadata on the opened descriptor, avoiding a
/// path-based time-of-check/time-of-use gap.
#[cfg(unix)]
pub fn read_trusted(path: &Path) -> Result<String, TrustError> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_RDONLY | libc::O_NONBLOCK)
        .open(path)
        .map_err(TrustError::Io)?;
    let metadata = file.metadata().map_err(TrustError::Io)?;

    if !metadata.file_type().is_file() {
        return Err(TrustError::NotARegularFile);
    }

    // SAFETY: `geteuid` takes no arguments and has no preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() != effective_uid {
        return Err(TrustError::NotOwned);
    }

    let mode = metadata.mode() & 0o7777;
    if mode & 0o022 != 0 {
        return Err(TrustError::LooseMode(mode));
    }

    let mut contents = String::new();
    file.read_to_string(&mut contents).map_err(TrustError::Io)?;
    Ok(contents)
}

/// Non-Unix platforms do not have the descriptor and ownership semantics used
/// by the Unix implementation, so preserve the existing plain-read behavior.
#[cfg(not(unix))]
pub fn read_trusted(path: &Path) -> Result<String, TrustError> {
    std::fs::read_to_string(path).map_err(TrustError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn rejects_group_writable() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o664)).unwrap();

        assert!(matches!(read_trusted(&path), Err(TrustError::LooseMode(_))));
    }

    #[test]
    #[cfg(unix)]
    fn rejects_world_writable() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o646)).unwrap();

        assert!(matches!(read_trusted(&path), Err(TrustError::LooseMode(_))));
    }

    #[test]
    #[cfg(unix)]
    fn rejects_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.json");
        let path = directory.path().join("config.json");
        std::fs::write(&target, "{}").unwrap();
        symlink(&target, &path).unwrap();

        assert!(matches!(read_trusted(&path), Err(TrustError::Io(_))));
    }

    #[test]
    #[cfg(unix)]
    #[ignore = "requires a foreign-owned fixture; unprivileged tests cannot chown"]
    fn rejects_foreign_owner_is_untestable_so_skip() {
        // A process without elevated privileges cannot create a stable
        // foreign-owned fixture. The fd-level uid comparison is covered by the
        // implementation and exercised in integration environments that can.
    }

    #[test]
    #[cfg(unix)]
    fn accepts_0644() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        std::fs::write(&path, "{\"trusted\":true}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert_eq!(read_trusted(&path).unwrap(), "{\"trusted\":true}");
    }

    #[test]
    #[cfg(unix)]
    fn accepts_0600() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        std::fs::write(&path, "{\"trusted\":true}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        assert_eq!(read_trusted(&path).unwrap(), "{\"trusted\":true}");
    }
}
