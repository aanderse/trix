//! Eval command - evaluate Nix expressions.
//!
//! For local flakes, evaluates WITHOUT copying to store (core value proposition).
//! For remote flakes and --expr/--file modes, delegates to `nix eval`.

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, instrument};

use crate::cli::build::parse_override_inputs;
use crate::eval;
use crate::flake::{current_system, resolve_installable_any};
use crate::lock;

#[derive(Args)]
pub struct EvalArgs {
    /// Installable to evaluate (e.g., '.#packages.x86_64-linux.default')
    #[arg(default_value = ".")]
    pub installable: String,

    /// Accepted for nix CLI compatibility (trix is always impure)
    #[arg(long, hide = true)]
    pub impure: bool,

    /// Interpret installable as attribute path relative to the Nix expression
    #[arg(long, value_name = "EXPR", allow_hyphen_values = true)]
    pub expr: Option<String>,

    /// Interpret installable as attribute path relative to the expression stored in file
    #[arg(long, short = 'f', value_name = "FILE")]
    pub file: Option<PathBuf>,

    /// Pass a Nix expression as argument (requires --file or --expr)
    /// Usage: --arg name 'expression'
    #[arg(long = "arg", num_args = 2, value_names = ["NAME", "EXPR"], action = clap::ArgAction::Append)]
    pub arg: Vec<String>,

    /// Pass a string as argument (requires --file or --expr)
    /// Usage: --argstr name 'value'
    #[arg(long = "argstr", num_args = 2, value_names = ["NAME", "VALUE"], action = clap::ArgAction::Append)]
    pub argstr: Vec<String>,

    /// Override a flake input with a local path (avoids store copy for the override)
    /// Usage: --override-input nixpkgs ~/nixpkgs
    #[arg(long = "override-input", num_args = 2, value_names = ["INPUT", "PATH"], action = clap::ArgAction::Append)]
    pub override_input: Vec<String>,

    /// Produce output in JSON format
    #[arg(long)]
    pub json: bool,

    /// Print strings without quotes (raw output)
    #[arg(long)]
    pub raw: bool,

    /// Apply the function to the result
    #[arg(long, value_name = "EXPR")]
    pub apply: Option<String>,
}

#[instrument(level = "debug", skip(args))]
pub fn run(args: EvalArgs) -> Result<()> {
    // Validate: --arg and --argstr require --file or --expr
    let has_args = !args.arg.is_empty() || !args.argstr.is_empty();
    if has_args && args.file.is_none() && args.expr.is_none() {
        return Err(anyhow!(
            "--arg and --argstr require --file or --expr to be specified"
        ));
    }

    // For --expr and --file modes, delegate to nix eval (no flakes involved)
    if args.expr.is_some() || args.file.is_some() {
        return run_nix_eval_delegation(&args);
    }

    // Default mode: evaluate flake reference
    // Check if it's local or remote
    let cwd = env::current_dir().context("failed to get current directory")?;
    let resolved = resolve_installable_any(&args.installable, &cwd);

    if resolved.is_local {
        // Local flake - evaluate without copying to store
        debug!("evaluating local flake without store copy");
        run_local_flake_eval(&args, &resolved)
    } else {
        // Remote flake - delegate to nix eval
        debug!("delegating remote flake to nix eval");
        run_nix_eval_delegation(&args)
    }
}

/// Evaluate a local flake without copying to store.
fn run_local_flake_eval(
    args: &EvalArgs,
    resolved: &crate::flake::ResolvedInstallable,
) -> Result<()> {
    let flake_path = resolved
        .path
        .as_ref()
        .ok_or_else(|| anyhow!("local flake must have path"))?;

    // Read flake.lock - if missing, use empty lock (flake may have no inputs)
    let lock = match lock::read_flake_lock(flake_path) {
        Ok(l) => l,
        Err(_) => {
            debug!("no flake.lock found, using empty lock");
            lock::FlakeLock::empty()
        }
    };

    // Parse override inputs
    let input_overrides = parse_override_inputs(&args.override_input);

    // Expand attribute path with system
    let system = current_system()?;
    let candidates = expand_eval_attr_path(&resolved.attribute, &system);
    debug!(?candidates, "expanded attribute candidates");

    // Try each candidate until one succeeds
    let mut last_result = None;
    for candidate in &candidates {
        debug!("trying candidate: {}", candidate.join("."));

        // Generate expression that evaluates the flake attribute
        let flake_dir = flake_path
            .to_str()
            .ok_or_else(|| anyhow!("invalid flake path"))?;

        // Prefetch inputs
        let store_paths = if input_overrides.is_empty() {
            eval::prefetch_all_inputs(&lock)?
        } else {
            let nodes_to_fetch: Vec<_> = lock
                .nodes
                .iter()
                .filter(|(name, _)| *name != &lock.root && !input_overrides.contains_key(*name))
                .collect();

            let mut paths = HashMap::new();
            for (name, node) in nodes_to_fetch {
                if let Some(ref locked) = node.locked {
                    let store_path = eval::prefetch_input(name, locked)?;
                    paths.insert(name.clone(), store_path);
                }
            }
            paths
        };

        // Generate expression
        let expr = eval::generate_flake_eval_expr(
            flake_dir,
            &lock,
            candidate,
            &input_overrides,
            &store_paths,
        )?;

        // Apply function if --apply specified
        let final_expr = if let Some(ref apply_expr) = args.apply {
            format!("({}) ({})", apply_expr, expr)
        } else {
            expr
        };

        // Evaluate the expression
        let result = if args.json {
            eval::eval_to_json(&final_expr)
        } else {
            // For non-JSON, use nix eval for proper Nix formatting
            let nix_args = if args.raw {
                vec!["eval", "--raw", "--impure", "--expr", &final_expr]
            } else {
                vec!["eval", "--impure", "--expr", &final_expr]
            };

            let output = Command::new("nix")
                .args(&nix_args)
                .output()
                .context("failed to execute nix eval")?;

            if output.status.success() {
                let result = String::from_utf8_lossy(&output.stdout).to_string();
                Ok(serde_json::Value::String(result))
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(anyhow!("evaluation failed: {}", stderr.trim()))
            }
        };

        match result {
            Ok(json_value) => {
                // Output the result
                if args.json {
                    println!("{}", serde_json::to_string(&json_value)?);
                } else if let serde_json::Value::String(s) = json_value {
                    // Already formatted by nix eval
                    print!("{}", s);
                } else {
                    println!("{}", serde_json::to_string(&json_value)?);
                }
                return Ok(());
            }
            Err(e) => {
                debug!("candidate {} failed: {}", candidate.join("."), e);
                last_result = Some(Err(e));
            }
        }
    }

    // All candidates failed
    last_result.unwrap_or_else(|| Err(anyhow!("no valid attribute path found")))
}

/// Delegate to nix eval for --expr, --file, or remote flakes.
fn run_nix_eval_delegation(args: &EvalArgs) -> Result<()> {
    let mut cmd = Command::new("nix");
    cmd.args(["--extra-experimental-features", "nix-command flakes"]);
    cmd.arg("eval");

    // Handle --expr mode
    if let Some(ref expr) = args.expr {
        cmd.arg("--expr").arg(expr);
        if args.installable != "." && !args.installable.is_empty() {
            // Navigate to attribute path
            let attr_path = format!(".{}", args.installable);
            cmd.arg("--apply").arg(format!("x: x{}", attr_path));
        }
    } else if let Some(ref file) = args.file {
        // Handle --file mode
        cmd.arg("--file").arg(file);
        if args.installable != "." && !args.installable.is_empty() {
            cmd.arg(&args.installable);
        }
    } else {
        // Default: flake reference (remote)
        cmd.arg(&args.installable);
    }

    // Add --arg and --argstr pairs
    for chunk in args.arg.chunks(2) {
        if chunk.len() == 2 {
            cmd.arg("--arg").arg(&chunk[0]).arg(&chunk[1]);
        }
    }

    for chunk in args.argstr.chunks(2) {
        if chunk.len() == 2 {
            cmd.arg("--argstr").arg(&chunk[0]).arg(&chunk[1]);
        }
    }

    // Add --override-input pairs
    for chunk in args.override_input.chunks(2) {
        if chunk.len() == 2 {
            cmd.arg("--override-input").arg(&chunk[0]).arg(&chunk[1]);
        }
    }

    // Add format flags
    if args.json {
        cmd.arg("--json");
    }

    if args.raw {
        cmd.arg("--raw");
    }

    // Add --apply if specified
    if let Some(ref apply_expr) = args.apply {
        cmd.arg("--apply").arg(apply_expr);
    }

    // Always use --impure for compatibility
    cmd.arg("--impure");

    let status = cmd.status().context("failed to run nix eval")?;

    if !status.success() {
        anyhow::bail!("nix eval failed");
    }

    Ok(())
}

/// Expand an attribute path for eval, returning multiple candidates to try.
///
/// For paths that start with a known category (packages, devShells, etc.),
/// inserts the system if needed.
///
/// For paths that don't start with a known category (e.g., ["hello"] or ["lib", "testValue"]),
/// tries packages.<system>.path, then legacyPackages.<system>.path, then the raw path.
fn expand_eval_attr_path(attr_path: &[String], system: &str) -> Vec<Vec<String>> {
    // Empty path - return as-is
    if attr_path.is_empty() {
        return vec![vec![]];
    }

    let first = &attr_path[0];

    // Check if first element is a known per-system category
    let per_system_categories = [
        "packages",
        "devShells",
        "apps",
        "checks",
        "legacyPackages",
        "formatter",
    ];
    let is_per_system = per_system_categories.iter().any(|&c| c == first);

    // Check if first element is a known top-level category (no system needed)
    let top_level_categories = [
        "overlays",
        "nixosModules",
        "nixosConfigurations",
        "darwinModules",
        "darwinConfigurations",
        "homeModules",
        "homeConfigurations",
        "templates",
        "lib",
    ];
    let is_top_level = top_level_categories.iter().any(|&c| c == first);

    if is_top_level {
        // Top-level: return as-is, no system insertion
        return vec![attr_path.to_vec()];
    }

    if is_per_system {
        // Per-system category: insert system after category if not already present
        let looks_like_system = |s: &str| -> bool {
            matches!(
                s,
                "x86_64-linux"
                    | "aarch64-linux"
                    | "x86_64-darwin"
                    | "aarch64-darwin"
                    | "i686-linux"
                    | "armv7l-linux"
            )
        };

        if attr_path.len() >= 2 && looks_like_system(&attr_path[1]) {
            // Already has system
            return vec![attr_path.to_vec()];
        } else {
            // Insert system after category
            let mut result = vec![first.clone(), system.to_string()];
            result.extend(attr_path[1..].iter().cloned());
            return vec![result];
        }
    }

    // Unknown first element - try packages, legacyPackages, then raw path
    let mut candidates = Vec::new();

    // Try packages.<system>.<path>
    let mut pkg_path = vec!["packages".to_string(), system.to_string()];
    pkg_path.extend(attr_path.iter().cloned());
    candidates.push(pkg_path);

    // Try legacyPackages.<system>.<path>
    let mut legacy_path = vec!["legacyPackages".to_string(), system.to_string()];
    legacy_path.extend(attr_path.iter().cloned());
    candidates.push(legacy_path);

    // Try raw path as fallback (important for things like lib.testValue)
    candidates.push(attr_path.to_vec());

    candidates
}
