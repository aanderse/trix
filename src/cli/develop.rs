use std::env;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, info, instrument};

use crate::eval::Evaluator;
use crate::flake::{current_system, expand_attribute, resolve_installable_any, OperationContext};
use crate::progress;

#[derive(Args)]
pub struct DevelopArgs {
    /// Flake reference to develop (default: .#devShells.<system>.default)
    #[arg(default_value = ".")]
    pub installable: String,

    /// Accepted for nix CLI compatibility (trix is always impure)
    #[arg(long, hide = true)]
    pub impure: bool,

    /// Enter shell from a Nix file instead of a flake (like nix-shell)
    #[arg(short = 'f', long = "file", conflicts_with = "installable")]
    pub file: Option<PathBuf>,

    /// Attribute to use from the file (requires --file)
    #[arg(short = 'A', long = "attr", requires = "file")]
    pub attr: Option<String>,

    /// Pass a Nix expression as argument (requires --file)
    /// Usage: --arg name 'expression'
    #[arg(long = "arg", num_args = 2, value_names = ["NAME", "EXPR"], action = clap::ArgAction::Append, requires = "file")]
    pub arg: Vec<String>,

    /// Pass a string as argument (requires --file)
    /// Usage: --argstr name 'value'
    #[arg(long = "argstr", num_args = 2, value_names = ["NAME", "VALUE"], action = clap::ArgAction::Append, requires = "file")]
    pub argstr: Vec<String>,

    /// Command and arguments to run instead of interactive shell
    #[arg(short = 'c', long = "command", num_args = 1.., allow_hyphen_values = true)]
    pub command: Vec<String>,

    /// Clear the entire environment, except for those specified with --keep-env-var
    #[arg(short = 'i', long = "ignore-env")]
    pub ignore_env: bool,

    /// Keep the environment variable when using --ignore-env
    #[arg(short = 'k', long = "keep-env-var", action = clap::ArgAction::Append)]
    pub keep_env_var: Vec<String>,

    /// Set an environment variable
    #[arg(short = 's', long = "set-env-var", num_args = 2, value_names = ["NAME", "VALUE"], action = clap::ArgAction::Append)]
    pub set_env_var: Vec<String>,

    /// Unset an environment variable
    #[arg(short = 'u', long = "unset-env-var", action = clap::ArgAction::Append)]
    pub unset_env_var: Vec<String>,
}

pub fn run(args: DevelopArgs) -> Result<()> {
    // Dispatch to file mode or flake mode
    if let Some(ref file_path) = args.file {
        return run_file_mode(&args, file_path);
    }

    run_flake_mode(&args)
}

/// Enter shell from a Nix file (non-flake mode, like nix-shell)
#[instrument(level = "debug", skip_all, fields(file = %file_path.display(), attr = ?args.attr))]
fn run_file_mode(args: &DevelopArgs, file_path: &PathBuf) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;

    // Resolve the file path
    let file_path = if file_path.is_absolute() {
        file_path.clone()
    } else {
        cwd.join(file_path)
    };

    if !file_path.exists() {
        return Err(anyhow!("file not found: {}", file_path.display()));
    }

    debug!(file = %file_path.display(), "resolved file path");

    // Determine the effective command to run
    let effective_command = get_effective_command(args);

    // Build display path for logging
    let display_path = if let Some(ref attr) = args.attr {
        format!("{}#{}", file_path.display(), attr)
    } else {
        file_path.display().to_string()
    };

    info!("entering shell from {}", display_path);

    // Use nix-shell directly with the file
    let mut cmd = Command::new("nix-shell");
    cmd.arg(&file_path);

    // Add attribute if specified
    if let Some(ref attr) = args.attr {
        cmd.args(["-A", attr]);
    }

    // Add --arg pairs
    for chunk in args.arg.chunks(2) {
        if chunk.len() == 2 {
            cmd.args(["--arg", &chunk[0], &chunk[1]]);
        }
    }

    // Add --argstr pairs
    for chunk in args.argstr.chunks(2) {
        if chunk.len() == 2 {
            cmd.args(["--argstr", &chunk[0], &chunk[1]]);
        }
    }

    // Set NIX_BUILD_SHELL to bash
    if env::var("NIX_BUILD_SHELL").is_err() {
        cmd.env("NIX_BUILD_SHELL", "bash");
    }

    // Add environment flags
    add_env_flags(&mut cmd, args);

    if let Some(ref command) = effective_command {
        cmd.args(["--command", command]);
    }

    debug!("+ nix-shell {}", file_path.display());

    // exec replaces the current process
    let err = cmd.exec();
    Err(anyhow!("failed to exec nix-shell: {}", err))
}

/// Determine the effective command to run based on --command args
fn get_effective_command(args: &DevelopArgs) -> Option<String> {
    if !args.command.is_empty() {
        // Join command and args into a single string for nix-shell --command
        // nix-shell --command expects a shell command string that will be executed via bash -c
        // So we just join all arguments with spaces - this matches nix develop -c behavior
        Some(args.command.join(" "))
    } else {
        None
    }
}

/// Add environment-related flags to nix-shell command.
/// nix-shell uses --pure for --ignore-env, --keep for --keep-env-var.
/// For --set-env-var and --unset-env-var, we apply them via Command::env/env_remove
/// since nix-shell doesn't have direct equivalents.
fn add_env_flags(cmd: &mut Command, args: &DevelopArgs) {
    // --ignore-env maps to nix-shell --pure
    if args.ignore_env {
        cmd.arg("--pure");
    }

    // --keep-env-var maps to nix-shell --keep
    for var in &args.keep_env_var {
        cmd.args(["--keep", var]);
    }

    // --set-env-var: nix-shell doesn't have this, so we set via Command::env
    for chunk in args.set_env_var.chunks(2) {
        if chunk.len() == 2 {
            cmd.env(&chunk[0], &chunk[1]);
        }
    }

    // --unset-env-var: nix-shell doesn't have this, so we unset via Command::env_remove
    for var in &args.unset_env_var {
        cmd.env_remove(var);
    }
}

#[instrument(level = "debug", skip_all, fields(installable = %args.installable))]
fn run_flake_mode(args: &DevelopArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;

    // Determine the effective command to run
    let effective_command = get_effective_command(args);

    // Step 1: Resolve the installable to a flake path and attribute
    debug!("resolving installable");
    let resolved = resolve_installable_any(&args.installable, &cwd);

    // Check if remote - passthrough to nix develop
    if !resolved.is_local {
        return run_remote(args, &effective_command);
    }

    let flake_path = resolved.path.expect("local flake should have path");
    debug!(
        flake_path = %flake_path.display(),
        has_lock = resolved.lock.is_some(),
        "resolved flake"
    );

    // Step 2: Determine the full attribute path for devShells
    let system = current_system()?;
    let candidates = expand_attribute(&resolved.attribute, OperationContext::Develop, &system);
    debug!(candidates = ?candidates, %system, "expanded attribute candidates");

    // Step 3: Evaluate the devShell to get drvPath using native evaluation
    // Try each candidate in order until one succeeds (fallback order: devShells, devShell, packages)
    let mut eval = Evaluator::new().context("failed to initialize evaluator")?;
    let mut last_error = None;
    let mut drv_path_str = None;
    let mut successful_attr = None;

    for attr_path in &candidates {
        let eval_target = format!("{}#{}", flake_path.display(), attr_path.join("."));
        debug!(attr = %attr_path.join("."), "trying candidate");

        let status = progress::evaluating(&eval_target);
        match eval.eval_flake_attr(&flake_path, attr_path) {
            Ok(value) => {
                match eval.get_drv_path(&value) {
                    Ok(path) => {
                        status.finish_and_clear();
                        drv_path_str = Some(path);
                        successful_attr = Some(attr_path.join("."));
                        break;
                    }
                    Err(e) => {
                        status.finish_and_clear();
                        debug!(attr = %attr_path.join("."), error = %e, "candidate not a derivation");
                        last_error = Some(e);
                    }
                }
            }
            Err(e) => {
                status.finish_and_clear();
                debug!(attr = %attr_path.join("."), error = %e, "candidate not found");
                last_error = Some(e);
            }
        }
    }

    let drv_path_str = match drv_path_str {
        Some(path) => path,
        None => {
            return Err(last_error
                .unwrap_or_else(|| anyhow!("no devShell found"))
                .context("failed to evaluate devShell"));
        }
    };

    info!("evaluating {}#{}", flake_path.display(), successful_attr.unwrap_or_default());
    debug!(drv = %drv_path_str, "got derivation path");

    // Step 5: Enter the dev shell using nix-shell
    info!("entering dev shell");

    let mut cmd = Command::new("nix-shell");
    cmd.arg(&drv_path_str);

    // Set NIX_BUILD_SHELL to bash to avoid nix-shell trying to get bashInteractive from nixpkgs
    if env::var("NIX_BUILD_SHELL").is_err() {
        cmd.env("NIX_BUILD_SHELL", "bash");
    }

    // Add environment flags
    add_env_flags(&mut cmd, args);

    if let Some(ref command) = effective_command {
        cmd.args(["--command", command]);
    }

    debug!("+ nix-shell {}", drv_path_str);

    // exec replaces the current process
    let err = cmd.exec();
    Err(anyhow!("failed to exec nix-shell: {}", err))
}

/// Passthrough to nix develop for remote flake references
fn run_remote(args: &DevelopArgs, effective_command: &Option<String>) -> Result<()> {
    info!(
        "entering dev shell for {} (remote, delegating to nix)",
        args.installable
    );

    let mut cmd = Command::new("nix");
    cmd.arg("develop").arg(&args.installable);

    // Add environment flags (nix develop uses same flag names)
    if args.ignore_env {
        cmd.arg("--ignore-env");
    }

    for var in &args.keep_env_var {
        cmd.args(["--keep-env-var", var]);
    }

    for chunk in args.set_env_var.chunks(2) {
        if chunk.len() == 2 {
            cmd.args(["--set-env-var", &chunk[0], &chunk[1]]);
        }
    }

    for var in &args.unset_env_var {
        cmd.args(["--unset-env-var", var]);
    }

    if let Some(ref command) = effective_command {
        cmd.args(["--command", "sh", "-c", command]);
    }

    debug!("+ nix develop {}", args.installable);

    // exec replaces the current process
    let err = cmd.exec();
    Err(anyhow!("failed to exec nix develop: {}", err))
}
