//! Integration tests for `trix profile` commands.
//!
//! Note: Profile commands interact with the Nix store, so we test:
//! 1. Error handling for missing profiles
//! 2. Read-only operations on the real user profile (if it exists)
//! 3. Basic command invocation without modifying state

use std::env;
use std::process::Command;

/// Get the path to the trix binary.
fn trix_bin() -> String {
    env::var("CARGO_BIN_EXE_trix").unwrap_or_else(|_| "target/debug/trix".to_string())
}

/// Run trix with custom HOME to test profile isolation.
fn trix_with_home(home: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(trix_bin())
        .env("HOME", home)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run trix: {}", e))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run trix with default environment.
fn trix(args: &[&str]) -> Result<String, String> {
    let output = Command::new(trix_bin())
        .args(args)
        .output()
        .map_err(|e| format!("failed to run trix: {}", e))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[test]
fn test_profile_list_no_profile_gives_error() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Without a .nix-profile symlink, should error
    let result = trix_with_home(home_path, &["profile", "list"]);
    assert!(result.is_err(), "should fail without profile link");
    let err = result.unwrap_err();
    assert!(
        err.contains("profile") || err.contains("link"),
        "error should mention profile: {}",
        err
    );
}

#[test]
fn test_profile_list_json_no_profile_gives_error() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Without a .nix-profile symlink, should error
    let result = trix_with_home(home_path, &["profile", "list", "--json"]);
    assert!(result.is_err(), "should fail without profile link");
}

#[test]
fn test_profile_history_falls_back_to_default() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Without a .nix-profile symlink, falls back to /nix/var/nix/profiles/per-user/$USER
    // This should succeed if the user has any profile history
    let result = trix_with_home(home_path, &["profile", "history"]);
    // Either succeeds (finds fallback profile) or fails cleanly
    // We can't assert failure because it may find the real user's profiles
    match result {
        Ok(output) => {
            // If it succeeded, it found the fallback profile
            assert!(
                output.contains("Version") || output.is_empty(),
                "unexpected history output: {}",
                output
            );
        }
        Err(err) => {
            // If it failed, it should be because there's no profile
            assert!(
                err.contains("profile") || err.contains("directory"),
                "unexpected error: {}",
                err
            );
        }
    }
}

#[test]
fn test_profile_remove_no_profile_gives_error() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Without a .nix-profile symlink, should error
    let result = trix_with_home(home_path, &["profile", "remove", "some-package"]);
    assert!(result.is_err(), "should fail without profile link");
}

#[test]
fn test_profile_list_with_real_profile() {
    // This test uses the real user's profile to verify list works
    // Skip if no profile exists
    let home = env::var("HOME").unwrap_or_default();
    let profile_link = format!("{}/.nix-profile", home);

    if !std::path::Path::new(&profile_link).exists() {
        // No profile, skip this test
        return;
    }

    // Should succeed with real profile
    let result = trix(&["profile", "list"]);
    assert!(result.is_ok(), "profile list failed: {:?}", result);
}

#[test]
fn test_profile_list_json_with_real_profile() {
    // This test uses the real user's profile to verify JSON output
    // Skip if no profile exists
    let home = env::var("HOME").unwrap_or_default();
    let profile_link = format!("{}/.nix-profile", home);

    if !std::path::Path::new(&profile_link).exists() {
        return;
    }

    let result = trix(&["profile", "list", "--json"]);
    assert!(result.is_ok(), "profile list --json failed: {:?}", result);

    // Verify it's valid JSON
    let output = result.unwrap();
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&output);
    assert!(parsed.is_ok(), "output should be valid JSON: {}", output);
}

#[test]
fn test_profile_history_with_real_profile() {
    // Skip if no profile exists
    let home = env::var("HOME").unwrap_or_default();
    let profile_link = format!("{}/.nix-profile", home);

    if !std::path::Path::new(&profile_link).exists() {
        return;
    }

    let result = trix(&["profile", "history"]);
    assert!(result.is_ok(), "profile history failed: {:?}", result);

    let output = result.unwrap();
    // History should show at least one generation
    assert!(
        output.contains("Generation") || output.lines().count() > 0,
        "history should show generations: {}",
        output
    );
}

#[test]
fn test_profile_wipe_history_dry_run() {
    // Skip if no profile exists - we just want to verify the command parses correctly
    let home = env::var("HOME").unwrap_or_default();
    let profile_link = format!("{}/.nix-profile", home);

    if !std::path::Path::new(&profile_link).exists() {
        return;
    }

    // Note: wipe-history without --dry-run would delete history
    // We just test that the command is recognized
    let result = trix(&["profile", "wipe-history", "--help"]);
    assert!(result.is_ok(), "wipe-history --help failed: {:?}", result);
    let output = result.unwrap();
    assert!(
        output.contains("Delete"),
        "help should describe the command: {}",
        output
    );
}

#[test]
fn test_profile_diff_closures_help() {
    // Just verify the command is recognized
    let result = trix(&["profile", "diff-closures", "--help"]);
    assert!(
        result.is_ok(),
        "diff-closures --help failed: {:?}",
        result
    );
    let output = result.unwrap();
    assert!(
        output.contains("closure") || output.contains("difference"),
        "help should describe the command: {}",
        output
    );
}

#[test]
fn test_profile_upgrade_help() {
    // Just verify the command is recognized
    let result = trix(&["profile", "upgrade", "--help"]);
    assert!(result.is_ok(), "upgrade --help failed: {:?}", result);
    let output = result.unwrap();
    assert!(
        output.contains("Upgrade"),
        "help should describe the command: {}",
        output
    );
}

#[test]
fn test_profile_rollback_help() {
    // Just verify the command is recognized
    let result = trix(&["profile", "rollback", "--help"]);
    assert!(result.is_ok(), "rollback --help failed: {:?}", result);
    let output = result.unwrap();
    assert!(
        output.contains("Roll back") || output.contains("previous"),
        "help should describe the command: {}",
        output
    );
}

#[test]
fn test_profile_add_invalid_installable() {
    // Test that adding an invalid package gives a reasonable error
    let result = trix(&["profile", "add", "nixpkgs#nonexistent-package-xyz123"]);
    // This should fail (package doesn't exist)
    assert!(
        result.is_err(),
        "adding nonexistent package should fail: {:?}",
        result
    );
}

#[test]
fn test_profile_install_is_alias_for_add() {
    // Verify install works as alias for add
    let result = trix(&["profile", "install", "--help"]);
    assert!(result.is_ok(), "install --help failed: {:?}", result);
    let output = result.unwrap();
    // Should show add-like help
    assert!(
        output.contains("package") || output.contains("profile"),
        "help should describe adding packages: {}",
        output
    );
}

#[test]
fn test_profile_with_fake_profile_link() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Create a fake .nix-profile symlink pointing to a non-existent store path
    let fake_profile_link = format!("{}/.nix-profile", home_path);
    let _ = std::os::unix::fs::symlink(
        "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-fake-profile",
        &fake_profile_link,
    );

    // Should fail gracefully when following a broken symlink
    let result = trix_with_home(home_path, &["profile", "list"]);
    assert!(result.is_err(), "should fail with broken symlink");
}

// =============================================================================
// Tests for profile commands with registry-resolved flakes
// =============================================================================

use std::fs;
use std::path::Path;

/// Helper to run trix with custom HOME and XDG_CONFIG_HOME for registry isolation
fn trix_with_config(home: &str, args: &[&str]) -> Result<String, String> {
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

/// Helper to create a minimal test flake in a directory
fn create_test_flake(dir: &Path, package_name: &str) {
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
      args = [ "-c" "mkdir -p $out/bin && echo '#!/bin/sh' > $out/bin/{package_name} && echo 'echo hello' >> $out/bin/{package_name} && chmod +x $out/bin/{package_name}" ];
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
fn test_profile_add_with_registry_resolved_flake() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Create a test flake
    let flake_dir = tempfile::TempDir::new().unwrap();
    create_test_flake(flake_dir.path(), "profile-test-pkg");

    // Create config dir and add flake to registry
    fs::create_dir_all(format!("{}/.config/nix", home_path)).unwrap();
    let add_registry_result = trix_with_config(
        home_path,
        &[
            "registry",
            "add",
            "profile-test",
            &format!("path:{}", flake_dir.path().display()),
        ],
    );
    assert!(
        add_registry_result.is_ok(),
        "registry add failed: {:?}",
        add_registry_result
    );

    // Note: We can't actually create profile directories without root permissions
    // So we test that the command at least resolves the registry correctly

    // Profile add should resolve "profile-test" from the registry
    // It will fail because we can't write to /nix/var/nix/profiles, but
    // the error should NOT be "remote flake references not yet supported"
    let add_result = trix_with_config(home_path, &["profile", "add", "profile-test"]);

    // The command should either succeed (if we have permissions) or fail with
    // a profile/build error, NOT a "remote flake references not yet supported" error
    match add_result {
        Ok(_output) => {
            // Success! The package was installed
        }
        Err(err) => {
            // Should NOT be a registry resolution error
            assert!(
                !err.contains("remote flake references not yet supported"),
                "should resolve registry flake, not fail with remote ref error: {}",
                err
            );
            // Acceptable errors are profile/permission related
            // or evaluation errors (which means registry resolved correctly)
        }
    }
}

#[test]
fn test_profile_install_alias_with_registry_flake() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Create a test flake
    let flake_dir = tempfile::TempDir::new().unwrap();
    create_test_flake(flake_dir.path(), "install-alias-pkg");

    // Create config dir and add flake to registry
    fs::create_dir_all(format!("{}/.config/nix", home_path)).unwrap();
    let add_registry_result = trix_with_config(
        home_path,
        &[
            "registry",
            "add",
            "install-alias-test",
            &format!("path:{}", flake_dir.path().display()),
        ],
    );
    assert!(
        add_registry_result.is_ok(),
        "registry add failed: {:?}",
        add_registry_result
    );

    // Profile install (alias for add) should also resolve registry
    let install_result = trix_with_config(home_path, &["profile", "install", "install-alias-test"]);

    match install_result {
        Ok(_output) => {
            // Success!
        }
        Err(err) => {
            // Should NOT be a registry resolution error
            assert!(
                !err.contains("remote flake references not yet supported"),
                "install alias should resolve registry flake: {}",
                err
            );
        }
    }
}

// =============================================================================
// Tests for priority-based conflict resolution
// =============================================================================

/// Helper to create a test flake that produces a package with a specific binary.
/// The package_name will be used for the derivation name, and the binary will contain an identifier.
fn create_test_flake_with_binary(dir: &Path, package_name: &str, binary_name: &str, identifier: &str) {
    // Write a flake that creates a package with a bin directory
    // Use runCommand to create a proper derivation with a custom name
    fs::write(
        dir.join("flake.nix"),
        format!(
            r#"{{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs = {{ self, nixpkgs }}: let
    systems = [ "x86_64-linux" "aarch64-linux" ];
    forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);
  in {{
    packages = forAllSystems (system: let
      pkgs = nixpkgs.legacyPackages.${{system}};
    in {{
      default = pkgs.runCommand "{package_name}" {{}} ''
        mkdir -p $out/bin
        echo '#!/bin/sh' > $out/bin/{binary_name}
        echo 'echo {identifier}' >> $out/bin/{binary_name}
        chmod +x $out/bin/{binary_name}
      '';
    }});
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
    // Generate lock file
    let _ = Command::new("nix")
        .args(["flake", "lock", dir_str])
        .output();
    let _ = Command::new("git")
        .args(["-C", dir_str, "add", "flake.lock"])
        .output();
    let _ = Command::new("git")
        .args(["-C", dir_str, "commit", "-m", "add lock", "--quiet", "--allow-empty"])
        .output();
}

/// Test that priority-based conflict resolution works correctly.
/// Lower priority numbers should win (take precedence) for conflicting files.
#[test]
#[ignore] // Slow: requires nixpkgs download
fn test_profile_priority_conflict_resolution() {
    // Use a unique isolated home directory with proper profile setup
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Create two test flakes that both provide a binary named "conflict-test"
    // but with different package names (which will appear in the store path)
    let flake_dir_a = tempfile::TempDir::new().unwrap();
    create_test_flake_with_binary(
        flake_dir_a.path(),
        "priority-pkg-a",  // package name (in store path)
        "conflict-test",   // binary name
        "pkg-a",           // identifier in output
    );

    let flake_dir_b = tempfile::TempDir::new().unwrap();
    create_test_flake_with_binary(
        flake_dir_b.path(),
        "priority-pkg-b",  // package name (in store path)
        "conflict-test",   // binary name
        "pkg-b",           // identifier in output
    );

    // Set up profile directory structure in the temp home
    // trix uses ~/.nix-profile which points to profile-N-link in ~/.local/state/nix/profiles
    let profile_state_dir = home.path().join(".local/state/nix/profiles");
    fs::create_dir_all(&profile_state_dir).unwrap();

    // Helper to run trix profile commands with isolated HOME
    let run_profile = |args: &[&str]| -> Result<String, String> {
        let mut full_args = vec!["profile"];
        full_args.extend(args);
        trix_with_home(home_path, &full_args)
    };

    // First, add pkg-a with default priority (5)
    let add_a = run_profile(&["add", flake_dir_a.path().to_str().unwrap()]);
    assert!(add_a.is_ok(), "failed to add pkg-a: {:?}", add_a);

    // Then add pkg-b with higher priority (lower number = wins)
    let add_b = run_profile(&[
        "add",
        "--priority",
        "1",
        flake_dir_b.path().to_str().unwrap(),
    ]);
    assert!(add_b.is_ok(), "failed to add pkg-b: {:?}", add_b);

    // The profile link should now exist at ~/.nix-profile
    let profile_link = home.path().join(".nix-profile");
    let profile_target = fs::read_link(&profile_link);
    assert!(profile_target.is_ok(), "profile link should exist at {:?}", profile_link);

    let profile_store_path = profile_target.unwrap();
    let binary_path = profile_store_path.join("bin/conflict-test");

    if binary_path.exists() {
        // The binary could be:
        // 1. A direct symlink to the store path binary
        // 2. Inside a bin directory that is a symlink
        // 3. A regular file (unlikely)
        let target_str = if binary_path.is_symlink() {
            // Direct symlink to the binary
            fs::read_link(&binary_path)
                .unwrap()
                .to_string_lossy()
                .to_string()
        } else {
            // The file itself - check parent bin dir
            let bin_dir = binary_path.parent().unwrap();
            if bin_dir.is_symlink() {
                // bin dir is symlinked to a package's bin
                fs::read_link(bin_dir)
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            } else {
                // Read the actual file content to check which package it came from
                fs::read_to_string(&binary_path).unwrap_or_default()
            }
        };

        // pkg-b should win because it has priority 1 (lower than pkg-a's 5)
        assert!(
            target_str.contains("pkg-b"),
            "pkg-b (priority 1) should win over pkg-a (priority 5), but got: {}",
            target_str
        );
    }

    // Verify both packages are listed in the profile
    let list_result = run_profile(&["list", "--json"]);
    assert!(list_result.is_ok(), "profile list failed: {:?}", list_result);

    let list_output = list_result.unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&list_output).unwrap();

    // Should have 2 packages
    let elements = manifest.as_array().unwrap();
    assert_eq!(
        elements.len(),
        2,
        "should have 2 packages installed: {:?}",
        elements
    );
}

/// Test that reinstalling a package with a different priority updates the conflict resolution.
#[test]
#[ignore] // Slow: requires nixpkgs download
fn test_profile_priority_update_on_reinstall() {
    // Use isolated home directory
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();

    // Create two test flakes with conflicting binaries
    let flake_dir_a = tempfile::TempDir::new().unwrap();
    create_test_flake_with_binary(
        flake_dir_a.path(),
        "update-priority-pkg-a",  // package name
        "update-conflict",        // binary name
        "update-a",               // identifier
    );

    let flake_dir_b = tempfile::TempDir::new().unwrap();
    create_test_flake_with_binary(
        flake_dir_b.path(),
        "update-priority-pkg-b",  // package name
        "update-conflict",        // binary name
        "update-b",               // identifier
    );

    // Set up profile directory structure
    let profile_state_dir = home.path().join(".local/state/nix/profiles");
    fs::create_dir_all(&profile_state_dir).unwrap();

    let run_profile = |args: &[&str]| -> Result<String, String> {
        let mut full_args = vec!["profile"];
        full_args.extend(args);
        trix_with_home(home_path, &full_args)
    };

    // Add pkg-a with priority 1 (wins)
    let add_a = run_profile(&[
        "add",
        "--priority",
        "1",
        flake_dir_a.path().to_str().unwrap(),
    ]);
    assert!(add_a.is_ok(), "failed to add pkg-a: {:?}", add_a);

    // Add pkg-b with priority 10 (loses)
    let add_b = run_profile(&[
        "add",
        "--priority",
        "10",
        flake_dir_b.path().to_str().unwrap(),
    ]);
    assert!(add_b.is_ok(), "failed to add pkg-b: {:?}", add_b);

    // Helper to get winning package from binary path
    let get_winner = |binary_path: &Path| -> String {
        if binary_path.is_symlink() {
            fs::read_link(binary_path)
                .unwrap()
                .to_string_lossy()
                .to_string()
        } else {
            let bin_dir = binary_path.parent().unwrap();
            if bin_dir.is_symlink() {
                fs::read_link(bin_dir)
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            } else {
                fs::read_to_string(binary_path).unwrap_or_default()
            }
        }
    };

    // Check that pkg-a wins initially
    let profile_link = home.path().join(".nix-profile");
    let profile_target = fs::read_link(&profile_link).unwrap();
    let binary_path = profile_target.join("bin/update-conflict");

    if binary_path.exists() {
        let target_str = get_winner(&binary_path);
        assert!(
            target_str.contains("update-priority-pkg-a"),
            "pkg-a (priority 1) should win initially: {}",
            target_str
        );
    }

    // Now reinstall pkg-b with priority 0 (should now win)
    // First remove it
    let remove_b = run_profile(&["remove", "update-priority-pkg-b"]);
    assert!(remove_b.is_ok(), "failed to remove pkg-b: {:?}", remove_b);

    // Re-add with higher priority (lower number)
    let readd_b = run_profile(&[
        "add",
        "--priority",
        "0",
        flake_dir_b.path().to_str().unwrap(),
    ]);
    assert!(readd_b.is_ok(), "failed to re-add pkg-b: {:?}", readd_b);

    // Check that pkg-b now wins
    let profile_target = fs::read_link(&profile_link).unwrap();
    let binary_path = profile_target.join("bin/update-conflict");

    if binary_path.exists() {
        let target_str = get_winner(&binary_path);
        assert!(
            target_str.contains("update-priority-pkg-b"),
            "pkg-b (priority 0) should win after reinstall: {}",
            target_str
        );
    }
}
