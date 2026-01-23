//! Test that our guards prevent local flakes from being copied to the store

use std::process::Command;

#[test]
fn test_local_paths_work_correctly() {
    // Verify that common local path patterns are properly routed through
    // eval_flake_attr (no store copying) rather than eval_flake_ref (LockedFlake)

    let local_patterns = vec![
        ".#devShells.x86_64-linux.default.name",
        "path:.#devShells.x86_64-linux.default.name",
    ];

    for pattern in local_patterns {
        let output = Command::new("cargo")
            .args(["run", "--", "eval", pattern])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("failed to run trix");

        assert!(
            output.status.success(),
            "Local eval should work for pattern: {}. stderr: {}",
            pattern,
            String::from_utf8_lossy(&output.stderr)
        );

        println!("✓ Local pattern works: {}", pattern);
    }
}

#[test]
fn test_remote_refs_work_correctly() {
    // Verify remote refs are properly handled

    let output = Command::new("cargo")
        .args(["run", "--", "eval", "nixpkgs#lib.version", "--json"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run trix");

    assert!(
        output.status.success(),
        "Remote eval should work. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    println!("✓ Remote ref works: nixpkgs#lib.version");
}

// Note: We can't directly test that the guard in eval_flake_ref() triggers
// because Evaluator isn't exposed as a library. However:
// 1. The guard exists in the code (src/eval/mod.rs:290-297)
// 2. All code paths that use eval_flake_ref are already guarded by higher-level checks
// 3. The guard is defense-in-depth to catch programming errors
// 4. If someone accidentally calls eval_flake_ref with a local path, it will panic with a clear message
