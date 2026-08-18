//! Regression test for audit F2: `ix --build` and the daemon must scope
//! identically when a `.ixd.toml` with `watch_roots` is present.
//!
//! Before the fix, `--build` ignored `watch_roots` and indexed the entire
//! tree (e.g. 3 files including `outer/drop.txt`), while the daemon honored
//! `watch_roots = ["src"]` and indexed only `src/file1.rs` (1 file).

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// Run `ix --build <root>` and return the list of indexed file paths (relative).
fn build_cli(root: &PathBuf) -> Vec<PathBuf> {
    let output = Command::new(env!("CARGO_BIN_EXE_ix"))
        .args(["--build"])
        .arg(root)
        .args(["--force"])
        .output()
        .expect("ix --build failed");
    assert!(output.status.success(), "ix --build failed: {}", String::from_utf8_lossy(&output.stderr));

    // Read the index and list files
    let ix_dir = root.join(".ix");
    let shard = ix_dir.join("shard.ix");
    let reader = ix::reader::Reader::open(&shard).expect("open shard");
    let mut files = Vec::new();
    for i in 0..reader.header.file_count {
        if let Ok(entry) = reader.get_file(i) {
            let rel = entry.path.strip_prefix(root).unwrap_or(&entry.path);
            files.push(rel.to_path_buf());
        }
    }
    files.sort();
    files
}

/// Start a daemon on `root`, wait for initial build, return indexed file paths.
fn build_daemon(root: &PathBuf) -> Vec<PathBuf> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ixd"))
        .arg(root)
        .arg("--daemon")
        .spawn()
        .expect("spawn ixd --daemon");

    // Wait for beacon to indicate initial build complete
    let ix_dir = root.join(".ix");
    let beacon_path = ix_dir.join("beacon.json");
    let mut files = Vec::new();
    for _ in 0..50 {
        if beacon_path.exists() {
            if let Ok(beacon) = ix::format::Beacon::read_from(&ix_dir) {
                if beacon.status == "idle" || beacon.status.starts_with("idle") {
                    // Build complete; read index
                    let shard = ix_dir.join("shard.ix");
                    if shard.exists() {
                        let reader = ix::reader::Reader::open(&shard).expect("open shard");
                        for i in 0..reader.header.file_count {
                            if let Ok(entry) = reader.get_file(i) {
                                let rel = entry.path.strip_prefix(root).unwrap_or(&entry.path);
                                files.push(rel.to_path_buf());
                            }
                        }
                        files.sort();
                        break;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    // Shutdown daemon
    let _ = Command::new("pkill")
        .args(["-f", &format!("ixd.*{}", root.display())])
        .status();
    child.kill().ok();
    let _ = child.wait();

    files
}

#[test]
fn test_build_scope_matches_daemon_with_watch_roots() {
    // Create fixture:
    // root/
    //   .ixd.toml with watch_roots = ["src"]
    //   src/file1.rs
    //   outer/drop.txt  (should be excluded)
    let base = std::env::temp_dir().join(format!("ix_build_scope_{}", std::process::id()));
    fs::create_dir_all(&base.join("src")).unwrap();
    fs::create_dir_all(&base.join("outer")).unwrap();
    fs::write(base.join("src/file1.rs"), "fn main() {}\n").unwrap();
    fs::write(base.join("outer/drop.txt"), "excluded\n").unwrap();
    fs::write(
        base.join(".ixd.toml"),
        "watch_roots = [\"src\"]\nexclude_patterns = [\".git\"]\n",
    )
    .unwrap();

    // CLI build
    let cli_files = build_cli(&base);
    assert_eq!(cli_files, vec![PathBuf::from("src/file1.rs")],
        "CLI --build must honor watch_roots and only index src/file1.rs");

    // Daemon build
    let daemon_files = build_daemon(&base);
    assert_eq!(daemon_files, vec![PathBuf::from("src/file1.rs")],
        "Daemon must honor watch_roots and only index src/file1.rs");

    // They must match exactly
    assert_eq!(cli_files, daemon_files,
        "CLI --build and daemon must produce identical file sets when watch_roots is set");

    // Cleanup
    let _ = fs::remove_dir_all(&base);
}