//! Conformance tests for `trix eval` vs `nix eval`.

use crate::conformance::fixtures::*;
use crate::conformance::harness::{ComparisonStrategy, ConformanceHarness};

fn harness() -> ConformanceHarness {
    ConformanceHarness::new()
}

// =============================================================================
// Basic eval --json tests
// =============================================================================

#[test]
fn eval_json_string() {
    let h = harness();
    let fixture = LIB_ONLY.setup().expect("failed to setup fixture");
    let path = format!("{}#lib.hello", fixture.path().display());

    h.assert_conformance(
        &["eval", "--json", &path],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn eval_json_number() {
    let h = harness();
    let fixture = ALL_TYPES.setup().expect("failed to setup fixture");
    let path = format!("{}#lib.anInt", fixture.path().display());

    h.assert_conformance(
        &["eval", "--json", &path],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn eval_json_float() {
    let h = harness();
    let fixture = ALL_TYPES.setup().expect("failed to setup fixture");
    let path = format!("{}#lib.aFloat", fixture.path().display());

    h.assert_conformance(
        &["eval", "--json", &path],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn eval_json_bool() {
    let h = harness();
    let fixture = ALL_TYPES.setup().expect("failed to setup fixture");
    let path = format!("{}#lib.aBool", fixture.path().display());

    h.assert_conformance(
        &["eval", "--json", &path],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn eval_json_null() {
    let h = harness();
    let fixture = ALL_TYPES.setup().expect("failed to setup fixture");
    let path = format!("{}#lib.aNull", fixture.path().display());

    h.assert_conformance(
        &["eval", "--json", &path],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn eval_json_list() {
    let h = harness();
    let fixture = ALL_TYPES.setup().expect("failed to setup fixture");
    let path = format!("{}#lib.aList", fixture.path().display());

    h.assert_conformance(
        &["eval", "--json", &path],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn eval_json_attrset() {
    let h = harness();
    let fixture = ALL_TYPES.setup().expect("failed to setup fixture");
    let path = format!("{}#lib.anAttrSet", fixture.path().display());

    h.assert_conformance(
        &["eval", "--json", &path],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn eval_json_nested() {
    let h = harness();
    let fixture = LIB_ONLY.setup().expect("failed to setup fixture");
    let path = format!("{}#lib.nested", fixture.path().display());

    h.assert_conformance(
        &["eval", "--json", &path],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn eval_json_deep_path() {
    let h = harness();
    let fixture = DEEP_NESTING.setup().expect("failed to setup fixture");
    let path = format!("{}#lib.a.b.c.d.e.value", fixture.path().display());

    h.assert_conformance(
        &["eval", "--json", &path],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

// =============================================================================
// eval --raw tests
// =============================================================================

#[test]
fn eval_raw_string() {
    let h = harness();
    let fixture = LIB_ONLY.setup().expect("failed to setup fixture");
    let path = format!("{}#lib.hello", fixture.path().display());

    h.assert_conformance(&["eval", "--raw", &path], &ComparisonStrategy::Exact);
}

#[test]
fn eval_raw_special_chars() {
    let h = harness();
    let fixture = SPECIAL_CHARS.setup().expect("failed to setup fixture");
    let path = format!("{}#lib.withNewlines", fixture.path().display());

    h.assert_conformance(&["eval", "--raw", &path], &ComparisonStrategy::Exact);
}

// =============================================================================
// Error cases
// =============================================================================

#[test]
fn eval_nonexistent_attr() {
    let h = harness();
    let fixture = LIB_ONLY.setup().expect("failed to setup fixture");
    let path = format!("{}#lib.nonexistent", fixture.path().display());

    // Both should fail
    h.assert_conformance(
        &["eval", "--json", &path],
        &ComparisonStrategy::SuccessMatch,
    );
}

#[test]
fn eval_nonexistent_deep_attr() {
    let h = harness();
    let fixture = LIB_ONLY.setup().expect("failed to setup fixture");
    let path = format!("{}#lib.foo.bar.baz", fixture.path().display());

    // Both should fail
    h.assert_conformance(
        &["eval", "--json", &path],
        &ComparisonStrategy::SuccessMatch,
    );
}

// =============================================================================
// Derivation attribute tests
// =============================================================================

#[test]
fn eval_derivation_name() {
    let h = harness();
    let fixture = SIMPLE_PACKAGE.setup().expect("failed to setup fixture");
    let path = format!(
        "{}#packages.x86_64-linux.default.name",
        fixture.path().display()
    );

    h.assert_conformance(
        &["eval", "--json", &path],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

#[test]
fn eval_derivation_system() {
    let h = harness();
    let fixture = SIMPLE_PACKAGE.setup().expect("failed to setup fixture");
    let path = format!(
        "{}#packages.x86_64-linux.default.system",
        fixture.path().display()
    );

    h.assert_conformance(
        &["eval", "--json", &path],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}

// =============================================================================
// Tests against the local trix flake
// =============================================================================

#[test]
fn eval_local_flake_description() {
    let h = harness();

    // Use the trix repo itself as a test flake
    h.assert_conformance(
        &["eval", "--json", ".#description"],
        &ComparisonStrategy::JsonSemantic {
            ignore_fields: vec![],
        },
    );
}
