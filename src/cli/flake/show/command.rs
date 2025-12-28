use super::common::{bold, magenta_bold};
use crate::flake::{ensure_lock, resolve_installable};
use crate::nix::eval_flake_outputs;
use anyhow::{Context, Result};

/// Show flake outputs structure
pub fn cmd_show(flake_ref: Option<&str>, all_systems: bool, legacy: bool) -> Result<()> {
    let flake_ref = flake_ref.unwrap_or(".");
    let resolved = resolve_installable(flake_ref);

    if !resolved.is_local {
        // Passthrough to nix flake show
        let full_ref = resolved.flake_ref.as_deref().unwrap_or(flake_ref);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.args(["flake", "show", full_ref]);

        if all_systems {
            cmd.arg("--all-systems");
        }

        if legacy {
            cmd.arg("--legacy");
        }

        return cmd.run();
    }

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;

    // Ensure lock exists
    ensure_lock(flake_dir, None)?;

    // Print flake URL header (bold, like nix)
    let canonical_path = flake_dir
        .canonicalize()
        .unwrap_or_else(|_| flake_dir.to_path_buf());
    // Check if this is a git repo
    let is_git = flake_dir.join(".git").exists()
        || std::process::Command::new("git")
            .args([
                "-C",
                &flake_dir.display().to_string(),
                "rev-parse",
                "--git-dir",
            ])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
    if is_git {
        println!("\x1b[1mgit+file://{}\x1b[0m", canonical_path.display());
    } else {
        println!("\x1b[1mpath:{}\x1b[0m", canonical_path.display());
    }

    // Get outputs structure
    let outputs = eval_flake_outputs(flake_dir, all_systems, legacy)?;

    if let Some(outputs) = outputs {
        print_flake_outputs(&outputs, "")?;
    } else {
        anyhow::bail!("Failed to evaluate flake outputs");
    }

    Ok(())
}

/// Check if a value has any displayable content (not empty at all levels)
fn has_displayable_content(value: &serde_json::Value) -> bool {
    if let Some(obj) = value.as_object() {
        // Check for special markers - these are displayable
        if obj.contains_key("_omitted")
            || obj.contains_key("_legacyOmitted")
            || obj.contains_key("_unknown")
            || obj.contains_key("_type")
        {
            return true;
        }
        // Empty object is not displayable
        if obj.is_empty() {
            return false;
        }
        // Object with null values (leaf nodes) is displayable
        if obj.values().all(|v| v.is_null()) {
            return !obj.is_empty();
        }
        // Otherwise, check if any children have displayable content
        obj.values().any(has_displayable_content)
    } else if value.is_null() {
        // Null is a leaf (displayable)
        true
    } else {
        true
    }
}

fn print_flake_outputs(outputs: &serde_json::Value, prefix: &str) -> Result<()> {
    if let Some(obj) = outputs.as_object() {
        // Filter keys to only those with displayable content
        let displayable_keys: Vec<_> = obj
            .keys()
            .filter(|k| has_displayable_content(&obj[*k]))
            .collect();
        let len = displayable_keys.len();

        for (i, key) in displayable_keys.iter().enumerate() {
            let is_last = i == len - 1;
            // Green+bold for tree characters (matches nix)
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
            // Note: The escape codes add visual spacing but the actual spacing matches nix

            let value = &obj[*key];

            if let Some(inner) = value.as_object() {
                // Check for special markers
                if inner.contains_key("_omitted") {
                    // Magenta+bold for "omitted" (matches nix)
                    println!(
                        "{}{}{} \x1b[35;1momitted\x1b[0m (use '--all-systems' to show)",
                        prefix,
                        connector,
                        bold(key)
                    );
                } else if inner.contains_key("_legacyOmitted") {
                    println!(
                        "{}{}{} \x1b[35;1momitted\x1b[0m (use '--legacy' to show)",
                        prefix,
                        connector,
                        bold(key)
                    );
                } else if inner.contains_key("_unknown") {
                    println!("{}{}{}: unknown", prefix, connector, bold(key));
                } else if inner.contains_key("_type") {
                    let description = format_output_description(inner);
                    println!("{}{}{}: {}", prefix, connector, bold(key), description);
                } else if inner.values().all(|v| v.is_null()) {
                    // Object with all null values = leaf nodes that should be printed
                    // This is how legacyPackages.x86_64-linux looks: {"cargo-clippy": null, ...}
                    println!("{}{}{}", prefix, connector, bold(key));
                    // Recurse to print each null child as a leaf
                    print_flake_outputs(value, &child_prefix)?;
                } else {
                    // Nested structure with non-null children
                    println!("{}{}{}", prefix, connector, bold(key));
                    print_flake_outputs(value, &child_prefix)?;
                }
            } else if value.is_null() {
                // Leaf
                println!("{}{}{}", prefix, connector, bold(key));
            } else {
                println!("{}{}{}", prefix, connector, bold(key));
            }
        }
    }

    Ok(())
}

/// Format a description for a flake output based on its type, name, and category
fn format_output_description(info: &serde_json::Map<String, serde_json::Value>) -> String {
    let type_val = info
        .get("_type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let name_val = info.get("_name").and_then(|v| v.as_str());
    let category = info.get("_category").and_then(|v| v.as_str());

    match type_val {
        "derivation" => {
            if let Some(name) = name_val {
                // Use category to determine display format (matching nix flake show output)
                match category {
                    Some("devShells") => format!("development environment '{}'", name),
                    Some("packages") => format!("package '{}'", name),
                    // checks, hydraJobs, and other categories use "derivation"
                    _ => format!("derivation '{}'", name),
                }
            } else {
                "derivation".to_string()
            }
        }
        "app" => "app".to_string(),
        "formatter" => {
            // nix uses "package" for formatter, not "formatter:"
            if let Some(name) = name_val {
                format!("package '{}'", name)
            } else {
                "package".to_string()
            }
        }
        "overlay" => magenta_bold("Nixpkgs overlay"),
        "module" => magenta_bold("NixOS module"),
        "template" => "template".to_string(),
        "configuration" => "NixOS configuration".to_string(),
        _ => type_val.to_string(),
    }
}
