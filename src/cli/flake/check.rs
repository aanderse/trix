//! Flake check command - check flake for issues.
//!
//! For local flakes: uses `nix eval` to avoid store copy.
//! For remote flakes: delegates to `nix flake check`.

use std::env;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, info, instrument};

use crate::eval;
use crate::flake::{current_system, resolve_installable_any};
use crate::lock;

#[derive(Args)]
pub struct CheckArgs {
    /// Flake reference (default: .)
    #[arg(default_value = ".")]
    pub flake_ref: String,
}

#[instrument(level = "debug", skip_all, fields(flake_ref = %args.flake_ref))]
pub fn run(args: CheckArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;
    let resolved = resolve_installable_any(&args.flake_ref, &cwd);

    // For remote flakes, delegate to nix flake check
    if !resolved.is_local {
        debug!("delegating to nix flake check for remote flake");
        info!("checking {} (delegating to nix)", args.flake_ref);

        let installable_str = resolved.to_installable_string();
        let mut cmd = Command::new("nix");
        cmd.args(["--extra-experimental-features", "nix-command flakes"]);
        cmd.arg("flake").arg("check").arg(&installable_str);

        let status = cmd.status().context("failed to run nix flake check")?;
        if !status.success() {
            return Err(anyhow!(
                "nix flake check exited with code: {}",
                status.code().unwrap_or(1)
            ));
        }
        return Ok(());
    }

    // Local flake: evaluate and build checks natively
    let flake_path = resolved
        .path
        .as_ref()
        .ok_or_else(|| anyhow!("local flake must have path"))?;

    let system = current_system()?;

    // Read flake.lock (missing lock is OK for flakes with no inputs)
    let flake_lock = lock::read_flake_lock(flake_path)
        .unwrap_or_else(|_| lock::FlakeLock::empty());

    // Get all check derivation paths
    let checks = eval::eval_flake_checks(flake_path, &flake_lock, &system)?;

    if checks.is_empty() {
        println!("No checks found for {}", system);
        return Ok(());
    }

    // Build each check
    let mut passed = 0;
    let mut failed = 0;

    for (name, drv_path) in &checks {
        print!("checking {}: ", name);

        let installable = format!("{}^*", drv_path);
        let output = Command::new("nix")
            .args(["build", &installable, "--no-link"])
            .output()
            .context("failed to run nix build")?;

        if output.status.success() {
            println!("ok");
            passed += 1;
        } else {
            println!("FAILED");
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!("  Error: {}", stderr);
            failed += 1;
        }
    }

    println!();
    println!("{} passed, {} failed", passed, failed);

    if failed > 0 {
        return Err(anyhow!("{} check(s) failed", failed));
    }

    Ok(())
}
