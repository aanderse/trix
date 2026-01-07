use std::env;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, info, instrument, trace};

use crate::eval::Evaluator;
use crate::flake::{current_system, expand_attribute, format_attribute_not_found_error, resolve_installable_any, OperationContext};
use crate::progress;

#[derive(Args)]
pub struct BuildArgs {
    /// Flake reference to build (default: .#default)
    #[arg(default_value = ".#default")]
    pub installable: String,

    /// Build from a Nix file instead of a flake
    #[arg(short = 'f', long = "file", conflicts_with = "installable")]
    pub file: Option<PathBuf>,

    /// Attribute to build from the file (requires --file)
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

    /// Symlink name for build output
    #[arg(short = 'o', long, default_value = "result")]
    pub out_link: String,

    /// Don't create result symlink
    #[arg(long)]
    pub no_link: bool,

    /// Force rebuild, ignore cache
    #[arg(long)]
    pub rebuild: bool,
}

pub fn run(args: BuildArgs) -> Result<()> {
    // Dispatch to file mode or flake mode
    if let Some(ref file_path) = args.file {
        run_file_mode(&args, file_path)
    } else {
        run_flake_mode(&args)
    }
}

/// Build from a Nix file (non-flake mode)
#[instrument(level = "debug", skip_all, fields(file = %file_path.display(), attr = ?args.attr))]
fn run_file_mode(args: &BuildArgs, file_path: &PathBuf) -> Result<()> {
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

    // Build display path for logging
    let display_path = if let Some(ref attr) = args.attr {
        format!("{}#{}", file_path.display(), attr)
    } else {
        file_path.display().to_string()
    };

    info!("evaluating {}", display_path);

    // Build an expression that imports the file and applies arguments
    let base_expr = build_file_with_args_expr(&file_path, args)?;

    // If an attribute is specified, navigate to it
    let derivation_expr = if let Some(ref attr) = args.attr {
        format!("({}).{}", base_expr, attr)
    } else {
        base_expr
    };

    trace!("generated expression:\n{}", derivation_expr);

    // Initialize the evaluator
    let mut eval = Evaluator::new().context("failed to initialize evaluator")?;

    // Evaluate the expression
    let status = progress::evaluating(&display_path);
    let value = eval
        .eval_string(&derivation_expr, &display_path)
        .context("evaluation failed")?;
    status.finish_and_clear();

    // Get the .drv path for logging
    let drv_path_str = eval.get_drv_path(&value)?;
    debug!(drv = %drv_path_str, "got derivation path");

    // Build the derivation using native build
    info!("building {}", drv_path_str);
    let status = progress::building(&drv_path_str);
    let output_path = eval.build_value(&value).context("build failed")?;
    status.finish_and_clear();

    debug!(output = %output_path, "build completed");

    // Create result symlink
    create_result_link(&output_path, args)?;

    Ok(())
}

/// Build a Nix expression that imports a file and applies arguments to it
fn build_file_with_args_expr(file_path: &PathBuf, args: &BuildArgs) -> Result<String> {
    let file_path_str = file_path
        .to_str()
        .ok_or_else(|| anyhow!("invalid file path"))?;

    // Parse --arg pairs (name, expression)
    let mut arg_bindings = Vec::new();
    for chunk in args.arg.chunks(2) {
        if chunk.len() == 2 {
            let name = &chunk[0];
            let expr = &chunk[1];
            debug!(name = %name, expr = %expr, "applying --arg");
            // Validate name is a valid identifier
            if !is_valid_nix_identifier(name) {
                return Err(anyhow!("invalid argument name: {}", name));
            }
            arg_bindings.push(format!("{} = {};", name, expr));
        }
    }

    // Parse --argstr pairs (name, string value)
    for chunk in args.argstr.chunks(2) {
        if chunk.len() == 2 {
            let name = &chunk[0];
            let value = &chunk[1];
            debug!(name = %name, value = %value, "applying --argstr");
            // Validate name is a valid identifier
            if !is_valid_nix_identifier(name) {
                return Err(anyhow!("invalid argument name: {}", name));
            }
            // Escape the string value
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            arg_bindings.push(format!("{} = \"{}\";", name, escaped));
        }
    }

    let args_attrset = if arg_bindings.is_empty() {
        "{}".to_string()
    } else {
        format!("{{ {} }}", arg_bindings.join(" "))
    };

    // Generate expression that:
    // 1. Imports the file
    // 2. If it's a function, applies the arguments
    // 3. If not, just returns the value
    Ok(format!(
        r#"
let
  _file = import {file_path};
  _args = {args};
  _result = if builtins.isFunction _file then _file _args else _file;
in _result
"#,
        file_path = file_path_str,
        args = args_attrset,
    ))
}

/// Check if a string is a valid Nix identifier
fn is_valid_nix_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    // First char must be letter or underscore
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    // Rest can be alphanumeric, underscore, hyphen, or apostrophe
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '\'')
}

/// Build from a flake reference
#[instrument(level = "debug", skip_all, fields(installable = %args.installable))]
fn run_flake_mode(args: &BuildArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;

    // Step 1: Resolve the installable
    debug!("resolving installable");
    let resolved = resolve_installable_any(&args.installable, &cwd);

    // Step 2: Check if local or remote
    if !resolved.is_local {
        // Remote flake - passthrough to nix build
        return run_remote_build(args, &resolved);
    }

    let flake_path = resolved.path.as_ref().expect("local flake should have path");
    debug!(
        flake_path = %flake_path.display(),
        has_lock = resolved.lock.is_some(),
        "resolved flake"
    );

    // Step 3: Determine the candidate attribute paths
    let system = current_system()?;
    let candidates = expand_attribute(&resolved.attribute, OperationContext::Build, &system);
    debug!(?candidates, %system, "expanded attribute candidates");

    // Step 4: Initialize the evaluator and try each candidate
    let mut eval = Evaluator::new().context("failed to initialize evaluator")?;

    // Build flake URL for error messages
    let canonical = flake_path
        .canonicalize()
        .unwrap_or_else(|_| flake_path.clone());
    let is_git = git2::Repository::discover(flake_path).is_ok();
    let flake_url = if is_git {
        format!("git+file://{}", canonical.display())
    } else {
        format!("path:{}", canonical.display())
    };

    let (attr_path, value) = {
        let mut found = None;

        for candidate in &candidates {
            let eval_target = format!("{}#{}", flake_path.display(), candidate.join("."));
            trace!("trying {}", eval_target);

            match eval.eval_flake_attr(flake_path, candidate) {
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

        found.ok_or_else(|| {
            anyhow!(format_attribute_not_found_error(&flake_url, &candidates))
        })?
    };

    debug!(attr = %attr_path.join("."), "found attribute");

    // Get the .drv path for logging
    let drv_path_str = eval.get_drv_path(&value)?;
    debug!(drv = %drv_path_str, "got derivation path");

    // Step 5: Build the derivation using native build
    info!("building {}", drv_path_str);
    let status = progress::building(&drv_path_str);
    let output_path = eval.build_value(&value).context("build failed")?;
    status.finish_and_clear();

    debug!(output = %output_path, "build completed");

    // Create result symlink
    create_result_link(&output_path, args)?;

    Ok(())
}

/// Passthrough to nix build for remote flake references
fn run_remote_build(args: &BuildArgs, resolved: &crate::flake::ResolvedInstallable) -> Result<()> {
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

    info!("building {} (remote, delegating to nix)", full_ref);

    let mut cmd = Command::new("nix");
    cmd.arg("build").arg(&full_ref);

    if args.no_link {
        cmd.arg("--no-link");
    } else {
        cmd.args(["-o", &args.out_link]);
    }

    if args.rebuild {
        cmd.arg("--rebuild");
    }

    debug!("+ nix build {}", full_ref);
    let status = cmd.status().context("failed to run nix build")?;

    if !status.success() {
        return Err(anyhow!("nix build failed"));
    }

    Ok(())
}

/// Create the result symlink and print output
fn create_result_link(output_path: &str, args: &BuildArgs) -> Result<()> {
    if !args.no_link {
        // Remove existing symlink if present
        let link_path = std::path::Path::new(&args.out_link);
        if link_path.exists() || link_path.is_symlink() {
            trace!("removing existing symlink");
            std::fs::remove_file(link_path).context("failed to remove existing result symlink")?;
        }

        symlink(output_path, link_path).context("failed to create result symlink")?;

        println!("{}", output_path);
        info!("-> {}", args.out_link);
    } else {
        println!("{}", output_path);
    }

    Ok(())
}
