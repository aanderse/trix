//! Conformance test suite for trix.
//!
//! This module provides infrastructure for systematically testing that trix
//! produces the same output as the nix flakes CLI. It runs the same commands
//! against both `trix` and `nix` and compares the results.
//!
//! # Usage
//!
//! ```bash
//! # Run all conformance tests
//! cargo test conformance
//!
//! # Run specific command tests
//! cargo test conformance::commands::eval
//!
//! # See detailed output on failures
//! cargo test conformance -- --nocapture
//! ```

pub mod commands;
pub mod fixtures;
pub mod harness;

pub use fixtures::Fixture;
pub use harness::{CommandResult, ComparisonStrategy, ConformanceHarness};
