//! Critical fix validation tests for v0.6.2
use std::fs;
use std::io::Write;
use tempfile::TempDir;

fn run_ix(args: &[&str]) -> (String, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ix"))
        .args(args)
        .output()
        .expect("Failed to run ix");
    (
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
    assert!(
        content.contains("prefix"),
        "Content should match: {}",
        content
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

#[test]
fn test_invalid_regex_no_panic() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "test content").unwrap();

    run_ix(&["--build", dir.path().to_str().unwrap()]);

    let bad_patterns = ["[", "(", "*", "+", "?", "{1,2}", "(?"];
    for pattern in bad_patterns {
        let (_stdout, stderr) = run_ix(&["--regex", pattern, dir.path().to_str().unwrap()]);
        assert!(
            !stderr.contains("panicked"),
            "Pattern '{}' caused panic: {}",
            pattern,
            stderr
        );
    }
}

#[test]
fn test_empty_pattern_no_panic() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "test").unwrap();

    run_ix(&["--build", dir.path().to_str().unwrap()]);
    let (_stdout, stderr) = run_ix(&["", dir.path().to_str().unwrap()]);

    assert!(
        !stderr.contains("panicked"),
        "Empty pattern caused panic: {}",
        stderr
    );
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

    assert!(json["file"].as_str().is_some());
    assert!(json["line"].as_u64().is_some());
    assert!(json["content"].as_str().is_some());
}
