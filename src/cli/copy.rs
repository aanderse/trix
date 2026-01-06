//! Copy command - copy store paths to another store.

use std::env;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, info, instrument};

use crate::eval::Evaluator;
use crate::flake::{current_system, expand_attribute, resolve_installable_any, OperationContext};
use crate::progress;

#[derive(Args)]
pub struct CopyArgs {
    /// Installable reference (e.g., '.#default')
    #[arg(default_value = ".#default")]
    pub installable: String,

    /// Destination store URL
    #[arg(long)]
    pub to: String,

    /// Don't check signatures
    #[arg(long)]
    pub no_check_sigs: bool,
}

#[instrument(level = "debug", skip_all, fields(installable = %args.installable))]
pub fn run(args: CopyArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;

    // Step 1: Resolve the installable
    debug!("resolving installable");
    let resolved = resolve_installable_any(&args.installable, &cwd);

    // Check if remote - passthrough to nix copy
    if !resolved.is_local {
        return run_remote(&args);
    }

    let flake_path = resolved.path.expect("local flake should have path");
    debug!(flake_path = %flake_path.display(), "resolved flake");

    // Step 2: Evaluate to get the store path using native evaluation
    let system = current_system()?;
    let candidates = expand_attribute(&resolved.attribute, OperationContext::Build, &system);
    let attr_path = &candidates[0];

    let eval_target = format!("{}#{}", flake_path.display(), attr_path.join("."));
    info!("evaluating {}", eval_target);

    let status = progress::evaluating(&eval_target);

    let mut eval = Evaluator::new().context("failed to initialize evaluator")?;
    let value = eval
        .eval_flake_attr(&flake_path, attr_path)
        .context("failed to evaluate derivation")?;

    status.finish_and_clear();

    let drv_path = eval.get_drv_path(&value)?;
    debug!(drv = %drv_path, "got derivation path");

    // Step 3: Build to get the store path
    info!("building {}", drv_path);
    let build_status = progress::building(&drv_path);

    let store_path = eval.build_value(&value)?;

    build_status.finish_and_clear();

    debug!(store_path = %store_path, "built store path");

    // Step 4: Copy to destination
    info!("copying {} to {}", store_path, args.to);
    let copy_status = progress::copying(&store_path);

    let mut cmd = Command::new("nix");
    cmd.args(["copy", "--to", &args.to, &store_path]);

    if args.no_check_sigs {
        cmd.arg("--no-check-sigs");
    }

    let copy_output = cmd.output().context("failed to run nix copy")?;

    copy_status.finish_and_clear();

    if !copy_output.status.success() {
        let stderr = String::from_utf8_lossy(&copy_output.stderr);
        return Err(anyhow!("copy failed: {}", stderr));
    }

    println!("{}", &store_path);

    Ok(())
}

/// Passthrough to nix copy for remote flake references
fn run_remote(args: &CopyArgs) -> Result<()> {
    info!(
        "copying {} to {} (remote, delegating to nix)",
        args.installable, args.to
    );

    // First build the remote package, then copy
    let mut build_cmd = Command::new("nix");
    build_cmd.args(["build", &args.installable, "--no-link", "--print-out-paths"]);

    debug!("+ nix build {} --no-link --print-out-paths", args.installable);
    let build_output = build_cmd.output().context("failed to run nix build")?;

    if !build_output.status.success() {
        let stderr = String::from_utf8_lossy(&build_output.stderr);
        return Err(anyhow!("build failed: {}", stderr));
    }

    let store_path = String::from_utf8_lossy(&build_output.stdout)
        .lines()
        .next()
        .ok_or_else(|| anyhow!("no output path from build"))?
        .to_string();

    // Now copy
    let mut copy_cmd = Command::new("nix");
    copy_cmd.args(["copy", "--to", &args.to, &store_path]);

    if args.no_check_sigs {
        copy_cmd.arg("--no-check-sigs");
    }

    debug!("+ nix copy --to {} {}", args.to, store_path);
    let copy_output = copy_cmd.output().context("failed to run nix copy")?;

    if !copy_output.status.success() {
        let stderr = String::from_utf8_lossy(&copy_output.stderr);
        return Err(anyhow!("copy failed: {}", stderr));
    }

    println!("{}", store_path);

    Ok(())
}
