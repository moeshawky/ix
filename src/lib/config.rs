//! Configuration loading for ix.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// ix runtime configuration.
#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    /// Root directories to watch for indexing.
    pub watch_roots: Vec<PathBuf>,
    /// Glob patterns for paths to exclude from indexing.
    pub exclude_patterns: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            watch_roots: Vec::new(),
            exclude_patterns: vec![
                ".git".to_string(),
                "node_modules".to_string(),
                "target".to_string(),
            ],
        }
    }
}
