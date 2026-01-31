//! Conformance tests for `trix flake show` vs `nix flake show`.

use crate::conformance::fixtures::*;
use crate::conformance::harness::{ComparisonStrategy, ConformanceHarness};

fn harness() -> ConformanceHarness {
    ConformanceHarness::new()
}

// =============================================================================
// Basic flake show --json tests
// =============================================================================

#[test]
fn flake_show_json_simple_package() {
    let h = harness();
    let fixture = SIMPLE_PACKAGE.setup().expect("failed to setup fixture");

    h.assert_conformance(
        &["flake", "show", "--json", fixture.path().to_str().unwrap()],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn flake_show_json_lib_only() {
    let h = harness();
    let fixture = LIB_ONLY.setup().expect("failed to setup fixture");

    h.assert_conformance(
        &["flake", "show", "--json", fixture.path().to_str().unwrap()],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn flake_show_json_empty_outputs() {
    let h = harness();
    let fixture = EMPTY_OUTPUTS.setup().expect("failed to setup fixture");

    h.assert_conformance(
        &["flake", "show", "--json", fixture.path().to_str().unwrap()],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn flake_show_json_multi_output() {
    let h = harness();
    let fixture = MULTI_OUTPUT.setup().expect("failed to setup fixture");

    h.assert_conformance(
        &["flake", "show", "--json", fixture.path().to_str().unwrap()],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn flake_show_json_all_types() {
    let h = harness();
    let fixture = ALL_TYPES.setup().expect("failed to setup fixture");

    h.assert_conformance(
        &["flake", "show", "--json", fixture.path().to_str().unwrap()],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn flake_show_json_deep_nesting() {
    let h = harness();
    let fixture = DEEP_NESTING.setup().expect("failed to setup fixture");

    h.assert_conformance(
        &["flake", "show", "--json", fixture.path().to_str().unwrap()],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn flake_show_json_special_chars() {
    let h = harness();
    let fixture = SPECIAL_CHARS.setup().expect("failed to setup fixture");

    h.assert_conformance(
        &["flake", "show", "--json", fixture.path().to_str().unwrap()],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

// =============================================================================
// flake show --all-systems tests
// =============================================================================

#[test]
fn flake_show_json_all_systems_simple() {
    let h = harness();
    let fixture = SIMPLE_PACKAGE.setup().expect("failed to setup fixture");

    h.assert_conformance(
        &[
            "flake",
            "show",
            "--json",
            "--all-systems",
            fixture.path().to_str().unwrap(),
        ],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn flake_show_json_all_systems_multi_output() {
    let h = harness();
    let fixture = MULTI_OUTPUT.setup().expect("failed to setup fixture");

    h.assert_conformance(
        &[
            "flake",
            "show",
            "--json",
            "--all-systems",
            fixture.path().to_str().unwrap(),
        ],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

// =============================================================================
// flake show text output tests
// =============================================================================

#[test]
fn flake_show_text_simple() {
    let h = harness();
    let fixture = SIMPLE_PACKAGE.setup().expect("failed to setup fixture");

    // Text output may have minor differences, use normalized comparison
    // The header line includes ref/rev in nix but not trix, so we ignore the first line
    h.assert_conformance(
        &["flake", "show", fixture.path().to_str().unwrap()],
        &ComparisonStrategy::TextNormalized {
            normalize_store_paths: false,
            // Ignore the first line (URL with ref/rev) and box-drawing char differences
            ignore_patterns: vec![
                r"(?m)^git\+file:.*$",  // First line with URL
                r"\?ref=[^&\s]*",        // ref parameter
                r"&rev=[a-f0-9]+",       // rev parameter
            ],
        },
    );
}

#[test]
fn flake_show_text_multi_output() {
    let h = harness();
    let fixture = MULTI_OUTPUT.setup().expect("failed to setup fixture");

    h.assert_conformance(
        &["flake", "show", fixture.path().to_str().unwrap()],
        &ComparisonStrategy::TextNormalized {
            normalize_store_paths: false,
            ignore_patterns: vec![
                r"(?m)^git\+file:.*$",
                r"\?ref=[^&\s]*",
                r"&rev=[a-f0-9]+",
            ],
        },
    );
}

// =============================================================================
// Tests against local trix flake
// =============================================================================

#[test]
fn flake_show_local_json() {
    let h = harness();

    h.assert_conformance(
        &["flake", "show", "--json", "."],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn flake_show_local_all_systems() {
    let h = harness();

    h.assert_conformance(
        &["flake", "show", "--json", "--all-systems", "."],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

// =============================================================================
// Remote flake tests (pinned commits for reproducibility)
// =============================================================================

const FLAKE_UTILS_REV: &str = "11707dc2f618dd54ca8739b309ec4fc024de578b";
const FLAKE_COMPAT_REV: &str = "5edf11c44bc78a0d334f6334cdaf7d60d732daab";

#[test]
#[ignore] // Requires network access, run with --ignored
fn flake_show_flake_utils() {
    let h = harness();
    let flake = fetch_github_flake("numtide", "flake-utils", FLAKE_UTILS_REV)
        .expect("failed to fetch flake-utils");

    h.assert_conformance(
        &["flake", "show", "--json", flake.path().to_str().unwrap()],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
#[ignore] // Requires network access, run with --ignored
fn flake_show_flake_compat() {
    let h = harness();
    let flake = fetch_github_flake("edolstra", "flake-compat", FLAKE_COMPAT_REV)
        .expect("failed to fetch flake-compat");

    h.assert_conformance(
        &["flake", "show", "--json", flake.path().to_str().unwrap()],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}
