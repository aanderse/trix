//! Integration tests for `trix build` with flakes.
//!
//! Tests the flake-based build command. Note that trix and nix produce different
//! output paths because trix doesn't copy the flake to the store first - this is
//! intentional and the whole point of trix.

use std::fs;
use std::process::Command;

/// Get the path to the trix binary.
fn trix_bin() -> String {
    std::env::var("CARGO_BIN_EXE_trix").unwrap_or_else(|_| "target/debug/trix".to_string())
}

/// Run trix build and return the result (stdout on success, stderr on failure).
fn trix_build(args: &[&str]) -> Result<String, String> {
    let output = Command::new(trix_bin())
        .args(["build"])
        .args(args)
        .output()
        .map_err(|e| format!("failed to run trix: {}", e))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// =============================================================================
// Local Flake Tests (current directory - trix itself)
// =============================================================================
// Tests that build trix itself (via .#default)
// These require nix-bindings-rust to be available as a flake input.

#[test]
fn build_local_default() {
    let result = trix_build(&["--no-link", ".#default"]);
    assert!(result.is_ok(), "build failed: {:?}", result);

    let output_path = result.unwrap();
    assert!(
        output_path.starts_with("/nix/store/"),
        "unexpected output: {}",
        output_path
    );
    assert!(
        output_path.contains("trix"),
        "wrong derivation name: {}",
        output_path
    );
}

#[test]
fn build_local_with_out_link() {
    let temp_dir = tempfile::TempDir::new().expect("failed to create temp dir");
    let link_path = temp_dir.path().join("my-result");

    let result = trix_build(&[
        "-o",
        link_path.to_str().unwrap(),
        ".#default",
    ]);
    assert!(result.is_ok(), "build failed: {:?}", result);

    // Check symlink was created
    assert!(
        link_path.is_symlink(),
        "result symlink not created at {:?}",
        link_path
    );

    // Check it points to a store path
    let target = fs::read_link(&link_path).expect("failed to read symlink");
    assert!(
        target.to_str().unwrap().starts_with("/nix/store/"),
        "symlink should point to store: {:?}",
        target
    );
}

#[test]
fn build_local_with_explicit_attr() {
    let result = trix_build(&["--no-link", ".#packages.x86_64-linux.default"]);
    assert!(result.is_ok(), "build failed: {:?}", result);

    let output_path = result.unwrap();
    assert!(
        output_path.contains("trix"),
        "wrong derivation: {}",
        output_path
    );
}


// =============================================================================
// Error Cases
// =============================================================================

#[test]
fn build_nonexistent_flake() {
    let result = trix_build(&["--no-link", "/nonexistent/path#default"]);
    assert!(result.is_err(), "should fail for nonexistent flake");
}

#[test]
fn build_nonexistent_attr() {
    let result = trix_build(&["--no-link", ".#nonexistent.attribute.path"]);
    assert!(result.is_err(), "should fail for nonexistent attribute");
}

// Note: We don't test full builds of external flakes because they require
// downloading/building many dependencies (slow) and trix intentionally produces
// different hashes than nix (doesn't copy flake to store).

// =============================================================================
// Store Copy Prevention Tests
// =============================================================================
// These tests verify trix's core design principle: local flakes are NEVER
// copied to the nix store during evaluation. This is what makes trix fast.

/// Test that trix build does NOT copy the local flake to the nix store.
///
/// This verifies trix's core design principle. The test creates a flake with
/// a unique UUID filename and verifies that filename never appears in /nix/store.
#[test]
fn build_does_not_copy_flake_to_store() {
    use uuid::Uuid;

    // Create a unique identifier that we can search for
    let uuid = Uuid::new_v4().to_string();
    let marker_filename = format!("trix-test-marker-{}.txt", uuid);

    // Create temp flake directory
    let temp_dir = tempfile::TempDir::new().expect("failed to create temp dir");
    let flake_dir = temp_dir.path();

    // Create the marker file
    fs::write(flake_dir.join(&marker_filename), "marker content")
        .expect("failed to write marker file");

    // Create a minimal flake.nix that builds a simple derivation
    let flake_nix = format!(
        r#"{{
  inputs = {{ }};
  outputs = {{ self }}: {{
    packages.x86_64-linux.default = derivation {{
      name = "test-{uuid}";
      system = "x86_64-linux";
      builder = "/bin/sh";
      args = [ "-c" "echo test > $out" ];
    }};
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

    // Run trix build - use absolute path to flake
    let flake_ref = format!("{}#default", flake_dir.display());
    let result = trix_build(&["--no-link", &flake_ref]);

    // Build should succeed
    assert!(result.is_ok(), "trix build failed: {:?}", result);

    // Now verify the marker file is NOT in the nix store
    // Search for paths containing our UUID
    let find_output = Command::new("find")
        .args(["/nix/store", "-maxdepth", "2", "-name", &format!("*{}*", &uuid)])
        .output()
        .expect("failed to run find");

    let found_paths = String::from_utf8_lossy(&find_output.stdout);
    assert!(
        found_paths.trim().is_empty(),
        "FAIL: trix copied flake to store! Found paths containing UUID:\n{}",
        found_paths
    );
}

/// Run trix eval and return the result (stdout on success, stderr on failure).
fn trix_eval(args: &[&str]) -> Result<String, String> {
    let output = Command::new(trix_bin())
        .args(["eval"])
        .args(args)
        .output()
        .map_err(|e| format!("failed to run trix: {}", e))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Test that trix eval does NOT copy the local flake to the nix store.
#[test]
fn eval_does_not_copy_flake_to_store() {
    use uuid::Uuid;

    // Create a unique identifier
    let uuid = Uuid::new_v4().to_string();
    let marker_filename = format!("trix-eval-marker-{}.txt", uuid);

    // Create temp flake directory
    let temp_dir = tempfile::TempDir::new().expect("failed to create temp dir");
    let flake_dir = temp_dir.path();

    // Create the marker file
    fs::write(flake_dir.join(&marker_filename), "marker content")
        .expect("failed to write marker file");

    // Create a minimal flake.nix with a lib attribute (doesn't need system prefix)
    let flake_nix = format!(
        r#"{{
  inputs = {{ }};
  outputs = {{ self }}: {{
    lib.testValue = "test-{uuid}";
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

    // Run trix eval - use absolute path to flake with full attribute path
    let flake_ref = format!("{}#lib.testValue", flake_dir.display());
    let result = trix_eval(&[&flake_ref]);

    // Eval should succeed
    assert!(result.is_ok(), "trix eval failed: {:?}", result);

    // Now verify the marker file is NOT in the nix store
    let find_output = Command::new("find")
        .args(["/nix/store", "-maxdepth", "2", "-name", &format!("*{}*", &uuid)])
        .output()
        .expect("failed to run find");

    let found_paths = String::from_utf8_lossy(&find_output.stdout);
    assert!(
        found_paths.trim().is_empty(),
        "FAIL: trix eval copied flake to store! Found paths containing UUID:\n{}",
        found_paths
    );
}
