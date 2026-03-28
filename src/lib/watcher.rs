//! File system watcher (notify crate).

use notify::{RecursiveMode, Result, Watcher};
use std::path::Path;

pub struct FileWatcher {
    watcher: Box<dyn Watcher>,
}

impl FileWatcher {
    pub fn new<F>(mut callback: F) -> Result<Self>
    where
        F: FnMut(notify::Event) + Send + 'static,
    {
        let watcher = notify::recommended_watcher(move |res| {
            if let Ok(event) = res {
                callback(event);
            }
        })?;

        Ok(Self {
            watcher: Box::new(watcher),
        })
    }

    pub fn watch(&mut self, path: &Path) -> Result<()> {
        self.watcher.watch(path, RecursiveMode::Recursive)
    }
}
