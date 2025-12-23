//! Flake subcommands.

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::flake::{ensure_lock, get_flake_description, get_flake_inputs, resolve_installable};
use crate::lock::{sync_inputs, update_lock};
use crate::nix::{eval_flake_outputs, get_system};

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
    ensure_lock(flake_dir)?;

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

/// Show flake metadata and inputs
pub fn cmd_metadata(flake_ref: Option<&str>) -> Result<()> {
    use chrono::{DateTime, Local};
    use std::os::unix::fs::MetadataExt;

    let flake_ref = flake_ref.unwrap_or(".");
    let resolved = resolve_installable(flake_ref);

    if !resolved.is_local {
        // Passthrough to nix flake metadata
        let full_ref = resolved.flake_ref.as_deref().unwrap_or(flake_ref);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.args(["flake", "metadata", full_ref]);

        return cmd.run();
    }

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;
    let flake_nix = flake_dir.join("flake.nix");

    if !flake_nix.exists() {
        anyhow::bail!("No flake.nix found in {}", flake_dir.display());
    }

    // Show description
    if let Some(desc) = get_flake_description(flake_dir) {
        println!("{} {}", bold("Description:"), desc);
    }

    println!("{} {}", bold("Path:"), flake_dir.display());

    // Show last modified from flake.nix mtime
    if let Ok(metadata) = flake_nix.metadata() {
        let mtime = metadata.mtime();
        let datetime = DateTime::from_timestamp(mtime, 0)
            .map(|dt| dt.with_timezone(&Local))
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!("{} {}", bold("Last modified:"), datetime);
    }

    // Read lock file for input details
    let lock_file = flake_dir.join("flake.lock");
    if lock_file.exists() {
        let lock_content = std::fs::read_to_string(&lock_file)?;
        let lock: serde_json::Value = serde_json::from_str(&lock_content)?;

        let nodes = lock.get("nodes").and_then(|n| n.as_object());
        let root_inputs = nodes
            .and_then(|n| n.get("root"))
            .and_then(|r| r.get("inputs"))
            .and_then(|i| i.as_object());

        if let (Some(nodes), Some(root_inputs)) = (nodes, root_inputs) {
            if !root_inputs.is_empty() {
                println!("{}", bold("Inputs:"));

                let mut names: Vec<_> = root_inputs.keys().collect();
                names.sort();

                for (i, name) in names.iter().enumerate() {
                    let is_last = i == names.len() - 1;
                    let node_ref = &root_inputs[*name];
                    print_input(name, node_ref, nodes, "", is_last);
                }
            }
        }
    } else {
        // No lock file, show unlocked inputs
        let inputs = get_flake_inputs(flake_dir)?;
        if let Some(input_map) = inputs.as_object() {
            if !input_map.is_empty() {
                println!("{}", bold("Inputs (unlocked):"));

                let mut names: Vec<_> = input_map.keys().collect();
                names.sort();

                for (i, name) in names.iter().enumerate() {
                    let is_last = i == names.len() - 1;
                    let branch = if is_last {
                        "└───"
                    } else {
                        "├───"
                    };
                    let spec = &input_map[*name];
                    let url = format_unlocked_input(spec);
                    println!("{}{}: {}", branch, bold(name), url);
                }
            }
        }
    }

    Ok(())
}

/// Format a locked input node as a flake URL.
fn format_input_url(node: &serde_json::Value) -> String {
    use chrono::{DateTime, Local};

    let locked = node.get("locked");
    if locked.is_none() {
        return String::new();
    }
    let locked = locked.unwrap();

    let typ = locked.get("type").and_then(|t| t.as_str()).unwrap_or("");

    let url = match typ {
        "github" => {
            let owner = locked.get("owner").and_then(|o| o.as_str()).unwrap_or("");
            let repo = locked.get("repo").and_then(|r| r.as_str()).unwrap_or("");
            let rev = locked.get("rev").and_then(|r| r.as_str()).unwrap_or("");
            let nar_hash = locked.get("narHash").and_then(|h| h.as_str()).unwrap_or("");
            format!("github:{}/{}/{}?narHash={}", owner, repo, rev, nar_hash)
        }
        "git" => {
            let url = locked.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let rev = locked.get("rev").and_then(|r| r.as_str()).unwrap_or("");
            format!("git+{}?rev={}", url, rev)
        }
        "path" => {
            let path = locked.get("path").and_then(|p| p.as_str()).unwrap_or("");
            format!("path:{}", path)
        }
        "tarball" => locked
            .get("url")
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string(),
        _ => locked
            .get("url")
            .and_then(|u| u.as_str())
            .unwrap_or(typ)
            .to_string(),
    };

    // Add timestamp if available
    if let Some(last_mod) = locked.get("lastModified").and_then(|l| l.as_i64()) {
        let datetime = DateTime::from_timestamp(last_mod, 0)
            .map(|dt| dt.with_timezone(&Local))
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        format!("{} ({})", url, datetime)
    } else {
        url
    }
}

/// Format an unlocked input spec as a URL.
fn format_unlocked_input(spec: &serde_json::Value) -> String {
    let input_type = spec
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("unknown");
    match input_type {
        "github" => {
            let owner = spec.get("owner").and_then(|o| o.as_str()).unwrap_or("");
            let repo = spec.get("repo").and_then(|r| r.as_str()).unwrap_or("");
            let ref_str = spec.get("ref").and_then(|r| r.as_str()).unwrap_or("");
            if ref_str.is_empty() {
                format!("github:{}/{}", owner, repo)
            } else {
                format!("github:{}/{}/{}", owner, repo, ref_str)
            }
        }
        "git" => {
            let url = spec.get("url").and_then(|u| u.as_str()).unwrap_or("");
            format!("git+{}", url)
        }
        "path" => {
            let path = spec.get("path").and_then(|p| p.as_str()).unwrap_or("");
            format!("path:{}", path)
        }
        _ => input_type.to_string(),
    }
}

/// Print an input and its transitive dependencies as a tree.
fn print_input(
    name: &str,
    node_ref: &serde_json::Value,
    nodes: &serde_json::Map<String, serde_json::Value>,
    prefix: &str,
    is_last: bool,
) {
    let branch = if is_last {
        "└───"
    } else {
        "├───"
    };

    // Handle .follows references (arrays like ["nixpkgs"])
    if let Some(follows_arr) = node_ref.as_array() {
        let follows_path: Vec<_> = follows_arr.iter().filter_map(|v| v.as_str()).collect();
        println!(
            "{}{}{} follows input '{}'",
            prefix,
            branch,
            bold(name),
            follows_path.join("/")
        );
        return;
    }

    // Get the node name (string reference)
    let node_name = node_ref.as_str().unwrap_or(name);
    let node = nodes.get(node_name);

    if let Some(node) = node {
        let url = format_input_url(node);
        println!("{}{}{}: {}", prefix, branch, bold(name), url);

        // Print transitive inputs
        if let Some(node_inputs) = node.get("inputs").and_then(|i| i.as_object()) {
            let child_prefix = if is_last {
                format!("{}    ", prefix)
            } else {
                format!("{}│   ", prefix)
            };

            let mut input_names: Vec<_> = node_inputs.keys().collect();
            input_names.sort();

            for (j, child_name) in input_names.iter().enumerate() {
                let child_is_last = j == input_names.len() - 1;
                let child_ref = &node_inputs[*child_name];
                print_input(child_name, child_ref, nodes, &child_prefix, child_is_last);
            }
        }
    } else {
        println!("{}{}{}", prefix, branch, bold(name));
    }
}

/// Wrap text in ANSI bold codes.
fn bold(text: &str) -> String {
    format!("\x1b[1m{}\x1b[0m", text)
}

/// Format a magenta+bold string (for type labels like "Nixpkgs overlay")
fn magenta_bold(text: &str) -> String {
    format!("\x1b[35;1m{}\x1b[0m", text)
}

/// Format a description for a flake output based on its type, name, and category
fn format_output_description(info: &serde_json::Map<String, serde_json::Value>) -> String {
    let type_val = info.get("_type").and_then(|v| v.as_str()).unwrap_or("unknown");
    let name_val = info.get("_name").and_then(|v| v.as_str());
    let category = info.get("_category").and_then(|v| v.as_str());

    match type_val {
        "derivation" => {
            if let Some(name) = name_val {
                // Use category to determine if this is a dev shell
                if category == Some("devShells") {
                    format!("development environment '{}'", name)
                } else if category == Some("checks") {
                    format!("check: '{}'", name)
                } else {
                    format!("package '{}'", name)
                }
            } else {
                "package".to_string()
            }
        }
        "app" => "app".to_string(),
        "formatter" => {
            if let Some(name) = name_val {
                format!("formatter: '{}'", name)
            } else {
                "formatter".to_string()
            }
        }
        "overlay" => magenta_bold("Nixpkgs overlay"),
        "module" => magenta_bold("NixOS module"),
        "template" => "template".to_string(),
        "configuration" => "NixOS configuration".to_string(),
        _ => type_val.to_string(),
    }
}

/// Update flake.lock to latest versions
pub fn cmd_update(
    input_name: Option<&str>,
    override_inputs: Option<&std::collections::HashMap<String, String>>,
) -> Result<()> {
    let flake_dir = std::env::current_dir().context("Could not get current directory")?;

    let updates = update_lock(&flake_dir, input_name, override_inputs)?;

    if let Some(updates) = updates {
        if updates.is_empty() {
            if input_name.is_some() {
                println!("Input is already up to date.");
            } else if override_inputs.map(|o| o.is_empty()).unwrap_or(true) {
                println!("All inputs are up to date.");
            }
        } else {
            println!("Updated {} input(s).", updates.len());
        }
    }

    Ok(())
}

/// Create or update flake.lock without building
pub fn cmd_lock(flake_ref: Option<&str>) -> Result<()> {
    let flake_ref = flake_ref.unwrap_or(".");
    let resolved = resolve_installable(flake_ref);

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;

    sync_inputs(flake_dir)?;
    println!("Wrote flake.lock");

    Ok(())
}

/// Run flake checks
pub fn cmd_check(flake_ref: Option<&str>, all_systems: bool) -> Result<()> {
    let flake_ref = flake_ref.unwrap_or(".");
    let resolved = resolve_installable(flake_ref);

    if !resolved.is_local {
        // Passthrough to nix flake check
        let full_ref = resolved.flake_ref.as_deref().unwrap_or(flake_ref);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.args(["flake", "check", full_ref]);

        return cmd.run();
    }

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;
    let system = get_system()?;

    // Ensure lock exists
    ensure_lock(flake_dir)?;

    // Get checks for current system
    let checks_attr = format!("checks.{}", system);

    // Build all checks
    let outputs = eval_flake_outputs(flake_dir, all_systems, false)?;

    if let Some(ref outputs) = outputs {
        if let Some(checks) = outputs.get("checks").and_then(|c| c.get(&system)) {
            if let Some(check_names) = checks.as_object() {
                let mut passed = 0;
                let mut failed = 0;

                let names: Vec<String> = check_names.keys().cloned().collect();
                let results: Vec<(String, Result<()>)> = names
                    .into_par_iter()
                    .map(|name| {
                        let attr = format!("{}.{}", checks_attr, name);
                        let options = crate::nix::BuildOptions {
                            out_link: None,
                            ..Default::default()
                        };

                        let res = crate::nix::run_nix_build(flake_dir, &attr, &options, true);
                        (name, res.map(|_| ()))
                    })
                    .collect();

                for (name, res) in results {
                    print!("checking {}: ", name);
                    match res {
                        Ok(_) => {
                            println!("ok");
                            passed += 1;
                        }
                        Err(e) => {
                            println!("FAILED");
                            tracing::debug!("  Error: {}", e);
                            failed += 1;
                        }
                    }
                }

                println!();
                println!("{} passed, {} failed", passed, failed);

                if failed > 0 {
                    anyhow::bail!("{} test(s) failed", failed);
                }

                return Ok(());
            }
        }
    }

    println!("No checks found for {}", system);

    Ok(())
}

/// Create a flake in the current directory from a template
pub fn cmd_init(template_ref: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    run_template_copy(&cwd, template_ref, false)
}

/// Create a new directory with a flake from a template
pub fn cmd_new(path: &str, template_ref: &str) -> Result<()> {
    let target_dir = std::path::Path::new(path);
    if target_dir.exists() {
        anyhow::bail!("Directory already exists: {}", path);
    }

    std::fs::create_dir_all(target_dir).context("Failed to create directory")?;

    match run_template_copy(target_dir, template_ref, true) {
        Ok(_) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_dir(target_dir);
            Err(e)
        }
    }
}

fn run_template_copy(target_dir: &std::path::Path, template_ref: &str, is_new: bool) -> Result<()> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let (flake_ref, template_name) = if let Some(idx) = template_ref.rfind('#') {
        (&template_ref[..idx], &template_ref[idx + 1..])
    } else {
        (template_ref, "default")
    };

    let flake_ref = if flake_ref == "templates" {
        "github:NixOS/templates"
    } else {
        flake_ref
    };

    tracing::info!("Fetching template from {}#{}", flake_ref, template_name);

    // Prefetch flake
    let mut cmd = crate::command::NixCommand::new("nix");
    cmd.args(["flake", "prefetch", "--json", flake_ref]);

    let prefetch_info: serde_json::Value = cmd.json()?;
    let flake_store_path = prefetch_info["storePath"]
        .as_str()
        .context("Could not determine flake store path")?;

    let flake_path = std::path::Path::new(flake_store_path);
    let flake_nix_path = flake_path.join("flake.nix");

    if !flake_nix_path.exists() {
        anyhow::bail!("No flake.nix found in {}", flake_store_path);
    }

    let nix_dir = crate::nix::get_nix_dir()?;
    let system = get_system()?;
    let lock_expr = crate::nix::get_lock_expr(flake_path);

    // Evaluate template info
    let template_attr = format!("templates.{}", template_name);
    let template_selector = if template_name == "default" {
        format!("outputs.defaultTemplate or outputs.{}", template_attr)
    } else {
        format!("outputs.{}", template_attr)
    };

    let eval_expr_str = format!(
        r#"
    let
      flake = import {};
      lock = {};
      inputs = import {}/inputs.nix {{
        inherit lock;
        flakeDirPath = {};
        system = "{}";
      }};
      outputs = flake.outputs (inputs // {{ self = inputs.self // outputs; }});
      template = {};
    in "${{template.path}}@@@${{template.description or ""}}@@@${{template.welcomeText or ""}}"
    "#,
        flake_nix_path.display(),
        lock_expr,
        nix_dir.display(),
        flake_path.display(),
        system,
        template_selector
    );

    tracing::debug!("Evaluating template info...");

    let mut cmd = crate::command::NixCommand::new("nix-instantiate");
    cmd.args([
        "--eval",
        "--readonly-mode",
        "--eval-store",
        "dummy://",
        "-E",
        &eval_expr_str,
    ]);

    let result_raw = cmd.output()?;
    // Remove surrounding quotes if any
    let result_raw = if result_raw.starts_with('"') && result_raw.ends_with('"') {
        &result_raw[1..result_raw.len() - 1]
    } else {
        &result_raw
    };

    // Unescape backslashes (nix-instantiate escapes them in output)
    let result_raw = result_raw.replace("\\\\", "\\").replace("\\\"", "\"");

    let parts: Vec<&str> = result_raw.split("@@@").collect();
    if parts.len() < 3 {
        anyhow::bail!("Unexpected template info format: {}", result_raw);
    }

    let template_path_str = parts[0];
    let _template_description = parts[1];
    let template_welcome_text = parts[2];

    let template_path = std::path::Path::new(template_path_str);

    if !template_path.exists() {
        anyhow::bail!("Template path does not exist: {}", template_path_str);
    }

    // Copy files
    let mut copied_count = 0;
    let mut skipped_count = 0;

    for entry in walkdir::WalkDir::new(template_path) {
        let entry = entry?;
        if entry.file_type().is_file() {
            let rel_path = entry.path().strip_prefix(template_path)?;
            let dest_file = target_dir.join(rel_path);

            if dest_file.exists() && !is_new {
                skipped_count += 1;
                continue;
            }

            if let Some(parent) = dest_file.parent() {
                fs::create_dir_all(parent)?;
            }

            fs::copy(entry.path(), &dest_file)?;

            // Make writable
            let mut perms = fs::metadata(&dest_file)?.permissions();
            perms.set_mode(perms.mode() | 0o200);
            fs::set_permissions(&dest_file, perms)?;

            copied_count += 1;
            tracing::debug!("  wrote: {}", rel_path.display());
        }
    }

    if copied_count > 0 {
        if is_new {
            println!("Created {} in {}", template_ref, target_dir.display());
        } else {
            println!("Initialized {} in current directory", template_ref);
        }
    }

    if skipped_count > 0 {
        println!("(skipped {} existing files)", skipped_count);
    }

    if !template_welcome_text.is_empty() {
        println!("\n{}", template_welcome_text);
    }

    Ok(())
}
