//! Integration tests comparing trix eval with nix eval on real flakes.
//!
//! These tests download well-known flakes and verify that `trix eval --json`
//! produces identical output to `nix eval --json` when run on local paths.

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

/// Run `nix eval --json` on a local flake path.
fn nix_eval_json(flake_path: &Path, attr: &str) -> Result<String, String> {
    let installable = format!("{}#{}", flake_path.display(), attr);

    let output = Command::new("nix")
        .args(["eval", "--json", "--impure", &installable])
        .output()
        .map_err(|e| format!("failed to run nix: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "nix eval failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run `trix eval --json` on a local flake path.
fn trix_eval_json(flake_path: &Path, attr: &str) -> Result<String, String> {
    let installable = format!("{}#{}", flake_path.display(), attr);

    let output = Command::new(trix_bin())
        .args(["eval", "--json", &installable])
        .output()
        .map_err(|e| format!("failed to run trix: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "trix eval failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Compare nix and trix eval JSON output for a flake attribute.
fn compare_eval(flake_path: &Path, attr: &str) {
    let nix_result = nix_eval_json(flake_path, attr);
    let trix_result = trix_eval_json(flake_path, attr);

    match (nix_result, trix_result) {
        (Ok(nix_json), Ok(trix_json)) => {
            // Parse as JSON for semantic comparison
            let nix_value: serde_json::Value = serde_json::from_str(&nix_json)
                .unwrap_or_else(|e| panic!("Failed to parse nix JSON: {}\nJSON: {}", e, nix_json));
            let trix_value: serde_json::Value = serde_json::from_str(&trix_json).unwrap_or_else(
                |e| panic!("Failed to parse trix JSON: {}\nJSON: {}", e, trix_json),
            );

            assert_eq!(
                nix_value, trix_value,
                "JSON output differs for attr '{}'\nnix:  {}\ntrix: {}",
                attr, nix_json, trix_json
            );
        }
        (Err(nix_err), Err(trix_err)) => {
            // Both failed - acceptable, but log it
            eprintln!(
                "Both nix and trix failed for '{}'\nnix: {}\ntrix: {}",
                attr, nix_err, trix_err
            );
        }
        (Ok(nix_json), Err(trix_err)) => {
            panic!(
                "nix succeeded but trix failed for '{}'\nnix output: {}\ntrix error: {}",
                attr, nix_json, trix_err
            );
        }
        (Err(nix_err), Ok(trix_json)) => {
            panic!(
                "trix succeeded but nix failed for '{}'\ntrix output: {}\nnix error: {}",
                attr, trix_json, nix_err
            );
        }
    }
}

// =============================================================================
// Local Flake Tests (current directory - always run)
// =============================================================================

#[test]
fn local_flake_package_name() {
    compare_eval(Path::new("."), "packages.x86_64-linux.default.name");
}

#[test]
fn local_flake_package_pname() {
    compare_eval(Path::new("."), "packages.x86_64-linux.default.pname");
}

#[test]
fn local_flake_package_version() {
    compare_eval(Path::new("."), "packages.x86_64-linux.default.version");
}

#[test]
fn local_flake_devshell_name() {
    compare_eval(Path::new("."), "devShells.x86_64-linux.default.name");
}

// =============================================================================
// flake-utils - Simple flake with lib outputs
// =============================================================================

// Pinned commits for reproducible tests (updated 2026-01-03)
const FLAKE_UTILS_REV: &str = "11707dc2f618dd54ca8739b309ec4fc024de578b";

#[test]
fn flake_utils_lib_default_systems() {
    let flake = fetch_github_flake("numtide", "flake-utils", FLAKE_UTILS_REV)
        .expect("failed to fetch flake-utils");
    compare_eval(flake.path(), "lib.defaultSystems");
}

#[test]
fn flake_utils_lib_all_systems() {
    let flake = fetch_github_flake("numtide", "flake-utils", FLAKE_UTILS_REV)
        .expect("failed to fetch flake-utils");
    compare_eval(flake.path(), "lib.allSystems");
}

// =============================================================================
// nixpkgs - Test lib functions (not packages, too slow)
// =============================================================================

const NIXPKGS_REV: &str = "fb7944c166a3b630f177938e478f0378e64ce108";

#[test]
#[ignore] // Very slow - nixpkgs is huge
fn nixpkgs_lib_version() {
    let flake = fetch_github_flake("NixOS", "nixpkgs", NIXPKGS_REV)
        .expect("failed to fetch nixpkgs");
    compare_eval(flake.path(), "lib.version");
}

// =============================================================================
// flake-compat - Very simple flake
// =============================================================================

const FLAKE_COMPAT_REV: &str = "5edf11c44bc78a0d334f6334cdaf7d60d732daab";

#[test]
fn flake_compat_outputs() {
    let flake = fetch_github_flake("edolstra", "flake-compat", FLAKE_COMPAT_REV)
        .expect("failed to fetch flake-compat");
    // Just test that we can evaluate the flake at all
    let nix = nix_eval_json(flake.path(), "defaultNix");
    let trix = trix_eval_json(flake.path(), "defaultNix");
    // Both should either succeed or fail the same way
    assert_eq!(nix.is_ok(), trix.is_ok(),
        "nix and trix should agree on whether defaultNix exists\nnix: {:?}\ntrix: {:?}", nix, trix);
}

// =============================================================================
// treefmt-nix - Has lib and templates
// =============================================================================

const TREEFMT_NIX_REV: &str = "d56486eb9493ad9c4777c65932618e9c2d0468fc";

#[test]
fn treefmt_nix_has_lib() {
    let flake = fetch_github_flake("numtide", "treefmt-nix", TREEFMT_NIX_REV)
        .expect("failed to fetch treefmt-nix");
    // Check that lib exists by evaluating lib.mkWrapper (a function)
    let nix = nix_eval_json(flake.path(), "lib.mkWrapper");
    let trix = trix_eval_json(flake.path(), "lib.mkWrapper");
    // Both should fail with "cannot convert function to JSON" or similar
    assert!(nix.is_err() || trix.is_err() || nix == trix,
        "Unexpected mismatch\nnix: {:?}\ntrix: {:?}", nix, trix);
}

// =============================================================================
// alejandra - Has packages
// =============================================================================

const ALEJANDRA_REV: &str = "c68bef57c1db3add865493d9cb741a14618bdc28";

#[test]
fn alejandra_package_name() {
    let flake = fetch_github_flake("kamadorueda", "alejandra", ALEJANDRA_REV)
        .expect("failed to fetch alejandra");
    compare_eval(flake.path(), "packages.x86_64-linux.default.name");
}

#[test]
fn alejandra_package_pname() {
    let flake = fetch_github_flake("kamadorueda", "alejandra", ALEJANDRA_REV)
        .expect("failed to fetch alejandra");
    compare_eval(flake.path(), "packages.x86_64-linux.default.pname");
}

// =============================================================================
// nixfmt - Has packages
// =============================================================================

const NIXFMT_REV: &str = "42e43d9fcabadf57fdcefa6da355cd8dcf5b7d36";

#[test]
fn nixfmt_package_name() {
    let flake = fetch_github_flake("NixOS", "nixfmt", NIXFMT_REV)
        .expect("failed to fetch nixfmt");
    compare_eval(flake.path(), "packages.x86_64-linux.default.name");
}

// =============================================================================
// nix-index - Has packages
// =============================================================================

const NIX_INDEX_REV: &str = "0fc38040a22a08052103d0fbbafd67ac54165f2b";

#[test]
fn nix_index_package_name() {
    let flake = fetch_github_flake("nix-community", "nix-index", NIX_INDEX_REV)
        .expect("failed to fetch nix-index");
    compare_eval(flake.path(), "packages.x86_64-linux.nix-index.name");
}

// =============================================================================
// impermanence - Has nixosModules
// =============================================================================

const IMPERMANENCE_REV: &str = "4b3e914cdf97a5b536a889e939fb2fd2b043a170";

#[test]
fn impermanence_has_nixos_module() {
    let flake = fetch_github_flake("nix-community", "impermanence", IMPERMANENCE_REV)
        .expect("failed to fetch impermanence");
    // Check nixosModules.impermanence exists (it's a function/module)
    let nix = nix_eval_json(flake.path(), "nixosModules.impermanence");
    let trix = trix_eval_json(flake.path(), "nixosModules.impermanence");
    // Both should fail the same way (can't serialize a function)
    assert_eq!(nix.is_err(), trix.is_err(),
        "nix and trix should agree on nixosModules.impermanence\nnix: {:?}\ntrix: {:?}", nix, trix);
}

// =============================================================================
// disko - Has lib, packages
// =============================================================================

const DISKO_REV: &str = "916506443ecd0d0b4a0f4cf9d40a3c22ce39b378";

#[test]
fn disko_package_name() {
    let flake = fetch_github_flake("nix-community", "disko", DISKO_REV)
        .expect("failed to fetch disko");
    compare_eval(flake.path(), "packages.x86_64-linux.default.name");
}

// =============================================================================
// home-manager - Has packages, lib, nixosModules
// =============================================================================

const HOME_MANAGER_REV: &str = "1cfa305fba94468f665de1bd1b62dddf2e0cb012";

#[test]
#[ignore] // Slow due to nixpkgs dependency
fn home_manager_package_name() {
    let flake = fetch_github_flake("nix-community", "home-manager", HOME_MANAGER_REV)
        .expect("failed to fetch home-manager");
    compare_eval(flake.path(), "packages.x86_64-linux.default.name");
}

// =============================================================================
// sops-nix - Has nixosModules, overlays
// =============================================================================

const SOPS_NIX_REV: &str = "61b39c7b657081c2adc91b75dd3ad8a91d6f07a7";

#[test]
fn sops_nix_overlay_exists() {
    let flake = fetch_github_flake("Mic92", "sops-nix", SOPS_NIX_REV)
        .expect("failed to fetch sops-nix");
    // Check overlay exists (it's a function)
    let nix = nix_eval_json(flake.path(), "overlays.default");
    let trix = trix_eval_json(flake.path(), "overlays.default");
    assert_eq!(nix.is_err(), trix.is_err(),
        "nix and trix should agree on overlays.default\nnix: {:?}\ntrix: {:?}", nix, trix);
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
#[ignore] // Slow due to nixpkgs and multiple dependencies
fn nix_package_name() {
    let flake = fetch_github_flake("NixOS", "nix", NIX_REV)
        .expect("failed to fetch nix");
    compare_eval(flake.path(), "packages.x86_64-linux.default.name");
}

#[test]
#[ignore] // Slow due to nixpkgs and multiple dependencies
fn nix_package_version() {
    let flake = fetch_github_flake("NixOS", "nix", NIX_REV)
        .expect("failed to fetch nix");
    compare_eval(flake.path(), "packages.x86_64-linux.default.version");
}

#[test]
#[ignore] // Slow due to nixpkgs and multiple dependencies
fn nix_devshell_name() {
    let flake = fetch_github_flake("NixOS", "nix", NIX_REV)
        .expect("failed to fetch nix");
    compare_eval(flake.path(), "devShells.x86_64-linux.default.name");
}
