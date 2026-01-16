//! Shell command - start a shell with specified packages available.

use std::env;
use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, info, instrument, trace};

use crate::cli::build::parse_override_inputs;
use crate::eval::Evaluator;
use crate::flake::{current_system, expand_attribute, format_attribute_not_found_error, resolve_installable_any, OperationContext};
use crate::progress;

#[derive(Args)]
pub struct ShellArgs {
    /// Package installables to make available (e.g., '.#hello', 'nixpkgs#cowsay')
    #[arg(required = true)]
    pub installables: Vec<String>,

    /// Command to run in the shell
    #[arg(short = 'c', long)]
    pub command: Option<String>,

    /// Override a flake input with a local path (avoids store copy for the override)
    /// Usage: --override-input nixpkgs ~/nixpkgs
    #[arg(long = "override-input", num_args = 2, value_names = ["INPUT", "PATH"], action = clap::ArgAction::Append)]
    pub override_input: Vec<String>,

    /// Accepted for nix CLI compatibility (trix is always impure)
    #[arg(long, hide = true)]
    pub impure: bool,
}

#[instrument(level = "debug", skip_all)]
pub fn run(args: ShellArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;

    // First, check if any installable is remote
    let mut has_remote = false;
    for installable in &args.installables {
        let resolved = resolve_installable_any(installable, &cwd);
        if !resolved.is_local {
            has_remote = true;
            break;
        }
    }

    // If any are remote, passthrough all to nix shell
    if has_remote {
        return run_remote(&args);
    }

    // All local - use native evaluation
    let system = current_system()?;
    let mut store_paths = Vec::new();
    let mut eval = Evaluator::new().context("failed to initialize evaluator")?;

    // Parse override inputs
    let input_overrides = parse_override_inputs(&args.override_input);
    if !input_overrides.is_empty() {
        debug!(?input_overrides, "using input overrides");
    }

    // Build each installable
    for installable in &args.installables {
        debug!("processing installable: {}", installable);

        let resolved = resolve_installable_any(installable, &cwd);
        let flake_path = resolved.path.expect("local flake should have path");
        let candidates = expand_attribute(&resolved.attribute, OperationContext::Build, &system);
        debug!(?candidates, "expanded attribute candidates");

        // Try each candidate until one succeeds
        let (attr_path, value) = {
            let mut found = None;

            for candidate in &candidates {
                let eval_target = format!("{}#{}", flake_path.display(), candidate.join("."));
                trace!("trying {}", eval_target);

                let result = if input_overrides.is_empty() {
                    eval.eval_flake_attr(&flake_path, candidate)
                } else {
                    eval.eval_flake_attr_with_overrides(&flake_path, candidate, &input_overrides)
                };

                match result {
                    Ok(value) => {
                        info!("evaluating {}", eval_target);
                        found = Some((candidate.clone(), value));
                        break;
                    }
                    Err(e) => {
                        trace!("candidate {} failed: {}", candidate.join("."), e);
                    }
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

        debug!(attr = %attr_path.join("."), "found attribute");

        let drv_path = eval.get_drv_path(&value)?;
        debug!(drv = %drv_path, "got derivation path");

        // Build it
        info!("building {}", drv_path);
        let build_status = progress::building(&drv_path);

        let store_path = eval.build_value(&value)?;

        build_status.finish_and_clear();
        store_paths.push(store_path);
    }

    // Build PATH with all package bin directories
    let mut bin_paths = Vec::new();
    for store_path in &store_paths {
        let bin_dir = std::path::Path::new(store_path).join("bin");
        if bin_dir.is_dir() {
            bin_paths.push(bin_dir.to_string_lossy().into_owned());
        }
    }

    if bin_paths.is_empty() {
        return Err(anyhow!("no bin directories found in packages"));
    }

    // Prepend to existing PATH
    let old_path = env::var("PATH").unwrap_or_default();
    let mut new_path_parts = bin_paths;
    if !old_path.is_empty() {
        new_path_parts.push(old_path);
    }
    let new_path = new_path_parts.join(":");

    if let Some(cmd_str) = &args.command {
        // Run command and exit
        debug!("running command: sh -c {}", cmd_str);

        let status = Command::new("sh")
            .args(["-c", cmd_str])
            .env("PATH", &new_path)
            .status()
            .context("failed to run command")?;

        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }
    } else {
        // Start interactive shell
        let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

        debug!("starting shell: {}", shell);
        info!("entering shell with {} packages", store_paths.len());

        let status = Command::new(&shell)
            .env("PATH", &new_path)
            .status()
            .context(format!("failed to run {}", shell))?;

        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }
    }

    Ok(())
}

/// Passthrough to nix shell for remote installables
fn run_remote(args: &ShellArgs) -> Result<()> {
    info!(
        "running shell with {} packages (remote, delegating to nix)",
        args.installables.len()
    );

    let mut cmd = Command::new("nix");
    cmd.arg("shell");

    // Add all installables
    for installable in &args.installables {
        cmd.arg(installable);
    }

    // Add command if specified
    if let Some(ref cmd_str) = args.command {
        cmd.args(["--command", "sh", "-c", cmd_str]);
    }

    debug!("+ nix shell {:?}", args.installables);

    // exec replaces the current process
    let err = cmd.exec();
    Err(anyhow!("failed to exec nix shell: {}", err))
}
