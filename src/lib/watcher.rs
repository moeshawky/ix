//! File system watcher (notify crate) with debouncing.

use crate::error::Result;
use crossbeam_channel::Receiver;
use llmosafe::ResourceGuard;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher as _};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// A file-system watcher that detects changes in a directory tree and
/// batches them into debounced event batches.
///
/// When `watch_roots` is non-empty, only events whose parent path falls
/// within a watch root are collected. `exclude_patterns` filter out
/// directory entries from the manual-walk fallback path.
pub struct Watcher {
    root: PathBuf,
    watch_roots: Vec<PathBuf>,
    exclude_patterns: Vec<String>,
    debounce_ms: u64,
    inner: Option<RecommendedWatcher>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl Watcher {
    /// Create a new watcher for the given root directory.
    ///
    /// - `root`: absolute or relative path to the directory tree to watch.
    /// - `watch_roots`: if non-empty, only events whose path has a parent in
    ///   `watch_roots` are collected. Empty means watch everything.
    /// - `exclude_patterns`: directory names to skip in the manual-walk
    ///   fallback (applied in addition to built-in exclusions).
    ///
    /// The watcher is not started yet; call [`Watcher::start`] to begin
    /// receiving events.
    #[must_use]
    pub fn new(root: &Path, watch_roots: &[PathBuf], exclude_patterns: &[String]) -> Self {
        Self {
            root: root.to_owned(),
            watch_roots: watch_roots.to_vec(),
            // Watcher receives exclude_patterns from its caller — the daemon
            // (daemon.rs:226) passes Config's patterns here. Hardcoded
            // patterns in the fallback walk below are Watcher-specific and
            // not driven by Config.
            // See also: src/lib/config.rs:39, src/lib/builder.rs:226
            exclude_patterns: exclude_patterns.to_vec(),
            debounce_ms: 500,
            inner: None,
            join_handle: None,
        }
    }

    /// Set the debounce interval in milliseconds.
    ///
    /// Minimum 50 ms. Maximum 10000 ms (10 s). Values outside this range are clamped.
    #[must_use]
    pub fn with_debounce(mut self, ms: u64) -> Self {
        self.debounce_ms = ms.clamp(50, 10_000);
        self
    }

    /// Start watching the file system for changes.
    ///
    /// # Errors
    ///
    /// Returns an error if the filesystem watcher cannot be created.
    #[allow(clippy::too_many_lines)]
    pub fn start(&mut self) -> Result<Receiver<Vec<PathBuf>>> {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (event_tx, event_rx) = mpsc::channel();

        let mut watcher = RecommendedWatcher::new(event_tx, Config::default())?;

        // Attempt recursive watch on root first (most efficient)
        if let Err(err) = watcher.watch(&self.root, RecursiveMode::Recursive) {
            eprintln!("ix: warning: recursive watch failed: {err}. Falling back to manual walk.");

            let exclude_patterns = self.exclude_patterns.clone();
            let walker = ignore::WalkBuilder::new(&self.root)
                .hidden(false)
                .git_ignore(true)
                .require_git(true) // within-repo .gitignore only; never ancestor ~/.gitignore (audit D4)
                .add_custom_ignore_filename(".ixignore")
                .filter_entry(move |entry| {
                    let path = entry.path();
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

                    // Built-in directory defaults
                    if entry.file_type().is_some_and(|t| t.is_dir())
                        && (name == "lost+found"
                            || name == ".git"
                            || name == "node_modules"
                            || name == "target"
                            || name == "__pycache__"
                            || name == ".tox"
                            || name == ".venv"
                            || name == "venv"
                            || name == ".ix"
                            || exclude_patterns.iter().any(|p| p == name))
                    {
                        return false;
                    }

                    // Built-in file noise defaults
                    if entry.file_type().is_some_and(|t| t.is_file()) {
                        if let Ok(metadata) = entry.metadata()
                            && metadata.len() > 10 * 1024 * 1024
                        {
                            return false;
                        }
                        if name == "Cargo.lock"
                            || name == "package-lock.json"
                            || name == "pnpm-lock.yaml"
                            || name == "shard.ix"
                            || name == "shard.ix.tmp"
                        {
                            return false;
                        }
                    }

                    // Built-in file extension defaults
                    if entry.file_type().is_some_and(|t| t.is_file()) {
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                        match ext {
                            // Binary extensions
                            "so" | "o" | "dylib" | "a" | "dll" | "exe" | "pyc" |
                            // Media
                            "jpg" | "png" | "gif" | "mp4" | "mp3" | "pdf" |
                            // Archives
                            "zip" | "7z" | "rar" |
                            // Data
                            "sqlite" | "db" | "bin" => return false,
                            _ => {}
                        }
                        if name.ends_with(".tar.gz") {
                            return false;
                        }
                    }
                    true
                })
                .build();

            let guard = ResourceGuard::auto(0.5);
            let mut file_count: u64 = 0;

            for result in walker {
                file_count += 1;

                // Memory pressure check every 250 files (same cadence as builder.rs)
                if file_count % 250 == 0 && guard.check().is_err() {
                    eprintln!(
                        "ix: critical memory pressure during watcher walk (ceiling breached) -- aborting."
                    );
                    break;
                }

                match result {
                    Ok(entry) => {
                        if entry.file_type().is_some_and(|t| t.is_dir()) {
                            let path = entry.path();
                            if let Err(e) = watcher.watch(path, RecursiveMode::NonRecursive) {
                                eprintln!(
                                    "ix: warning: watcher failed for {}: {}",
                                    path.display(),
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("ix: warning: watcher skipping path: {e}");
                    }
                }
            }
        }

        self.inner = Some(watcher);

        let watch_roots = self.watch_roots.clone();
        let ix_dir = self.root.join(".ix");
        let debounce_dur = Duration::from_millis(self.debounce_ms);
        let handle = thread::spawn(move || {
            let mut changed_paths: HashMap<PathBuf, notify::EventKind> = HashMap::new();
            loop {
                // Wait for the first event
                match event_rx.recv() {
                    Ok(Ok(event)) => {
                        Self::collect_paths(&mut changed_paths, event, &watch_roots, &ix_dir);

                        // Debounce loop: keep collecting for debounce_ms after the last event
                        loop {
                            match event_rx.recv_timeout(debounce_dur) {
                                Ok(Ok(event)) => {
                                    Self::collect_paths(
                                        &mut changed_paths,
                                        event,
                                        &watch_roots,
                                        &ix_dir,
                                    );
                                }
                                Ok(Err(_)) => {} // notify error, skip
                                Err(mpsc::RecvTimeoutError::Timeout) => {
                                    // Debounce period over
                                    if !changed_paths.is_empty() {
                                        let paths: Vec<PathBuf> =
                                            changed_paths.drain().map(|(p, _)| p).collect();
                                        if tx.send(paths).is_err() {
                                            return; // Receiver dropped
                                        }
                                    }
                                    break;
                                }
                                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                            }
                        }
                    }
                    Ok(Err(_)) => {}
                    Err(_) => return, // Watcher dropped
                }
            }
        });

        self.join_handle = Some(handle);
        Ok(rx)
    }

    /// Stop watching and join the background event-loop thread.
    pub fn stop(&mut self) {
        self.inner.take(); // Dropping the watcher stops events
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }

    /// Returns `true` while the underlying notify watcher is active.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.inner.is_some()
    }

    fn collect_paths(
        map: &mut HashMap<PathBuf, notify::EventKind>,
        event: Event,
        watch_roots: &[PathBuf],
        ix_dir: &Path,
    ) {
        let kind = event.kind;
        if kind.is_modify() || kind.is_create() || kind.is_remove() {
            for path in event.paths {
                if path.starts_with(ix_dir) {
                    continue;
                }
                if !watch_roots.is_empty()
                    && !watch_roots.iter().any(|wr| {
                        path.starts_with(wr) || path.parent().is_some_and(|p| p.starts_with(wr))
                    })
                {
                    continue;
                }
                let prev = map.get(&path);
                let should_insert = !matches!(prev, Some(notify::EventKind::Remove(_)));
                if should_insert {
                    map.insert(path, kind);
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::as_conversions, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::error::Error;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_watcher_basic() -> Result<()> {
        let dir = tempdir().map_err(Error::Io)?;
        let mut watcher = Watcher::new(dir.path(), &[], &[]);
        let rx = watcher.start()?;

        let file_path = dir.path().join("test.txt");
        {
            let mut file = File::create(&file_path).map_err(Error::Io)?;
            file.write_all(b"hello").map_err(Error::Io)?;
            file.sync_all().map_err(Error::Io)?;
        }

        let events = rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| Error::Config("Timeout waiting for watcher event".into()))?;

        if events.is_empty() {
            return Err(Error::Config("No watcher events received".into()));
        }
        if !events.iter().any(|p: &PathBuf| p.ends_with("test.txt")) {
            return Err(Error::Config("test.txt not found in watcher events".into()));
        }

        watcher.stop();
        Ok(())
    }
}
