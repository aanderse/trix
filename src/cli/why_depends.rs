//! Why-depends command - show why a package depends on another.

use std::collections::HashMap;
use std::env;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, trace};

use crate::eval::generate_flake_eval_expr;
use crate::flake::{current_system, expand_attribute, format_attribute_not_found_error, resolve_installable, OperationContext};

#[derive(Args)]
pub struct WhyDependsArgs {
    /// Package to check (installable or store path)
    pub package: String,

    /// Dependency to trace (installable or store path)
    pub dependency: String,
}

pub fn run(args: WhyDependsArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;

    // Resolve both arguments to store paths
    let pkg_path = resolve_to_store_path(&args.package, &cwd)?;
    let dep_path = resolve_to_store_path(&args.dependency, &cwd)?;

    debug!(package = %pkg_path, dependency = %dep_path, "Running nix why-depends");

    // Run nix why-depends
    let status = Command::new("nix")
        .args(["why-depends", &pkg_path, &dep_path])
        .status()
        .context("failed to run nix why-depends")?;

    if !status.success() {
        return Err(anyhow!("nix why-depends failed"));
    }

    Ok(())
}

/// Resolve an installable reference to a store path.
/// If it's already a store path, return it directly.
/// Otherwise, build the package and return its store path.
fn resolve_to_store_path(ref_str: &str, cwd: &std::path::Path) -> Result<String> {
    // If already a store path, return it directly
    if ref_str.starts_with("/nix/store/") {
        return Ok(ref_str.to_string());
    }

    let resolved = resolve_installable(ref_str, cwd)?;

    // For non-local flakes, use nix build to get the store path
    if resolved.lock.is_none() {
        debug!("building non-local reference: {}", ref_str);
        let output = Command::new("nix")
            .args(["build", "--no-link", "--print-out-paths", ref_str])
            .output()
            .context("failed to run nix build")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("failed to build {}: {}", ref_str, stderr.trim()));
        }

        let store_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Ok(store_path);
    }

    // For local flakes, build using our method
    let lock = resolved.lock.as_ref().unwrap();
    let flake_path = &resolved.path;
    let system = current_system()?;

    let candidates = expand_attribute(&resolved.attribute, OperationContext::Build, &system);
    debug!(?candidates, "expanded attribute candidates");

    let flake_dir = flake_path
        .to_str()
        .ok_or_else(|| anyhow!("invalid flake path"))?;

    // Try each candidate until one succeeds
    let drv_path = {
        let mut found = None;

        for candidate in &candidates {
            trace!("trying candidate: {}", candidate.join("."));

            let expr = match generate_flake_eval_expr(flake_dir, lock, candidate, &HashMap::new()) {
                Ok(e) => e,
                Err(e) => {
                    trace!("candidate {} failed to generate expr: {}", candidate.join("."), e);
                    continue;
                }
            };

            let output = Command::new("nix-instantiate")
                .args(["-E", &expr])
                .output()
                .context("failed to run nix-instantiate")?;

            if output.status.success() {
                let drv = String::from_utf8_lossy(&output.stdout).trim().to_string();
                debug!(attr = %candidate.join("."), drv = %drv, "found derivation");
                found = Some(drv);
                break;
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                trace!("candidate {} failed: {}", candidate.join("."), stderr.trim());
            }
        }

        // Build flake URL for error message
        let canonical = flake_path
            .canonicalize()
            .unwrap_or_else(|_| flake_path.clone());
        let flake_url = format!("path:{}", canonical.display());

        found.ok_or_else(|| {
            anyhow!(format_attribute_not_found_error(&flake_url, &candidates))
        })?
    };

    // Build the derivation
    let output = Command::new("nix-store")
        .args(["--realise", &drv_path, "--no-gc-warning"])
        .output()
        .context("failed to run nix-store --realise")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("failed to build: {}", stderr.trim()));
    }

    let store_path = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // nix-store --realise can return multiple paths, we want the first one
    let store_path = store_path.lines().next().unwrap_or(&store_path).to_string();

    Ok(store_path)
}
