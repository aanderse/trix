//! Flake show command - display flake outputs structure.
//!
//! For local flakes: uses `nix eval` to avoid store copy.
//! For remote flakes: delegates to `nix flake show`.

use std::env;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, info, instrument};

use crate::eval;
use crate::flake::resolve_installable_any;
use crate::lock;

#[derive(Args)]
pub struct ShowArgs {
    /// Flake reference (default: .)
    #[arg(default_value = ".")]
    pub flake_ref: String,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Show outputs for all systems
    #[arg(long)]
    pub all_systems: bool,

    /// Show the contents of the legacyPackages output
    #[arg(long)]
    pub legacy: bool,
}

#[instrument(level = "debug", skip_all, fields(flake_ref = %args.flake_ref, json = args.json))]
pub fn run(args: ShowArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;
    let resolved = resolve_installable_any(&args.flake_ref, &cwd);

    // For remote flakes, delegate to nix flake show
    if !resolved.is_local {
        debug!("delegating to nix flake show for remote flake");
        info!("showing {} (delegating to nix)", args.flake_ref);

        let installable_str = resolved.to_installable_string();
        let mut cmd = Command::new("nix");
        cmd.args(["--extra-experimental-features", "nix-command flakes"]);
        cmd.arg("flake").arg("show").arg(&installable_str);

        if args.json {
            cmd.arg("--json");
        }
        if args.all_systems {
            cmd.arg("--all-systems");
        }
        if args.legacy {
            cmd.arg("--legacy");
        }

        let status = cmd.status().context("failed to run nix flake show")?;
        if !status.success() {
            return Err(anyhow!(
                "nix flake show exited with code: {}",
                status.code().unwrap_or(1)
            ));
        }
        return Ok(());
    }

    // Local flake: evaluate natively without store copy
    let flake_path = resolved
        .path
        .as_ref()
        .ok_or_else(|| anyhow!("local flake must have path"))?;

    // Read flake.lock (missing lock is OK for flakes with no inputs)
    let flake_lock = lock::read_flake_lock(flake_path)
        .unwrap_or_else(|_| lock::FlakeLock::empty());

    // Evaluate outputs structure
    let outputs_json = eval::eval_flake_show_json(flake_path, &flake_lock, args.all_systems, args.legacy)?;

    if args.json {
        // JSON mode: print the raw JSON
        println!("{}", serde_json::to_string(&outputs_json)
            .context("failed to serialize JSON")?);
    } else {
        // Tree mode: print the tree structure like nix flake show
        let canonical = flake_path
            .canonicalize()
            .unwrap_or_else(|_| flake_path.clone());

        // Detect git repo
        let is_git = flake_path.join(".git").exists()
            || Command::new("git")
                .args(["-C", &flake_path.display().to_string(), "rev-parse", "--git-dir"])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

        if is_git {
            println!("\x1b[1mgit+file://{}\x1b[0m", canonical.display());
        } else {
            println!("\x1b[1mpath:{}\x1b[0m", canonical.display());
        }

        print_flake_outputs(&outputs_json, "")?;
    }

    Ok(())
}

/// Print flake outputs as a tree (matches nix flake show format).
fn print_flake_outputs(outputs: &serde_json::Value, prefix: &str) -> Result<()> {
    if let Some(obj) = outputs.as_object() {
        // Filter out empty entries
        let displayable_keys: Vec<_> = obj
            .keys()
            .filter(|k| has_displayable_content(&obj[*k]))
            .collect();
        let len = displayable_keys.len();

        for (i, key) in displayable_keys.iter().enumerate() {
            let is_last = i == len - 1;
            let connector = if is_last {
                "\x1b[32;1m└───\x1b[0m"
            } else {
                "\x1b[32;1m├───\x1b[0m"
            };
            let child_prefix = if is_last {
                format!("{}    ", prefix)
            } else {
                format!("{}\x1b[32;1m│\x1b[0m   ", prefix)
            };

            let value = &obj[*key];

            if let Some(inner) = value.as_object() {
                if let Some(type_val) = inner.get("type").and_then(|v| v.as_str()) {
                    // Leaf node with type info
                    let description = format_output_type(type_val, inner);
                    println!("{}{}\x1b[1m{}\x1b[0m: {}", prefix, connector, key, description);
                } else if inner.is_empty() {
                    // Empty object = omitted
                    // Don't print anything for empty objects at leaf level
                } else {
                    // Nested structure
                    println!("{}{}\x1b[1m{}\x1b[0m", prefix, connector, key);
                    print_flake_outputs(value, &child_prefix)?;
                }
            } else {
                println!("{}{}\x1b[1m{}\x1b[0m", prefix, connector, key);
            }
        }
    }

    Ok(())
}

/// Check if a value has any displayable content.
fn has_displayable_content(value: &serde_json::Value) -> bool {
    if let Some(obj) = value.as_object() {
        if obj.is_empty() {
            return false;
        }
        // Has type = it's a displayable leaf
        if obj.contains_key("type") {
            return true;
        }
        // Check children recursively
        obj.values().any(has_displayable_content)
    } else {
        true
    }
}

/// Format an output type for display.
fn format_output_type(type_val: &str, info: &serde_json::Map<String, serde_json::Value>) -> String {
    match type_val {
        "derivation" => {
            let name = info.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
            // Determine display based on parent context
            format!("package '{}'", name)
        }
        "app" => "app".to_string(),
        "nixpkgs-overlay" => "\x1b[35;1mNixpkgs overlay\x1b[0m".to_string(),
        "nixos-module" => "\x1b[35;1mNixOS module\x1b[0m".to_string(),
        "nixos-configuration" => "NixOS configuration".to_string(),
        "template" => {
            let desc = info.get("description").and_then(|v| v.as_str());
            if let Some(d) = desc {
                if d.is_empty() {
                    "template".to_string()
                } else {
                    format!("template: {}", d)
                }
            } else {
                "template".to_string()
            }
        }
        _ => type_val.to_string(),
    }
}
