//! Run command - build and execute a package or app from a flake.

use std::env;
use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, info, instrument};

use crate::cli::build::parse_override_inputs;
use crate::eval;
use crate::flake::{current_system, expand_attribute, resolve_installable_any, OperationContext, ResolvedInstallable};
use crate::lock;
use crate::progress;

#[derive(Args)]
pub struct RunArgs {
    /// Installable reference (e.g., '.#hello', 'nixpkgs#cowsay')
    #[arg(default_value = ".#default")]
    pub installable: String,

    /// Override a flake input with a local path (avoids store copy for the override)
    /// Usage: --override-input nixpkgs ~/nixpkgs
    #[arg(long = "override-input", num_args = 2, value_names = ["INPUT", "PATH"], action = clap::ArgAction::Append)]
    pub override_input: Vec<String>,

    /// Accepted for nix CLI compatibility (trix is always impure)
    #[arg(long, hide = true)]
    pub impure: bool,

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

    // Step 3: Read flake.lock (missing lock is OK for flakes with no inputs)
    let lock = lock::read_flake_lock(flake_path).unwrap_or_else(|_| lock::FlakeLock::empty());

    // Step 4: Get candidate attribute paths (apps, packages, legacyPackages)
    let system = current_system()?;
    let candidates = expand_attribute(&resolved.attribute, OperationContext::Run, &system);
    debug!(?candidates, "expanded attribute candidates");

    // Parse override inputs
    let input_overrides = parse_override_inputs(&args.override_input);
    if !input_overrides.is_empty() {
        debug!(?input_overrides, "using input overrides");
    }

    // Try each candidate until one succeeds
    let (_attr_path, exe_path) = {
        let mut last_err = None;
        let mut found = None;

        for candidate in &candidates {
            // First, try to get it as an app (has .program attribute)
            if let Ok(Some(program_path)) = eval::try_get_app_program(
                flake_path,
                &lock,
                candidate,
                &input_overrides,
            ) {
                info!("running app at {}", candidate.join("."));
                found = Some((candidate.clone(), program_path));
                break;
            }

            // Otherwise, treat as a package - evaluate to drv and build
            let result = eval::generate_and_eval_local_flake(
                flake_path,
                &lock,
                candidate,
                &input_overrides,
            );

            match result {
                Ok(drv_info) => {
                    debug!(attr = %candidate.join("."), drv = %drv_info.drv_path, "found package");

                    info!("building {}", candidate.join("."));
                    let build_status = progress::building(&drv_info.drv_path);

                    let store_path = eval::build_drv(&drv_info.drv_path, &drv_info.outputs_to_install)
                        .context("build failed")?;

                    build_status.finish_and_clear();

                    // Get the main program name
                    let attr_name = resolved.attribute.last().map(|s| s.as_str()).unwrap_or("default");
                    let main_program = eval::get_main_program(
                        flake_path,
                        &lock,
                        candidate,
                        &input_overrides,
                        attr_name,
                    )?;

                    let exe_path = format!("{}/bin/{}", store_path, main_program);
                    found = Some((candidate.clone(), exe_path));
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
    cmd.args(["--extra-experimental-features", "nix-command flakes"]);
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
