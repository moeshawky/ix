use std::path::{Path, PathBuf};

/// Resolves the socket path for a given watched root directory.
///
/// Tries in order:
/// 1. `$XDG_RUNTIME_DIR/ixd/{hash}.sock`
/// 2. `$HOME/.local/run/ixd/{hash}.sock`
/// 3. `/tmp/ixd-{uid}-{hash}.sock`
///
/// Where `hash` is the first 16 hex characters of `XXH64(canonical_root, 0)`.
#[must_use]
pub fn socket_path(root: &Path) -> PathBuf {
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

    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        let dir = PathBuf::from(xdg).join("ixd");
        return dir.join(format!("{hash}.sock"));
    }

    if let Ok(home) = std::env::var("HOME") {
        let dir = PathBuf::from(home).join(".local/run/ixd");
        return dir.join(format!("{hash}.sock"));
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

/// Ensure the parent directory of a socket path exists.
pub(super) fn ensure_socket_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}
