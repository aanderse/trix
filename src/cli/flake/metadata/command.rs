use super::common::bold;
use crate::flake::{get_flake_description, get_flake_inputs, resolve_installable};
use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use std::os::unix::fs::MetadataExt;

/// Show flake metadata and inputs
pub fn cmd_metadata(flake_ref: Option<&str>) -> Result<()> {
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
