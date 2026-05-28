//! Configuration loading for ix.
//!
//! The daemon discovers `.ixd.toml` files to scope its watch and index
//! behaviour. Each config file specifies which subdirectories to watch
//! and which patterns to exclude.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// ix runtime configuration, loaded from `.ixd.toml`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    /// Root directories to watch for indexing.
    #[serde(default)]
    pub watch_roots: Vec<PathBuf>,
    /// Glob patterns for paths to exclude from indexing.
    #[serde(default)]
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

impl Config {
    /// Load configuration from a `.ixd.toml` file at the given path.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or parsed.
    pub fn load(path: &Path) -> crate::error::Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            crate::error::Error::Config(format!("cannot read config file {}: {e}", path.display()))
        })?;
        toml::from_str(&content).map_err(|e| {
            crate::error::Error::Config(format!("cannot parse config file {}: {e}", path.display()))
        })
    }

    /// Discover `.ixd.toml` files under the given root directory by
    /// walking up to two levels deep.
    ///
    /// Returns the **merged** configuration: `exclude_patterns` from
    /// the root-level config are applied globally; `watch_roots` from
    /// each discovered file scope the daemon to those subdirectories.
    ///
    /// # Errors
    ///
    /// Returns an error only if a discovered file cannot be parsed.
    /// Missing or absent config files are silently skipped.
    pub fn discover_under(root: &Path) -> crate::error::Result<Self> {
        let root_config_path = root.join(".ixd.toml");
        let mut merged = if root_config_path.exists() {
            Self::load(&root_config_path)?
        } else {
            Self::default()
        };

        // Walk one level of subdirectories looking for `.ixd.toml`
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let sub_path = entry.path();
                if sub_path.is_dir() {
                    let config_path = sub_path.join(".ixd.toml");
                    if config_path.exists()
                        && let Ok(sub_config) = Self::load(&config_path)
                    {
                        if !sub_config.watch_roots.is_empty() {
                            merged.watch_roots.extend(sub_config.watch_roots);
                        }
                        merged
                            .exclude_patterns
                            .extend(sub_config.exclude_patterns.clone());
                    }
                }
            }
        }

        // Deduplicate
        merged.watch_roots.sort();
        merged.watch_roots.dedup();
        merged.exclude_patterns.sort();
        merged.exclude_patterns.dedup();

        Ok(merged)
    }
}
