use anyhow::{Context, Result};

use crate::flake::{ensure_lock, ResolvedInstallable};
use crate::nix::{run_nix_build, BuildOptions};

/// Build a resolved flake attribute.
///
/// This helper handles the common logic for local builds:
/// 1. Getting the flake directory
/// 2. Ensuring the lock file exists
/// 3. Running nix-build
pub fn build_resolved_attribute(
    resolved: &ResolvedInstallable,
    attr: &str,
    options: &BuildOptions,
    capture_output: bool,
) -> Result<Option<String>> {
    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;

    // Ensure lock exists
    ensure_lock(flake_dir, None)?;

    run_nix_build(flake_dir, attr, options, capture_output)
}
