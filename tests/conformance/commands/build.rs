//! Conformance tests for `trix build` vs `nix build`.
//!
//! Note: Build outputs (store paths) will differ between trix and nix
//! because trix doesn't copy local flakes to the store. These tests focus
//! on success/failure matching and other behavioral aspects.

use crate::conformance::fixtures::*;
use crate::conformance::harness::{ComparisonStrategy, ConformanceHarness};

fn harness() -> ConformanceHarness {
    ConformanceHarness::new()
}

// =============================================================================
// Basic build tests (success/failure matching)
// =============================================================================

#[test]
fn build_simple_package() {
    let h = harness();
    let fixture = SIMPLE_PACKAGE.setup().expect("failed to setup fixture");
    let attr = format!(
        "{}#packages.x86_64-linux.default",
        fixture.path().display()
    );

    // Both should succeed (output paths will differ, that's by design)
    h.assert_conformance(
        &["build", "--no-link", &attr],
        &ComparisonStrategy::SuccessMatch,
    );
}

#[test]
fn build_nonexistent_package() {
    let h = harness();
    let fixture = SIMPLE_PACKAGE.setup().expect("failed to setup fixture");
    let attr = format!("{}#packages.x86_64-linux.nonexistent", fixture.path().display());

    // Both should fail
    h.assert_conformance(
        &["build", "--no-link", &attr],
        &ComparisonStrategy::SuccessMatch,
    );
}

#[test]
fn build_nonexistent_system() {
    let h = harness();
    let fixture = SIMPLE_PACKAGE.setup().expect("failed to setup fixture");
    let attr = format!(
        "{}#packages.aarch64-darwin.default",
        fixture.path().display()
    );

    // Both should fail (package only defined for x86_64-linux)
    h.assert_conformance(
        &["build", "--no-link", &attr],
        &ComparisonStrategy::SuccessMatch,
    );
}

#[test]
fn build_lib_not_derivation() {
    let h = harness();
    let fixture = LIB_ONLY.setup().expect("failed to setup fixture");
    let attr = format!("{}#lib", fixture.path().display());

    // Both should fail (lib is not a derivation)
    h.assert_conformance(
        &["build", "--no-link", &attr],
        &ComparisonStrategy::SuccessMatch,
    );
}

// =============================================================================
// Build with default attribute resolution
// =============================================================================

#[test]
fn build_default_package() {
    let h = harness();
    let fixture = SIMPLE_PACKAGE.setup().expect("failed to setup fixture");

    // Building just the flake path should resolve to packages.<system>.default
    h.assert_conformance(
        &["build", "--no-link", fixture.path().to_str().unwrap()],
        &ComparisonStrategy::SuccessMatch,
    );
}

#[test]
fn build_empty_outputs_default() {
    let h = harness();
    let fixture = EMPTY_OUTPUTS.setup().expect("failed to setup fixture");

    // Should fail - no default package
    h.assert_conformance(
        &["build", "--no-link", fixture.path().to_str().unwrap()],
        &ComparisonStrategy::SuccessMatch,
    );
}

// =============================================================================
// Build output path tests
// =============================================================================

#[test]
fn build_prints_store_path() {
    let h = harness();
    let fixture = SIMPLE_PACKAGE.setup().expect("failed to setup fixture");
    let attr = format!(
        "{}#packages.x86_64-linux.default",
        fixture.path().display()
    );

    // Both should print a store path (paths will differ, but format should match)
    let (trix, nix, _) = h.test_conformance(
        &["build", "--no-link", "--print-out-paths", &attr],
        &ComparisonStrategy::SuccessMatch,
    );

    // Verify both outputs look like store paths
    if trix.success() {
        assert!(
            trix.stdout.trim().starts_with("/nix/store/"),
            "trix output should be a store path: {}",
            trix.stdout
        );
    }
    if nix.success() {
        assert!(
            nix.stdout.trim().starts_with("/nix/store/"),
            "nix output should be a store path: {}",
            nix.stdout
        );
    }
}

// =============================================================================
// Tests against local trix flake
// =============================================================================

#[test]
fn build_local_default() {
    let h = harness();

    // Build trix's own default package
    h.assert_conformance(
        &["build", "--no-link", ".#default"],
        &ComparisonStrategy::SuccessMatch,
    );
}
