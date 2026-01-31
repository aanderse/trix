//! Conformance test suite entry point.
//!
//! This module provides systematic testing that trix produces the same
//! output as the nix flakes CLI for equivalent commands.
//!
//! # Running Tests
//!
//! ```bash
//! # Run all conformance tests
//! cargo test --test conformance_tests
//!
//! # Run specific command tests
//! cargo test --test conformance_tests eval
//! cargo test --test conformance_tests flake_show
//!
//! # Run with verbose output
//! cargo test --test conformance_tests -- --nocapture
//!
//! # Include network tests (slow)
//! cargo test --test conformance_tests -- --include-ignored
//! ```

mod conformance;

// Re-export for convenience in submodules
pub use conformance::*;
