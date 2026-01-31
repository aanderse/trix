//! Conformance tests for `trix flake metadata` vs `nix flake metadata`.

use crate::conformance::fixtures::*;
use crate::conformance::harness::{ComparisonStrategy, ConformanceHarness};
use std::process::Command;

fn harness() -> ConformanceHarness {
    ConformanceHarness::new()
}

/// Helper to lock a flake before testing metadata
fn lock_flake(dir: &std::path::Path) {
    // Use nix to lock so we have a consistent baseline
    let output = Command::new("nix")
        .args(["flake", "lock", dir.to_str().unwrap()])
        .output()
        .expect("failed to run nix flake lock");

    if !output.status.success() {
        panic!(
            "nix flake lock failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

// =============================================================================
// Basic flake metadata --json tests
// =============================================================================

/// Note: This test documents known incompatibilities in trix's flake metadata output.
/// The following fields are missing or different in trix:
/// - fingerprint: not implemented (complex hash calculation)
/// - narHash: not implemented (requires computing NAR hash)
/// - revCount: not implemented
/// - ref: git ref not tracked
/// These are tracked as known issues.
#[test]
#[ignore] // Known incompatibility - see comment above
fn flake_metadata_json_simple() {
    let h = harness();
    let fixture = SIMPLE_PACKAGE.setup().expect("failed to setup fixture");

    h.assert_conformance(
        &[
            "flake",
            "metadata",
            "--json",
            fixture.path().to_str().unwrap(),
        ],
        &ComparisonStrategy::JsonSemantic {
            // Path will differ because trix doesn't copy to store
            // fingerprint/narHash/revCount/ref not implemented
            ignore_fields: vec!["path", "resolvedUrl", "url", "fingerprint", "narHash", "revCount", "ref", "shortRev"],
        },
    );
}

#[test]
#[ignore] // Known incompatibility - trix missing fingerprint, narHash, revCount, ref
fn flake_metadata_json_lib_only() {
    let h = harness();
    let fixture = LIB_ONLY.setup().expect("failed to setup fixture");

    h.assert_conformance(
        &[
            "flake",
            "metadata",
            "--json",
            fixture.path().to_str().unwrap(),
        ],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec!["path", "resolvedUrl", "url", "fingerprint", "narHash", "revCount", "ref", "shortRev"],
        },
    );
}

#[test]
#[ignore] // Known incompatibility - trix missing fingerprint, narHash, revCount, ref
fn flake_metadata_json_empty() {
    let h = harness();
    let fixture = EMPTY_OUTPUTS.setup().expect("failed to setup fixture");

    h.assert_conformance(
        &[
            "flake",
            "metadata",
            "--json",
            fixture.path().to_str().unwrap(),
        ],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec!["path", "resolvedUrl", "url", "fingerprint", "narHash", "revCount", "ref", "shortRev"],
        },
    );
}

// =============================================================================
// Flakes with inputs (require locking first)
// =============================================================================

#[test]
#[ignore] // Requires network for locking, run with --ignored
fn flake_metadata_json_with_nixpkgs() {
    let h = harness();
    let fixture = WITH_NIXPKGS.setup().expect("failed to setup fixture");

    // Lock the flake first
    lock_flake(fixture.path());

    h.assert_conformance(
        &[
            "flake",
            "metadata",
            "--json",
            fixture.path().to_str().unwrap(),
        ],
        &ComparisonStrategy::JsonSemantic {
            // Ignore path-related fields that differ by design
            // fingerprint/narHash/revCount/ref not implemented
            ignore_fields: vec!["path", "resolvedUrl", "url", "lastModified", "fingerprint", "narHash", "revCount", "ref", "shortRev"],
        },
    );
}

#[test]
#[ignore] // Requires network for locking, run with --ignored
fn flake_metadata_json_with_follows() {
    let h = harness();
    let fixture = WITH_FOLLOWS.setup().expect("failed to setup fixture");

    // Lock the flake first
    lock_flake(fixture.path());

    h.assert_conformance(
        &[
            "flake",
            "metadata",
            "--json",
            fixture.path().to_str().unwrap(),
        ],
        &ComparisonStrategy::JsonSemantic {
            // fingerprint/narHash/revCount/ref not implemented
            ignore_fields: vec!["path", "resolvedUrl", "url", "lastModified", "fingerprint", "narHash", "revCount", "ref", "shortRev"],
        },
    );
}

// =============================================================================
// Tests against local trix flake
// =============================================================================

#[test]
#[ignore] // Known incompatibility - trix missing fingerprint, narHash, revCount, ref
fn flake_metadata_local() {
    let h = harness();

    h.assert_conformance(
        &["flake", "metadata", "--json", "."],
        &ComparisonStrategy::JsonSemantic {
            // fingerprint/narHash/revCount/ref not implemented
            ignore_fields: vec!["path", "resolvedUrl", "url", "fingerprint", "narHash", "revCount", "ref", "shortRev"],
        },
    );
}

// =============================================================================
// Text output tests
// =============================================================================

#[test]
#[ignore] // Known incompatibility - text format differs significantly (trix shows minimal output)
fn flake_metadata_text_simple() {
    let h = harness();
    let fixture = SIMPLE_PACKAGE.setup().expect("failed to setup fixture");

    // Text output format differs significantly between trix and nix
    // trix shows minimal output (Path, Last modified) while nix shows full details
    // Just verify both succeed for now
    h.assert_conformance(
        &["flake", "metadata", fixture.path().to_str().unwrap()],
        &ComparisonStrategy::SuccessMatch,
    );
}
