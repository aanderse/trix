//! Run command - build and execute a package or app from a flake.

use std::env;
use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, info, instrument};

use crate::eval::Evaluator;
use crate::flake::{current_system, expand_attribute, resolve_installable_any, OperationContext, ResolvedInstallable};
use crate::progress;

#[derive(Args)]
pub struct RunArgs {
    /// Installable reference (e.g., '.#hello', 'nixpkgs#cowsay')
    #[arg(default_value = ".#default")]
    pub installable: String,

    /// Arguments to pass to the program
    #[arg(last = true)]
    pub program_args: Vec<String>,
}

#[instrument(level = "debug", skip_all, fields(installable = %args.installable))]
pub fn run(args: RunArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;

    // Step 1: Resolve the installable
    debug!("resolving installable");
    let resolved = resolve_installable_any(&args.installable, &cwd);

    // Step 2: Check if local or remote
    if !resolved.is_local {
        // Remote flake - passthrough to nix run
        return run_remote(&args, &resolved);
    }

    let flake_path = resolved.path.as_ref().expect("local flake should have path");
    debug!(
        flake_path = %flake_path.display(),
        has_lock = resolved.lock.is_some(),
        "resolved flake"
    );

    // Step 3: Get candidate attribute paths (apps, packages, legacyPackages)
    let system = current_system()?;
    let candidates = expand_attribute(&resolved.attribute, OperationContext::Run, &system);
    debug!(?candidates, "expanded attribute candidates");

    let mut evaluator = Evaluator::new().context("failed to initialize Nix evaluator")?;

    // Try each candidate until one succeeds
    let (attr_path, value) = {
        let mut last_err = None;
        let mut found = None;

        for candidate in &candidates {
            match evaluator.eval_flake_attr(flake_path, candidate) {
                Ok(value) => {
                    debug!(attr = %candidate.join("."), "found attribute");
                    found = Some((candidate.clone(), value));
                    break;
                }
                Err(e) => {
                    debug!("candidate {} failed: {}", candidate.join("."), e);
                    last_err = Some(e);
                }
            }
        }

        found.ok_or_else(|| {
            last_err.unwrap_or_else(|| anyhow!("no runnable attribute found"))
        })?
    };

    // Determine how to run based on the value (app vs derivation)
    let exe_path = if let Some(program) = evaluator.get_attr(&value, "program")? {
        // It's an app - get the program path directly
        info!("running app at {}", attr_path.join("."));
        evaluator.require_string(&program)?
    } else {
        // It's a derivation - build and find executable
        let attr_name = resolved.attribute.last().map(|s| s.as_str()).unwrap_or("default");

        let drv_path = evaluator.get_drv_path(&value)?;
        debug!(drv = %drv_path, "got derivation path");

        info!("building {}", attr_path.join("."));
        let build_status = progress::building(&drv_path);

        let store_path = evaluator.build_value(&value)?;

        build_status.finish_and_clear();

        // Get the main program name
        let main_program = evaluator.get_main_program(&value, attr_name)?;
        format!("{}/bin/{}", store_path, main_program)
    };

    // Run the executable
    debug!("executing: {} {:?}", exe_path, args.program_args);

    let mut cmd = Command::new(&exe_path);
    cmd.args(&args.program_args);

    // exec replaces the current process
    let err = cmd.exec();
    Err(anyhow!("failed to exec {}: {}", exe_path, err))
}

/// Passthrough to nix run for remote flake references
fn run_remote(args: &RunArgs, resolved: &ResolvedInstallable) -> Result<()> {
    let flake_ref = resolved.flake_ref.as_deref().unwrap_or("");
    let attr_str = if resolved.attribute.is_empty() {
        String::new()
    } else {
        resolved.attribute.join(".")
    };

    let full_ref = if attr_str.is_empty() {
        flake_ref.to_string()
    } else {
        format!("{}#{}", flake_ref, attr_str)
    };

    info!("running {} (remote, delegating to nix)", full_ref);

    let mut cmd = Command::new("nix");
    cmd.arg("run").arg(&full_ref);

    // Add -- separator and program args
    if !args.program_args.is_empty() {
        cmd.arg("--");
        cmd.args(&args.program_args);
    }

    debug!("+ nix run {} -- {:?}", full_ref, args.program_args);

    // exec replaces the current process
    let err = cmd.exec();
    Err(anyhow!("failed to exec nix run: {}", err))
}
