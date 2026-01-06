//! Fmt command - format Nix files using the flake's formatter.

use std::env;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, info, instrument};

use crate::eval::Evaluator;
use crate::flake::{current_system, resolve_installable_any};
use crate::progress;

#[derive(Args)]
pub struct FmtArgs {
    /// Flake reference (default: .)
    #[arg(default_value = ".")]
    pub flake_ref: String,

    /// Files to format (passed to the formatter)
    #[arg(last = true)]
    pub files: Vec<String>,
}

#[instrument(level = "debug", skip_all, fields(flake_ref = %args.flake_ref))]
pub fn run(args: FmtArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;

    // Step 1: Resolve the flake
    debug!("resolving flake reference");
    let resolved = resolve_installable_any(&args.flake_ref, &cwd);

    // Check if remote - passthrough to nix fmt
    if !resolved.is_local {
        return run_remote(&args);
    }

    let flake_path = resolved.path.expect("local flake should have path");
    debug!(flake_path = %flake_path.display(), "resolved flake");

    // Step 2: Get the formatter for the current system using native evaluation
    let system = current_system()?;
    let attr_path = vec!["formatter".to_string(), system.clone()];

    let eval_target = format!("{}#formatter.{}", flake_path.display(), system);
    info!("evaluating {}", eval_target);

    let status = progress::evaluating(&eval_target);

    let mut eval = Evaluator::new().context("failed to initialize evaluator")?;
    let value = eval
        .eval_flake_attr(&flake_path, &attr_path)
        .context("failed to evaluate formatter - does the flake have a formatter output?")?;

    status.finish_and_clear();

    let drv_path = eval.get_drv_path(&value)?;
    debug!(drv = %drv_path, "got derivation path");

    // Step 4: Build the formatter
    info!("building {}", drv_path);
    let build_status = progress::building(&drv_path);

    let store_path = eval.build_value(&value)?;

    build_status.finish_and_clear();

    // Step 5: Get the main program name
    let main_program = eval.get_main_program(&value, "formatter")?;
    let exe_path = format!("{}/bin/{}", store_path, main_program);

    debug!("formatter executable: {}", exe_path);

    // Step 6: Run the formatter
    // If no files specified, format in current directory
    let files_to_format: Vec<&str> = if args.files.is_empty() {
        vec!["."]
    } else {
        args.files.iter().map(|s| s.as_str()).collect()
    };

    info!("formatting with {}", main_program);

    let status = Command::new(&exe_path)
        .args(&files_to_format)
        .status()
        .context(format!("failed to run formatter: {}", exe_path))?;

    if !status.success() {
        return Err(anyhow!(
            "formatter exited with code: {}",
            status.code().unwrap_or(1)
        ));
    }

    Ok(())
}

/// Passthrough to nix fmt for remote flake references
fn run_remote(args: &FmtArgs) -> Result<()> {
    info!("formatting with {} (remote, delegating to nix)", args.flake_ref);

    let mut cmd = Command::new("nix");
    cmd.arg("fmt").arg(&args.flake_ref);

    // Add files if specified
    if !args.files.is_empty() {
        cmd.arg("--");
        cmd.args(&args.files);
    }

    debug!("+ nix fmt {}", args.flake_ref);
    let status = cmd.status().context("failed to run nix fmt")?;

    if !status.success() {
        return Err(anyhow!(
            "nix fmt exited with code: {}",
            status.code().unwrap_or(1)
        ));
    }

    Ok(())
}
