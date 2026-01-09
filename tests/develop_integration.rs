//! Integration tests for trix develop on real flakes.
//!
//! These tests verify that `trix develop` can successfully evaluate devShells
//! for various flakes with complex input patterns.

use std::path::Path;
use std::process::Command;

/// Get the path to the trix binary.
fn trix_bin() -> String {
    std::env::var("CARGO_BIN_EXE_trix").unwrap_or_else(|_| "target/debug/trix".to_string())
}

/// Clone a GitHub flake to a temporary directory at a specific commit.
/// Uses shallow fetch for speed.
fn fetch_github_flake(owner: &str, repo: &str, rev: &str) -> Result<tempfile::TempDir, String> {
    let temp_dir =
        tempfile::TempDir::new().map_err(|e| format!("failed to create temp dir: {}", e))?;

    let repo_url = format!("https://github.com/{}/{}.git", owner, repo);
    let dir = temp_dir.path().to_str().unwrap();

    // Initialize empty repo
    let init = Command::new("git")
        .args(["init", "--quiet", dir])
        .output()
        .map_err(|e| format!("failed to run git init: {}", e))?;

    if !init.status.success() {
        return Err(format!(
            "git init failed: {}",
            String::from_utf8_lossy(&init.stderr)
        ));
    }

    // Add remote
    let remote = Command::new("git")
        .args(["-C", dir, "remote", "add", "origin", &repo_url])
        .output()
        .map_err(|e| format!("failed to add remote: {}", e))?;

    if !remote.status.success() {
        return Err(format!(
            "git remote add failed: {}",
            String::from_utf8_lossy(&remote.stderr)
        ));
    }

    // Fetch the specific commit (shallow)
    let fetch = Command::new("git")
        .args(["-C", dir, "fetch", "--depth", "1", "--quiet", "origin", rev])
        .output()
        .map_err(|e| format!("failed to fetch: {}", e))?;

    if !fetch.status.success() {
        return Err(format!(
            "git fetch failed: {}",
            String::from_utf8_lossy(&fetch.stderr)
        ));
    }

    // Checkout FETCH_HEAD
    let checkout = Command::new("git")
        .args(["-C", dir, "checkout", "--quiet", "FETCH_HEAD"])
        .output()
        .map_err(|e| format!("failed to checkout: {}", e))?;

    if !checkout.status.success() {
        return Err(format!(
            "git checkout failed: {}",
            String::from_utf8_lossy(&checkout.stderr)
        ));
    }

    // Verify flake.nix exists
    if !temp_dir.path().join("flake.nix").exists() {
        return Err("flake.nix not found in cloned repository".to_string());
    }

    Ok(temp_dir)
}

/// Test that trix develop can evaluate a devShell by running a simple command.
/// Uses `-c true` to just verify the shell can be entered.
fn test_develop_evaluates(flake_path: &Path) {
    let output = Command::new(trix_bin())
        .args(["develop", flake_path.to_str().unwrap(), "-c", "true"])
        .output()
        .expect("failed to run trix");

    assert!(
        output.status.success(),
        "trix develop failed for {}: {}",
        flake_path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Compare trix develop with nix develop - both should succeed or both should fail.
fn compare_develop(flake_path: &Path) {
    let trix_output = Command::new(trix_bin())
        .args(["develop", flake_path.to_str().unwrap(), "-c", "true"])
        .output()
        .expect("failed to run trix");

    let nix_output = Command::new("nix")
        .args(["develop", flake_path.to_str().unwrap(), "-c", "true"])
        .output()
        .expect("failed to run nix");

    match (nix_output.status.success(), trix_output.status.success()) {
        (true, true) => {
            // Both succeeded - good
        }
        (false, false) => {
            // Both failed - acceptable
            eprintln!(
                "Both nix and trix develop failed for {}\nnix: {}\ntrix: {}",
                flake_path.display(),
                String::from_utf8_lossy(&nix_output.stderr),
                String::from_utf8_lossy(&trix_output.stderr)
            );
        }
        (true, false) => {
            panic!(
                "nix develop succeeded but trix develop failed for {}\ntrix error: {}",
                flake_path.display(),
                String::from_utf8_lossy(&trix_output.stderr)
            );
        }
        (false, true) => {
            panic!(
                "trix develop succeeded but nix develop failed for {}\nnix error: {}",
                flake_path.display(),
                String::from_utf8_lossy(&nix_output.stderr)
            );
        }
    }
}

// =============================================================================
// Local Flake Tests (current directory - always run)
// =============================================================================

#[test]
fn local_develop_evaluates() {
    test_develop_evaluates(Path::new("."));
}

#[test]
fn local_develop_runs_command() {
    // trix's -c now takes trailing args like nix develop
    let output = Command::new(trix_bin())
        .args(["develop", ".", "-c", "echo", "hello"])
        .output()
        .expect("failed to run trix");

    assert!(
        output.status.success(),
        "trix develop failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("hello"),
        "expected 'hello' in output, got: {}",
        stdout
    );
}

// =============================================================================
// flake-utils - Simple flake with lib outputs (has devShell)
// =============================================================================

const FLAKE_UTILS_REV: &str = "11707dc2f618dd54ca8739b309ec4fc024de578b";

#[test]
fn flake_utils_develop() {
    let flake = fetch_github_flake("numtide", "flake-utils", FLAKE_UTILS_REV)
        .expect("failed to fetch flake-utils");
    compare_develop(flake.path());
}

// =============================================================================
// alejandra - Uses deprecated devShell (singular) output, trix supports fallback
// =============================================================================

const ALEJANDRA_REV: &str = "c68bef57c1db3add865493d9cb741a14618bdc28";

#[test]
fn alejandra_develop() {
    let flake = fetch_github_flake("kamadorueda", "alejandra", ALEJANDRA_REV)
        .expect("failed to fetch alejandra");
    compare_develop(flake.path());
}

// =============================================================================
// nixfmt - Has packages
// =============================================================================

const NIXFMT_REV: &str = "42e43d9fcabadf57fdcefa6da355cd8dcf5b7d36";

#[test]
fn nixfmt_develop() {
    let flake = fetch_github_flake("NixOS", "nixfmt", NIXFMT_REV)
        .expect("failed to fetch nixfmt");
    compare_develop(flake.path());
}

// =============================================================================
// NixOS/nix - Complex flake with empty follows, flake-parts, tarball inputs
// This flake exercises:
// - Empty follows (e.g., "gitignore": [], "flake-compat": [])
// - flake-parts integration
// - Tarball inputs (nixpkgs from releases.nixos.org)
// - Complex input graph with multiple dependencies
// =============================================================================

const NIX_REV: &str = "b474e8d249964ac24cf003837a1aba0b2c700156";

#[test]
#[ignore] // Slow - downloads nixpkgs and builds devShell
fn nix_develop_evaluates() {
    let flake = fetch_github_flake("NixOS", "nix", NIX_REV)
        .expect("failed to fetch nix");
    test_develop_evaluates(flake.path());
}

#[test]
#[ignore] // Slow - downloads nixpkgs and builds devShell
fn nix_develop_compare() {
    let flake = fetch_github_flake("NixOS", "nix", NIX_REV)
        .expect("failed to fetch nix");
    compare_develop(flake.path());
}

// =============================================================================
// home-manager - Uses packages fallback (no devShells), trix supports this
// =============================================================================

const HOME_MANAGER_REV: &str = "1cfa305fba94468f665de1bd1b62dddf2e0cb012";

#[test]
fn home_manager_develop() {
    let flake = fetch_github_flake("nix-community", "home-manager", HOME_MANAGER_REV)
        .expect("failed to fetch home-manager");
    compare_develop(flake.path());
}

// =============================================================================
// disko - Has packages, lib
// =============================================================================

const DISKO_REV: &str = "916506443ecd0d0b4a0f4cf9d40a3c22ce39b378";

#[test]
fn disko_develop() {
    let flake = fetch_github_flake("nix-community", "disko", DISKO_REV)
        .expect("failed to fetch disko");
    compare_develop(flake.path());
}
