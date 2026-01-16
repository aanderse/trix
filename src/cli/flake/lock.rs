use std::env;
use std::fs;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, info, instrument, trace, warn};

use crate::eval::Evaluator;
use crate::flake::resolve_installable;

#[derive(Args)]
pub struct LockArgs {
    /// Flake reference to lock (default: .)
    #[arg(default_value = ".")]
    pub flake_ref: String,

    /// Don't actually write the lock file
    #[arg(long)]
    pub dry_run: bool,

    /// Update all inputs
    #[arg(long)]
    pub update_all: bool,

    /// Update specific input(s)
    #[arg(long = "update", value_name = "INPUT")]
    pub update_inputs: Vec<String>,

    /// Override inputs (pairs of INPUT, FLAKE_REF) - set programmatically by update command
    #[arg(skip)]
    pub override_inputs: Vec<(String, String)>,
}

/// Represents a parsed flake input
#[derive(Debug, Clone)]
struct FlakeInput {
    name: String,
    url: Option<String>,
    follows: Option<String>,
    flake: bool,
    /// Nested follows overrides: maps input name to follows path
    /// e.g., for `inputs.home-manager.inputs.nixpkgs.follows = "nixpkgs"`
    /// this would be {"nixpkgs": "nixpkgs"}
    nested_follows: std::collections::HashMap<String, String>,
}

#[instrument(level = "debug", skip_all, fields(flake_ref = %args.flake_ref))]
pub fn run(args: LockArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;

    // Resolve the flake reference
    debug!("resolving flake reference");
    let resolved = resolve_installable(&args.flake_ref, &cwd)?;

    debug!(
        flake_path = %resolved.path.display(),
        flake_nix = %resolved.flake_nix_path().display(),
        has_lock = resolved.lock.is_some(),
        "resolved flake"
    );

    let flake_path = &resolved.path;
    let flake_path_str = flake_path
        .to_str()
        .ok_or_else(|| anyhow!("invalid flake path"))?;

    // Step 1: Extract inputs from flake.nix WITHOUT using builtins.getFlake
    // This imports the flake.nix directly and reads only the inputs attribute
    debug!("extracting inputs from flake.nix");

    let inputs = extract_flake_inputs(flake_path_str)?;

    if inputs.is_empty() {
        info!("no inputs to lock");
        return Ok(());
    }

    debug!(count = inputs.len(), "found inputs");
    for input in &inputs {
        if let Some(ref url) = input.url {
            trace!(name = %input.name, %url, "input");
        } else if let Some(ref follows) = input.follows {
            trace!(name = %input.name, %follows, "input follows");
        }
    }

    // Step 2: Read existing lock file if present
    let lock_path = resolved.flake_lock_path();
    debug!(lock_path = %lock_path.display(), "checking for existing lock file");
    let mut lock_data = if lock_path.exists() {
        trace!("reading existing lock file");
        let content = fs::read_to_string(&lock_path)
            .context("failed to read existing flake.lock")?;
        serde_json::from_str(&content).unwrap_or_else(|_| default_lock_data())
    } else {
        trace!("no existing lock file, starting fresh");
        default_lock_data()
    };

    // Step 3: For each input that needs locking, prefetch it
    let mut changed = false;
    for input in &inputs {
        if input.follows.is_some() {
            // Handle follows - just update the root inputs
            trace!(name = %input.name, follows = %input.follows.as_ref().unwrap(), "processing follows");
            update_follows_in_lock(&mut lock_data, &input.name, input.follows.as_ref().unwrap())?;
            changed = true;
            continue;
        }

        // Check for URL override
        let override_url = args
            .override_inputs
            .iter()
            .find(|(name, _)| name == &input.name)
            .map(|(_, url)| url.clone());

        let url = if let Some(ref override_url) = override_url {
            debug!(name = %input.name, override_url = %override_url, "using override URL");
            override_url.clone()
        } else {
            match &input.url {
                Some(u) => u.clone(),
                None => continue,
            }
        };

        // Check if already locked (but always update if there's an override)
        let already_locked = is_input_locked(&lock_data, &input.name);
        if already_locked
            && !args.update_all
            && !args.update_inputs.contains(&input.name)
            && override_url.is_none()
        {
            debug!(name = %input.name, "already locked, skipping");
            continue;
        }

        // Prefetch the input
        info!(name = %input.name, "locking input");

        match prefetch_input(&url) {
            Ok(locked_info) => {
                update_input_in_lock(
                    &mut lock_data,
                    &input.name,
                    &url,
                    &locked_info,
                    input.flake,
                    &input.nested_follows,
                )?;
                changed = true;
                info!(name = %input.name, "locked");
            }
            Err(e) => {
                warn!(name = %input.name, error = %e, "failed to lock input");
            }
        }
    }

    // Step 4: Write the lock file
    if changed {
        if args.dry_run {
            info!(lock_path = %lock_path.display(), "dry-run: would write lock file");
            let json = serde_json::to_string_pretty(&lock_data)?;
            trace!("lock file contents:\n{}", json);
        } else {
            debug!("writing lock file");
            let json = serde_json::to_string_pretty(&lock_data)?;
            fs::write(&lock_path, json.as_bytes())
                .context("failed to write flake.lock")?;
            info!(lock_path = %lock_path.display(), "wrote lock file");
        }
    } else {
        info!("lock file is up to date");
    }

    Ok(())
}

/// Extract inputs from flake.nix without using builtins.getFlake
#[instrument(level = "debug", skip_all, fields(flake_path = %flake_path))]
fn extract_flake_inputs(flake_path: &str) -> Result<Vec<FlakeInput>> {
    // Use nix-bindings evaluator to extract just the inputs
    trace!("initializing evaluator for input extraction");
    let mut evaluator = Evaluator::new().context("failed to initialize Nix evaluator")?;

    // This expression imports flake.nix directly and extracts inputs
    // It does NOT use builtins.getFlake, so it won't copy to the store
    let expr = format!(
        r#"
        let
          flake = import {}/flake.nix;
          inputs = flake.inputs or {{}};

          # Convert a value to string, handling paths without store import
          toStr = v: if builtins.isPath v then builtins.toString v else v;

          # Extract nested follows from an input's inputs attribute
          # e.g., inputs.home-manager.inputs.nixpkgs.follows = "nixpkgs"
          # Returns a list of {{inputName, followsPath}} pairs
          getNestedFollows = attrs:
            if attrs ? inputs && builtins.isAttrs attrs.inputs
            then builtins.filter (x: x != null) (builtins.attrValues (builtins.mapAttrs
              (inputName: inputSpec:
                if builtins.isAttrs inputSpec && inputSpec ? follows
                then {{ inherit inputName; followsPath = inputSpec.follows; }}
                else null)
              attrs.inputs))
            else [];

          # Extract info for each input
          getInfo = name: let
            input = inputs.${{name}};
            attrs = if builtins.isAttrs input then input else {{ url = input; }};
          in {{
            inherit name;
            url = toStr (attrs.url or null);
            follows = attrs.follows or null;
            flake = attrs.flake or true;
            nestedFollows = getNestedFollows attrs;
          }};
        in
          map getInfo (builtins.attrNames inputs)
        "#,
        flake_path
    );

    let value = evaluator.eval_string(&expr, "<trix inputs>")?;

    // Parse the result - it's a list of attrsets
    let list_size = evaluator.require_list_size(&value)?;
    trace!(count = list_size, "found raw inputs");

    let mut inputs = Vec::new();
    for i in 0..list_size {
        let item = evaluator.require_list_elem(&value, i)?;

        let name = evaluator.get_attr(&item, "name")?
            .and_then(|v| evaluator.require_string(&v).ok())
            .unwrap_or_default();

        let url = evaluator.get_attr(&item, "url")?
            .and_then(|v| evaluator.require_string(&v).ok())
            .filter(|s| s != "null" && !s.is_empty());

        let follows = evaluator.get_attr(&item, "follows")?
            .and_then(|v| evaluator.require_string(&v).ok())
            .filter(|s| s != "null" && !s.is_empty());

        let flake = evaluator.get_attr(&item, "flake")?
            .and_then(|v| evaluator.require_bool(&v).ok())
            .unwrap_or(true);

        // Extract nested follows (list of {inputName, followsPath} pairs)
        let mut nested_follows = std::collections::HashMap::new();
        if let Ok(Some(nf_list)) = evaluator.get_attr(&item, "nestedFollows") {
            if let Ok(list_size) = evaluator.require_list_size(&nf_list) {
                for j in 0..list_size {
                    if let Ok(nf_item) = evaluator.require_list_elem(&nf_list, j) {
                        let input_name = evaluator.get_attr(&nf_item, "inputName")
                            .ok()
                            .flatten()
                            .and_then(|v| evaluator.require_string(&v).ok());
                        let follows_path = evaluator.get_attr(&nf_item, "followsPath")
                            .ok()
                            .flatten()
                            .and_then(|v| evaluator.require_string(&v).ok());
                        if let (Some(inp), Some(fol)) = (input_name, follows_path) {
                            nested_follows.insert(inp, fol);
                        }
                    }
                }
            }
        }

        if !name.is_empty() {
            inputs.push(FlakeInput { name, url, follows, flake, nested_follows });
        }
    }

    Ok(inputs)
}

/// Prefetch a flake input using `nix flake prefetch`
#[instrument(level = "debug", skip_all, fields(%url))]
fn prefetch_input(url: &str) -> Result<serde_json::Value> {
    debug!("+ nix flake prefetch --json {}", url);
    let output = Command::new("nix")
        .args(["flake", "prefetch", "--json", url])
        .output()
        .context("failed to run nix flake prefetch")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("prefetch failed: {}", stderr));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("failed to parse prefetch output")?;

    trace!("prefetch result: {}", json);
    Ok(json)
}

/// Get flake metadata including its lock tree using `nix flake metadata`
#[instrument(level = "trace", skip_all, fields(%url))]
fn get_flake_metadata(url: &str) -> Result<serde_json::Value> {
    trace!("+ nix flake metadata --json {}", url);
    let output = Command::new("nix")
        .args(["flake", "metadata", "--json", url])
        .output()
        .context("failed to run nix flake metadata")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("metadata failed: {}", stderr));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("failed to parse metadata output")?;

    Ok(json)
}

fn default_lock_data() -> serde_json::Value {
    serde_json::json!({
        "nodes": {
            "root": {
                "inputs": {}
            }
        },
        "root": "root",
        "version": 7
    })
}

fn is_input_locked(lock_data: &serde_json::Value, name: &str) -> bool {
    lock_data["nodes"]["root"]["inputs"]
        .get(name)
        .is_some()
}

fn update_follows_in_lock(
    lock_data: &mut serde_json::Value,
    name: &str,
    follows: &str,
) -> Result<()> {
    let follows_path: Vec<&str> = follows.split('/').collect();
    let follows_value: Vec<serde_json::Value> = follows_path
        .iter()
        .map(|s| serde_json::json!(s))
        .collect();

    lock_data["nodes"]["root"]["inputs"][name] = serde_json::json!(follows_value);
    Ok(())
}

#[instrument(level = "trace", skip_all, fields(%name, %url))]
fn update_input_in_lock(
    lock_data: &mut serde_json::Value,
    name: &str,
    url: &str,
    locked_info: &serde_json::Value,
    is_flake: bool,
    nested_follows: &std::collections::HashMap<String, String>,
) -> Result<()> {
    // Parse the URL to determine type and extract info
    let (_lock_type, original, locked) = parse_url_for_lock(url, locked_info)?;
    trace!(lock_type = %_lock_type, "parsed URL for lock");

    let mut node = serde_json::json!({
        "locked": locked,
        "original": original,
    });

    if !is_flake {
        trace!("marking as non-flake input");
        node["flake"] = serde_json::json!(false);
    }

    // If this is a flake, get its metadata to discover transitive inputs
    if is_flake {
        if let Ok(metadata) = get_flake_metadata(url) {
            if let Some(locks) = metadata.get("locks") {
                merge_transitive_locks(lock_data, name, &mut node, locks, nested_follows)?;
            }
        }
    }

    // Add the node
    lock_data["nodes"][name] = node;

    // Reference it from root
    lock_data["nodes"]["root"]["inputs"][name] = serde_json::json!(name);

    Ok(())
}

/// Merge transitive locks from an input's lock tree into our lock data
#[instrument(level = "trace", skip_all, fields(%input_name))]
fn merge_transitive_locks(
    lock_data: &mut serde_json::Value,
    input_name: &str,
    input_node: &mut serde_json::Value,
    input_locks: &serde_json::Value,
    nested_follows: &std::collections::HashMap<String, String>,
) -> Result<()> {
    let Some(input_nodes) = input_locks.get("nodes").and_then(|n| n.as_object()) else {
        return Ok(());
    };

    // Get the input's root node to find its inputs
    let Some(input_root) = input_nodes.get("root") else {
        return Ok(());
    };

    let Some(input_inputs) = input_root.get("inputs").and_then(|i| i.as_object()) else {
        return Ok(());
    };

    if input_inputs.is_empty() {
        return Ok(());
    }

    trace!(count = input_inputs.len(), "processing transitive inputs");

    // Track name mappings for transitive inputs
    // key: original name in input's lock, value: name in our lock
    let mut name_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    // First pass: add all transitive nodes to our lock, handling name collisions
    // Skip nodes that have follows overrides (they won't be added as separate nodes)
    for (trans_name, trans_ref) in input_inputs {
        // Check if this input has a follows override
        if nested_follows.contains_key(trans_name) {
            trace!(
                parent = %input_name,
                input = %trans_name,
                follows = %nested_follows[trans_name],
                "input follows override"
            );
            continue; // Don't add node, will be handled as follows
        }

        // trans_ref is either a string (node name) or an array (follows path)
        if let Some(node_name) = trans_ref.as_str() {
            // It's a reference to a node, get the node data
            if let Some(trans_node) = input_nodes.get(node_name) {
                // Check for name collision in our lock
                let final_name = find_unique_node_name(lock_data, node_name);
                name_map.insert(node_name.to_string(), final_name.clone());

                if final_name != node_name {
                    trace!(
                        parent = %input_name,
                        original = %node_name,
                        renamed = %final_name,
                        "renamed transitive input"
                    );
                }

                // Clone the node (it might have its own inputs that need remapping)
                let mut cloned_node = trans_node.clone();

                // Recursively remap any inputs this node might have
                if let Some(node_inputs) = cloned_node.get_mut("inputs") {
                    if let Some(obj) = node_inputs.as_object_mut() {
                        for (_key, value) in obj.iter_mut() {
                            if let Some(ref_name) = value.as_str() {
                                if let Some(mapped) = name_map.get(ref_name) {
                                    *value = serde_json::json!(mapped);
                                }
                            }
                        }
                    }
                }

                // Add to our lock
                lock_data["nodes"][&final_name] = cloned_node;

                trace!(
                    parent = %input_name,
                    input = %trans_name,
                    "locked transitive input"
                );
            }
        }
        // If it's an array, it's a follows path - we'll handle that in the input's inputs mapping
    }

    // Second pass: build the input's inputs mapping with remapped names
    let mut input_inputs_mapped = serde_json::Map::new();
    for (trans_name, trans_ref) in input_inputs {
        // Check for nested follows override
        if let Some(follows_path) = nested_follows.get(trans_name) {
            // Convert follows path to array format
            let follows_array: Vec<serde_json::Value> = follows_path
                .split('/')
                .map(|s| serde_json::json!(s))
                .collect();
            input_inputs_mapped.insert(trans_name.clone(), serde_json::json!(follows_array));
        } else if let Some(node_name) = trans_ref.as_str() {
            // Map to the (possibly renamed) node
            let final_name = name_map.get(node_name).cloned().unwrap_or_else(|| node_name.to_string());
            input_inputs_mapped.insert(trans_name.clone(), serde_json::json!(final_name));
        } else if trans_ref.is_array() {
            // It's a follows path, keep as-is
            input_inputs_mapped.insert(trans_name.clone(), trans_ref.clone());
        }
    }

    // Add the inputs mapping to the input node
    if !input_inputs_mapped.is_empty() {
        input_node["inputs"] = serde_json::Value::Object(input_inputs_mapped);
    }

    Ok(())
}

/// Find a unique node name, appending _2, _3, etc. if needed
fn find_unique_node_name(lock_data: &serde_json::Value, base_name: &str) -> String {
    let nodes = lock_data.get("nodes").and_then(|n| n.as_object());

    if nodes.map(|n| n.contains_key(base_name)).unwrap_or(false) {
        // Name collision, find a unique variant
        for i in 2..100 {
            let candidate = format!("{}_{}", base_name, i);
            if !nodes.map(|n| n.contains_key(&candidate)).unwrap_or(false) {
                return candidate;
            }
        }
        // Fallback (shouldn't happen)
        format!("{}_transitive", base_name)
    } else {
        base_name.to_string()
    }
}

/// Check if a URL is a bare filesystem path (not a flake URL scheme)
fn is_bare_path(url: &str) -> bool {
    url.starts_with('/') || url.starts_with("./") || url.starts_with("../")
}

fn parse_url_for_lock(
    url: &str,
    prefetch_result: &serde_json::Value,
) -> Result<(String, serde_json::Value, serde_json::Value)> {
    // The prefetch result has:
    // - "hash": the narHash
    // - "locked": object with type-specific locked info
    // - "original": object with original reference info

    // Get the hash from prefetch result
    let hash = prefetch_result
        .get("hash")
        .and_then(|h| h.as_str())
        .or_else(|| prefetch_result.get("narHash").and_then(|h| h.as_str()));

    // If the original URL is a bare path, preserve it as type "path"
    // even if Nix converts it to git (which would cause store copies)
    if is_bare_path(url) {
        let original = serde_json::json!({
            "type": "path",
            "path": url,
        });
        let mut locked = serde_json::json!({
            "type": "path",
            "path": url,
        });
        if let Some(h) = hash {
            locked["narHash"] = serde_json::json!(h);
        }
        // Preserve lastModified from the prefetch result for Nix CLI compatibility
        if let Some(last_modified) = prefetch_result
            .get("locked")
            .and_then(|l| l.get("lastModified"))
        {
            locked["lastModified"] = last_modified.clone();
        }
        return Ok(("path".to_string(), original, locked));
    }

    // If prefetch already gives us locked/original, use them directly
    if prefetch_result.get("locked").is_some() && prefetch_result.get("original").is_some() {
        let mut locked = prefetch_result["locked"].clone();
        let original = prefetch_result["original"].clone();
        let lock_type = locked["type"].as_str().unwrap_or("unknown").to_string();

        // The hash is at the top level of prefetch output, not inside locked
        // We need to copy it into the locked object as narHash
        if let Some(h) = hash {
            if locked.get("narHash").is_none() {
                locked["narHash"] = serde_json::json!(h);
            }
        }

        return Ok((lock_type, original, locked));
    }

    // Fallback: manually construct from hash and URL
    let hash = prefetch_result["hash"].as_str()
        .or_else(|| prefetch_result["narHash"].as_str())
        .unwrap_or("");

    if url.starts_with("github:") {
        let parts: Vec<&str> = url.trim_start_matches("github:").split('/').collect();
        if parts.len() >= 2 {
            let owner = parts[0];
            let repo = parts[1];
            let git_ref = parts.get(2).copied();

            let mut original = serde_json::json!({
                "type": "github",
                "owner": owner,
                "repo": repo,
            });
            if let Some(r) = git_ref {
                original["ref"] = serde_json::json!(r);
            }

            let locked = serde_json::json!({
                "type": "github",
                "owner": owner,
                "repo": repo,
                "narHash": hash,
            });

            return Ok(("github".to_string(), original, locked));
        }
    }

    // Fallback for other types
    Ok((
        "indirect".to_string(),
        serde_json::json!({"type": "indirect", "id": url}),
        serde_json::json!({"type": "indirect", "id": url, "narHash": hash}),
    ))
}
