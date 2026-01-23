//! Integration tests for `trix run` command.
//!
//! Tests running applications from flakes with various options.
//! Note: The run command uses exec() which replaces the process, so we mainly test
//! that it starts successfully and that errors are caught properly.

use std::process::Command;
use tempfile::TempDir;

/// Get the path to the trix binary.
fn trix_bin() -> String {
    std::env::var("CARGO_BIN_EXE_trix").unwrap_or_else(|_| {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .unwrap_or_else(|_| ".".to_string());
        format!("{}/target/debug/trix", manifest_dir)
    })
}

// =============================================================================
// Basic Run Tests
// =============================================================================

#[test]
fn run_simple_package() {
    // Run hello package - this should work
    let output = Command::new(trix_bin())
        .args(["run", "nixpkgs#hello"])
        .output()
        .expect("failed to run trix");

    // Should execute hello successfully
    assert!(output.status.success(), "run failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Hello") || stdout.contains("hello"),
        "unexpected output: {}",
        stdout
    );
}

#[test]
fn run_with_arguments() {
    // Run a program that takes arguments
    let output = Command::new(trix_bin())
        .args(["run", "nixpkgs#hello", "--", "--greeting=Hi"])
        .output()
        .expect("failed to run trix");

    assert!(output.status.success(), "run failed: {}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn run_validates_local_flake() {
    // Test that trix run can recognize and evaluate a local flake
    // We create a flake with an invalid app to test error handling
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let flake_nix = temp_dir.path().join("flake.nix");

    std::fs::write(&flake_nix, r#"
{
  description = "Test flake for validation";

  outputs = { self }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f: builtins.listToAttrs (map (system: {
        name = system;
        value = f system;
      }) systems);
    in {
      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "/nonexistent/program";
        };
      });
    };
}
"#).expect("failed to write flake.nix");

    // Try to run the app - should evaluate successfully but fail to exec
    let output = Command::new(trix_bin())
        .args(["run"])
        .current_dir(&temp_dir)
        .output()
        .expect("failed to run trix");

    // Should fail when trying to exec the nonexistent program
    assert!(
        !output.status.success(),
        "should fail for nonexistent program"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should show it tried to exec but failed (meaning evaluation succeeded)
    assert!(
        stderr.contains("failed to exec") || stderr.contains("No such file"),
        "should show exec error, got: {}",
        stderr
    );
}

// =============================================================================
// Error Cases
// =============================================================================

#[test]
fn run_nonexistent_package() {
    let output = Command::new(trix_bin())
        .args(["run", "nixpkgs#this-package-definitely-does-not-exist-12345"])
        .output()
        .expect("failed to run trix");

    assert!(!output.status.success(), "should fail for nonexistent package");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found") || stderr.contains("does not exist") || stderr.contains("attribute"),
        "unexpected error message: {}",
        stderr
    );
}

#[test]
fn run_invalid_flake_ref() {
    let output = Command::new(trix_bin())
        .args(["run", "::invalid::"])
        .output()
        .expect("failed to run trix");

    assert!(!output.status.success(), "should fail for invalid flake ref");
}

// =============================================================================
// Override Input Tests
// =============================================================================

#[test]
fn run_accepts_override_input_flag() {
    // Test that --override-input flag is recognized by the CLI parser
    // We don't test actual override functionality here since that requires a lock file
    let output = Command::new(trix_bin())
        .args([
            "run",
            "--help"
        ])
        .output()
        .expect("failed to run trix");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("override-input"),
        "help should mention override-input flag"
    );
}

// =============================================================================
// Help and Usage Tests
// =============================================================================

#[test]
fn run_help_displays_usage() {
    let output = Command::new(trix_bin())
        .args(["run", "--help"])
        .output()
        .expect("failed to run trix");

    assert!(output.status.success(), "help should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("build and execute") || stdout.contains("run"), "help should mention run functionality");
    assert!(stdout.contains("installable") || stdout.contains("INSTALLABLE"), "help should mention installable");
    assert!(stdout.contains("override-input"), "help should mention override-input");
}
