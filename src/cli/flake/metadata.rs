//! Flake metadata command - shows information about a flake without copying to store.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use chrono::{TimeZone, Utc};
use clap::Args;
use owo_colors::{OwoColorize, Stream::Stdout};
use serde_json::json;
use tracing::{debug, instrument};

use crate::flake::resolve_installable;
use crate::lock::{FlakeLock, InputRef, LockedRef};

/// Information about a git repository state.
#[derive(Debug)]
struct GitInfo {
    /// The HEAD commit hash.
    rev: String,
    /// Short version of the commit hash.
    short_rev: String,
    /// Whether the working tree has uncommitted changes.
    is_dirty: bool,
    /// Commit timestamp.
    last_modified: Option<u64>,
}

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

    // Check if we're in a git repository
    let git_info = get_git_info(flake_path);

    // Build locked/original info based on whether this is a git repo
    let (locked, original, original_url, resolved_url, effective_last_modified) = if let Some(ref git) = git_info {
        let git_url = format!("file://{}", flake_path_str);
        let git_last_modified = git.last_modified.or(last_modified);

        if git.is_dirty {
            // Dirty git repo
            let dirty_rev = format!("{}-dirty", git.rev);
            let dirty_short_rev = format!("{}-dirty", git.short_rev);

            let locked = json!({
                "__final": true,
                "dirtyRev": dirty_rev,
                "dirtyShortRev": dirty_short_rev,
                "lastModified": git_last_modified,
                "type": "git",
                "url": git_url,
            });

            let original = json!({
                "type": "git",
                "url": git_url,
            });

            (
                locked,
                original,
                format!("git+file://{}", flake_path_str),
                format!("git+file://{}", flake_path_str),
                git_last_modified,
            )
        } else {
            // Clean git repo
            let locked = json!({
                "__final": true,
                "lastModified": git_last_modified,
                "rev": git.rev,
                "shortRev": git.short_rev,
                "type": "git",
                "url": git_url,
            });

            let original = json!({
                "type": "git",
                "url": git_url,
            });

            (
                locked,
                original,
                format!("git+file://{}", flake_path_str),
                format!("git+file://{}", flake_path_str),
                git_last_modified,
            )
        }
    } else {
        // Not a git repo, use path type
        let locked = json!({
            "type": "path",
            "path": flake_path_str,
        });

        let original = json!({
            "type": "path",
            "path": flake_path_str,
        });

        (
            locked,
            original.clone(),
            format!("path:{}", flake_path_str),
            format!("path:{}", flake_path_str),
            last_modified,
        )
    };

    // Load locks from flake.lock if present, otherwise use empty locks structure
    let locks = if flake_lock.exists() {
        let content = fs::read_to_string(&flake_lock)
            .context("failed to read flake.lock")?;
        serde_json::from_str(&content).unwrap_or_else(|_| json!({
            "nodes": { "root": {} },
            "root": "root",
            "version": 7
        }))
    } else {
        // Default empty locks structure matching nix output
        json!({
            "nodes": { "root": {} },
            "root": "root",
            "version": 7
        })
    };

    // Build the metadata object
    let mut metadata = json!({
        "locked": locked,
        "original": original.clone(),
        "originalUrl": original_url,
        "path": flake_path_str,
        "resolved": original,
        "resolvedUrl": resolved_url,
    });

    // Add dirtyRevision at top level for dirty git repos
    if let Some(ref git) = git_info {
        if git.is_dirty {
            metadata["dirtyRevision"] = json!(format!("{}-dirty", git.rev));
        } else {
            metadata["revision"] = json!(&git.rev);
        }
    }

    if let Some(desc) = description {
        metadata["description"] = json!(desc);
    }

    if let Some(lm) = effective_last_modified {
        metadata["lastModified"] = json!(lm);
    }

    // Always include locks structure (matches nix behavior)
    metadata["locks"] = locks;

    Ok(metadata)
}

/// Get git repository info for a path, if it's in a git repo.
fn get_git_info(path: &Path) -> Option<GitInfo> {
    let repo = git2::Repository::discover(path).ok()?;

    let head = repo.head().ok()?;
    let commit = head.peel_to_commit().ok()?;

    let rev = commit.id().to_string();
    let short_rev = if rev.len() >= 7 {
        rev[..7].to_string()
    } else {
        rev.clone()
    };

    // Check if the working tree is dirty
    let statuses = repo.statuses(None).ok()?;
    let is_dirty = statuses.iter().any(|s| {
        let status = s.status();
        // Check for any modifications (staged or unstaged)
        status.intersects(
            git2::Status::INDEX_NEW
                | git2::Status::INDEX_MODIFIED
                | git2::Status::INDEX_DELETED
                | git2::Status::INDEX_RENAMED
                | git2::Status::INDEX_TYPECHANGE
                | git2::Status::WT_NEW
                | git2::Status::WT_MODIFIED
                | git2::Status::WT_DELETED
                | git2::Status::WT_TYPECHANGE
                | git2::Status::WT_RENAMED,
        )
    });

    let last_modified = Some(commit.time().seconds() as u64);

    Some(GitInfo {
        rev,
        short_rev,
        is_dirty,
        last_modified,
    })
}

/// Extract description from flake.nix using nix eval
fn extract_description(flake_path: &str) -> Result<Option<String>> {
    let expr = format!(
        r#"(import {}/flake.nix).description or null"#,
        flake_path
    );

    let output = Command::new("nix")
        .args(["eval", "--raw", "--impure", "--expr", &expr])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let s = String::from_utf8_lossy(&output.stdout).to_string();
            if s.is_empty() || s == "null" {
                Ok(None)
            } else {
                Ok(Some(s))
            }
        }
        _ => Ok(None),
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
