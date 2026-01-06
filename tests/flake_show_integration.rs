//! Integration tests comparing trix flake show with nix flake show on real flakes.
//!
//! These tests download well-known flakes and verify that `trix flake show --json`
//! produces identical output to `nix flake show --json` when run on local paths.

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

/// Run `nix flake show --json` on a local flake path.
fn nix_flake_show_json(flake_path: &Path, all_systems: bool) -> Result<String, String> {
    let mut args = vec!["flake", "show", "--json"];
    if all_systems {
        args.push("--all-systems");
    }
    args.push(flake_path.to_str().unwrap());

    let output = Command::new("nix")
        .args(&args)
        .output()
        .map_err(|e| format!("failed to run nix: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "nix flake show failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run `trix flake show --json` on a local flake path.
fn trix_flake_show_json(flake_path: &Path, all_systems: bool) -> Result<String, String> {
    let mut args = vec!["flake", "show", "--json"];
    if all_systems {
        args.push("--all-systems");
    }
    args.push(flake_path.to_str().unwrap());

    let output = Command::new(trix_bin())
        .args(&args)
        .output()
        .map_err(|e| format!("failed to run trix: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "trix flake show failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Compare nix and trix flake show JSON output.
fn compare_flake_show(flake_path: &Path, all_systems: bool) {
    let nix_result = nix_flake_show_json(flake_path, all_systems);
    let trix_result = trix_flake_show_json(flake_path, all_systems);

    match (nix_result, trix_result) {
        (Ok(nix_json), Ok(trix_json)) => {
            // Parse both as JSON for semantic comparison
            let nix_value: serde_json::Value = serde_json::from_str(&nix_json)
                .unwrap_or_else(|e| panic!("Failed to parse nix JSON: {}\nJSON: {}", e, nix_json));
            let trix_value: serde_json::Value = serde_json::from_str(&trix_json).unwrap_or_else(
                |e| panic!("Failed to parse trix JSON: {}\nJSON: {}", e, trix_json),
            );

            assert_eq!(
                nix_value, trix_value,
                "JSON output differs\nnix:  {}\ntrix: {}",
                nix_json, trix_json
            );
        }
        (Err(nix_err), Err(trix_err)) => {
            // Both failed - acceptable
            eprintln!(
                "Both nix and trix failed\nnix: {}\ntrix: {}",
                nix_err, trix_err
            );
        }
        (Ok(nix_json), Err(trix_err)) => {
            panic!(
                "nix succeeded but trix failed\nnix output: {}\ntrix error: {}",
                nix_json, trix_err
            );
        }
        (Err(nix_err), Ok(trix_json)) => {
            panic!(
                "trix succeeded but nix failed\ntrix output: {}\nnix error: {}",
                trix_json, nix_err
            );
        }
    }
}

// =============================================================================
// Local Flake Tests (current directory)
// =============================================================================

#[test]
fn local_flake_show() {
    compare_flake_show(Path::new("."), false);
}

#[test]
fn local_flake_show_all_systems() {
    compare_flake_show(Path::new("."), true);
}

// =============================================================================
// flake-utils - Simple flake with lib outputs
// =============================================================================

// Pinned commits for reproducible tests (updated 2026-01-03)
const FLAKE_UTILS_REV: &str = "11707dc2f618dd54ca8739b309ec4fc024de578b";

#[test]
fn flake_utils_show() {
    let flake =
        fetch_github_flake("numtide", "flake-utils", FLAKE_UTILS_REV).expect("failed to fetch flake-utils");
    compare_flake_show(flake.path(), false);
}

#[test]
fn flake_utils_show_all_systems() {
    let flake =
        fetch_github_flake("numtide", "flake-utils", FLAKE_UTILS_REV).expect("failed to fetch flake-utils");
    compare_flake_show(flake.path(), true);
}

// =============================================================================
// flake-compat - Very simple flake
// =============================================================================

const FLAKE_COMPAT_REV: &str = "5edf11c44bc78a0d334f6334cdaf7d60d732daab";

#[test]
fn flake_compat_show() {
    let flake = fetch_github_flake("edolstra", "flake-compat", FLAKE_COMPAT_REV)
        .expect("failed to fetch flake-compat");
    compare_flake_show(flake.path(), false);
}

// =============================================================================
// treefmt-nix - Has lib and templates
// =============================================================================

const TREEFMT_NIX_REV: &str = "d56486eb9493ad9c4777c65932618e9c2d0468fc";

#[test]
fn treefmt_nix_show() {
    let flake =
        fetch_github_flake("numtide", "treefmt-nix", TREEFMT_NIX_REV).expect("failed to fetch treefmt-nix");
    compare_flake_show(flake.path(), false);
}

// =============================================================================
// impermanence - Has nixosModules and homeModules
// =============================================================================

const IMPERMANENCE_REV: &str = "4b3e914cdf97a5b536a889e939fb2fd2b043a170";

#[test]
fn impermanence_show() {
    let flake = fetch_github_flake("nix-community", "impermanence", IMPERMANENCE_REV)
        .expect("failed to fetch impermanence");
    compare_flake_show(flake.path(), false);
}

// =============================================================================
// alejandra - Has packages
// =============================================================================

const ALEJANDRA_REV: &str = "c68bef57c1db3add865493d9cb741a14618bdc28";

#[test]
fn alejandra_show() {
    let flake = fetch_github_flake("kamadorueda", "alejandra", ALEJANDRA_REV)
        .expect("failed to fetch alejandra");
    compare_flake_show(flake.path(), false);
}

#[test]
fn alejandra_show_all_systems() {
    let flake = fetch_github_flake("kamadorueda", "alejandra", ALEJANDRA_REV)
        .expect("failed to fetch alejandra");
    compare_flake_show(flake.path(), true);
}

// =============================================================================
// nixfmt - Has packages
// =============================================================================

const NIXFMT_REV: &str = "42e43d9fcabadf57fdcefa6da355cd8dcf5b7d36";

#[test]
fn nixfmt_show() {
    let flake =
        fetch_github_flake("NixOS", "nixfmt", NIXFMT_REV).expect("failed to fetch nixfmt");
    compare_flake_show(flake.path(), false);
}

// =============================================================================
// nix-index - Has packages and overlays
// =============================================================================

const NIX_INDEX_REV: &str = "0fc38040a22a08052103d0fbbafd67ac54165f2b";

#[test]
fn nix_index_show() {
    // This flake uses `cargoLock.lockFile = ./Cargo.lock` which requires path coercion.
    // Path coercion now works correctly with our fix to nix-bindings-rust.
    let flake = fetch_github_flake("nix-community", "nix-index", NIX_INDEX_REV)
        .expect("failed to fetch nix-index");
    compare_flake_show(flake.path(), false);
}

// =============================================================================
// disko - Has lib, nixosModules, packages
// =============================================================================

const DISKO_REV: &str = "916506443ecd0d0b4a0f4cf9d40a3c22ce39b378";

#[test]
fn disko_show() {
    let flake =
        fetch_github_flake("nix-community", "disko", DISKO_REV).expect("failed to fetch disko");
    compare_flake_show(flake.path(), false);
}

// =============================================================================
// sops-nix - Has nixosModules, overlays
// =============================================================================

const SOPS_NIX_REV: &str = "61b39c7b657081c2adc91b75dd3ad8a91d6f07a7";

#[test]
#[ignore] // Slow due to nix-darwin dependency
fn sops_nix_show() {
    let flake =
        fetch_github_flake("Mic92", "sops-nix", SOPS_NIX_REV).expect("failed to fetch sops-nix");
    compare_flake_show(flake.path(), false);
}

// =============================================================================
// home-manager - Has packages, lib, nixosModules (slow due to nixpkgs dep)
// =============================================================================

const HOME_MANAGER_REV: &str = "1cfa305fba94468f665de1bd1b62dddf2e0cb012";

#[test]
#[ignore] // Slow due to nixpkgs dependency
fn home_manager_show() {
    let flake = fetch_github_flake("nix-community", "home-manager", HOME_MANAGER_REV)
        .expect("failed to fetch home-manager");
    compare_flake_show(flake.path(), false);
}

// =============================================================================
// NixOS branding - Has packages, lib
// =============================================================================

const NIXOS_BRANDING_REV: &str = "4872f76c52f6c5aec7c234a6ff7dedfbfd0b4fad";

#[test]
fn nixos_branding_show() {
    let flake =
        fetch_github_flake("NixOS", "branding", NIXOS_BRANDING_REV).expect("failed to fetch nixos branding");
    compare_flake_show(flake.path(), false);
}

#[test]
fn nixos_branding_show_all_systems() {
    let flake =
        fetch_github_flake("NixOS", "branding", NIXOS_BRANDING_REV).expect("failed to fetch nixos branding");
    compare_flake_show(flake.path(), true);
}
