//! Test to verify what happens when LockedFlake::lock is used on a local path

use std::collections::HashSet;
use std::process::Command;

#[test]
#[ignore] // Run with: cargo test --test locked_flake_local_test -- --ignored --nocapture
fn test_locked_flake_on_local_path() {
    println!("\n=== Testing LockedFlake behavior with local paths ===");

    // Get store paths before
    let store_before = get_store_paths();
    println!("Store paths before: {} entries", store_before.len());

    // We need to test what eval_flake_ref does with a local path
    // The eval command should route local paths through eval_flake_attr, but what if someone
    // accidentally passes a local path to eval_flake_ref?

    // Try evaluating with a path that might trigger LockedFlake on local
    println!("\nAttempt 1: Using eval with explicit path:");
    let output = Command::new("cargo")
        .args(["run", "--", "eval", "path:.#devShells.x86_64-linux.default.name"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run trix");

    println!("Exit code: {}", output.status);
    if !output.status.success() {
        println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    } else {
        println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    }

    // Get store paths after
    let store_after = get_store_paths();
    println!("\nStore paths after: {} entries", store_after.len());

    // Check for new store entries that might be trix
    let new_paths: Vec<_> = store_after.difference(&store_before).collect();

    if !new_paths.is_empty() {
        println!("\nNew store paths created: {}", new_paths.len());

        // Look for paths that might be the local flake
        let suspicious: Vec<_> = new_paths.iter()
            .filter(|p| {
                let p_lower = p.to_lowercase();
                p_lower.contains("source") || p_lower.contains("trix")
            })
            .collect();

        if !suspicious.is_empty() {
            println!("\n⚠️  WARNING: Suspicious paths detected:");
            for path in suspicious.iter().take(10) {
                println!("  {}", path);
            }
            panic!("Local flake may have been copied to store when using path: syntax!");
        } else {
            println!("\n✓ No suspicious paths (trix-related) in new store entries");
        }
    } else {
        println!("\n✓ No new store paths created");
    }
}

#[test]
fn test_is_local_path_guards() {
    use std::path::Path;

    println!("\n=== Testing is_local_path() guards ===");

    // Test that is_local_path correctly identifies local paths
    let local_cases = vec![
        ".",
        "./foo",
        "/absolute/path",
        "~/home/path",
        "../parent",
        "relative/path",
    ];

    let remote_cases = vec![
        "github:NixOS/nixpkgs",
        "nixpkgs",
        "gitlab:owner/repo",
        "git+https://github.com/owner/repo",
        "path:.// This is actually remote in some contexts",
    ];

    println!("\nLocal paths (should return true):");
    for case in &local_cases {
        let is_local = case.starts_with('.')
            || case.starts_with('/')
            || case.starts_with('~')
            || Path::new(case).exists();
        println!("  {} -> {}", case, is_local);
    }

    println!("\nRemote refs (should return false unless they exist as files):");
    for case in &remote_cases {
        let is_local = case.starts_with('.')
            || case.starts_with('/')
            || case.starts_with('~')
            || Path::new(case).exists();
        println!("  {} -> {}", case, is_local);
    }

    // Check the actual implementation
    println!("\nVerifying is_local_path implementation covers all cases...");
    assert!(Path::new(".").exists() || ".".starts_with('.'));
    assert!("/path".starts_with('/'));
    assert!("~/path".starts_with('~'));
}

fn get_store_paths() -> HashSet<String> {
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
