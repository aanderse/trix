//! Integration tests for the lock command.
//!
//! These tests verify that `trix flake lock` produces the same flake.lock
//! as `nix flake lock` would.

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// Helper to create a flake.nix with given content
fn create_flake(dir: &Path, content: &str) {
    fs::write(dir.join("flake.nix"), content).expect("failed to write flake.nix");
}

/// Run trix flake lock on a directory
fn run_trix_lock(dir: &Path) -> std::io::Result<std::process::Output> {
    Command::new(env!("CARGO_BIN_EXE_trix"))
        .args(["flake", "lock", dir.to_str().unwrap()])
        .output()
}

/// Run nix flake lock on a directory
fn run_nix_lock(dir: &Path) -> std::io::Result<std::process::Output> {
    Command::new("nix")
        .args(["flake", "lock", dir.to_str().unwrap()])
        .output()
}

/// Parse a flake.lock file and return normalized JSON for comparison
fn parse_lock_file(dir: &Path) -> serde_json::Value {
    let content = fs::read_to_string(dir.join("flake.lock")).expect("failed to read flake.lock");
    serde_json::from_str(&content).expect("failed to parse flake.lock")
}

/// Compare two lock files, ignoring fields that may legitimately differ
fn compare_locks(trix_lock: &serde_json::Value, nix_lock: &serde_json::Value) -> Result<(), String> {
    // Compare structure
    if trix_lock["version"] != nix_lock["version"] {
        return Err(format!(
            "version mismatch: trix={}, nix={}",
            trix_lock["version"], nix_lock["version"]
        ));
    }

    if trix_lock["root"] != nix_lock["root"] {
        return Err(format!(
            "root mismatch: trix={}, nix={}",
            trix_lock["root"], nix_lock["root"]
        ));
    }

    // Compare nodes
    let trix_nodes = trix_lock["nodes"].as_object().ok_or("trix nodes not object")?;
    let nix_nodes = nix_lock["nodes"].as_object().ok_or("nix nodes not object")?;

    // Check all nodes exist in both
    for name in trix_nodes.keys() {
        if !nix_nodes.contains_key(name) {
            return Err(format!("node '{}' in trix but not in nix", name));
        }
    }
    for name in nix_nodes.keys() {
        if !trix_nodes.contains_key(name) {
            return Err(format!("node '{}' in nix but not in trix", name));
        }
    }

    // Compare each node
    for (name, trix_node) in trix_nodes {
        let nix_node = &nix_nodes[name];

        // Compare inputs (for root node)
        if let Some(trix_inputs) = trix_node.get("inputs") {
            let nix_inputs = nix_node.get("inputs").ok_or_else(|| {
                format!("node '{}' has inputs in trix but not nix", name)
            })?;
            if trix_inputs != nix_inputs {
                return Err(format!(
                    "inputs mismatch for '{}': trix={}, nix={}",
                    name, trix_inputs, nix_inputs
                ));
            }
        }

        // Compare locked info (for input nodes)
        if let Some(trix_locked) = trix_node.get("locked") {
            let nix_locked = nix_node.get("locked").ok_or_else(|| {
                format!("node '{}' has locked in trix but not nix", name)
            })?;

            // Compare key fields
            compare_locked_fields(name, trix_locked, nix_locked)?;
        }

        // Compare original info
        if let Some(trix_original) = trix_node.get("original") {
            let nix_original = nix_node.get("original").ok_or_else(|| {
                format!("node '{}' has original in trix but not nix", name)
            })?;

            if trix_original != nix_original {
                return Err(format!(
                    "original mismatch for '{}': trix={}, nix={}",
                    name, trix_original, nix_original
                ));
            }
        }

        // Compare flake field
        if trix_node.get("flake") != nix_node.get("flake") {
            return Err(format!(
                "flake field mismatch for '{}': trix={:?}, nix={:?}",
                name,
                trix_node.get("flake"),
                nix_node.get("flake")
            ));
        }
    }

    Ok(())
}

/// Compare locked fields, allowing for timing differences
fn compare_locked_fields(
    name: &str,
    trix: &serde_json::Value,
    nix: &serde_json::Value,
) -> Result<(), String> {
    // Fields that must match exactly
    let exact_fields = ["type", "owner", "repo", "rev", "narHash"];

    for field in exact_fields {
        if trix.get(field) != nix.get(field) {
            return Err(format!(
                "locked.{} mismatch for '{}': trix={:?}, nix={:?}",
                field,
                name,
                trix.get(field),
                nix.get(field)
            ));
        }
    }

    // lastModified can differ slightly due to timing, but should be close
    // For now, just check it exists in both if present in either
    if trix.get("lastModified").is_some() != nix.get("lastModified").is_some() {
        return Err(format!(
            "lastModified presence mismatch for '{}': trix={:?}, nix={:?}",
            name,
            trix.get("lastModified"),
            nix.get("lastModified")
        ));
    }

    Ok(())
}

/// Test: Single github input
#[test]
#[ignore] // Requires network access
fn lock_single_github_input() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
  outputs = { self, nixpkgs }: {};
}"#,
    );

    // Run trix lock
    let trix_output = run_trix_lock(tmp.path()).expect("failed to run trix lock");
    assert!(
        trix_output.status.success(),
        "trix lock failed: {}",
        String::from_utf8_lossy(&trix_output.stderr)
    );

    let trix_lock = parse_lock_file(tmp.path());

    // Remove lock and run nix lock
    fs::remove_file(tmp.path().join("flake.lock")).ok();

    let nix_output = run_nix_lock(tmp.path()).expect("failed to run nix lock");
    assert!(
        nix_output.status.success(),
        "nix lock failed: {}",
        String::from_utf8_lossy(&nix_output.stderr)
    );

    let nix_lock = parse_lock_file(tmp.path());

    // Compare
    compare_locks(&trix_lock, &nix_lock).expect("lock files differ");
}

/// Test: Multiple github inputs (using nixpkgs branches to avoid transitive deps)
#[test]
#[ignore] // Requires network access
fn lock_multiple_github_inputs() {
    let tmp = TempDir::new().unwrap();

    // Use two nixpkgs branches - they have no transitive inputs
    create_flake(
        tmp.path(),
        r#"{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
    nixpkgs-unstable.url = "github:NixOS/nixpkgs/nixos-unstable";
  };
  outputs = { self, nixpkgs, nixpkgs-unstable }: {};
}"#,
    );

    let trix_output = run_trix_lock(tmp.path()).expect("failed to run trix lock");
    assert!(
        trix_output.status.success(),
        "trix lock failed: {}",
        String::from_utf8_lossy(&trix_output.stderr)
    );

    let trix_lock = parse_lock_file(tmp.path());

    fs::remove_file(tmp.path().join("flake.lock")).ok();

    let nix_output = run_nix_lock(tmp.path()).expect("failed to run nix lock");
    assert!(
        nix_output.status.success(),
        "nix lock failed: {}",
        String::from_utf8_lossy(&nix_output.stderr)
    );

    let nix_lock = parse_lock_file(tmp.path());

    compare_locks(&trix_lock, &nix_lock).expect("lock files differ");
}

/// Test: Input with follows
#[test]
#[ignore] // Requires network access
fn lock_with_follows() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
    home-manager = {
      url = "github:nix-community/home-manager/release-24.05";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs = { self, nixpkgs, home-manager }: {};
}"#,
    );

    let trix_output = run_trix_lock(tmp.path()).expect("failed to run trix lock");
    assert!(
        trix_output.status.success(),
        "trix lock failed: {}",
        String::from_utf8_lossy(&trix_output.stderr)
    );

    let trix_lock = parse_lock_file(tmp.path());

    fs::remove_file(tmp.path().join("flake.lock")).ok();

    let nix_output = run_nix_lock(tmp.path()).expect("failed to run nix lock");
    assert!(
        nix_output.status.success(),
        "nix lock failed: {}",
        String::from_utf8_lossy(&nix_output.stderr)
    );

    let nix_lock = parse_lock_file(tmp.path());

    compare_locks(&trix_lock, &nix_lock).expect("lock files differ");
}

/// Test: Root-level follows
#[test]
#[ignore] // Requires network access
fn lock_root_follows() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
    nixpkgs-stable.follows = "nixpkgs";
  };
  outputs = { self, nixpkgs, nixpkgs-stable }: {};
}"#,
    );

    let trix_output = run_trix_lock(tmp.path()).expect("failed to run trix lock");
    assert!(
        trix_output.status.success(),
        "trix lock failed: {}",
        String::from_utf8_lossy(&trix_output.stderr)
    );

    let trix_lock = parse_lock_file(tmp.path());

    fs::remove_file(tmp.path().join("flake.lock")).ok();

    let nix_output = run_nix_lock(tmp.path()).expect("failed to run nix lock");
    assert!(
        nix_output.status.success(),
        "nix lock failed: {}",
        String::from_utf8_lossy(&nix_output.stderr)
    );

    let nix_lock = parse_lock_file(tmp.path());

    compare_locks(&trix_lock, &nix_lock).expect("lock files differ");
}

/// Test: Non-flake input
#[test]
#[ignore] // Requires network access
fn lock_non_flake_input() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
    nix-colors = {
      url = "github:Misterio77/nix-colors";
      flake = false;
    };
  };
  outputs = { self, nixpkgs, nix-colors }: {};
}"#,
    );

    let trix_output = run_trix_lock(tmp.path()).expect("failed to run trix lock");
    assert!(
        trix_output.status.success(),
        "trix lock failed: {}",
        String::from_utf8_lossy(&trix_output.stderr)
    );

    let trix_lock = parse_lock_file(tmp.path());

    fs::remove_file(tmp.path().join("flake.lock")).ok();

    let nix_output = run_nix_lock(tmp.path()).expect("failed to run nix lock");
    assert!(
        nix_output.status.success(),
        "nix lock failed: {}",
        String::from_utf8_lossy(&nix_output.stderr)
    );

    let nix_lock = parse_lock_file(tmp.path());

    compare_locks(&trix_lock, &nix_lock).expect("lock files differ");
}

/// Test: GitLab input (as non-flake since many GitLab repos aren't flakes)
#[test]
#[ignore] // Requires network access
fn lock_gitlab_input() {
    let tmp = TempDir::new().unwrap();

    // Use nur-expressions as a non-flake input since it doesn't have flake.nix
    create_flake(
        tmp.path(),
        r#"{
  inputs.nur-expressions = {
    url = "gitlab:rycee/nur-expressions";
    flake = false;
  };
  outputs = { self, nur-expressions }: {};
}"#,
    );

    let trix_output = run_trix_lock(tmp.path()).expect("failed to run trix lock");
    assert!(
        trix_output.status.success(),
        "trix lock failed: {}",
        String::from_utf8_lossy(&trix_output.stderr)
    );

    let trix_lock = parse_lock_file(tmp.path());

    fs::remove_file(tmp.path().join("flake.lock")).ok();

    let nix_output = run_nix_lock(tmp.path()).expect("failed to run nix lock");
    assert!(
        nix_output.status.success(),
        "nix lock failed: {}",
        String::from_utf8_lossy(&nix_output.stderr)
    );

    let nix_lock = parse_lock_file(tmp.path());

    compare_locks(&trix_lock, &nix_lock).expect("lock files differ");
}

/// Test: Verify lock file structure
#[test]
#[ignore] // Requires network access
fn lock_structure_valid() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
  outputs = { self, nixpkgs }: {};
}"#,
    );

    run_trix_lock(tmp.path()).expect("failed to run trix lock");

    let lock = parse_lock_file(tmp.path());

    // Check required fields
    assert!(lock.get("version").is_some(), "missing version");
    assert!(lock.get("root").is_some(), "missing root");
    assert!(lock.get("nodes").is_some(), "missing nodes");

    // Check version is 7
    assert_eq!(lock["version"], 7, "version should be 7");

    // Check root points to root node
    assert_eq!(lock["root"], "root", "root should be 'root'");

    // Check root node exists
    let nodes = lock["nodes"].as_object().expect("nodes should be object");
    assert!(nodes.contains_key("root"), "missing root node");

    // Check nixpkgs node exists
    assert!(nodes.contains_key("nixpkgs"), "missing nixpkgs node");

    // Check nixpkgs node has required fields
    let nixpkgs = &nodes["nixpkgs"];
    assert!(nixpkgs.get("locked").is_some(), "nixpkgs missing locked");
    assert!(nixpkgs.get("original").is_some(), "nixpkgs missing original");

    // Check locked has required fields
    let locked = &nixpkgs["locked"];
    assert!(locked.get("type").is_some(), "locked missing type");
    assert!(locked.get("owner").is_some(), "locked missing owner");
    assert!(locked.get("repo").is_some(), "locked missing repo");
    assert!(locked.get("rev").is_some(), "locked missing rev");
    assert!(locked.get("narHash").is_some(), "locked missing narHash");
}

/// Test: Updating an existing lock preserves unchanged inputs
#[test]
#[ignore] // Requires network access
fn lock_preserves_unchanged_inputs() {
    let tmp = TempDir::new().unwrap();

    // Create initial flake with one input
    create_flake(
        tmp.path(),
        r#"{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
  outputs = { self, nixpkgs }: {};
}"#,
    );

    run_trix_lock(tmp.path()).expect("failed to run initial trix lock");

    let first_lock = parse_lock_file(tmp.path());
    let first_nixpkgs_rev = first_lock["nodes"]["nixpkgs"]["locked"]["rev"].clone();

    // Run lock again without changes
    run_trix_lock(tmp.path()).expect("failed to run second trix lock");

    let second_lock = parse_lock_file(tmp.path());
    let second_nixpkgs_rev = second_lock["nodes"]["nixpkgs"]["locked"]["rev"].clone();

    // Rev should be the same (input wasn't updated)
    assert_eq!(
        first_nixpkgs_rev, second_nixpkgs_rev,
        "nixpkgs rev should be preserved on re-lock"
    );
}

/// Test: Empty inputs
#[test]
fn lock_empty_inputs() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  inputs = {};
  outputs = { self }: {};
}"#,
    );

    let output = run_trix_lock(tmp.path()).expect("failed to run trix lock");
    assert!(
        output.status.success(),
        "trix lock failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // No lock file should be created (or empty one)
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no inputs") || !tmp.path().join("flake.lock").exists(),
        "should report no inputs or not create lock file"
    );
}

/// Test: Flake with no inputs attribute
#[test]
fn lock_no_inputs_attribute() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  outputs = { self }: {
    hello = "world";
  };
}"#,
    );

    let output = run_trix_lock(tmp.path()).expect("failed to run trix lock");
    assert!(
        output.status.success(),
        "trix lock failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Test: Inputs with transitive dependencies
/// When flake-utils depends on systems, both should be locked.
#[test]
#[ignore] // Requires network access
fn lock_transitive_inputs() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
    flake-utils.url = "github:numtide/flake-utils";
  };
  outputs = { self, nixpkgs, flake-utils }: {};
}"#,
    );

    let trix_output = run_trix_lock(tmp.path()).expect("failed to run trix lock");
    assert!(
        trix_output.status.success(),
        "trix lock failed: {}",
        String::from_utf8_lossy(&trix_output.stderr)
    );

    let trix_lock = parse_lock_file(tmp.path());

    fs::remove_file(tmp.path().join("flake.lock")).ok();

    let nix_output = run_nix_lock(tmp.path()).expect("failed to run nix lock");
    assert!(
        nix_output.status.success(),
        "nix lock failed: {}",
        String::from_utf8_lossy(&nix_output.stderr)
    );

    let nix_lock = parse_lock_file(tmp.path());

    compare_locks(&trix_lock, &nix_lock).expect("lock files differ");
}

/// Test: Dry run doesn't write lock file
#[test]
#[ignore] // Requires network access
fn lock_dry_run() {
    let tmp = TempDir::new().unwrap();

    create_flake(
        tmp.path(),
        r#"{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
  outputs = { self, nixpkgs }: {};
}"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_trix"))
        .args(["flake", "lock", "--dry-run", tmp.path().to_str().unwrap()])
        .output()
        .expect("failed to run trix flake lock");

    assert!(
        output.status.success(),
        "trix flake lock --dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Lock file should not exist
    assert!(
        !tmp.path().join("flake.lock").exists(),
        "lock file should not be created with --dry-run"
    );
}
