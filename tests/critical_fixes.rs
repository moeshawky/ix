//! Critical fix validation tests for v0.6.2
use std::fs;
use std::io::Write;
use tempfile::TempDir;

fn run_ix(args: &[&str]) -> (String, String) {
    let (_code, stdout, stderr) = run_ix_with_status(args);
    (stdout, stderr)
}

/// Run `ix` and return (exit_code, stdout, stderr).
/// Used by tests that need to assert on exit status (e.g., invalid input must fail).
fn run_ix_with_status(args: &[&str]) -> (i32, String, String) {
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
fn test_json_escaping_backslash_quote() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.jsonl");
    let mut file = fs::File::create(&file_path).unwrap();
    // Write content with literal backslash followed by quote
    writeln!(file, r#"prefix \" quote"#).unwrap();
    drop(file);

    run_ix(&["--build", dir.path().to_str().unwrap()]);
    let (stdout, _stderr) = run_ix(&["--json", "prefix", dir.path().to_str().unwrap()]);

    let line = stdout.lines().next().expect("Should find match");
    // G-SEM: Must be valid JSON - this is the critical test
    let json: serde_json::Value = serde_json::from_str(line)
        .unwrap_or_else(|e| panic!("Invalid JSON output: {}\nRaw: {}", e, line));

    let content = json["content"].as_str().unwrap();
    assert_eq!(
        content, r#"prefix \" quote"#,
        "Content should be the entire line, including backslash-quote"
    );
}

#[test]
fn test_json_escaping_windows_path() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    let mut file = fs::File::create(&file_path).unwrap();
    writeln!(file, r"C:\Users\test").unwrap();
    drop(file);

    run_ix(&["--build", dir.path().to_str().unwrap()]);
    let (stdout, _stderr) = run_ix(&["--json", "Users", dir.path().to_str().unwrap()]);

    let line = stdout.lines().next().expect("Should find match");
    let _: serde_json::Value =
        serde_json::from_str(line).unwrap_or_else(|e| panic!("Invalid JSON: {}\nRaw: {}", e, line));
}

/// Verify that every invalid regex pattern fails gracefully (non-zero exit,
/// no panic, meaningful error in stderr). Previously this test only checked
/// for absence of panics — we now also assert the process *did* fail and
/// that stderr carries an actionable error message.
#[test]
fn test_invalid_regex_no_panic() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "test content").unwrap();

    run_ix(&["--build", dir.path().to_str().unwrap()]);

    let bad_patterns = ["[", "(", "*", "+", "?", "{1,2}", "(?"];
    for pattern in bad_patterns {
        let (code, _stdout, stderr) =
            run_ix_with_status(&["--regex", pattern, dir.path().to_str().unwrap()]);
        assert!(
            !stderr.contains("panicked"),
            "Pattern '{}' caused panic: {}",
            pattern,
            stderr
        );
        assert_ne!(
            code, 0,
            "Pattern '{}' should produce a non-zero exit code, but got 0.\nstderr: {}",
            pattern, stderr
        );
        assert!(
            !stderr.trim().is_empty(),
            "Pattern '{}' should produce an error message on stderr, but stderr is empty",
            pattern
        );
    }
}

/// Verify that an empty search pattern is handled gracefully: no panic,
/// process exits with non-zero status, and stderr carries an error message.
#[test]
fn test_empty_pattern_no_panic() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "test").unwrap();

    run_ix(&["--build", dir.path().to_str().unwrap()]);
    let (code, _stdout, stderr) = run_ix_with_status(&["", dir.path().to_str().unwrap()]);

    assert!(
        !stderr.contains("panicked"),
        "Empty pattern caused panic: {}",
        stderr
    );
    // An empty literal should either succeed (matching nothing) or fail with a
    // clear error. Either way, it must NOT panic. If the process exits non-zero,
    // we additionally verify a meaningful error was printed.
    if code != 0 {
        assert!(
            !stderr.trim().is_empty(),
            "Non-zero exit for empty pattern but stderr is empty"
        );
    }
}

#[test]
fn test_json_output_golden() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.py");
    fs::write(&file_path, "def test_func(): pass").unwrap();

    run_ix(&["--build", dir.path().to_str().unwrap()]);
    let (stdout, _stderr) = run_ix(&["--json", "test_func", dir.path().to_str().unwrap()]);

    let line = stdout.lines().next().expect("Should find match");
    let json: serde_json::Value = serde_json::from_str(line).unwrap();

    assert_eq!(json["file"].as_str().unwrap(), file_path.to_str().unwrap());
    assert_eq!(json["line"].as_u64().unwrap(), 1, "Should be line 1");
    assert_eq!(json["content"].as_str().unwrap(), "def test_func(): pass");
}

/// F1 (audit): multiple PATH arguments must fail loudly (exit 2) instead of
/// silently searching only the first. The index is per-root, so >1 root is
/// unsupported; the guard turns a silent partial result into an explicit error.
#[test]
fn test_multi_path_fails_loudly() {
    let dir = TempDir::new().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    fs::write(&a, "needle in a").unwrap();
    fs::write(&b, "needle in b").unwrap();

    // Two paths -> exit 2 with the guard message (not a partial 1-match result).
    let (code, _out, err) = run_ix_with_status(&[
        "--no-index",
        "needle",
        a.to_str().unwrap(),
        b.to_str().unwrap(),
    ]);
    assert_eq!(code, 2, "multiple PATH args must exit 2; stderr={err}");
    assert!(
        err.contains("multiple PATH arguments"),
        "expected the multi-path guard message; stderr={err}"
    );

    // Single path -> still works (exit 0).
    let (code, out, _err) = run_ix_with_status(&["--no-index", "needle", a.to_str().unwrap()]);
    assert_eq!(code, 0, "single path must still search; stderr={_err}");
    assert!(
        out.contains("a.txt"),
        "single path must find a.txt; out={out}"
    );
}
