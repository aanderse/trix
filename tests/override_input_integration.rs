//! Integration tests for `--override-input` functionality.
//!
//! These tests verify that:
//! 1. The --override-input flag works correctly across commands
//! 2. Overridden inputs are NOT copied to the nix store (key trix feature)
//! 3. The override is actually used during evaluation

use std::fs;
use std::process::Command;
use uuid::Uuid;

/// Get the path to the trix binary.
fn trix_bin() -> String {
    std::env::var("CARGO_BIN_EXE_trix").unwrap_or_else(|_| "target/debug/trix".to_string())
}

/// Run trix build and return the result.
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

/// Run trix eval and return the result.
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

/// Create a minimal flake that depends on another input.
fn create_main_flake(dir: &std::path::Path, input_name: &str, marker: &str) {
    // flake.nix that imports a value from an input
    let flake_nix = format!(
        r#"{{
  inputs.{input_name}.url = "path:/nonexistent";  # Will be overridden
  outputs = {{ self, {input_name} }}: {{
    lib.testValue = {input_name}.lib.value;
    lib.mainMarker = "{marker}";
    packages.x86_64-linux.default = derivation {{
      name = "test-with-override";
      system = "x86_64-linux";
      builder = "/bin/sh";
      args = [ "-c" "echo ${{builtins.toJSON {input_name}.lib.value}} > $out" ];
    }};
  }};
}}"#
    );
    fs::write(dir.join("flake.nix"), flake_nix).expect("failed to write flake.nix");

    // Create a marker file to detect if this gets copied to store
    fs::write(dir.join(format!("main-marker-{}.txt", marker)), "main")
        .expect("failed to write marker");

    // flake.lock that references the input
    let flake_lock = format!(
        r#"{{
  "nodes": {{
    "{input_name}": {{
      "flake": false,
      "locked": {{
        "path": "/nonexistent",
        "type": "path"
      }},
      "original": {{
        "path": "/nonexistent",
        "type": "path"
      }}
    }},
    "root": {{
      "inputs": {{
        "{input_name}": "{input_name}"
      }}
    }}
  }},
  "root": "root",
  "version": 7
}}"#
    );
    fs::write(dir.join("flake.lock"), flake_lock).expect("failed to write flake.lock");
}

/// Create a simple flake that can be used as an override.
fn create_override_flake(dir: &std::path::Path, value: &str, marker: &str) {
    let flake_nix = format!(
        r#"{{
  inputs = {{ }};
  outputs = {{ self }}: {{
    lib.value = "{value}";
    lib.overrideMarker = "{marker}";
  }};
}}"#
    );
    fs::write(dir.join("flake.nix"), flake_nix).expect("failed to write flake.nix");

    // Create a marker file to detect if this gets copied to store
    fs::write(dir.join(format!("override-marker-{}.txt", marker)), "override")
        .expect("failed to write marker");

    let flake_lock = r#"{
  "nodes": {
    "root": {}
  },
  "root": "root",
  "version": 7
}"#;
    fs::write(dir.join("flake.lock"), flake_lock).expect("failed to write flake.lock");
}

// =============================================================================
// Override Input Tests - Eval
// =============================================================================

/// Test that --override-input works with trix eval.
#[test]
fn eval_override_input_basic() {
    let uuid = Uuid::new_v4().to_string();
    let override_value = format!("override-value-{}", &uuid[..8]);

    // Create main flake
    let main_dir = tempfile::TempDir::new().expect("failed to create main temp dir");
    create_main_flake(main_dir.path(), "myinput", &uuid);

    // Create override flake
    let override_dir = tempfile::TempDir::new().expect("failed to create override temp dir");
    create_override_flake(override_dir.path(), &override_value, &uuid);

    // Run trix eval with override
    let flake_ref = format!("{}#lib.testValue", main_dir.path().display());
    let result = trix_eval(&[
        "--override-input",
        "myinput",
        override_dir.path().to_str().unwrap(),
        &flake_ref,
    ]);

    assert!(result.is_ok(), "trix eval failed: {:?}", result);
    let output = result.unwrap();
    assert!(
        output.contains(&override_value),
        "override not applied, got: {}",
        output
    );
}

/// Test that --override-input does NOT copy the override to the store.
#[test]
fn eval_override_input_no_store_copy() {
    let uuid = Uuid::new_v4().to_string();

    // Create main flake
    let main_dir = tempfile::TempDir::new().expect("failed to create main temp dir");
    create_main_flake(main_dir.path(), "myinput", &uuid);

    // Create override flake
    let override_dir = tempfile::TempDir::new().expect("failed to create override temp dir");
    create_override_flake(override_dir.path(), "test-value", &uuid);

    // Run trix eval with override
    let flake_ref = format!("{}#lib.testValue", main_dir.path().display());
    let result = trix_eval(&[
        "--override-input",
        "myinput",
        override_dir.path().to_str().unwrap(),
        &flake_ref,
    ]);

    assert!(result.is_ok(), "trix eval failed: {:?}", result);

    // Verify NEITHER the main flake NOR the override was copied to the store
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
        "FAIL: trix copied flake or override to store! Found:\n{}",
        found_paths
    );
}

// =============================================================================
// Override Input Tests - Build
// =============================================================================

/// Test that --override-input works with trix build.
#[test]
fn build_override_input_basic() {
    let uuid = Uuid::new_v4().to_string();

    // Create main flake
    let main_dir = tempfile::TempDir::new().expect("failed to create main temp dir");
    create_main_flake(main_dir.path(), "myinput", &uuid);

    // Create override flake
    let override_dir = tempfile::TempDir::new().expect("failed to create override temp dir");
    create_override_flake(override_dir.path(), "build-test-value", &uuid);

    // Run trix build with override
    let flake_ref = format!("{}#default", main_dir.path().display());
    let result = trix_build(&[
        "--no-link",
        "--override-input",
        "myinput",
        override_dir.path().to_str().unwrap(),
        &flake_ref,
    ]);

    assert!(result.is_ok(), "trix build failed: {:?}", result);
    let output_path = result.unwrap();
    assert!(
        output_path.starts_with("/nix/store/"),
        "unexpected output: {}",
        output_path
    );
}

/// Test that --override-input with build does NOT copy the override to the store.
#[test]
fn build_override_input_no_store_copy() {
    let uuid = Uuid::new_v4().to_string();

    // Create main flake
    let main_dir = tempfile::TempDir::new().expect("failed to create main temp dir");
    create_main_flake(main_dir.path(), "myinput", &uuid);

    // Create override flake
    let override_dir = tempfile::TempDir::new().expect("failed to create override temp dir");
    create_override_flake(override_dir.path(), "no-copy-test", &uuid);

    // Run trix build with override
    let flake_ref = format!("{}#default", main_dir.path().display());
    let result = trix_build(&[
        "--no-link",
        "--override-input",
        "myinput",
        override_dir.path().to_str().unwrap(),
        &flake_ref,
    ]);

    assert!(result.is_ok(), "trix build failed: {:?}", result);

    // Verify the override marker is NOT in the store
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
        "FAIL: trix copied override to store! Found:\n{}",
        found_paths
    );
}

// =============================================================================
// Override Input Tests - Multiple Overrides
// =============================================================================

/// Create a flake with multiple inputs.
fn create_multi_input_flake(dir: &std::path::Path, marker: &str) {
    let flake_nix = format!(
        r#"{{
  inputs.input1.url = "path:/nonexistent1";
  inputs.input2.url = "path:/nonexistent2";
  outputs = {{ self, input1, input2 }}: {{
    lib.combined = "${{input1.lib.value}}-${{input2.lib.value}}";
    lib.marker = "{marker}";
  }};
}}"#
    );
    fs::write(dir.join("flake.nix"), flake_nix).expect("failed to write flake.nix");

    fs::write(dir.join(format!("multi-marker-{}.txt", marker)), "multi")
        .expect("failed to write marker");

    let flake_lock = r#"{
  "nodes": {
    "input1": {
      "flake": false,
      "locked": { "path": "/nonexistent1", "type": "path" },
      "original": { "path": "/nonexistent1", "type": "path" }
    },
    "input2": {
      "flake": false,
      "locked": { "path": "/nonexistent2", "type": "path" },
      "original": { "path": "/nonexistent2", "type": "path" }
    },
    "root": {
      "inputs": { "input1": "input1", "input2": "input2" }
    }
  },
  "root": "root",
  "version": 7
}"#;
    fs::write(dir.join("flake.lock"), flake_lock).expect("failed to write flake.lock");
}

/// Test that multiple --override-input flags work together.
#[test]
fn eval_multiple_override_inputs() {
    let uuid = Uuid::new_v4().to_string();

    // Create main flake with multiple inputs
    let main_dir = tempfile::TempDir::new().expect("failed to create main temp dir");
    create_multi_input_flake(main_dir.path(), &uuid);

    // Create override flakes
    let override1_dir = tempfile::TempDir::new().expect("failed to create override1 temp dir");
    create_override_flake(override1_dir.path(), "VALUE1", &format!("{}-1", &uuid[..8]));

    let override2_dir = tempfile::TempDir::new().expect("failed to create override2 temp dir");
    create_override_flake(override2_dir.path(), "VALUE2", &format!("{}-2", &uuid[..8]));

    // Run trix eval with multiple overrides
    let flake_ref = format!("{}#lib.combined", main_dir.path().display());
    let result = trix_eval(&[
        "--override-input",
        "input1",
        override1_dir.path().to_str().unwrap(),
        "--override-input",
        "input2",
        override2_dir.path().to_str().unwrap(),
        &flake_ref,
    ]);

    assert!(result.is_ok(), "trix eval failed: {:?}", result);
    let output = result.unwrap();
    assert!(
        output.contains("VALUE1") && output.contains("VALUE2"),
        "multiple overrides not applied, got: {}",
        output
    );
}

// =============================================================================
// Override Input Tests - Error Cases
// =============================================================================

/// Test that override with non-existent input name gives a useful error.
#[test]
fn eval_override_nonexistent_input() {
    let uuid = Uuid::new_v4().to_string();

    // Create main flake
    let main_dir = tempfile::TempDir::new().expect("failed to create main temp dir");
    create_main_flake(main_dir.path(), "myinput", &uuid);

    // Create override flake
    let override_dir = tempfile::TempDir::new().expect("failed to create override temp dir");
    create_override_flake(override_dir.path(), "test", &uuid);

    // Try to override a non-existent input
    let flake_ref = format!("{}#lib.testValue", main_dir.path().display());
    let result = trix_eval(&[
        "--override-input",
        "nonexistent",
        override_dir.path().to_str().unwrap(),
        &flake_ref,
    ]);

    // Should fail because the input doesn't exist
    assert!(result.is_err(), "should fail for nonexistent input");
}

/// Test that override with non-existent path gives a useful error.
#[test]
fn eval_override_nonexistent_path() {
    let uuid = Uuid::new_v4().to_string();

    // Create main flake
    let main_dir = tempfile::TempDir::new().expect("failed to create main temp dir");
    create_main_flake(main_dir.path(), "myinput", &uuid);

    // Try to override with a non-existent path
    let flake_ref = format!("{}#lib.testValue", main_dir.path().display());
    let result = trix_eval(&[
        "--override-input",
        "myinput",
        "/nonexistent/path/to/override",
        &flake_ref,
    ]);

    // Should fail because the override path doesn't exist
    assert!(result.is_err(), "should fail for nonexistent override path");
}

// =============================================================================
// Override Input Tests - Tilde Expansion
// =============================================================================

/// Test that ~ in override path is expanded correctly.
/// Note: This test may be skipped if HOME is not set.
#[test]
fn eval_override_tilde_expansion() {
    // Skip if HOME is not set
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => {
            eprintln!("Skipping tilde expansion test - HOME not set");
            return;
        }
    };

    let uuid = Uuid::new_v4().to_string();

    // Create main flake
    let main_dir = tempfile::TempDir::new().expect("failed to create main temp dir");
    create_main_flake(main_dir.path(), "myinput", &uuid);

    // Create override flake in a temp dir under HOME
    let override_dir = tempfile::TempDir::new_in(&home).expect("failed to create override in home");
    create_override_flake(override_dir.path(), "tilde-test", &uuid);

    // Get the relative path from HOME
    let override_path = override_dir.path();
    let relative_path = override_path
        .strip_prefix(&home)
        .expect("override should be under HOME");
    let tilde_path = format!("~/{}", relative_path.display());

    // Run trix eval with tilde path
    let flake_ref = format!("{}#lib.testValue", main_dir.path().display());
    let result = trix_eval(&[
        "--override-input",
        "myinput",
        &tilde_path,
        &flake_ref,
    ]);

    assert!(result.is_ok(), "trix eval with ~ path failed: {:?}", result);
    let output = result.unwrap();
    assert!(
        output.contains("tilde-test"),
        "tilde expansion failed, got: {}",
        output
    );
}
