//! Integration tests for trix repl.
//!
//! These tests verify that `trix repl` correctly evaluates flakes
//! without copying them to the Nix store (a core trix guarantee).

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

/// Get the path to the trix binary.
fn trix_bin() -> String {
    // CARGO_BIN_EXE_trix is set by cargo test and is an absolute path
    std::env::var("CARGO_BIN_EXE_trix").unwrap_or_else(|_| {
        // Fallback: construct absolute path from CARGO_MANIFEST_DIR
        let manifest_dir =
            std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
        format!("{}/target/debug/trix", manifest_dir)
    })
}

/// Test that trix repl can load a local flake and access its outputs.
#[test]
fn repl_loads_local_flake() {
    // Start trix repl with the current flake
    let mut child = Command::new(trix_bin())
        .args(["repl", ".#"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start trix repl");

    // Send commands to the repl
    let stdin = child.stdin.as_mut().expect("failed to get stdin");

    // Query the packages attribute and quit
    writeln!(stdin, "builtins.attrNames packages").expect("failed to write to stdin");
    writeln!(stdin, ":q").expect("failed to write to stdin");

    let output = child
        .wait_with_output()
        .expect("failed to wait for trix repl");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // The repl should have loaded successfully
    assert!(
        stdout.contains("x86_64-linux")
            || stdout.contains("aarch64-linux")
            || stdout.contains("x86_64-darwin")
            || stdout.contains("aarch64-darwin"),
        "expected system name in output\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
}

/// Test that trix repl does NOT copy the local flake to the nix store.
///
/// This verifies trix's core design principle. The test creates a flake with
/// a unique UUID filename and verifies that filename never appears in /nix/store.
#[test]
fn repl_does_not_copy_flake_to_store() {
    use uuid::Uuid;

    // Create a unique identifier that we can search for
    let uuid = Uuid::new_v4().to_string();
    let marker_filename = format!("trix-repl-marker-{}.txt", uuid);

    // Create temp flake directory
    let temp_dir = tempfile::TempDir::new().expect("failed to create temp dir");
    let flake_dir = temp_dir.path();

    // Create the marker file
    fs::write(flake_dir.join(&marker_filename), "marker content")
        .expect("failed to write marker file");

    // Create a minimal flake.nix
    let flake_nix = format!(
        r#"{{
  inputs = {{ }};
  outputs = {{ self }}: {{
    testValue = "test-{uuid}";
  }};
}}"#,
        uuid = &uuid[..8]
    );
    fs::write(flake_dir.join("flake.nix"), flake_nix).expect("failed to write flake.nix");

    // Create flake.lock (empty inputs)
    let flake_lock = r#"{
  "nodes": {
    "root": {}
  },
  "root": "root",
  "version": 7
}"#;
    fs::write(flake_dir.join("flake.lock"), flake_lock).expect("failed to write flake.lock");

    // Start trix repl with this flake
    let mut child = Command::new(trix_bin())
        .args(["repl", ".#"])
        .current_dir(flake_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start trix repl");

    // Access the test output and quit
    let stdin = child.stdin.as_mut().expect("failed to get stdin");
    writeln!(stdin, "testValue").expect("failed to write to stdin");
    writeln!(stdin, ":q").expect("failed to write to stdin");

    let output = child
        .wait_with_output()
        .expect("failed to wait for trix repl");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Verify the repl worked
    assert!(
        stdout.contains(&format!("test-{}", &uuid[..8])),
        "repl should have evaluated the flake\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    // Now verify the marker file is NOT in the nix store
    // Search for paths containing our UUID
    let find_output = Command::new("find")
        .args([
            "/nix/store",
            "-maxdepth",
            "2",
            "-name",
            &format!("*{}*", &uuid),
        ])
        .output()
        .expect("failed to run find");

    let found_paths = String::from_utf8_lossy(&find_output.stdout);
    assert!(
        found_paths.trim().is_empty(),
        "FAIL: trix repl copied flake to store! Found paths containing UUID:\n{}",
        found_paths
    );
}

/// Test that the repl uses --expr for local flakes (not path: URL).
/// We verify this by checking the debug output.
#[test]
fn repl_uses_expr_for_local_flakes() {
    // Start trix repl with debug logging
    let mut child = Command::new(trix_bin())
        .args(["repl", ".#"])
        .env("TRIX_LOG", "debug")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start trix repl");

    // Just quit immediately
    let stdin = child.stdin.as_mut().expect("failed to get stdin");
    writeln!(stdin, ":q").expect("failed to write to stdin");

    let output = child
        .wait_with_output()
        .expect("failed to wait for trix repl");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // The debug output should indicate we're using --expr
    assert!(
        stderr.contains("using --expr to evaluate flake in-place"),
        "expected debug message about --expr usage\nstderr: {}",
        stderr
    );
}
