//! Log command - show build logs for a package.

use std::collections::HashMap;
use std::env;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, instrument, trace};

use crate::eval;
use crate::flake::{current_system, expand_attribute, format_attribute_not_found_error, resolve_installable_any, OperationContext};
use crate::lock;

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

    // Read flake.lock (missing lock is OK for flakes with no inputs)
    let flake_lock = lock::read_flake_lock(flake_path)
        .unwrap_or_else(|_| lock::FlakeLock::empty());

    let system = current_system()?;
    let candidates = expand_attribute(&resolved.attribute, OperationContext::Build, &system);
    debug!(?candidates, "expanded attribute candidates");

    // Try each candidate until one succeeds
    let drv_path = {
        let mut found = None;

        for candidate in &candidates {
            trace!("trying candidate: {}", candidate.join("."));

            match eval::generate_and_eval_local_flake(
                flake_path,
                &flake_lock,
                candidate,
                &HashMap::new(),
            ) {
                Ok(drv_info) => {
                    debug!(attr = %candidate.join("."), drv = %drv_info.drv_path, "found derivation");
                    found = Some(drv_info.drv_path);
                    break;
                }
                Err(e) => {
                    trace!("candidate {} failed: {}", candidate.join("."), e);
                }
            }
        }

        let canonical = flake_path
            .canonicalize()
            .unwrap_or_else(|_| flake_path.clone());
        let flake_url = format!("path:{}", canonical.display());

        found.ok_or_else(|| {
            anyhow!(format_attribute_not_found_error(&flake_url, &candidates))
        })?
    };

    // Use nix log to show the build log (no store copy)
    let output = Command::new("nix")
        .args(["log", &drv_path])
        .output()
        .context("failed to run nix log")?;

    if !output.status.success() {
        return Err(anyhow!("no build log available for {}", drv_path));
    }

    let log_content = String::from_utf8_lossy(&output.stdout);
    print!("{}", log_content);

    Ok(())
}
