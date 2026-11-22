//! File system watcher (notify crate) with debouncing.

use crate::error::Result;
use crossbeam_channel::{Receiver, Sender};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher as _};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::thread;
use std::time::{Duration, Instant};

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
    inner: Option<Arc<Mutex<RecommendedWatcher>>>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl Watcher {
    /// Create a new watcher for the given root directory.
    ///
    /// - `root`: absolute or relative path to the directory tree to watch.
    /// - `watch_roots`: if non-empty, only events whose path has a parent in
    ///   `watch_roots` are collected. Empty means watch everything. Paths are
    ///   normalized and canonicalized against `root`.
    /// - `exclude_patterns`: directory names to skip in the manual-walk
    ///   fallback (applied in addition to built-in exclusions).
    ///
    /// The watcher is not started yet; call [`Watcher::start`] to begin
    /// receiving events.
    #[must_use]
    pub fn new(root: &Path, watch_roots: &[PathBuf], exclude_patterns: &[String]) -> Self {
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let normalized_watch_roots = watch_roots
            .iter()
            .map(|wr| {
                let abs = if wr.is_relative() {
                    canonical_root.join(wr)
                } else {
                    wr.clone()
                };
                abs.canonicalize().unwrap_or_else(|_| {
                    abs.strip_prefix(root)
                        .map_or_else(|_| abs.clone(), |rel| canonical_root.join(rel))
                })
            })
            .collect();
        Self {
            root: canonical_root,
            watch_roots: normalized_watch_roots,
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
    pub fn start(&mut self) -> Result<Receiver<Vec<PathBuf>>> {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (event_tx, event_rx) = mpsc::channel();

        let mut watcher = RecommendedWatcher::new(event_tx, Config::default())?;
        let mut fallback_mode = false;

        // Attempt recursive watch on root first (most efficient)
        if let Err(err) = watcher.watch(&self.root, RecursiveMode::Recursive) {
            eprintln!("ix: warning: recursive watch failed: {err}. Falling back to manual walk.");
            fallback_mode = true;
            register_non_recursive_dirs(
                &mut watcher,
                &self.root,
                &self.exclude_patterns,
                &self.watch_roots,
            );
        }

        let watcher_arc = Arc::new(Mutex::new(watcher));
        let fallback_watcher = if fallback_mode {
            Some(Arc::downgrade(&watcher_arc))
        } else {
            None
        };
        self.inner = Some(watcher_arc);

        let root_clone = self.root.clone();
        let watch_roots = self.watch_roots.clone();
        let exclude_patterns = self.exclude_patterns.clone();
        let debounce_dur = Duration::from_millis(self.debounce_ms);
        let handle = thread::spawn(move || {
            let ctx = EventLoopCtx {
                root: root_clone,
                watch_roots,
                exclude_patterns,
                debounce_dur,
                fallback_watcher,
            };
            run_event_loop(&event_rx, &tx, &ctx);
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

    /// Collects paths from notify events that pass the admission policy.
    ///
    /// Returns `true` if at least one path was admitted and inserted into `map`,
    /// or `false` when all paths were excluded or the event kind is ignored.
    /// Missing/deleted paths are still screened against directory exclusions so
    /// removed excluded directories (e.g. `.git`, `.ix`) do not trigger updates.
    pub fn collect_paths(
        map: &mut HashMap<PathBuf, notify::EventKind>,
        event: Event,
        root: &Path,
        watch_roots: &[PathBuf],
        exclude_patterns: &[String],
    ) -> bool {
        let kind = event.kind;
        if !(kind.is_modify() || kind.is_create() || kind.is_remove()) {
            return false;
        }
        let mut admitted = false;
        for path in event.paths {
            let meta = std::fs::metadata(&path);
            let is_dir = meta.as_ref().is_ok_and(std::fs::Metadata::is_dir);
            if meta.is_err() && is_known_dir_exclusion(&path, root, exclude_patterns) {
                continue;
            }
            if !crate::builder::is_path_admitted(
                &path,
                is_dir,
                Some(root),
                exclude_patterns,
                watch_roots,
            ) {
                continue;
            }
            // Treat the latest event as authoritative: a file deleted then
            // recreated within one debounce window must end up indexed as
            // its final (created/modified) state, not as a deletion.
            // Previously a `Remove` "stuck" and dropped any later
            // Create/Modify for the same path (C G-EDGE-2).
            map.insert(path, kind);
            admitted = true;
        }
        admitted
    }
}

struct EventLoopCtx {
    root: PathBuf,
    watch_roots: Vec<PathBuf>,
    exclude_patterns: Vec<String>,
    debounce_dur: Duration,
    fallback_watcher: Option<Weak<Mutex<RecommendedWatcher>>>,
}

fn is_known_dir_exclusion(path: &Path, root: &Path, exclude_patterns: &[String]) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    for component in rel.components() {
        if let std::path::Component::Normal(os_str) = component {
            let name = os_str.to_string_lossy();
            if matches!(name.as_ref(), "lost+found" | ".git" | ".ix" | ".codegraph")
                || exclude_patterns.iter().any(|p| {
                    let trimmed = p.trim_end_matches('/');
                    !trimmed.is_empty() && trimmed == name.as_ref()
                })
            {
                return true;
            }
        }
    }
    false
}

fn register_non_recursive_dirs(
    watcher: &mut RecommendedWatcher,
    walk_root: &Path,
    exclude_patterns: &[String],
    watch_roots: &[PathBuf],
) {
    let walker = ignore::WalkBuilder::new(walk_root)
        .hidden(false)
        .git_ignore(true)
        .require_git(true) // within-repo .gitignore only; never ancestor ~/.gitignore (audit D4)
        .add_custom_ignore_filename(".ixignore")
        .filter_entry({
            let exclude_patterns = exclude_patterns.to_vec();
            let self_watch_roots = watch_roots.to_vec();
            let root_clone = walk_root.to_path_buf();
            move |entry| {
                crate::builder::default_filter_entry(
                    entry,
                    &root_clone,
                    &exclude_patterns,
                    &self_watch_roots,
                )
            }
        })
        .build();

    let guard = crate::cache_policy::resource_guard_auto(0.5);
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
                        eprintln!("ix: warning: watcher failed for {}: {}", path.display(), e);
                    }
                }
            }
            Err(e) => {
                eprintln!("ix: warning: watcher skipping path: {e}");
            }
        }
    }
}

fn maybe_watch_new_subdirs(event: &Event, ctx: &EventLoopCtx) {
    let Some(ref weak) = ctx.fallback_watcher else {
        return;
    };
    if !(event.kind.is_create()
        || matches!(
            event.kind,
            notify::EventKind::Modify(notify::event::ModifyKind::Name(_))
        ))
    {
        return;
    }
    let Some(watcher_arc) = weak.upgrade() else {
        return;
    };
    for path in &event.paths {
        if path.is_dir()
            && crate::builder::is_path_admitted(
                path,
                true,
                Some(&ctx.root),
                &ctx.exclude_patterns,
                &ctx.watch_roots,
            )
            && let Ok(mut w) = watcher_arc.lock()
        {
            register_non_recursive_dirs(&mut w, path, &ctx.exclude_patterns, &ctx.watch_roots);
        }
    }
}

fn run_event_loop(
    event_rx: &mpsc::Receiver<notify::Result<Event>>,
    tx: &Sender<Vec<PathBuf>>,
    ctx: &EventLoopCtx,
) {
    let mut changed_paths: HashMap<PathBuf, notify::EventKind> = HashMap::new();
    let max_debounce = ctx.debounce_dur.saturating_mul(2);

    loop {
        match event_rx.recv() {
            Ok(Ok(event)) => {
                maybe_watch_new_subdirs(&event, ctx);
                let admitted = Watcher::collect_paths(
                    &mut changed_paths,
                    event,
                    &ctx.root,
                    &ctx.watch_roots,
                    &ctx.exclude_patterns,
                );
                if !admitted {
                    continue;
                }

                let batch_deadline = Instant::now() + max_debounce;
                let mut idle_deadline = Instant::now() + ctx.debounce_dur;

                // Debounce loop: reset idle_deadline only on admitted paths and
                // flush no later than batch_deadline (2 * debounce_dur).
                loop {
                    let now = Instant::now();
                    let effective = idle_deadline.min(batch_deadline);
                    let Some(remaining) = effective.checked_duration_since(now) else {
                        break;
                    };
                    if remaining.is_zero() {
                        break;
                    }
                    match event_rx.recv_timeout(remaining) {
                        Ok(Ok(event)) => {
                            maybe_watch_new_subdirs(&event, ctx);
                            if Watcher::collect_paths(
                                &mut changed_paths,
                                event,
                                &ctx.root,
                                &ctx.watch_roots,
                                &ctx.exclude_patterns,
                            ) {
                                idle_deadline = Instant::now() + ctx.debounce_dur;
                            }
                        }
                        Ok(Err(_)) => {} // notify error, do not reset idle_deadline
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }

                if !changed_paths.is_empty() {
                    let paths: Vec<PathBuf> = changed_paths.drain().map(|(p, _)| p).collect();
                    if tx.send(paths).is_err() {
                        return; // Receiver dropped
                    }
                }
            }
            Ok(Err(_)) => {}
            Err(_) => return, // Watcher dropped
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

    #[test]
    fn collect_paths_returns_admitted_flag_and_blocks_deleted_excluded_dirs() {
        let mut map = HashMap::new();
        let root = PathBuf::from("/repo");
        let excludes = vec!["node_modules".to_string(), "build/".to_string()];

        // Deleted excluded dir (.git) must be rejected even though metadata() fails.
        let remove_git = Event::new(notify::EventKind::Remove(notify::event::RemoveKind::Folder))
            .add_path(root.join(".git"));
        assert!(!Watcher::collect_paths(
            &mut map,
            remove_git,
            &root,
            &[],
            &excludes
        ));
        assert!(map.is_empty());

        // Deleted excluded dir matching trailing-slash exclude pattern must be rejected.
        let remove_build = Event::new(notify::EventKind::Remove(notify::event::RemoveKind::Folder))
            .add_path(root.join("build"));
        assert!(!Watcher::collect_paths(
            &mut map,
            remove_build,
            &root,
            &[],
            &excludes
        ));
        assert!(map.is_empty());

        // Deleted regular source file must still be admitted so Builder::update tombstones it.
        let remove_src = Event::new(notify::EventKind::Remove(notify::event::RemoveKind::File))
            .add_path(root.join("src/main.rs"));
        assert!(Watcher::collect_paths(
            &mut map,
            remove_src,
            &root,
            &[],
            &excludes
        ));
        assert!(map.contains_key(&root.join("src/main.rs")));
    }

    #[test]
    fn watcher_normalizes_relative_watch_roots_to_canonical_root() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir src");
        let watcher = Watcher::new(dir.path(), &[PathBuf::from("src")], &[]);
        assert_eq!(watcher.watch_roots.len(), 1);
        assert!(watcher.watch_roots[0].starts_with(&watcher.root));
    }

    #[test]
    fn debounce_loop_not_starved_by_excluded_or_sustained_events() {
        let dir = tempdir().expect("tempdir");
        let canonical = dir
            .path()
            .canonicalize()
            .unwrap_or_else(|_| dir.path().to_path_buf());
        let (event_tx, event_rx) = mpsc::channel();
        let (tx, rx) = crossbeam_channel::unbounded();

        let ctx = EventLoopCtx {
            root: canonical.clone(),
            watch_roots: Vec::new(),
            exclude_patterns: vec!["target".to_string()],
            debounce_dur: Duration::from_millis(60),
            fallback_watcher: None,
        };

        let handle = thread::spawn(move || run_event_loop(&event_rx, &tx, &ctx));

        // Send an excluded event first: must NOT enter debounce or emit anything.
        event_tx
            .send(Ok(Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Data(notify::event::DataChange::Any),
            ))
            .add_path(canonical.join("target/debug/out.rs"))))
            .expect("send excluded");
        assert!(rx.recv_timeout(Duration::from_millis(90)).is_err());

        // Send sustained admitted events every 25ms for 180ms; max_debounce (120ms)
        // must force at least one batch emission before the burst ends.
        let admitted_path = canonical.join("lib.rs");
        for _ in 0..7 {
            event_tx
                .send(Ok(Event::new(notify::EventKind::Create(
                    notify::event::CreateKind::File,
                ))
                .add_path(admitted_path.clone())))
                .expect("send admitted");
            thread::sleep(Duration::from_millis(25));
        }

        let batch = rx
            .recv_timeout(Duration::from_millis(300))
            .expect("batch must flush at max_debounce");
        assert!(batch.contains(&admitted_path));

        drop(event_tx);
        handle.join().expect("event loop exits");
    }
}
