//! Log command - show build logs for a package.

use std::collections::HashMap;
use std::env;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, instrument};

use crate::eval::generate_flake_eval_expr;
use crate::flake::{current_system, expand_attribute, resolve_installable_any, OperationContext};

#[derive(Args)]
pub struct LogArgs {
    /// Installable reference (default: .#default)
    #[arg(default_value = ".#default")]
    pub installable: String,
}

#[instrument(level = "debug", skip_all, fields(installable = %args.installable))]
pub fn run(args: LogArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;

    // Resolve the installable (handles local paths, registry names, and remote refs)
    let resolved = resolve_installable_any(&args.installable, &cwd);

    // For non-local flakes, pass through to nix log
    if !resolved.is_local {
        debug!("passing through to nix log for remote flake");
        let installable_str = resolved.to_installable_string();
        let status = Command::new("nix")
            .args(["log", &installable_str])
            .status()
            .context("failed to run nix log")?;

        if !status.success() {
            return Err(anyhow!("nix log failed"));
        }
        return Ok(());
    }

    let flake_path = resolved
        .path
        .as_ref()
        .ok_or_else(|| anyhow!("local flake must have path"))?;

    // For flakes without a lock, pass through to nix log
    if resolved.lock.is_none() {
        debug!("passing through to nix log for flake without lock");
        let installable_str = resolved.to_installable_string();
        let status = Command::new("nix")
            .args(["log", &installable_str])
            .status()
            .context("failed to run nix log")?;

        if !status.success() {
            return Err(anyhow!("nix log failed"));
        }
        return Ok(());
    }

    // For local flakes with lock, get the derivation path and show its log
    let lock = resolved.lock.as_ref().unwrap();
    let system = current_system()?;

    // Expand the attribute path and try each candidate
    let candidates = expand_attribute(&resolved.attribute, OperationContext::Build, &system);
    let attr_path = &candidates[0]; // TODO: try multiple candidates like build.rs

    debug!(attr = %attr_path.join("."), "getting derivation path");

    // Generate expression and instantiate to get drv path
    let flake_dir = flake_path
        .to_str()
        .ok_or_else(|| anyhow!("invalid flake path"))?;
    let expr = generate_flake_eval_expr(flake_dir, lock, &attr_path, &HashMap::new())?;

    let output = Command::new("nix-instantiate")
        .args(["-E", &expr])
        .output()
        .context("failed to run nix-instantiate")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("failed to get derivation: {}", stderr.trim()));
    }

    let drv_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    debug!(drv = %drv_path, "got derivation path");

    // Use nix log to show the build log
    let status = Command::new("nix")
        .args(["log", &drv_path])
        .status()
        .context("failed to run nix log")?;

    if !status.success() {
        return Err(anyhow!("no build log available for {}", drv_path));
    }

    Ok(())
}
