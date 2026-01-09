//! Flake check command - check whether the flake evaluates and run its tests.
//!
//! This matches nix flake check behavior:
//! - Evaluates all flake outputs to verify they're valid
//! - Only builds checks.* by default (use --no-build to skip)
//! - Warns about omitted systems

use std::collections::HashSet;
use std::env;
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use owo_colors::{OwoColorize, Stream::Stdout};
use tracing::{debug, instrument, warn};

use crate::eval::Evaluator;
use crate::flake::{current_system, resolve_installable};

/// Per-system output types that must be derivations
const PER_SYSTEM_DERIVATION_OUTPUTS: &[&str] = &[
    "checks",
    "packages",
    "devShells",
    // Deprecated but still supported
    "defaultPackage",
    "devShell",
];

/// Per-system output types that must be app definitions
const PER_SYSTEM_APP_OUTPUTS: &[&str] = &[
    "apps",
    // Deprecated
    "defaultApp",
];

/// Top-level outputs (not per-system)
const TOP_LEVEL_OUTPUTS: &[&str] = &[
    "overlays",
    "nixosModules",
    "templates",
    // Deprecated
    "overlay",
    "nixosModule",
    "defaultTemplate",
];

#[derive(Args)]
pub struct CheckArgs {
    /// Flake reference (default: .)
    #[arg(default_value = ".")]
    pub flake_ref: String,

    /// Run checks for all systems
    #[arg(long)]
    pub all_systems: bool,

    /// Do not build checks
    #[arg(long)]
    pub no_build: bool,

    /// Accepted for nix CLI compatibility (trix is always impure)
    #[arg(long, hide = true)]
    pub impure: bool,
}

#[instrument(level = "debug", skip_all, fields(flake_ref = %args.flake_ref))]
pub fn run(args: CheckArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;

    // Resolve the flake reference
    let resolved = resolve_installable(&args.flake_ref, &cwd)?;

    // nix flake check doesn't accept attributes - reject them
    if !resolved.attribute.is_empty() {
        return Err(anyhow!(
            "unexpected fragment '{}' in flake reference '{}'",
            resolved.attribute.join("."),
            args.flake_ref
        ));
    }

    // Check if this is a local flake
    if resolved.lock.is_none() {
        // Not a local flake - pass through to nix flake check
        debug!("passing through to nix flake check for non-local flake");
        let mut cmd = Command::new("nix");
        cmd.args(["flake", "check", &args.flake_ref]);
        if args.all_systems {
            cmd.arg("--all-systems");
        }
        if args.no_build {
            cmd.arg("--no-build");
        }
        let status = cmd.status().context("failed to run nix flake check")?;

        if !status.success() {
            return Err(anyhow!("nix flake check failed"));
        }
        return Ok(());
    }

    let flake_path = resolved.path.clone();
    let current_sys = current_system()?;

    eprintln!("evaluating flake...");

    let mut eval = Evaluator::new().context("failed to initialize evaluator")?;

    // Track all systems we find and which we check
    let mut all_systems_found: HashSet<String> = HashSet::new();
    let mut systems_checked: HashSet<String> = HashSet::new();

    // Collect checks to build later
    let mut checks_to_build: Vec<(String, String, String)> = Vec::new(); // (system, name, drv_path)

    let mut had_errors = false;

    // Check per-system derivation outputs
    for &output_type in PER_SYSTEM_DERIVATION_OUTPUTS {
        // Get systems that have this output (without showing progress yet)
        let systems = match eval.eval_flake_attr_names(&flake_path, &[output_type]) {
            Ok(s) => s,
            Err(_) => {
                // Output doesn't exist, skip
                continue;
            }
        };

        if systems.is_empty() {
            continue;
        }

        eprintln!("checking flake output '{}'...", output_type);

        for system in &systems {
            all_systems_found.insert(system.clone());

            // Skip systems if not --all-systems and not current system
            if !args.all_systems && system != &current_sys {
                continue;
            }

            systems_checked.insert(system.clone());

            // Handle deprecated singular outputs (defaultPackage, devShell)
            if output_type == "defaultPackage" || output_type == "devShell" {
                // These are single derivations, not attrsets
                let attr_path = vec![output_type.to_string(), system.clone()];
                match check_derivation(&mut eval, &flake_path, &attr_path) {
                    Ok(drv_path) => {
                        eprintln!("checking derivation {}.{}...", output_type, system);
                        eprintln!("derivation evaluated to {}", drv_path);
                    }
                    Err(e) => {
                        eprintln!(
                            "{} checking {}.{}: {}",
                            "error:".if_supports_color(Stdout, |t| t.red()),
                            output_type,
                            system,
                            e
                        );
                        had_errors = true;
                    }
                }
                continue;
            }

            // Get derivation names for this system
            let names = match eval.eval_flake_attr_names(&flake_path, &[output_type, system]) {
                Ok(n) => n,
                Err(e) => {
                    warn!("failed to get {}.{} names: {}", output_type, system, e);
                    continue;
                }
            };

            for name in names {
                let attr_path = vec![output_type.to_string(), system.clone(), name.clone()];
                match check_derivation(&mut eval, &flake_path, &attr_path) {
                    Ok(drv_path) => {
                        eprintln!(
                            "checking derivation {}.{}.{}...",
                            output_type, system, name
                        );
                        eprintln!("derivation evaluated to {}", drv_path);

                        // Collect checks for building
                        if output_type == "checks" {
                            checks_to_build.push((system.clone(), name, drv_path));
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "{} checking {}.{}.{}: {}",
                            "error:".if_supports_color(Stdout, |t| t.red()),
                            output_type,
                            system,
                            name,
                            e
                        );
                        had_errors = true;
                    }
                }
            }
        }
    }

    // Check per-system app outputs
    for &output_type in PER_SYSTEM_APP_OUTPUTS {
        let systems = match eval.eval_flake_attr_names(&flake_path, &[output_type]) {
            Ok(s) => s,
            Err(_) => continue,
        };

        if systems.is_empty() {
            continue;
        }

        eprintln!("checking flake output '{}'...", output_type);

        for system in &systems {
            all_systems_found.insert(system.clone());

            if !args.all_systems && system != &current_sys {
                continue;
            }

            systems_checked.insert(system.clone());

            // Handle deprecated singular output (defaultApp)
            if output_type == "defaultApp" {
                let attr_path = vec![output_type.to_string(), system.clone()];
                match check_app(&mut eval, &flake_path, &attr_path) {
                    Ok(()) => {
                        debug!("app {}.{} is valid", output_type, system);
                    }
                    Err(e) => {
                        eprintln!(
                            "{} checking {}.{}: {}",
                            "error:".if_supports_color(Stdout, |t| t.red()),
                            output_type,
                            system,
                            e
                        );
                        had_errors = true;
                    }
                }
                continue;
            }

            let names = match eval.eval_flake_attr_names(&flake_path, &[output_type, system]) {
                Ok(n) => n,
                Err(_) => continue,
            };

            for name in names {
                let attr_path = vec![output_type.to_string(), system.clone(), name.clone()];
                match check_app(&mut eval, &flake_path, &attr_path) {
                    Ok(()) => {
                        debug!("app {}.{}.{} is valid", output_type, system, name);
                    }
                    Err(e) => {
                        eprintln!(
                            "{} checking {}.{}.{}: {}",
                            "error:".if_supports_color(Stdout, |t| t.red()),
                            output_type,
                            system,
                            name,
                            e
                        );
                        had_errors = true;
                    }
                }
            }
        }
    }

    // Check top-level outputs (overlays, nixosModules, templates)
    for &output_type in TOP_LEVEL_OUTPUTS {
        // Just try to evaluate to verify it exists and is well-formed
        let attr_path: Vec<String> = vec![output_type.to_string()];
        if eval.eval_flake_attr(&flake_path, &attr_path).is_ok() {
            debug!("top-level output '{}' exists", output_type);
        }
    }

    // Build checks if not --no-build
    if !args.no_build && !checks_to_build.is_empty() {
        eprintln!("building {} check(s)...", checks_to_build.len());

        let drv_paths: Vec<&str> = checks_to_build.iter().map(|(_, _, p)| p.as_str()).collect();

        match build_derivations_batch(&drv_paths) {
            Ok(()) => {
                // All builds succeeded
                for (system, name, _) in &checks_to_build {
                    println!(
                        "{} checks.{}.{}",
                        "✓".if_supports_color(Stdout, |t| t.green()),
                        system,
                        name
                    );
                }
            }
            Err(_) => {
                // Some builds failed - run individually
                for (system, name, drv_path) in &checks_to_build {
                    match build_single_derivation(drv_path) {
                        Ok(()) => {
                            println!(
                                "{} checks.{}.{}",
                                "✓".if_supports_color(Stdout, |t| t.green()),
                                system,
                                name
                            );
                        }
                        Err(e) => {
                            println!(
                                "{} checks.{}.{}: {}",
                                "✗".if_supports_color(Stdout, |t| t.red()),
                                system,
                                name,
                                e
                            );
                            had_errors = true;
                        }
                    }
                }
            }
        }
    }

    // Warn about omitted systems
    if !args.all_systems {
        let omitted: Vec<_> = all_systems_found
            .difference(&systems_checked)
            .cloned()
            .collect();

        if !omitted.is_empty() {
            let mut sorted = omitted;
            sorted.sort();
            eprintln!(
                "{} The check omitted these incompatible systems: {}",
                "warning:".if_supports_color(Stdout, |t| t.yellow()),
                sorted.join(", ")
            );
            eprintln!("Use '--all-systems' to check all.");
        }
    }

    if had_errors {
        Err(anyhow!("some checks failed"))
    } else {
        Ok(())
    }
}

/// Check that an attribute path evaluates to a valid derivation.
/// Returns the .drv path on success.
fn check_derivation(eval: &mut Evaluator, flake_path: &Path, attr_path: &[String]) -> Result<String> {
    let value = eval
        .eval_flake_attr(flake_path, attr_path)
        .context("evaluation failed")?;

    eval.get_drv_path(&value)
}

/// Check that an attribute path evaluates to a valid app definition.
/// An app must have a `type = "app"` attribute and a `program` attribute.
fn check_app(eval: &mut Evaluator, flake_path: &Path, attr_path: &[String]) -> Result<()> {
    let value = eval
        .eval_flake_attr(flake_path, attr_path)
        .context("evaluation failed")?;

    // Check it's an attrset with type = "app"
    let type_attr = {
        let mut path = attr_path.to_vec();
        path.push("type".to_string());
        eval.eval_flake_attr(flake_path, &path)
    };

    match type_attr {
        Ok(type_val) => {
            let type_str = eval.require_string(&type_val)?;
            if type_str != "app" {
                return Err(anyhow!("app type must be 'app', got '{}'", type_str));
            }
        }
        Err(_) => {
            // type attribute missing - might be a derivation (legacy)
            // Try to get drv path - if it works, it's a valid legacy app
            if eval.get_drv_path(&value).is_ok() {
                return Ok(());
            }
            return Err(anyhow!("app must have 'type' attribute or be a derivation"));
        }
    }

    // Check program attribute exists
    let mut program_path = attr_path.to_vec();
    program_path.push("program".to_string());
    eval.eval_flake_attr(flake_path, &program_path)
        .context("app must have 'program' attribute")?;

    Ok(())
}

/// Build multiple derivations in a single nix-store call.
fn build_derivations_batch(drv_paths: &[&str]) -> Result<()> {
    if drv_paths.is_empty() {
        return Ok(());
    }

    debug!("building {} derivations in batch", drv_paths.len());

    let mut cmd = Command::new("nix-store");
    cmd.arg("--realise");
    cmd.args(drv_paths);

    let output = cmd.output().context("failed to run nix-store --realise")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("batch build failed: {}", stderr.trim()));
    }

    Ok(())
}

/// Build a single derivation.
fn build_single_derivation(drv_path: &str) -> Result<()> {
    debug!("building derivation: {}", drv_path);

    let output = Command::new("nix-store")
        .args(["--realise", drv_path])
        .output()
        .context("failed to run nix-store --realise")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("build failed: {}", stderr.trim()));
    }

    Ok(())
}
