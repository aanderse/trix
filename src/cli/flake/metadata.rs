//! Flake metadata command - shows information about a flake without copying to store.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use chrono::{TimeZone, Utc};
use clap::Args;
use owo_colors::{OwoColorize, Stream::Stdout};
use serde_json::json;
use tracing::{debug, instrument};

use crate::eval::Evaluator;
use crate::flake::resolve_installable;
use crate::lock::{FlakeLock, InputRef, LockedRef};

#[derive(Args)]
pub struct MetadataArgs {
    /// Flake reference (default: .)
    #[arg(default_value = ".")]
    pub flake_ref: String,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[instrument(level = "debug", skip_all, fields(flake_ref = %args.flake_ref))]
pub fn run(args: MetadataArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;

    // Try to resolve as local flake first
    match resolve_installable(&args.flake_ref, &cwd) {
        Ok(resolved) => {
            // Local flake - extract metadata without copying to store
            debug!(path = %resolved.path.display(), "extracting local metadata");
            let metadata = extract_local_metadata(&resolved.path)?;

            if args.json {
                println!("{}", serde_json::to_string_pretty(&metadata)?);
            } else {
                print_metadata(&metadata);
            }
        }
        Err(_) => {
            // Not a local flake, delegate to nix flake metadata
            debug!("delegating to nix flake metadata for remote ref");
            delegate_to_nix(&args.flake_ref, args.json)?;
        }
    }

    Ok(())
}

/// Extract metadata from a local flake without copying to store
fn extract_local_metadata(flake_path: &std::path::Path) -> Result<serde_json::Value> {
    let flake_nix = flake_path.join("flake.nix");
    let flake_lock = flake_path.join("flake.lock");

    if !flake_nix.exists() {
        return Err(anyhow!("no flake.nix found in {}", flake_path.display()));
    }

    let flake_path_str = flake_path
        .to_str()
        .ok_or_else(|| anyhow!("invalid flake path"))?;

    // Extract description from flake.nix
    let description = extract_description(flake_path_str)?;

    // Get last modified time from flake.nix
    let last_modified = fs::metadata(&flake_nix)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());

    // Build locked info for local path
    let locked = json!({
        "type": "path",
        "path": flake_path_str,
    });

    let original = json!({
        "type": "path",
        "path": flake_path_str,
    });

    // Load locks from flake.lock if present
    let locks = if flake_lock.exists() {
        let content = fs::read_to_string(&flake_lock)
            .context("failed to read flake.lock")?;
        serde_json::from_str(&content).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };

    // Build the metadata object
    let mut metadata = json!({
        "locked": locked,
        "original": original,
        "originalUrl": format!("path:{}", flake_path_str),
        "path": flake_path_str,
        "resolved": original.clone(),
        "resolvedUrl": format!("path:{}", flake_path_str),
    });

    if let Some(desc) = description {
        metadata["description"] = json!(desc);
    }

    if let Some(lm) = last_modified {
        metadata["lastModified"] = json!(lm);
    }

    if !locks.is_null() && locks.get("nodes").is_some() {
        metadata["locks"] = locks;
    }

    Ok(metadata)
}

/// Extract description from flake.nix using the evaluator
fn extract_description(flake_path: &str) -> Result<Option<String>> {
    let mut evaluator = Evaluator::new().context("failed to initialize Nix evaluator")?;

    let expr = format!(
        r#"(import {}/flake.nix).description or null"#,
        flake_path
    );

    match evaluator.eval_string(&expr, "<trix metadata>") {
        Ok(value) => {
            match evaluator.require_string(&value) {
                Ok(s) if !s.is_empty() && s != "null" => Ok(Some(s)),
                _ => Ok(None),
            }
        }
        Err(_) => Ok(None),
    }
}

/// Delegate to nix flake metadata for remote refs
fn delegate_to_nix(flake_ref: &str, json_output: bool) -> Result<()> {
    let mut cmd = Command::new("nix");
    cmd.args(["flake", "metadata"]);

    if json_output {
        cmd.arg("--json");
    }

    cmd.arg(flake_ref);

    let status = cmd.status().context("failed to run nix flake metadata")?;

    if !status.success() {
        return Err(anyhow!("nix flake metadata failed"));
    }

    Ok(())
}

/// Print metadata in human-readable format
fn print_metadata(metadata: &serde_json::Value) {
    if let Some(desc) = metadata.get("description").and_then(|d| d.as_str()) {
        println!(
            "{}   {}",
            "Description:".if_supports_color(Stdout, |t| t.bold()),
            desc
        );
    }

    if let Some(path) = metadata.get("path").and_then(|p| p.as_str()) {
        println!(
            "{}          {}",
            "Path:".if_supports_color(Stdout, |t| t.bold()),
            path
        );
    }

    if let Some(lm) = metadata.get("lastModified").and_then(|l| l.as_u64()) {
        println!(
            "{} {}",
            "Last modified:".if_supports_color(Stdout, |t| t.bold()),
            format_timestamp(lm)
        );
    }

    // Try to parse the lock file for proper tree display
    if let Some(locks) = metadata.get("locks") {
        if let Ok(lock) = serde_json::from_value::<FlakeLock>(locks.clone()) {
            print_inputs_tree(&lock);
        }
    }
}

/// Print the inputs tree with nested dependencies and follows relationships
fn print_inputs_tree(lock: &FlakeLock) {
    let root = match lock.root_node() {
        Some(r) => r,
        None => return,
    };

    if root.inputs.is_empty() {
        return;
    }

    println!("{}", "Inputs:".if_supports_color(Stdout, |t| t.bold()));

    // Sort inputs by name for consistent output
    let mut inputs: Vec<_> = root.inputs.iter().collect();
    inputs.sort_by_key(|(name, _)| *name);

    let mut printed: HashSet<String> = HashSet::new();
    let last_idx = inputs.len() - 1;

    for (idx, (input_name, input_ref)) in inputs.iter().enumerate() {
        let is_last = idx == last_idx;
        print_input_node(lock, input_name, input_ref, "", is_last, &mut printed);
    }
}

/// Print a single input node and its children recursively
fn print_input_node(
    lock: &FlakeLock,
    input_name: &str,
    input_ref: &InputRef,
    prefix: &str,
    is_last: bool,
    printed: &mut HashSet<String>,
) {
    let connector = if is_last { "└───" } else { "├───" };
    let child_prefix = if is_last {
        format!("{}    ", prefix)
    } else {
        format!("{}│   ", prefix)
    };

    match input_ref {
        InputRef::Follows(path) => {
            // This input follows another input
            let follows_str = format!("follows input '{}'", path.join("."));
            println!(
                "{}{}{}{}",
                prefix,
                connector.if_supports_color(Stdout, |t| t.cyan()),
                input_name.if_supports_color(Stdout, |t| t.bold()),
                format!(" {}", follows_str).if_supports_color(Stdout, |t| t.dimmed())
            );
        }
        InputRef::Direct(node_name) => {
            // Direct reference to a node
            if let Some(node) = lock.nodes.get(node_name) {
                let info = node
                    .locked
                    .as_ref()
                    .map(format_locked_ref)
                    .unwrap_or_default();

                let timestamp = node
                    .locked
                    .as_ref()
                    .and_then(|l| match l {
                        LockedRef::GitHub { last_modified, .. } => *last_modified,
                        LockedRef::Git { last_modified, .. } => *last_modified,
                        LockedRef::Path { last_modified, .. } => *last_modified,
                        _ => None,
                    })
                    .map(|lm| format!(" ({})", format_timestamp(lm)))
                    .unwrap_or_default();

                println!(
                    "{}{}{}: {}{}",
                    prefix,
                    connector.if_supports_color(Stdout, |t| t.cyan()),
                    input_name.if_supports_color(Stdout, |t| t.bold()),
                    info,
                    timestamp
                );

                // Print nested inputs if we haven't already printed this node
                if !node.inputs.is_empty() && !printed.contains(node_name) {
                    printed.insert(node_name.clone());

                    let mut child_inputs: Vec<_> = node.inputs.iter().collect();
                    child_inputs.sort_by_key(|(name, _)| *name);
                    let child_last_idx = child_inputs.len() - 1;

                    for (child_idx, (child_name, child_ref)) in child_inputs.iter().enumerate() {
                        let child_is_last = child_idx == child_last_idx;
                        print_input_node(
                            lock,
                            child_name,
                            child_ref,
                            &child_prefix,
                            child_is_last,
                            printed,
                        );
                    }
                }
            }
        }
    }
}

/// Format a LockedRef for display
fn format_locked_ref(locked: &LockedRef) -> String {
    match locked {
        LockedRef::GitHub { owner, repo, rev, .. } => {
            format!("github:{}/{}/{}", owner, repo, rev)
        }
        LockedRef::GitLab { owner, repo, rev, .. } => {
            format!("gitlab:{}/{}/{}", owner, repo, rev)
        }
        LockedRef::Sourcehut { owner, repo, rev, .. } => {
            format!("sourcehut:{}/{}/{}", owner, repo, rev)
        }
        LockedRef::Git { url, rev, dirty_rev, .. } => {
            let effective_rev = rev.as_ref().or(dirty_rev.as_ref());
            match effective_rev {
                Some(r) => format!("git+{}?rev={}", url, r),
                None => format!("git+{}", url),
            }
        }
        LockedRef::Path { path, .. } => {
            format!("path:{}", path)
        }
        LockedRef::Tarball { url, .. } => {
            url.clone()
        }
        LockedRef::Indirect { id, .. } => {
            format!("flake:{}", id)
        }
    }
}

/// Format a Unix timestamp as human-readable UTC date (matches nix behavior)
fn format_timestamp(timestamp: u64) -> String {
    match Utc.timestamp_opt(timestamp as i64, 0) {
        chrono::LocalResult::Single(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        _ => timestamp.to_string(),
    }
}
