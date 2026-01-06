//! Integration tests for the flake metadata command.
//!
//! These tests verify that `trix flake metadata` produces correct output
//! and matches `nix flake metadata` for key fields.

use std::fs;
use std::process::Command;

use tempfile::TempDir;

/// Helper to create a flake.nix with given content
fn create_flake(dir: &std::path::Path, content: &str) {
    fs::write(dir.join("flake.nix"), content).expect("failed to write flake.nix");
}

/// Run trix flake metadata on a directory
fn run_trix_metadata(dir: &std::path::Path) -> std::io::Result<std::process::Output> {
    Command::new(env!("CARGO_BIN_EXE_trix"))
        .args(["flake", "metadata", "--json", dir.to_str().unwrap()])
        .output()
}

/// Run trix lock on a directory
fn run_trix_lock(dir: &std::path::Path) -> std::io::Result<std::process::Output> {
    Command::new(env!("CARGO_BIN_EXE_trix"))
        .args(["flake", "lock", dir.to_str().unwrap()])
        .output()
}

/// Run nix flake metadata on a directory
fn run_nix_metadata(dir: &std::path::Path) -> std::io::Result<std::process::Output> {
    Command::new("nix")
        .args(["flake", "metadata", "--json", dir.to_str().unwrap()])
        .output()
}

/// Parse JSON output
fn parse_json(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).expect("failed to parse JSON")
}

/// Test: Local flake metadata includes description
#[test]
fn metadata_local_description() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  description = "Test flake for metadata";
  inputs = {};
  outputs = { self }: {};
}"#,
    );

    let output = run_trix_metadata(tmp.path()).expect("failed to run trix");
    assert!(output.status.success(), "trix failed: {:?}", output);

    let metadata = parse_json(&output);
    assert_eq!(
        metadata["description"].as_str(),
        Some("Test flake for metadata")
    );
}

/// Test: Local flake metadata includes path
#[test]
fn metadata_local_path() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  inputs = {};
  outputs = { self }: {};
}"#,
    );

    let output = run_trix_metadata(tmp.path()).expect("failed to run trix");
    assert!(output.status.success());

    let metadata = parse_json(&output);
    let path = metadata["path"].as_str().expect("path should be string");
    assert!(path.contains(tmp.path().to_str().unwrap()));
}

/// Test: Local flake metadata includes locks from flake.lock
#[test]
#[ignore] // Requires network access
fn metadata_includes_locks() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
  outputs = { self, nixpkgs }: {};
}"#,
    );

    // First lock the flake
    let lock_output = run_trix_lock(tmp.path()).expect("failed to run trix lock");
    assert!(
        lock_output.status.success(),
        "trix lock failed: {}",
        String::from_utf8_lossy(&lock_output.stderr)
    );

    // Now get metadata
    let output = run_trix_metadata(tmp.path()).expect("failed to run trix");
    assert!(output.status.success());

    let metadata = parse_json(&output);

    // Should have locks
    assert!(metadata.get("locks").is_some(), "should have locks");
    assert!(
        metadata["locks"]["nodes"]["nixpkgs"].is_object(),
        "should have nixpkgs in locks"
    );
}

/// Test: Metadata locks match flake.lock content
#[test]
#[ignore] // Requires network access
fn metadata_locks_match_lockfile() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
  outputs = { self, nixpkgs }: {};
}"#,
    );

    // Lock the flake
    run_trix_lock(tmp.path()).expect("failed to run trix lock");

    // Get metadata
    let output = run_trix_metadata(tmp.path()).expect("failed to run trix");
    let metadata = parse_json(&output);

    // Read flake.lock directly
    let lock_content = fs::read_to_string(tmp.path().join("flake.lock")).unwrap();
    let lock_data: serde_json::Value = serde_json::from_str(&lock_content).unwrap();

    // The locks in metadata should match the lock file
    assert_eq!(metadata["locks"], lock_data);
}

/// Test: Compare trix metadata with nix metadata for locked flake
#[test]
#[ignore] // Requires network access
fn metadata_compare_with_nix() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  description = "Comparison test flake";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
  outputs = { self, nixpkgs }: {};
}"#,
    );

    // Lock with trix
    run_trix_lock(tmp.path()).expect("failed to run trix lock");

    // Get trix metadata
    let trix_output = run_trix_metadata(tmp.path()).expect("failed to run trix");
    assert!(trix_output.status.success());
    let trix_metadata = parse_json(&trix_output);

    // Get nix metadata
    let nix_output = run_nix_metadata(tmp.path()).expect("failed to run nix");
    assert!(nix_output.status.success());
    let nix_metadata = parse_json(&nix_output);

    // Compare key fields
    assert_eq!(
        trix_metadata["description"], nix_metadata["description"],
        "description should match"
    );

    // Both should have the same locks structure
    assert_eq!(
        trix_metadata["locks"]["nodes"]["nixpkgs"]["locked"]["rev"],
        nix_metadata["locks"]["nodes"]["nixpkgs"]["locked"]["rev"],
        "nixpkgs rev should match"
    );

    assert_eq!(
        trix_metadata["locks"]["nodes"]["nixpkgs"]["locked"]["narHash"],
        nix_metadata["locks"]["nodes"]["nixpkgs"]["locked"]["narHash"],
        "nixpkgs narHash should match"
    );
}

/// Test: Metadata for flake with transitive inputs
#[test]
#[ignore] // Requires network access
fn metadata_transitive_inputs() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
    flake-utils.url = "github:numtide/flake-utils";
  };
  outputs = { self, nixpkgs, flake-utils }: {};
}"#,
    );

    // Lock with trix
    run_trix_lock(tmp.path()).expect("failed to run trix lock");

    // Get metadata
    let output = run_trix_metadata(tmp.path()).expect("failed to run trix");
    assert!(output.status.success());
    let metadata = parse_json(&output);

    // Should have both direct inputs and transitive inputs
    let nodes = metadata["locks"]["nodes"].as_object().unwrap();
    assert!(nodes.contains_key("nixpkgs"), "should have nixpkgs");
    assert!(nodes.contains_key("flake-utils"), "should have flake-utils");
    assert!(nodes.contains_key("systems"), "should have transitive systems input");
}

/// Test: Metadata for flake with follows
#[test]
#[ignore] // Requires network access
fn metadata_with_follows() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
    home-manager = {
      url = "github:nix-community/home-manager/release-24.05";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs = { self, nixpkgs, home-manager }: {};
}"#,
    );

    // Lock with trix
    run_trix_lock(tmp.path()).expect("failed to run trix lock");

    // Get trix metadata
    let trix_output = run_trix_metadata(tmp.path()).expect("failed to run trix");
    let trix_metadata = parse_json(&trix_output);

    // Get nix metadata for comparison
    let nix_output = run_nix_metadata(tmp.path()).expect("failed to run nix");
    let nix_metadata = parse_json(&nix_output);

    // home-manager's nixpkgs should follow root nixpkgs (array format)
    assert_eq!(
        trix_metadata["locks"]["nodes"]["home-manager"]["inputs"]["nixpkgs"],
        nix_metadata["locks"]["nodes"]["home-manager"]["inputs"]["nixpkgs"],
        "follows structure should match"
    );
}

/// Test: Empty flake has minimal metadata
#[test]
fn metadata_empty_flake() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  inputs = {};
  outputs = { self }: { hello = "world"; };
}"#,
    );

    let output = run_trix_metadata(tmp.path()).expect("failed to run trix");
    assert!(output.status.success());

    let metadata = parse_json(&output);

    // Should have basic fields
    assert!(metadata.get("path").is_some());
    assert!(metadata.get("locked").is_some());
    assert!(metadata.get("original").is_some());

    // Should NOT have locks (no flake.lock)
    assert!(
        metadata.get("locks").is_none() || metadata["locks"].is_null(),
        "empty flake should not have locks"
    );
}

/// Test: Flake without description
#[test]
fn metadata_no_description() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  inputs = {};
  outputs = { self }: {};
}"#,
    );

    let output = run_trix_metadata(tmp.path()).expect("failed to run trix");
    assert!(output.status.success());

    let metadata = parse_json(&output);

    // Description should not be present (not null, just absent)
    // or if present, should be null
    if let Some(desc) = metadata.get("description") {
        assert!(desc.is_null(), "description should be null if not set");
    }
}
