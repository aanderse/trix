//! Integration tests for `trix build --file` with --arg and --argstr.

use std::fs;
use std::process::Command;

/// Get the path to the trix binary.
fn trix_bin() -> String {
    std::env::var("CARGO_BIN_EXE_trix").unwrap_or_else(|_| "target/debug/trix".to_string())
}

/// Create a temporary directory with test Nix files.
fn setup_test_files() -> tempfile::TempDir {
    let temp_dir = tempfile::TempDir::new().expect("failed to create temp dir");

    // Simple derivation (no function)
    fs::write(
        temp_dir.path().join("simple.nix"),
        r#"
derivation {
  name = "simple-test";
  builder = "/bin/sh";
  args = [ "-c" "echo hello > $out" ];
  system = builtins.currentSystem;
}
"#,
    )
    .expect("failed to write simple.nix");

    // Function with default arguments
    fs::write(
        temp_dir.path().join("with-defaults.nix"),
        r#"
{ name ? "default", greeting ? "Hello" }:
derivation {
  name = "test-${name}";
  builder = "/bin/sh";
  args = [ "-c" "echo '${greeting}' > $out" ];
  system = builtins.currentSystem;
}
"#,
    )
    .expect("failed to write with-defaults.nix");

    // Attrset with multiple derivations
    fs::write(
        temp_dir.path().join("multi.nix"),
        r#"
{
  hello = derivation {
    name = "hello-drv";
    builder = "/bin/sh";
    args = [ "-c" "echo hello > $out" ];
    system = builtins.currentSystem;
  };
  world = derivation {
    name = "world-drv";
    builder = "/bin/sh";
    args = [ "-c" "echo world > $out" ];
    system = builtins.currentSystem;
  };
}
"#,
    )
    .expect("failed to write multi.nix");

    // Function returning attrset
    fs::write(
        temp_dir.path().join("func-multi.nix"),
        r#"
{ prefix ? "test" }:
{
  foo = derivation {
    name = "${prefix}-foo";
    builder = "/bin/sh";
    args = [ "-c" "echo foo > $out" ];
    system = builtins.currentSystem;
  };
  bar = derivation {
    name = "${prefix}-bar";
    builder = "/bin/sh";
    args = [ "-c" "echo bar > $out" ];
    system = builtins.currentSystem;
  };
}
"#,
    )
    .expect("failed to write func-multi.nix");

    temp_dir
}

/// Run trix build and return the output path.
fn trix_build(args: &[&str]) -> Result<String, String> {
    let output = Command::new(trix_bin())
        .args(["build"])
        .args(args)
        .args(["--no-link"])
        .output()
        .map_err(|e| format!("failed to run trix: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "trix build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// =============================================================================
// Basic --file tests
// =============================================================================

#[test]
fn build_simple_file() {
    let temp = setup_test_files();
    let file_path = temp.path().join("simple.nix");

    let result = trix_build(&["-f", file_path.to_str().unwrap()]);
    assert!(result.is_ok(), "build failed: {:?}", result);

    let output_path = result.unwrap();
    assert!(output_path.starts_with("/nix/store/"), "unexpected output: {}", output_path);
    assert!(output_path.contains("simple-test"), "wrong derivation name: {}", output_path);
}

#[test]
fn build_file_with_defaults() {
    let temp = setup_test_files();
    let file_path = temp.path().join("with-defaults.nix");

    let result = trix_build(&["-f", file_path.to_str().unwrap()]);
    assert!(result.is_ok(), "build failed: {:?}", result);

    let output_path = result.unwrap();
    assert!(output_path.contains("test-default"), "wrong derivation name: {}", output_path);
}

// =============================================================================
// --attr tests
// =============================================================================

#[test]
fn build_file_with_attr() {
    let temp = setup_test_files();
    let file_path = temp.path().join("multi.nix");

    let result = trix_build(&["-f", file_path.to_str().unwrap(), "-A", "hello"]);
    assert!(result.is_ok(), "build failed: {:?}", result);

    let output_path = result.unwrap();
    assert!(output_path.contains("hello-drv"), "wrong derivation: {}", output_path);
}

#[test]
fn build_file_with_other_attr() {
    let temp = setup_test_files();
    let file_path = temp.path().join("multi.nix");

    let result = trix_build(&["-f", file_path.to_str().unwrap(), "-A", "world"]);
    assert!(result.is_ok(), "build failed: {:?}", result);

    let output_path = result.unwrap();
    assert!(output_path.contains("world-drv"), "wrong derivation: {}", output_path);
}

// =============================================================================
// --argstr tests
// =============================================================================

#[test]
fn build_file_with_argstr() {
    let temp = setup_test_files();
    let file_path = temp.path().join("with-defaults.nix");

    let result = trix_build(&[
        "-f", file_path.to_str().unwrap(),
        "--argstr", "name", "custom",
    ]);
    assert!(result.is_ok(), "build failed: {:?}", result);

    let output_path = result.unwrap();
    assert!(output_path.contains("test-custom"), "wrong derivation name: {}", output_path);
}

#[test]
fn build_file_with_multiple_argstr() {
    let temp = setup_test_files();
    let file_path = temp.path().join("with-defaults.nix");

    let result = trix_build(&[
        "-f", file_path.to_str().unwrap(),
        "--argstr", "name", "foo",
        "--argstr", "greeting", "Hi",
    ]);
    assert!(result.is_ok(), "build failed: {:?}", result);

    let output_path = result.unwrap();
    assert!(output_path.contains("test-foo"), "wrong derivation name: {}", output_path);
}

// =============================================================================
// --arg tests
// =============================================================================

#[test]
fn build_file_with_arg_expression() {
    let temp = setup_test_files();
    let file_path = temp.path().join("with-defaults.nix");

    let result = trix_build(&[
        "-f", file_path.to_str().unwrap(),
        "--arg", "name", r#""computed""#,
    ]);
    assert!(result.is_ok(), "build failed: {:?}", result);

    let output_path = result.unwrap();
    assert!(output_path.contains("test-computed"), "wrong derivation name: {}", output_path);
}

#[test]
fn build_file_with_arg_concatenation() {
    let temp = setup_test_files();
    let file_path = temp.path().join("with-defaults.nix");

    let result = trix_build(&[
        "-f", file_path.to_str().unwrap(),
        "--arg", "name", r#""hello-" + "world""#,
    ]);
    assert!(result.is_ok(), "build failed: {:?}", result);

    let output_path = result.unwrap();
    assert!(output_path.contains("test-hello-world"), "wrong derivation name: {}", output_path);
}

// =============================================================================
// Combined tests
// =============================================================================

#[test]
fn build_file_with_argstr_and_attr() {
    let temp = setup_test_files();
    let file_path = temp.path().join("func-multi.nix");

    let result = trix_build(&[
        "-f", file_path.to_str().unwrap(),
        "--argstr", "prefix", "myprefix",
        "-A", "foo",
    ]);
    assert!(result.is_ok(), "build failed: {:?}", result);

    let output_path = result.unwrap();
    assert!(output_path.contains("myprefix-foo"), "wrong derivation name: {}", output_path);
}

#[test]
fn build_file_with_argstr_and_different_attr() {
    let temp = setup_test_files();
    let file_path = temp.path().join("func-multi.nix");

    let result = trix_build(&[
        "-f", file_path.to_str().unwrap(),
        "--argstr", "prefix", "custom",
        "-A", "bar",
    ]);
    assert!(result.is_ok(), "build failed: {:?}", result);

    let output_path = result.unwrap();
    assert!(output_path.contains("custom-bar"), "wrong derivation name: {}", output_path);
}

// =============================================================================
// Error cases
// =============================================================================

#[test]
fn build_file_not_found() {
    let result = trix_build(&["-f", "/nonexistent/path.nix"]);
    assert!(result.is_err(), "should fail for nonexistent file");
    assert!(result.unwrap_err().contains("file not found"), "wrong error message");
}

#[test]
fn build_file_missing_attr() {
    let temp = setup_test_files();
    let file_path = temp.path().join("multi.nix");

    let result = trix_build(&["-f", file_path.to_str().unwrap(), "-A", "nonexistent"]);
    assert!(result.is_err(), "should fail for missing attribute");
}
