//! Eval command - evaluate Nix expressions.

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use nix_bindings_expr::value::ValueType;

use crate::cli::build::parse_override_inputs;
use crate::eval::Evaluator;
use crate::flake::{current_system, resolve_installable_any};
use crate::progress;

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

pub fn run(args: EvalArgs) -> Result<()> {
    // Validate: --arg and --argstr require --file or --expr
    let has_args = !args.arg.is_empty() || !args.argstr.is_empty();
    if has_args && args.file.is_none() && args.expr.is_none() {
        return Err(anyhow!(
            "--arg and --argstr require --file or --expr to be specified"
        ));
    }

    let status = progress::evaluating(&args.installable);
    let mut evaluator = Evaluator::new().context("failed to initialize Nix evaluator")?;

    // Parse override inputs
    let input_overrides = parse_override_inputs(&args.override_input);
    if !input_overrides.is_empty() {
        use tracing::debug;
        debug!(?input_overrides, "using input overrides");
    }

    // Build args expression if needed
    let args_expr = build_args_expr(&args.arg, &args.argstr)?;

    // Determine what to evaluate
    let value = if let Some(ref expr) = args.expr {
        // --expr: evaluate the expression directly
        // If args are provided, wrap in a let binding that applies them
        let full_expr = if let Some(ref args_expr) = args_expr {
            format!(
                "let _f = {}; _args = {}; in if builtins.isFunction _f then _f _args else _f",
                expr, args_expr
            )
        } else {
            expr.clone()
        };

        let base_value = evaluator.eval_string(&full_expr, "<cmdline>")?;

        if args.installable != "." && !args.installable.is_empty() {
            // Parse attribute path from installable
            let attr_path: Vec<String> = args
                .installable
                .split('.')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
            evaluator.navigate_attr_path(base_value, &attr_path)?
        } else {
            base_value
        }
    } else if let Some(ref file) = args.file {
        // --file: evaluate file and navigate to attribute path
        // If args are provided, apply them to the result if it's a function
        let file_path = file
            .to_str()
            .ok_or_else(|| anyhow!("invalid file path"))?;

        let full_expr = if let Some(ref args_expr) = args_expr {
            format!(
                "let _f = import {}; _args = {}; in if builtins.isFunction _f then _f _args else _f",
                file_path, args_expr
            )
        } else {
            format!("import {}", file_path)
        };

        let base_value = evaluator.eval_string(&full_expr, "<file>")?;

        if args.installable != "." && !args.installable.is_empty() {
            let attr_path: Vec<String> = args
                .installable
                .split('.')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
            evaluator.navigate_attr_path(base_value, &attr_path)?
        } else {
            base_value
        }
    } else {
        // Default: evaluate as installable (flake reference)
        eval_installable(&mut evaluator, &args.installable, &input_overrides)?
    };

    // Apply function if --apply is specified
    let value = if let Some(ref apply_expr) = args.apply {
        let func = evaluator.eval_string(apply_expr, "<apply>")?;
        evaluator.apply(func, value)?
    } else {
        value
    };

    status.finish_and_clear();

    // Output the result
    if args.json {
        let json_value = evaluator.value_to_json(&value)?;
        println!("{}", serde_json::to_string(&json_value)?);
    } else if args.raw {
        // Raw mode: print strings without quotes
        let vtype = evaluator.value_type(&value)?;
        match vtype {
            ValueType::String => {
                let s = evaluator.require_string(&value)?;
                print!("{}", s);
            }
            _ => {
                // For non-strings, use Nix format
                let s = evaluator.value_to_nix_string(&value)?;
                print!("{}", s);
            }
        }
    } else {
        // Default: Nix format
        let s = evaluator.value_to_nix_string(&value)?;
        println!("{}", s);
    }

    Ok(())
}

/// Build a Nix attrset expression from --arg and --argstr pairs.
fn build_args_expr(arg: &[String], argstr: &[String]) -> Result<Option<String>> {
    let mut bindings = Vec::new();

    // Parse --arg pairs (name, expression)
    for chunk in arg.chunks(2) {
        if chunk.len() == 2 {
            let name = &chunk[0];
            let expr = &chunk[1];
            if !is_valid_nix_identifier(name) {
                return Err(anyhow!("invalid argument name: {}", name));
            }
            bindings.push(format!("{} = {};", name, expr));
        }
    }

    // Parse --argstr pairs (name, string value)
    for chunk in argstr.chunks(2) {
        if chunk.len() == 2 {
            let name = &chunk[0];
            let value = &chunk[1];
            if !is_valid_nix_identifier(name) {
                return Err(anyhow!("invalid argument name: {}", name));
            }
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            bindings.push(format!("{} = \"{}\";", name, escaped));
        }
    }

    if bindings.is_empty() {
        Ok(None)
    } else {
        Ok(Some(format!("{{ {} }}", bindings.join(" "))))
    }
}

/// Check if a string is a valid Nix identifier.
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

/// Evaluate an installable (flake reference with optional attribute path).
fn eval_installable(
    evaluator: &mut Evaluator,
    installable: &str,
    input_overrides: &HashMap<String, String>,
) -> Result<crate::eval::NixValue> {
    use tracing::debug;

    let cwd = env::current_dir().context("failed to get current directory")?;
    let system = current_system()?;

    // Resolve the installable (handles local paths, registry names, and remote refs)
    let resolved = resolve_installable_any(installable, &cwd);

    if resolved.is_local {
        let flake_path = resolved
            .path
            .as_ref()
            .ok_or_else(|| anyhow!("local flake must have path"))?;

        // Expand the attribute path with system and fallbacks
        let candidates = expand_eval_attr_path(&resolved.attribute, &system);

        // Try each candidate until one works
        let mut last_err = None;
        for candidate in &candidates {
            let result = if input_overrides.is_empty() {
                evaluator.eval_flake_attr(flake_path, candidate)
            } else {
                evaluator.eval_flake_attr_with_overrides(flake_path, candidate, input_overrides)
            };
            match result {
                Ok(value) => return Ok(value),
                Err(e) => {
                    debug!("candidate {} failed: {}", candidate.join("."), e);
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow!("no candidates to try")))
    } else {
        // Remote flake - use native flake API
        // Build the full flake ref with attribute path for system expansion
        let flake_ref_base = resolved
            .flake_ref
            .as_deref()
            .unwrap_or(installable.split('#').next().unwrap_or(installable));

        // If there's an attribute path, try with system expansion
        if !resolved.attribute.is_empty() {
            let candidates = expand_eval_attr_path(&resolved.attribute, &system);

            let mut last_err = None;
            for candidate in &candidates {
                let full_ref = format!("{}#{}", flake_ref_base, candidate.join("."));
                debug!("trying remote flake ref: {}", full_ref);
                match evaluator.eval_flake_ref(&full_ref, &cwd) {
                    Ok(value) => return Ok(value),
                    Err(e) => {
                        debug!("candidate {} failed: {}", full_ref, e);
                        last_err = Some(e);
                    }
                }
            }

            Err(last_err.unwrap_or_else(|| anyhow!("no candidates to try")))
        } else {
            // No attribute path - evaluate the whole flake
            evaluator.eval_flake_ref(flake_ref_base, &cwd)
        }
    }
}

/// Expand an attribute path for eval, returning multiple candidates to try.
///
/// For paths that start with a known category (packages, devShells, etc.),
/// inserts the system if needed.
///
/// For paths that don't start with a known category (e.g., ["hello"] or ["rclone", "name"]),
/// tries packages.<system>.path, then legacyPackages.<system>.path, then the raw path.
fn expand_eval_attr_path(attr_path: &[String], system: &str) -> Vec<Vec<String>> {
    // Empty path - return as-is
    if attr_path.is_empty() {
        return vec![vec![]];
    }

    let first = &attr_path[0];

    // Check if first element is a known per-system category
    let per_system_categories = ["packages", "devShells", "apps", "checks", "legacyPackages", "formatter"];
    let is_per_system = per_system_categories.iter().any(|&c| c == first);

    // Check if first element is a known top-level category (no system needed)
    let top_level_categories = [
        "overlays", "nixosModules", "nixosConfigurations", "darwinModules",
        "darwinConfigurations", "homeModules", "homeConfigurations", "templates", "lib",
    ];
    let is_top_level = top_level_categories.iter().any(|&c| c == first);

    if is_top_level {
        // Top-level: return as-is, no system insertion
        return vec![attr_path.to_vec()];
    }

    if is_per_system {
        // Per-system category: insert system after category if not already present
        let looks_like_system = |s: &str| -> bool {
            matches!(s, "x86_64-linux" | "aarch64-linux" | "x86_64-darwin" | "aarch64-darwin" | "i686-linux" | "armv7l-linux")
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

    // Try raw path as fallback
    candidates.push(attr_path.to_vec());

    candidates
}
