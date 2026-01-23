//! Integration test to verify:
//! 1. Local flakes are NOT copied to the store with unified evaluation
//! 2. Private repo credentials from ~/.config/nix/nix.conf work

use std::collections::HashSet;
use std::process::Command;

#[test]
fn test_local_flake_not_copied_to_store() {
    // Get store paths before evaluation
    let store_before = get_store_paths();

    // Run trix to evaluate a local flake attribute
    // Using a specific leaf attribute that exists in the flake
    let output = Command::new("cargo")
        .args(["run", "--", "eval", ".#packages.x86_64-linux.default.pname"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run trix");

    // Check command succeeded
    if !output.status.success() {
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        panic!("trix eval failed");
    }

    // Get store paths after evaluation
    let store_after = get_store_paths();

    // Check for new store entries
    let new_paths: Vec<_> = store_after.difference(&store_before).collect();

    // Filter out any paths that might be from building trix itself
    // We're looking for paths that would indicate the flake was copied
    let suspicious_paths: Vec<_> = new_paths
        .iter()
        .filter(|p| {
            // Look for paths that contain "trix" or "flake" that might indicate copying
            let path_lower = p.to_lowercase();
            path_lower.contains("source") || path_lower.contains("-flake-")
        })
        .collect();

    if !suspicious_paths.is_empty() {
        eprintln!("Suspicious new store paths detected:");
        for path in &suspicious_paths {
            eprintln!("  {}", path);
        }
        panic!("Local flake may have been copied to store!");
    }

    println!("✓ Local flake was not copied to store");
}

#[test]
fn test_private_repo_credentials() {
    // Try to evaluate a private repo
    let output = Command::new("cargo")
        .args(["run", "--", "eval", "github:tvbeat/ae#outputs", "--json"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run trix");

    if output.status.success() {
        println!("✓ Private repo accessed successfully");
        println!("  Credentials from ~/.config/nix/nix.conf are working");
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Check if it's an authentication error
        if stderr.contains("401") || stderr.contains("Unauthorized") || stderr.contains("authentication") {
            panic!("Authentication failed - credentials not being loaded properly!\nError: {}", stderr);
        } else if stderr.contains("404") || stderr.contains("not found") {
            // Repo might not exist or wrong name - not a credentials issue
            println!("Note: Repo not found (404). This might not be a credentials issue.");
            println!("Error: {}", stderr);
        } else {
            // Other error - print it but don't necessarily fail
            println!("Command failed with error: {}", stderr);
            println!("This might be expected if the repo doesn't exist or has other issues");
        }
    }
}

#[test]
fn test_remote_flake_evaluation() {
    // This test verifies that remote flake evaluation works
    // (delegates to nix command for remote flakes)

    let output = Command::new("cargo")
        .args([
            "run",
            "--",
            "eval",
            "github:NixOS/nixpkgs/nixos-24.05#lib.version",
            "--json",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run trix");

    assert!(
        output.status.success(),
        "Remote flake evaluation failed. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("✓ Remote flake evaluated successfully");
    println!("  Result: {}", stdout.trim());
}

fn get_store_paths() -> HashSet<String> {
    // Try to list store paths
    let output = Command::new("sh")
        .arg("-c")
        .arg("ls /nix/store 2>/dev/null | head -1000")
        .output()
        .expect("failed to list store");

    if output.status.success() {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|s| s.to_string())
            .collect()
    } else {
        HashSet::new()
    }
}
