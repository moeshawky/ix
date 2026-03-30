//! File system watcher (notify crate) with debouncing.

use crate::error::Result;
use crossbeam_channel::Receiver;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher as _};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub struct Watcher {
    root: PathBuf,
    watcher: Option<RecommendedWatcher>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Watcher {
    pub fn new(root: &Path) -> Result<Self> {
        Ok(Self {
            root: root.to_owned(),
            watcher: None,
            thread: None,
        })
    }

    pub fn start(&mut self) -> Result<Receiver<Vec<PathBuf>>> {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (event_tx, event_rx) = mpsc::channel();

        let mut watcher = RecommendedWatcher::new(event_tx, Config::default())?;
        
        // Instead of recursive watch which crashes on permission denied, 
        // we walk and add non-recursive watches to each directory.
        let walker = ignore::WalkBuilder::new(&self.root)
            .hidden(false)
            .git_ignore(true)
            .require_git(false)
            .add_custom_ignore_filename(".ixignore")
            .filter_entry(move |entry| {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                
                // Built-in directory defaults
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
                    && (name == "lost+found" || name == ".git" || name == "node_modules" || 
                       name == "target" || name == "__pycache__" || name == ".tox" || 
                       name == ".venv" || name == "venv" || name == ".ix") 
                {
                    return false;
                }

                // Built-in file extension defaults
                if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
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

        for result in walker {
            match result {
                Ok(entry) => {
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        let path = entry.path();
                        if let Err(e) = watcher.watch(path, RecursiveMode::NonRecursive) {
                            eprintln!("ix: warning: watcher failed for {}: {}", path.display(), e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("ix: warning: watcher skipping path: {}", e);
                }
            }
        }

        self.watcher = Some(watcher);

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
                                Ok(Err(_)) => continue, // notify error, skip
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
                    Ok(Err(_)) => continue,
                    Err(_) => return, // Watcher dropped
                }
            }
        });

        self.thread = Some(handle);
        Ok(rx)
    }

    pub fn stop(&mut self) {
        self.watcher.take(); // Dropping the watcher stops events
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }

    pub fn is_running(&self) -> bool {
        self.watcher.is_some()
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
mod tests {
    use super::*;
    use crate::error::Error;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_watcher_basic() -> Result<()> {
        let dir = tempdir().map_err(Error::Io)?;
        let mut watcher = Watcher::new(dir.path())?;
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

        assert!(!events.is_empty());
        assert!(events.iter().any(|p: &PathBuf| p.ends_with("test.txt")));

        watcher.stop();
        Ok(())
    }
}
