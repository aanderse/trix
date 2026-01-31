//! Command-specific conformance tests.
//!
//! Each submodule tests a specific trix command against the corresponding
//! nix command to ensure output conformance.

pub mod build;
pub mod eval;
pub mod flake_metadata;
pub mod flake_show;
