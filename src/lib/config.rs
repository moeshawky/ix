//! Configuration loading for ix.
//!
//! The daemon discovers `.ixd.toml` files to scope its watch and index
//! behaviour. Each config file specifies which subdirectories to watch
//! and which patterns to exclude.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// ix runtime configuration, loaded from `.ixd.toml`.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Root directories to watch for indexing.
    #[serde(default)]
    pub watch_roots: Vec<PathBuf>,
    /// Glob patterns for paths to exclude from indexing.
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
    /// Debounce interval in milliseconds for file-watch events.
    ///
    /// Minimum 50 ms, maximum 10000 ms (clamped by [`crate::watcher::Watcher::with_debounce`]).
    /// `None` uses the default (500 ms).
    ///
    /// # Merged-config precedence
    ///
    /// When [`discover_under`](Self::discover_under) merges multiple `.ixd.toml`
    /// files, the **root-level** `debounce_ms` takes precedence over subdirectory
    /// configs. Subdirectory `debounce_ms` values are ignored to avoid conflicting
    /// timer strategies within a single daemon instance. If you need different
    /// debounce intervals per watched subtree, run separate `ixd` instances.
    #[serde(default)]
    pub debounce_ms: Option<u64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            watch_roots: Vec::new(),
            // Default exclude patterns for the `.ixd.toml` daemon config.
            // The daemon (daemon.rs) reads these and bridges them to both
            // Builder (via with_exclude_patterns) and Watcher (via Watcher::new).
            // See also: src/lib/builder.rs:226, src/lib/watcher.rs:40
            exclude_patterns: vec![
                ".git".to_string(),
                "node_modules".to_string(),
                "target".to_string(),
            ],
            debounce_ms: None,
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
            // Resolve root-level `watch_roots` against `root`. The Builder and
            // Watcher compare absolute file paths with `Path::starts_with`, so a
            // relative entry (e.g. `watch_roots = ["src"]`) would never match and
            // every file would be filtered out, yielding an empty index (audit D1).
            let mut cfg = Self::load(&root_config_path)?;
            cfg.watch_roots = cfg
                .watch_roots
                .into_iter()
                .map(|wr| if wr.is_absolute() { wr } else { root.join(wr) })
                .collect();
            cfg
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
                        && let Ok(mut sub_config) = Self::load(&config_path)
                    {
                        if !sub_config.watch_roots.is_empty() {
                            // Resolve subdir `watch_roots` against the subdir (not
                            // the root) so they survive the absolute-path match
                            // in Builder/Watcher (audit D1).
                            sub_config.watch_roots = sub_config
                                .watch_roots
                                .into_iter()
                                .map(|wr| {
                                    if wr.is_absolute() {
                                        wr
                                    } else {
                                        sub_path.join(wr)
                                    }
                                })
                                .collect();
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert_eq!(
            config,
            Config {
                watch_roots: Vec::new(),
                exclude_patterns: vec![
                    ".git".to_string(),
                    "node_modules".to_string(),
                    "target".to_string(),
                ],
                debounce_ms: None,
            }
        );
    }

    #[test]
    fn test_discover_under_resolves_relative_watch_roots() {
        // Regression for audit D1: `watch_roots = ["src", "test"]` are written
        // relative to the search root. `discover_under` must resolve them to
        // absolute paths, otherwise `Builder`/`Watcher` (which compare absolute
        // paths with `Path::starts_with`) never match and the index ends up
        // empty.
        let base = std::env::temp_dir().join(format!("ix_cfg_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&base);
        std::fs::write(
            base.join(".ixd.toml"),
            "watch_roots = [\"src\", \"test\"]\nexclude_patterns = [\".git\"]\n",
        )
        .unwrap();

        let cfg = Config::discover_under(&base).unwrap();
        assert_eq!(cfg.watch_roots.len(), 2);
        for wr in &cfg.watch_roots {
            assert!(wr.is_absolute(), "watch_root {wr:?} must be absolute");
            assert!(
                wr.starts_with(&base),
                "watch_root {wr:?} must be under the search root"
            );
        }
        assert!(cfg.watch_roots.contains(&base.join("src")));
        assert!(cfg.watch_roots.contains(&base.join("test")));

        let _ = std::fs::remove_dir_all(&base);
    }
}
