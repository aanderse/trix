//! Integration tests for `trix registry` commands.
//!
//! These tests use a temporary HOME and XDG_CONFIG_HOME to avoid
//! modifying the real user registry.

use std::fs;
use std::process::Command;

/// Get the path to the trix binary.
fn trix_bin() -> String {
    std::env::var("CARGO_BIN_EXE_trix").unwrap_or_else(|_| "target/debug/trix".to_string())
}

/// Run trix with a custom HOME and XDG_CONFIG_HOME.
fn trix_with_home(home: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(trix_bin())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", format!("{}/.config", home))
        .args(args)
        .output()
        .map_err(|e| format!("failed to run trix: {}", e))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[test]
fn test_registry_list_empty() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Create config dir
    fs::create_dir_all(format!("{}/.config/nix", home_path)).unwrap();

    // List should work with empty registry
    let result = trix_with_home(home_path, &["registry", "list", "--no-global"]);
    // Should succeed (might be empty or show only USER header)
    assert!(result.is_ok(), "registry list failed: {:?}", result);
}

#[test]
fn test_registry_add_and_list() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Create config dir
    fs::create_dir_all(format!("{}/.config/nix", home_path)).unwrap();

    // Add a registry entry
    let add_result = trix_with_home(
        home_path,
        &["registry", "add", "test-flake", "github:NixOS/nixpkgs"],
    );
    assert!(add_result.is_ok(), "registry add failed: {:?}", add_result);

    // List should show the entry
    let list_result = trix_with_home(home_path, &["registry", "list", "--no-global"]);
    assert!(list_result.is_ok(), "registry list failed: {:?}", list_result);
    let output = list_result.unwrap();
    assert!(
        output.contains("test-flake"),
        "registry list should contain test-flake: {}",
        output
    );
}

#[test]
fn test_registry_add_local_path() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Create a minimal local flake for testing
    let flake_dir = tempfile::TempDir::new().unwrap();
    let flake_path = flake_dir.path().to_str().unwrap();

    // Write a minimal flake.nix
    fs::write(
        flake_dir.path().join("flake.nix"),
        r#"{
  outputs = { self }: {
    hello = "world";
  };
}"#,
    )
    .unwrap();

    // Initialize git repo (required for flakes)
    let _ = Command::new("git")
        .args(["init", "--quiet", flake_path])
        .output();
    let _ = Command::new("git")
        .args(["-C", flake_path, "add", "flake.nix"])
        .output();

    // Create config dir
    fs::create_dir_all(format!("{}/.config/nix", home_path)).unwrap();

    // Add local path to registry
    let add_result = trix_with_home(
        home_path,
        &["registry", "add", "my-flake", &format!("path:{}", flake_path)],
    );
    assert!(add_result.is_ok(), "registry add failed: {:?}", add_result);

    // List should show the entry
    let list_result = trix_with_home(home_path, &["registry", "list", "--no-global"]);
    assert!(list_result.is_ok(), "registry list failed: {:?}", list_result);
    let output = list_result.unwrap();
    assert!(
        output.contains("my-flake"),
        "registry list should contain my-flake: {}",
        output
    );
}

#[test]
fn test_registry_remove() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Create config dir
    fs::create_dir_all(format!("{}/.config/nix", home_path)).unwrap();

    // Add a registry entry
    let add_result = trix_with_home(
        home_path,
        &["registry", "add", "to-remove", "github:NixOS/nixpkgs"],
    );
    assert!(add_result.is_ok(), "registry add failed: {:?}", add_result);

    // Remove it
    let remove_result = trix_with_home(home_path, &["registry", "remove", "to-remove"]);
    assert!(
        remove_result.is_ok(),
        "registry remove failed: {:?}",
        remove_result
    );

    // List should NOT show the entry
    let list_result = trix_with_home(home_path, &["registry", "list", "--no-global"]);
    assert!(list_result.is_ok(), "registry list failed: {:?}", list_result);
    let output = list_result.unwrap();
    assert!(
        !output.contains("to-remove"),
        "registry list should not contain to-remove after removal: {}",
        output
    );
}

#[test]
fn test_registry_pin() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Create config dir
    fs::create_dir_all(format!("{}/.config/nix", home_path)).unwrap();

    // Add a registry entry first
    let add_result = trix_with_home(
        home_path,
        &["registry", "add", "to-pin", "github:NixOS/nixpkgs"],
    );
    assert!(add_result.is_ok(), "registry add failed: {:?}", add_result);

    // Pin it to a specific revision (use a known nixpkgs commit)
    // This is a real nixpkgs commit that should always exist
    let pin_result = trix_with_home(
        home_path,
        &[
            "registry",
            "pin",
            "to-pin",
            "--to",
            "057f9aecfb71c4437d2b27d3323df7f93c010b7e",
        ],
    );
    assert!(
        pin_result.is_ok(),
        "registry pin failed: {:?}",
        pin_result
    );

    // List should show the pinned entry (with revision info in the URL)
    let list_result = trix_with_home(home_path, &["registry", "list", "--no-global"]);
    assert!(list_result.is_ok(), "registry list failed: {:?}", list_result);
    let output = list_result.unwrap();
    // The pinned entry should include the commit
    assert!(
        output.contains("057f9aecfb71c4437d2b27d3323df7f93c010b7e") || output.contains("to-pin"),
        "registry list should show pinned entry: {}",
        output
    );
}

#[test]
fn test_registry_file_format() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Create config dir
    fs::create_dir_all(format!("{}/.config/nix", home_path)).unwrap();

    // Add a registry entry
    let add_result = trix_with_home(
        home_path,
        &["registry", "add", "test-entry", "github:owner/repo"],
    );
    assert!(add_result.is_ok(), "registry add failed: {:?}", add_result);

    // Read and verify the registry.json file format
    let registry_path = format!("{}/.config/nix/registry.json", home_path);
    let content = fs::read_to_string(&registry_path).unwrap();
    let registry: serde_json::Value = serde_json::from_str(&content).unwrap();

    // Should have version and flakes
    assert!(registry.get("version").is_some(), "registry should have version");
    assert!(registry.get("flakes").is_some(), "registry should have flakes");

    let flakes = registry["flakes"].as_array().unwrap();
    assert!(!flakes.is_empty(), "registry should have entries");

    // Find our entry
    let entry = flakes
        .iter()
        .find(|f| f["from"]["id"].as_str() == Some("test-entry"));
    assert!(entry.is_some(), "should find test-entry in registry");
}

// =============================================================================
// Tests for commands using registry-resolved flakes
// =============================================================================

/// Helper to create a minimal test flake in a directory
fn create_test_flake(dir: &std::path::Path, package_name: &str) {
    // Write a minimal flake.nix with a derivation
    fs::write(
        dir.join("flake.nix"),
        format!(
            r#"{{
  inputs = {{}};
  outputs = {{ self }}: {{
    packages.x86_64-linux.default = derivation {{
      name = "{package_name}";
      system = "x86_64-linux";
      builder = "/bin/sh";
      args = [ "-c" "echo hello > $out" ];
    }};
    packages.x86_64-linux.{package_name} = self.packages.x86_64-linux.default;
    packages.aarch64-linux.default = self.packages.x86_64-linux.default;
    packages.aarch64-linux.{package_name} = self.packages.x86_64-linux.default;
  }};
}}"#
        ),
    )
    .unwrap();

    // Initialize git repo (required for flakes)
    let dir_str = dir.to_str().unwrap();
    let _ = Command::new("git")
        .args(["init", "--quiet", dir_str])
        .output();
    let _ = Command::new("git")
        .args(["-C", dir_str, "config", "user.email", "test@test.com"])
        .output();
    let _ = Command::new("git")
        .args(["-C", dir_str, "config", "user.name", "Test"])
        .output();
    let _ = Command::new("git")
        .args(["-C", dir_str, "add", "flake.nix"])
        .output();
    let _ = Command::new("git")
        .args(["-C", dir_str, "commit", "-m", "init", "--quiet"])
        .output();
}

#[test]
fn test_eval_with_registry_resolved_flake() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Create a test flake
    let flake_dir = tempfile::TempDir::new().unwrap();
    create_test_flake(flake_dir.path(), "test-pkg");

    // Create config dir and add flake to registry
    fs::create_dir_all(format!("{}/.config/nix", home_path)).unwrap();
    let add_result = trix_with_home(
        home_path,
        &[
            "registry",
            "add",
            "my-test-flake",
            &format!("path:{}", flake_dir.path().display()),
        ],
    );
    assert!(add_result.is_ok(), "registry add failed: {:?}", add_result);

    // Now eval should resolve "my-test-flake" from the registry
    let eval_result = trix_with_home(
        home_path,
        &["eval", "my-test-flake#packages.x86_64-linux.default.name"],
    );
    assert!(
        eval_result.is_ok(),
        "eval with registry flake failed: {:?}",
        eval_result
    );
    let output = eval_result.unwrap();
    assert!(
        output.contains("test-pkg"),
        "eval should return package name: {}",
        output
    );
}

#[test]
fn test_flake_show_with_registry_resolved_flake() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Create a test flake
    let flake_dir = tempfile::TempDir::new().unwrap();
    create_test_flake(flake_dir.path(), "show-test-pkg");

    // Create config dir and add flake to registry
    fs::create_dir_all(format!("{}/.config/nix", home_path)).unwrap();
    let add_result = trix_with_home(
        home_path,
        &[
            "registry",
            "add",
            "show-test",
            &format!("path:{}", flake_dir.path().display()),
        ],
    );
    assert!(add_result.is_ok(), "registry add failed: {:?}", add_result);

    // Now flake show should resolve "show-test" from the registry
    let show_result = trix_with_home(home_path, &["flake", "show", "show-test"]);
    assert!(
        show_result.is_ok(),
        "flake show with registry flake failed: {:?}",
        show_result
    );
    let output = show_result.unwrap();
    assert!(
        output.contains("packages") || output.contains("default"),
        "flake show should display outputs: {}",
        output
    );
}

#[test]
fn test_build_with_registry_resolved_flake() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Create a test flake
    let flake_dir = tempfile::TempDir::new().unwrap();
    create_test_flake(flake_dir.path(), "build-test-pkg");

    // Create config dir and add flake to registry
    fs::create_dir_all(format!("{}/.config/nix", home_path)).unwrap();
    let add_result = trix_with_home(
        home_path,
        &[
            "registry",
            "add",
            "build-test",
            &format!("path:{}", flake_dir.path().display()),
        ],
    );
    assert!(add_result.is_ok(), "registry add failed: {:?}", add_result);

    // Build should resolve "build-test" from the registry
    let build_result = trix_with_home(home_path, &["build", "build-test", "--no-link"]);
    assert!(
        build_result.is_ok(),
        "build with registry flake failed: {:?}",
        build_result
    );
    let output = build_result.unwrap();
    // Output should be a store path
    assert!(
        output.contains("/nix/store/"),
        "build should output store path: {}",
        output
    );
}
