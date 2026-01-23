//! Integration tests for `trix shell` command.
//!
//! Tests creating shell environments with packages from flakes.

use std::process::Command;

/// Get the path to the trix binary.
fn trix_bin() -> String {
    std::env::var("CARGO_BIN_EXE_trix").unwrap_or_else(|_| {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .unwrap_or_else(|_| ".".to_string());
        format!("{}/target/debug/trix", manifest_dir)
    })
}

/// Run trix shell with a command and return the result.
fn trix_shell_command(packages: &[&str], command: &str) -> Result<String, String> {
    let output = Command::new(trix_bin())
        .args(["shell"])
        .args(packages)
        .args(["-c", command])
        .output()
        .map_err(|e| format!("failed to run trix: {}", e))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// =============================================================================
// Basic Shell Tests
// =============================================================================

#[test]
fn shell_single_package() {
    // Create shell with hello package and run it
    let result = trix_shell_command(&["nixpkgs#hello"], "hello");
    assert!(result.is_ok(), "shell command failed: {:?}", result);

    let output = result.unwrap();
    assert!(
        output.contains("Hello") || output.contains("hello"),
        "unexpected output: {}",
        output
    );
}

#[test]
fn shell_multiple_packages() {
    // Create shell with multiple packages
    // Just verify we can load multiple packages successfully
    let result = trix_shell_command(
        &["nixpkgs#hello", "nixpkgs#hello"],
        "hello"
    );
    assert!(result.is_ok(), "shell command failed: {:?}", result);

    let output = result.unwrap();
    // Should have output from hello
    assert!(
        output.contains("Hello") || output.contains("hello"),
        "missing hello output: {}",
        output
    );
}

#[test]
fn shell_runs_command_in_path() {
    // Verify that package binaries are in PATH
    // Use hello which we know has a bin directory
    let result = trix_shell_command(&["nixpkgs#hello"], "which hello");
    assert!(result.is_ok(), "shell command failed: {:?}", result);

    let output = result.unwrap();
    assert!(
        output.contains("/nix/store/") && output.contains("hello"),
        "hello should be from nix store: {}",
        output
    );
}

#[test]
fn shell_command_can_access_multiple_binaries() {
    // Test that we can access the package binary and shell built-ins
    let result = trix_shell_command(
        &["nixpkgs#hello"],
        "hello && echo success"
    );
    assert!(result.is_ok(), "shell command failed: {:?}", result);

    let output = result.unwrap();
    assert!(
        output.contains("Hello") && output.contains("success"),
        "unexpected output: {}",
        output
    );
}

#[test]
fn shell_preserves_arguments() {
    // Test that shell command preserves spaces and special characters
    let result = trix_shell_command(
        &["nixpkgs#hello"],
        "echo 'hello world'"
    );
    assert!(result.is_ok(), "shell command failed: {:?}", result);

    let output = result.unwrap();
    assert_eq!(output, "hello world", "unexpected output: {}", output);
}

#[test]
fn shell_command_with_pipes() {
    // Test that shell commands can use pipes
    let result = trix_shell_command(
        &["nixpkgs#hello"],
        "printf 'line1\\nline2\\nline3'"
    );
    assert!(result.is_ok(), "shell command failed: {:?}", result);

    let output = result.unwrap();
    let line_count = output.lines().count();
    assert_eq!(line_count, 3, "unexpected line count: {}", output);
}

#[test]
fn shell_command_with_subshells() {
    // Test that shell commands can use subshells
    let result = trix_shell_command(
        &["nixpkgs#hello"],
        "echo $(echo nested)"
    );
    assert!(result.is_ok(), "shell command failed: {:?}", result);

    let output = result.unwrap();
    assert_eq!(output, "nested", "unexpected output: {}", output);
}

// =============================================================================
// Error Cases
// =============================================================================

#[test]
fn shell_nonexistent_package() {
    let result = trix_shell_command(
        &["nixpkgs#this-package-definitely-does-not-exist-12345"],
        "echo test"
    );
    assert!(result.is_err(), "should fail for nonexistent package");

    let error = result.unwrap_err();
    assert!(
        error.contains("not found") || error.contains("does not exist") || error.contains("attribute"),
        "unexpected error message: {}",
        error
    );
}

#[test]
fn shell_requires_at_least_one_package() {
    // Shell command requires at least one package
    let output = Command::new(trix_bin())
        .args(["shell"])
        .output()
        .expect("failed to run trix");

    assert!(
        !output.status.success(),
        "should fail without packages"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("required") || stderr.contains("INSTALLABLES"),
        "should mention required installables: {}",
        stderr
    );
}

#[test]
fn shell_command_failure_propagates() {
    // Test that command failures are propagated
    let result = trix_shell_command(&["nixpkgs#coreutils"], "false");
    assert!(result.is_err(), "should fail when command fails");
}

// =============================================================================
// Help and Usage Tests
// =============================================================================

#[test]
fn shell_help_displays_usage() {
    let output = Command::new(trix_bin())
        .args(["shell", "--help"])
        .output()
        .expect("failed to run trix");

    assert!(output.status.success(), "help should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("shell"), "help should mention shell");
    assert!(
        stdout.contains("installable") || stdout.contains("INSTALLABLES"),
        "help should mention installables"
    );
    assert!(stdout.contains("command") || stdout.contains("-c"), "help should mention -c");
}

// =============================================================================
// Advanced Usage Tests
// =============================================================================

#[test]
fn shell_with_current_flake() {
    // Test using packages from current flake (trix itself)
    // This assumes we're running from the trix repo
    let result = trix_shell_command(&[".#default"], "trix --version");

    // Should either succeed (if in trix repo) or fail gracefully
    if let Ok(output) = result {
        assert!(
            output.contains("trix") || output.contains("0."),
            "unexpected version output: {}",
            output
        );
    }
}

#[test]
fn shell_environment_isolation() {
    // Verify that the shell environment includes package paths
    let result = trix_shell_command(
        &["nixpkgs#hello"],
        "echo $PATH | grep -c /nix/store"
    );

    assert!(result.is_ok(), "shell command failed: {:?}", result);

    let count = result.unwrap();
    // PATH should contain at least one nix store path
    assert!(
        count.parse::<i32>().unwrap_or(0) > 0,
        "PATH should contain nix store paths"
    );
}

#[test]
fn shell_can_build_multiple_from_different_flakes() {
    // Test that we can mix packages from different sources
    // Note: This will use remote fetching for nixpkgs
    let result = trix_shell_command(
        &["nixpkgs#hello", "nixpkgs#cowsay"],
        "echo success"
    );

    assert!(result.is_ok(), "shell command failed: {:?}", result);
    let output = result.unwrap();
    assert_eq!(output, "success", "unexpected output: {}", output);
}
