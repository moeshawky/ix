//! File system watcher (notify crate) with debouncing.

use crate::error::Result;
use crossbeam_channel::Receiver;
use llmosafe::ResourceGuard;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher as _};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// A file-system watcher that detects changes in a directory tree and
/// batches them into debounced event batches.
pub struct Watcher {
    root: PathBuf,
    inner: Option<RecommendedWatcher>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl Watcher {
    /// Create a new watcher for the given root directory.
    ///
    /// The watcher is not started yet; call [`Watcher::start`] to begin
    /// receiving events.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            inner: None,
            join_handle: None,
        }
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

            let walker = ignore::WalkBuilder::new(&self.root)
                .hidden(false)
                .git_ignore(true)
                .require_git(false)
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
                            || name == ".ix")
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

        let handle = thread::spawn(move || {
            let mut changed_paths = HashSet::new();
            loop {
                // Wait for the first event
                match event_rx.recv() {
                    Ok(Ok(event)) => {
                        Self::collect_paths(&mut changed_paths, event);

                        // Debounce loop: keep collecting for 500ms after the last event
                        loop {
                            match event_rx.recv_timeout(Duration::from_millis(500)) {
                                Ok(Ok(event)) => {
                                    Self::collect_paths(&mut changed_paths, event);
                                }
                                Ok(Err(_)) => {} // notify error, skip
                                Err(mpsc::RecvTimeoutError::Timeout) => {
                                    // Debounce period over
                                    if !changed_paths.is_empty() {
                                        let paths: Vec<PathBuf> = changed_paths.drain().collect();
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

    fn collect_paths(set: &mut HashSet<PathBuf>, event: Event) {
        if event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove() {
            for path in event.paths {
                // Ignore the .ix directory changes to avoid loops
                if path.components().any(|c| c.as_os_str() == ".ix") {
                    continue;
                }
                set.insert(path);
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
        let mut watcher = Watcher::new(dir.path());
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
