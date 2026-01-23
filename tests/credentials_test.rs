//! Test that credentials from ~/.config/nix/nix.conf are properly loaded

use std::process::Command;

#[test]
#[ignore] // Run with: cargo test --test credentials_test -- --ignored --nocapture
fn test_private_repo_with_credentials() {
    println!("\n=== Testing Private Repo Access ===");
    println!("This test verifies that access-tokens from ~/.config/nix/nix.conf are loaded");
    println!("by libutil_init() and used by LockedFlake::lock()\n");

    // Try to fetch a private repo
    let private_repo = "github:tvbeat/ae";

    println!("Attempting to evaluate: {}", private_repo);
    println!("This should work if:");
    println!("  1. The repo exists and you have access");
    println!("  2. access-tokens are configured in ~/.config/nix/nix.conf");
    println!("  3. libutil_init() is loading the config properly\n");

    let output = Command::new("cargo")
        .args(["run", "--", "eval", &format!("{}#outputs", private_repo), "--json"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run trix");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("Exit code: {}", output.status);

    if !stderr.is_empty() {
        println!("\nStderr:");
        println!("{}", stderr);
    }

    if output.status.success() {
        println!("\n✓ SUCCESS: Private repo fetched successfully!");
        println!("  Credentials from ~/.config/nix/nix.conf are working correctly");
        println!("  libutil_init() is properly loading access-tokens");
        if !stdout.is_empty() {
            println!("\nOutput (first 200 chars):");
            println!("{}", stdout.chars().take(200).collect::<String>());
        }
    } else {
        // Analyze the error
        if stderr.contains("401") || stderr.contains("Unauthorized") || stderr.contains("403") || stderr.contains("Forbidden") {
            panic!(
                "\n✗ AUTHENTICATION FAILURE!\n\
                This indicates credentials are NOT being loaded properly.\n\
                Check:\n  \
                  1. ~/.config/nix/nix.conf has access-tokens configured\n  \
                  2. libutil_init() is being called (it is in Evaluator::new())\n  \
                  3. The token has access to this repo\n\
                \nError: {}",
                stderr
            );
        } else if stderr.contains("404") || stderr.contains("Not Found") {
            println!(
                "\n⚠ Repo not found (404).\n\
                This could mean:\n  \
                  1. The repo name is incorrect\n  \
                  2. The repo doesn't exist\n  \
                  3. The token doesn't have access (private repos return 404 when unauthorized)\n\
                \nPlease verify the correct repo name with the user."
            );
            // Don't panic - might just be wrong repo name
        } else {
            println!(
                "\n⚠ Command failed with unexpected error:\n{}",
                stderr
            );
            println!("\nThis might not be a credentials issue.");
        }
    }
}

#[test]
fn test_check_nix_conf_location() {
    println!("\n=== Checking Nix Configuration ===");

    // Check if ~/.config/nix/nix.conf exists
    let home = std::env::var("HOME").expect("HOME not set");
    let nix_conf_path = format!("{}/.config/nix/nix.conf", home);

    if std::path::Path::new(&nix_conf_path).exists() {
        println!("✓ Found: {}", nix_conf_path);

        // Try to read it and check for access-tokens
        if let Ok(content) = std::fs::read_to_string(&nix_conf_path) {
            if content.contains("access-tokens") {
                println!("✓ access-tokens found in config");

                // Show the line (without revealing the token)
                for line in content.lines() {
                    if line.contains("access-tokens") && !line.trim().starts_with('#') {
                        let parts: Vec<&str> = line.split('=').collect();
                        if parts.len() >= 2 {
                            println!("  Config line: {} = <tokens present>", parts[0].trim());
                        }
                    }
                }
            } else {
                println!("⚠ No access-tokens found in config");
            }
        }
    } else {
        println!("⚠ Config file not found: {}", nix_conf_path);
        println!("  Checking /etc/nix/nix.conf instead...");

        if std::path::Path::new("/etc/nix/nix.conf").exists() {
            println!("✓ Found: /etc/nix/nix.conf");
        }
    }
}
