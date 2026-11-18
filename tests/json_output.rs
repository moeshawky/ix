//! Regression tests for `ix --json` output validity (G-SEM-1 fix).

use std::fs;
use std::io::Write;
use tempfile::TempDir;

fn run_ix(args: &[&str]) -> (i32, String, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ix"))
        .args(args)
        .output()
        .expect("Failed to run ix");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
fn json_output_valid_with_control_chars() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    // File with a tab, carriage return, and other control chars in the matched line.
    let mut f = fs::File::create(root.join("t.txt")).unwrap();
    writeln!(f, "a\tb\rc\x07d").unwrap();
    drop(f);

    run_ix(&["--build", root.to_str().unwrap()]);
    let (code, stdout, _stderr) = run_ix(&["--json", "a", root.to_str().unwrap()]);
    assert_eq!(code, 0);
    for line in stdout.lines() {
        // Every emitted line must be parseable JSON.
        let v: serde_json::Value =
            serde_json::from_str(line).expect("ix --json must emit valid JSON");
        assert_eq!(v["content"], "a\tb\rc\u{07}d");
    }
}
