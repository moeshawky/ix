//! Configuration loading for ix.
//!
//! The daemon discovers `.ixd.toml` files to scope its watch and index
//! behaviour. Each config file specifies which subdirectories to watch
//! and which patterns to exclude.

use crate::builder::Builder;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// ix runtime configuration, loaded from `.ixd.toml`.
/// Nested `[watch]` table for backward-compatibility with alternative `.ixd.toml` styles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WatchSection {
    /// Subdirectory paths to watch.
    #[serde(default)]
    pub paths: Vec<PathBuf>,
    /// Debounce interval in milliseconds.
    #[serde(default)]
    pub debounce_ms: Option<u64>,
    /// Ignore patterns specified in `[watch]` table.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Exclude patterns specified in `[watch]` table.
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
}

/// Nested `[build]` table for backward-compatibility with alternative `.ixd.toml` styles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BuildSection {
    /// Exclude patterns specified in `[build]` table.
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
    /// Ignore patterns specified in `[build]` table.
    #[serde(default)]
    pub ignore: Vec<String>,
}

/// ix runtime configuration, loaded from `.ixd.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Root directories to watch for indexing.
    #[serde(default)]
    pub watch_roots: Vec<PathBuf>,
    /// Directory component exclusion patterns passed to
    /// [`crate::builder::Builder::with_exclude_patterns`] and
    /// [`crate::watcher::Watcher`].
    ///
    /// Matched against path directory components by `builder::is_path_admitted`
    /// (plus built-in `.git`, `.ix`, `.codegraph`, and `lost+found` exclusions).
    /// When omitted in `.ixd.toml`, `#[serde(default)]` yields an empty `Vec`,
    /// whereas [`Config::default`] populates `[".codegraph", ".git",
    /// "node_modules", "target"]`.
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

    /// Deprecated `[watch]` table for backward compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch: Option<WatchSection>,

    /// Deprecated `[build]` table for backward compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildSection>,
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
                ".codegraph".to_string(),
                ".git".to_string(),
                "node_modules".to_string(),
                "target".to_string(),
            ],
            debounce_ms: None,
            watch: None,
            build: None,
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
        let mut cfg: Self = toml::from_str(&content).map_err(|e| {
            crate::error::Error::Config(format!("cannot parse config file {}: {e}", path.display()))
        })?;
        for key in unknown_toml_keys(&content) {
            tracing::warn!(
                "unknown key `{key}` in config file {} (ignored)",
                path.display()
            );
        }
        cfg.normalize_and_warn(path);
        Ok(cfg)
    }

    /// Normalize legacy nested `[watch]` and `[build]` tables into flat fields,
    /// emitting a deprecation warning if nested tables are present.
    fn normalize_and_warn(&mut self, path: &Path) {
        let has_nested = self.watch.is_some() || self.build.is_some();
        if has_nested {
            tracing::warn!(
                "deprecated .ixd.toml format in {}: [watch]/[build] tables should be migrated to flat keys (see docs/.ixd.toml.md)",
                path.display()
            );
        }

        if let Some(ref w) = self.watch {
            if self.watch_roots.is_empty() {
                self.watch_roots.clone_from(&w.paths);
            } else {
                self.watch_roots.extend(w.paths.clone());
            }

            if self.debounce_ms.is_none() && w.debounce_ms.is_some() {
                self.debounce_ms = w.debounce_ms;
            }

            self.exclude_patterns.extend(w.ignore.clone());
            self.exclude_patterns.extend(w.exclude_patterns.clone());
        }

        if let Some(ref b) = self.build {
            self.exclude_patterns.extend(b.exclude_patterns.clone());
            self.exclude_patterns.extend(b.ignore.clone());
        }

        // Deduplicate
        self.watch_roots.sort();
        self.watch_roots.dedup();
        self.exclude_patterns.sort();
        self.exclude_patterns.dedup();
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
    /// Returns an error if the root-level `.ixd.toml` exists but cannot be read
    /// or parsed. Missing config files are skipped, and malformed subdirectory
    /// `.ixd.toml` files emit a warning via `tracing::warn!` and are skipped.
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
                    if config_path.exists() {
                        match Self::load(&config_path) {
                            Ok(mut sub_config) => {
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
                            Err(e) => {
                                tracing::warn!(
                                    "ixd: failed to parse sub-config {}: {e}",
                                    config_path.display()
                                );
                            }
                        }
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

    /// Apply this configuration's `exclude_patterns` and `watch_roots`
    /// to a [`Builder`], returning the configured builder.
    ///
    /// This is the single source of truth for wiring config → builder,
    /// used by both the CLI (`ix --build`) and the daemon (`ixd`).
    /// Ensures `--build` and the daemon scope identically (audit F2).
    #[must_use]
    pub fn apply_to_builder(&self, mut builder: Builder) -> Builder {
        if !self.exclude_patterns.is_empty() {
            builder = builder.with_exclude_patterns(self.exclude_patterns.clone());
        }
        if !self.watch_roots.is_empty() {
            builder = builder.with_watch_roots(self.watch_roots.clone());
        }
        builder
    }
}

/// Collect unknown keys in a `.ixd.toml` document so typos such as
/// `watch_root` or `exclude` can be surfaced via `tracing::warn!`.
fn unknown_toml_keys(content: &str) -> Vec<String> {
    const ROOT_KEYS: &[&str] = &[
        "watch_roots",
        "exclude_patterns",
        "debounce_ms",
        "watch",
        "build",
    ];
    const WATCH_KEYS: &[&str] = &["paths", "debounce_ms", "ignore", "exclude_patterns"];
    const BUILD_KEYS: &[&str] = &["exclude_patterns", "ignore"];

    let Ok(toml::Value::Table(root)) = toml::from_str::<toml::Value>(content) else {
        return Vec::new();
    };

    let mut unknown = Vec::new();
    for (key, val) in &root {
        if !ROOT_KEYS.contains(&key.as_str()) {
            unknown.push(key.clone());
        } else if key == "watch"
            && let Some(sub) = val.as_table()
        {
            for sub_key in sub.keys() {
                if !WATCH_KEYS.contains(&sub_key.as_str()) {
                    unknown.push(format!("watch.{sub_key}"));
                }
            }
        } else if key == "build"
            && let Some(sub) = val.as_table()
        {
            for sub_key in sub.keys() {
                if !BUILD_KEYS.contains(&sub_key.as_str()) {
                    unknown.push(format!("build.{sub_key}"));
                }
            }
        }
    }
    unknown
}

#[cfg(test)]
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
                    ".codegraph".to_string(),
                    ".git".to_string(),
                    "node_modules".to_string(),
                    "target".to_string(),
                ],
                debounce_ms: None,
                watch: None,
                build: None,
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

    #[test]
    fn test_config_nested_table_compatibility() {
        let toml_str = r#"
[watch]
debounce_ms = 50
paths = ["src/", "tests/"]
ignore = ["*.pyc", "__pycache__", ".git"]

[build]
exclude_patterns = ["build/", "dist/"]
"#;
        let mut config: Config = toml::from_str(toml_str).unwrap();
        config.normalize_and_warn(Path::new(".ixd.toml"));

        assert_eq!(config.debounce_ms, Some(50));
        assert_eq!(
            config.watch_roots,
            vec![PathBuf::from("src/"), PathBuf::from("tests/")]
        );
        assert_eq!(
            config.exclude_patterns,
            vec![
                "*.pyc".to_string(),
                ".git".to_string(),
                "__pycache__".to_string(),
                "build/".to_string(),
                "dist/".to_string(),
            ]
        );
    }

    #[test]
    fn test_unknown_toml_keys_detected() {
        let valid = r#"
watch_roots = ["src"]
exclude_patterns = ["target"]
debounce_ms = 200

[watch]
paths = ["lib"]
debounce_ms = 50
ignore = ["tmp"]
exclude_patterns = ["out"]

[build]
exclude_patterns = ["dist"]
ignore = ["cache"]
"#;
        assert!(unknown_toml_keys(valid).is_empty());

        let with_typos = r#"
watch_root = ["src"]

[watch]
unknown_watch = 1

[build]
exclude = ["dist"]
"#;
        let unknown = unknown_toml_keys(with_typos);
        assert_eq!(
            unknown,
            vec![
                "build.exclude".to_string(),
                "watch.unknown_watch".to_string(),
                "watch_root".to_string(),
            ]
        );
    }

    #[test]
    fn test_load_unknown_keys_does_not_fail_and_preserves_fields() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join(".ixd.toml");
        std::fs::write(
            &cfg_path,
            "watch_roots = [\"src\"]\nwatch_root = [\"bad\"]\n",
        )
        .unwrap();

        let cfg = Config::load(&cfg_path).unwrap();
        assert_eq!(cfg.watch_roots, vec![PathBuf::from("src")]);
        assert!(cfg.exclude_patterns.is_empty());
    }

    #[test]
    fn test_discover_under_root_error_and_subdir_warn_skip() {
        let dir = tempfile::tempdir().unwrap();
        let sub_bad = dir.path().join("bad_sub");
        let sub_good = dir.path().join("good_sub");
        std::fs::create_dir_all(&sub_bad).unwrap();
        std::fs::create_dir_all(&sub_good).unwrap();
        std::fs::write(sub_bad.join(".ixd.toml"), "invalid = [unclosed").unwrap();
        std::fs::write(sub_good.join(".ixd.toml"), "watch_roots = [\"inner\"]\n").unwrap();

        let cfg = Config::discover_under(dir.path()).unwrap();
        assert_eq!(cfg.watch_roots, vec![sub_good.join("inner")]);

        std::fs::write(dir.path().join(".ixd.toml"), "broken = [unclosed").unwrap();
        assert!(Config::discover_under(dir.path()).is_err());
    }
}
