use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Maximum byte length permitted for `XDG_RUNTIME_DIR` / `HOME` socket paths
/// so the path fits inside `sockaddr_un::sun_path` (104 bytes on macOS, 108 on
/// Linux) including the trailing NUL byte.
const MAX_UNIX_SOCKET_PATH_BYTES: usize = 100;

/// Resolves the socket path for a given watched root directory.
///
/// Tries in order:
/// 1. `$XDG_RUNTIME_DIR/ixd/{hash}.sock` (when non-empty, absolute, and `< 100` bytes)
/// 2. `$HOME/.local/run/ixd/{hash}.sock` (when non-empty, absolute, and `< 100` bytes)
/// 3. `/tmp/ixd-{uid}-{hash}.sock`
///
/// Where `hash` is the first 16 hex characters of `XXH64(canonical_root, 0)`.
#[must_use]
pub fn socket_path(root: &Path) -> PathBuf {
    let xdg = std::env::var("XDG_RUNTIME_DIR").ok();
    let home = std::env::var("HOME").ok();
    socket_path_from_env(root, xdg.as_deref(), home.as_deref())
}

/// Resolves the socket path using explicit environment values rather than
/// reading process-wide environment variables directly.
pub(super) fn socket_path_from_env(
    root: &Path,
    xdg_runtime_dir: Option<&str>,
    home_dir: Option<&str>,
) -> PathBuf {
    let canonical = match root.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("cannot canonicalize root {}: {e}", root.display());
            root.to_path_buf()
        }
    };
    let hash = format!(
        "{:016x}",
        xxhash_rust::xxh64::xxh64(canonical.to_string_lossy().as_bytes(), 0,)
    );

    if let Some(xdg) = xdg_runtime_dir
        && !xdg.is_empty()
        && Path::new(xdg).is_absolute()
    {
        let candidate = PathBuf::from(xdg).join("ixd").join(format!("{hash}.sock"));
        if candidate.as_os_str().as_bytes().len() < MAX_UNIX_SOCKET_PATH_BYTES {
            return candidate;
        }
    }

    if let Some(home) = home_dir
        && !home.is_empty()
        && Path::new(home).is_absolute()
    {
        let candidate = PathBuf::from(home)
            .join(".local/run/ixd")
            .join(format!("{hash}.sock"));
        if candidate.as_os_str().as_bytes().len() < MAX_UNIX_SOCKET_PATH_BYTES {
            return candidate;
        }
    }

    #[cfg(unix)]
    // SAFETY: libc::getuid() is safe on all POSIX-compliant systems — it
    // always succeeds and has no observable side effects. The return type
    // uid_t fits in u32 on all supported platforms.
    let uid = unsafe { libc::getuid() };
    #[cfg(not(unix))]
    let uid = 0u32;
    PathBuf::from(format!("/tmp/ixd-{uid}-{hash}.sock"))
}

/// Ensure the parent directory of a socket path exists with private (`0o700`)
/// permissions for daemon-owned directories.
pub(super) fn ensure_socket_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        if parent != Path::new("/tmp") && parent != std::env::temp_dir() {
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_relative_env_paths() {
        let root = Path::new("/tmp/ix-rslv-test");
        let p = socket_path_from_env(root, Some(""), Some("relative/home"));
        assert!(
            p.parent() == Some(Path::new("/tmp"))
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|s| s.starts_with("ixd-")),
            "empty/relative XDG & HOME must fall back to /tmp, got {}",
            p.display()
        );
    }

    #[test]
    fn rejects_oversized_sun_path_and_falls_back() {
        let root = Path::new("/tmp/ix-rslv-long");
        let long_xdg = format!("/{}", "a".repeat(95));
        let p = socket_path_from_env(root, Some(&long_xdg), Some("/tmp/short-home"));
        assert!(
            p.starts_with("/tmp/short-home/.local/run/ixd/"),
            "oversized XDG path must fall back to HOME, got {}",
            p.display()
        );
        assert!(p.as_os_str().as_bytes().len() < MAX_UNIX_SOCKET_PATH_BYTES);

        let long_home = format!("/{}", "b".repeat(95));
        let p2 = socket_path_from_env(root, Some(&long_xdg), Some(&long_home));
        assert!(
            p2.parent() == Some(Path::new("/tmp"))
                && p2
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|s| s.starts_with("ixd-")),
            "oversized XDG and HOME must fall back to /tmp, got {}",
            p2.display()
        );
    }

    #[test]
    fn ensure_socket_dir_sets_private_mode() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sock = tmp.path().join("ixd").join("abc.sock");
        ensure_socket_dir(&sock).expect("ensure_socket_dir");
        let meta = std::fs::metadata(sock.parent().expect("parent")).expect("metadata");
        assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    }
}
