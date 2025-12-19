//! Version locking using nix flake prefetch.
//!
//! Produces flake.lock files in the native nix format (version 7).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::flake::get_flake_inputs;

/// Lock file structure (version 7)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LockFile {
    #[serde(default)]
    pub nodes: HashMap<String, LockNode>,
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LockNode {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs: Option<HashMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked: Option<LockedInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flake: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LockedInfo {
    #[serde(rename = "type")]
    pub lock_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(rename = "narHash", skip_serializing_if = "Option::is_none")]
    pub nar_hash: Option<String>,
    #[serde(rename = "lastModified", skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

fn prefetch_flake(flake_ref: &str) -> Result<Option<Value>> {
    let mut cmd = crate::command::NixCommand::new("nix");
    cmd.args(["flake", "prefetch", "--json", flake_ref]);

    Ok(cmd.json().ok())
}

/// Lock a single input, returning a node in native flake.lock format.
fn lock_input(name: &str, spec: &Value) -> Result<Option<LockNode>> {
    let input_type = spec["type"].as_str().unwrap_or("unknown");

    // Build flake reference for prefetch
    let flake_ref = match input_type {
        "github" => {
            let owner = spec["owner"].as_str().unwrap_or("");
            let repo = spec["repo"].as_str().unwrap_or("");
            let mut url = format!("github:{}/{}", owner, repo);
            if let Some(git_ref) = spec["ref"].as_str() {
                url.push('/');
                url.push_str(git_ref);
            }
            if let Some(rev) = spec["rev"].as_str() {
                url.push('/');
                url.push_str(rev);
            }
            url
        }
        "sourcehut" => {
            let owner = spec["owner"].as_str().unwrap_or("");
            let repo = spec["repo"].as_str().unwrap_or("");
            let mut url = format!("sourcehut:{}/{}", owner, repo);
            if let Some(git_ref) = spec["ref"].as_str() {
                url.push('/');
                url.push_str(git_ref);
            }
            if let Some(rev) = spec["rev"].as_str() {
                url.push('/');
                url.push_str(rev);
            }
            url
        }
        "git" => {
            let url = spec["url"].as_str().unwrap_or("");
            let mut flake_url = format!("git+{}", url);
            let mut params = Vec::new();
            if let Some(git_ref) = spec["ref"].as_str() {
                params.push(format!("ref={}", git_ref));
            }
            if let Some(rev) = spec["rev"].as_str() {
                params.push(format!("rev={}", rev));
            }
            if !params.is_empty() {
                flake_url.push('?');
                flake_url.push_str(&params.join("&"));
            }
            flake_url
        }
        "path" => {
            // Path inputs don't need prefetching
            let path = spec["path"].as_str().unwrap_or("");
            return Ok(Some(LockNode {
                locked: Some(LockedInfo {
                    lock_type: "path".to_string(),
                    path: Some(path.to_string()),
                    ..Default::default()
                }),
                original: Some(json!({
                    "type": "path",
                    "path": path,
                })),
                flake: if spec["flake"].as_bool() == Some(false) {
                    Some(false)
                } else {
                    None
                },
                ..Default::default()
            }));
        }
        "follows" => {
            // Follows references are handled separately
            return Ok(None);
        }
        _ => {
            crate::nix::warn(&format!(
                "unknown input type '{}' for input '{}'",
                input_type, name
            ));
            return Ok(None);
        }
    };

    // Prefetch to get hash and revision
    let prefetch_result = prefetch_flake(&flake_ref)?;

    if let Some(result) = prefetch_result {
        let mut locked = LockedInfo {
            lock_type: input_type.to_string(),
            ..Default::default()
        };

        // Helper to get a string value from either top level or "locked" object
        let get_field = |obj: &Value, field: &str| -> Option<String> {
            obj[field]
                .as_str()
                .or_else(|| obj["locked"][field].as_str())
                .map(|s| s.to_string())
        };

        // Helper to get an i64 value from either top level or "locked" object
        let get_int_field = |obj: &Value, field: &str| -> Option<i64> {
            obj[field]
                .as_i64()
                .or_else(|| obj["locked"][field].as_i64())
        };

        match input_type {
            "github" | "gitlab" | "sourcehut" => {
                locked.owner = spec["owner"]
                    .as_str()
                    .or_else(|| result["locked"]["owner"].as_str())
                    .map(|s| s.to_string());
                locked.repo = spec["repo"]
                    .as_str()
                    .or_else(|| result["locked"]["repo"].as_str())
                    .map(|s| s.to_string());
                locked.rev = get_field(&result, "rev").or_else(|| get_field(&result, "revision"));
                locked.nar_hash =
                    get_field(&result, "hash").or_else(|| get_field(&result, "narHash"));
                locked.last_modified = get_int_field(&result, "lastModified");

                // For GitLab/Sourcehut, they might have host field
                if let Some(host) = get_field(&result, "host") {
                    locked.host = Some(host);
                }
            }
            "git" | "hg" => {
                locked.url = spec["url"]
                    .as_str()
                    .or_else(|| result["locked"]["url"].as_str())
                    .map(|s| s.to_string());
                locked.git_ref = spec["ref"]
                    .as_str()
                    .or_else(|| result["locked"]["ref"].as_str())
                    .map(|s| s.to_string());
                locked.rev = get_field(&result, "rev").or_else(|| get_field(&result, "revision"));
                locked.nar_hash =
                    get_field(&result, "hash").or_else(|| get_field(&result, "narHash"));
                locked.last_modified = get_int_field(&result, "lastModified");
            }
            _ => {
                // Generic handling for other types
                locked.rev = get_field(&result, "rev").or_else(|| get_field(&result, "revision"));
                locked.nar_hash =
                    get_field(&result, "hash").or_else(|| get_field(&result, "narHash"));
                locked.last_modified = get_int_field(&result, "lastModified");
            }
        }

        // Build original
        let mut original = serde_json::Map::new();
        original.insert("type".to_string(), json!(input_type));

        match input_type {
            "github" | "gitlab" | "sourcehut" => {
                if let Some(owner) = spec["owner"]
                    .as_str()
                    .or_else(|| result["locked"]["owner"].as_str())
                {
                    original.insert("owner".to_string(), json!(owner));
                }
                if let Some(repo) = spec["repo"]
                    .as_str()
                    .or_else(|| result["locked"]["repo"].as_str())
                {
                    original.insert("repo".to_string(), json!(repo));
                }
                if let Some(git_ref) = spec["ref"]
                    .as_str()
                    .or_else(|| result["locked"]["ref"].as_str())
                {
                    original.insert("ref".to_string(), json!(git_ref));
                }
            }
            "git" | "hg" => {
                if let Some(url) = spec["url"]
                    .as_str()
                    .or_else(|| result["locked"]["url"].as_str())
                {
                    original.insert("url".to_string(), json!(url));
                }
                if let Some(git_ref) = spec["ref"]
                    .as_str()
                    .or_else(|| result["locked"]["ref"].as_str())
                {
                    original.insert("ref".to_string(), json!(git_ref));
                }
            }
            _ => {
                // Generic original copy if needed
            }
        }

        Ok(Some(LockNode {
            locked: Some(locked),
            original: Some(Value::Object(original)),
            flake: if spec["flake"].as_bool() == Some(false) {
                Some(false)
            } else {
                None
            },
            ..Default::default()
        }))
    } else {
        Ok(None)
    }
}

/// Read existing lock file or return empty structure.
fn read_lock(flake_lock: &Path) -> LockFile {
    let default_lock = || {
        let mut nodes = HashMap::new();
        nodes.insert(
            "root".to_string(),
            LockNode {
                inputs: Some(HashMap::new()),
                ..Default::default()
            },
        );
        LockFile {
            nodes,
            root: "root".to_string(),
            version: 7,
        }
    };

    if !flake_lock.exists() {
        return default_lock();
    }

    match fs::read_to_string(flake_lock) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|_| default_lock()),
        Err(_) => default_lock(),
    }
}

/// Write lock file with consistent formatting.
fn write_lock(flake_lock: &Path, lock_data: &LockFile) -> Result<()> {
    let content = serde_json::to_string_pretty(lock_data)?;
    fs::write(flake_lock, format!("{}\n", content))?;
    Ok(())
}

/// Sync flake.nix inputs to lock file.
///
/// Uses nix flake prefetch which respects access-tokens for private repos.
/// Produces native flake.lock format (version 7).
pub fn sync_inputs(flake_dir: &Path) -> Result<bool> {
    let flake_lock = flake_dir.join("flake.lock");
    let inputs = get_flake_inputs(flake_dir)?;

    let input_map = match inputs.as_object() {
        Some(m) if !m.is_empty() => m,
        _ => return Ok(true), // No inputs to lock
    };

    // Read existing lock
    let mut lock_data = read_lock(&flake_lock);
    let mut changed = false;

    // Ensure root node exists
    if !lock_data.nodes.contains_key("root") {
        lock_data.nodes.insert(
            "root".to_string(),
            LockNode {
                inputs: Some(HashMap::new()),
                ..Default::default()
            },
        );
        changed = true;
    }

    // Ensure root has inputs map
    if let Some(root) = lock_data.nodes.get_mut("root") {
        if root.inputs.is_none() {
            root.inputs = Some(HashMap::new());
        }
    }

    // Collect input names for tracking
    let input_names: std::collections::HashSet<String> = input_map.keys().cloned().collect();

    // Collect existing root input keys for removal check (done after processing)
    let existing_root_keys: Vec<String> = lock_data
        .nodes
        .get("root")
        .and_then(|r| r.inputs.as_ref())
        .map(|i| i.keys().cloned().collect())
        .unwrap_or_default();

    // Process each input
    for (name, spec) in input_map {
        let input_type = spec["type"].as_str().unwrap_or("unknown");

        // Handle follows
        if input_type == "follows" {
            if let Some(follows) = spec["follows"].as_array() {
                let follows_path: Vec<Value> = follows
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| json!(s)))
                    .collect();
                if let Some(root) = lock_data.nodes.get_mut("root") {
                    if let Some(ref mut root_inputs) = root.inputs {
                        root_inputs.insert(name.clone(), Value::Array(follows_path));
                        changed = true;
                    }
                }
            }
            continue;
        }

        // Check if already locked with same spec
        let already_locked = {
            lock_data
                .nodes
                .get("root")
                .and_then(|r| r.inputs.as_ref())
                .and_then(|i| i.get(name))
                .and_then(|v| v.as_str())
                .map(|ref_str| lock_data.nodes.contains_key(ref_str))
                .unwrap_or(false)
        };

        if already_locked {
            continue;
        }

        // Lock the input
        if let Some(node) = lock_input(name, spec)? {
            lock_data.nodes.insert(name.clone(), node);
            if let Some(root) = lock_data.nodes.get_mut("root") {
                if let Some(ref mut root_inputs) = root.inputs {
                    root_inputs.insert(name.clone(), json!(name));
                }
            }
            changed = true;

            tracing::info!("• locked input '{}'", name);
        }
    }

    // Remove inputs that are no longer in flake.nix
    let to_remove: Vec<String> = existing_root_keys
        .into_iter()
        .filter(|k| !input_names.contains(k))
        .collect();

    for name in to_remove {
        if let Some(root) = lock_data.nodes.get_mut("root") {
            if let Some(ref mut root_inputs) = root.inputs {
                root_inputs.remove(&name);
            }
        }
        lock_data.nodes.remove(&name);
        changed = true;

        tracing::info!("• removed input '{}'", name);
    }

    // Write if changed
    if changed {
        write_lock(&flake_lock, &lock_data)?;
    }

    Ok(true)
}

/// Ensure lock file exists and is up to date with flake inputs.
pub fn ensure_lock(flake_dir: &Path) -> Result<()> {
    sync_inputs(flake_dir)?;
    Ok(())
}

/// Update locked inputs to latest versions.
pub fn update_lock(
    flake_dir: &Path,
    input_name: Option<&str>,
) -> Result<Option<HashMap<String, (Value, Value)>>> {
    let flake_lock = flake_dir.join("flake.lock");
    let inputs = get_flake_inputs(flake_dir)?;

    let input_map = match inputs.as_object() {
        Some(m) if !m.is_empty() => m,
        _ => return Ok(Some(HashMap::new())),
    };

    // Read existing lock
    let mut lock_data = read_lock(&flake_lock);
    let mut updates = HashMap::new();

    // Determine which inputs to update
    let inputs_to_update: Vec<_> = if let Some(name) = input_name {
        if input_map.contains_key(name) {
            vec![name.to_string()]
        } else {
            eprintln!("warning: input '{}' not found in flake.nix", name);
            return Ok(None);
        }
    } else {
        input_map.keys().cloned().collect()
    };

    for name in inputs_to_update {
        let spec = match input_map.get(&name) {
            Some(s) => s,
            None => continue,
        };

        let input_type = spec["type"].as_str().unwrap_or("unknown");

        // Skip follows
        if input_type == "follows" {
            continue;
        }

        // Get old locked info
        let old_locked = lock_data
            .nodes
            .get(&name)
            .and_then(|n| n.locked.as_ref())
            .map(|l| serde_json::to_value(l).unwrap_or_default());

        // Re-lock the input
        if let Some(new_node) = lock_input(&name, spec)? {
            let new_locked = new_node
                .locked
                .as_ref()
                .map(|l| serde_json::to_value(l).unwrap_or_default());

            // Check if changed
            if old_locked != new_locked {
                if let (Some(old), Some(new)) = (old_locked, new_locked) {
                    updates.insert(name.clone(), (old, new));
                }

                lock_data.nodes.insert(name.clone(), new_node);

                // Ensure root inputs reference
                if let Some(root) = lock_data.nodes.get_mut("root") {
                    if let Some(ref mut inputs) = root.inputs {
                        inputs.insert(name.clone(), json!(name));
                    }
                }

                tracing::info!("• updated input '{}'", name);
            }
        }
    }

    // Write if changed
    if !updates.is_empty() {
        write_lock(&flake_lock, &lock_data)?;
    }

    Ok(Some(updates))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_read_lock_nonexistent() {
        let dir = tempdir().unwrap();
        let lock_file = dir.path().join("flake.lock");
        let lock = read_lock(&lock_file);
        assert_eq!(lock.version, 7);
        assert!(lock.nodes.contains_key("root"));
    }

    #[test]
    fn test_read_write_lock() {
        let dir = tempdir().unwrap();
        let lock_file = dir.path().join("flake.lock");
        let mut lock = LockFile {
            version: 7,
            root: "root".to_string(),
            ..Default::default()
        };

        let mut root_node = LockNode::default();
        let mut inputs = HashMap::new();
        inputs.insert("nixpkgs".to_string(), json!("nixpkgs"));
        root_node.inputs = Some(inputs);
        lock.nodes.insert("root".to_string(), root_node);

        write_lock(&lock_file, &lock).expect("Failed to write lock");
        let read = read_lock(&lock_file);
        assert_eq!(read.version, 7);
        assert_eq!(read.root, "root");
        assert!(read.nodes.contains_key("root"));
    }

    #[test]
    fn test_lock_input_path() {
        let _spec = json!({
            "type": "path",
            "path": "/tmp/test"
        });
        let res = lock_input("test", &_spec).unwrap();
        assert!(res.is_some());
        let node = res.unwrap();
        assert_eq!(node.locked.as_ref().unwrap().lock_type, "path");
        assert_eq!(
            node.locked.as_ref().unwrap().path.as_ref().unwrap(),
            "/tmp/test"
        );
    }

    #[test]
    fn test_lock_node_serialization() {
        let node = LockNode {
            inputs: None,
            locked: Some(LockedInfo {
                lock_type: "github".to_string(),
                owner: Some("NixOS".to_string()),
                repo: Some("nixpkgs".to_string()),
                rev: Some("abc".to_string()),
                ..Default::default()
            }),
            original: None,
            flake: None,
        };
        let json = serde_json::to_value(&node).unwrap();
        assert_eq!(json["locked"]["type"], "github");
        assert_eq!(json["locked"]["owner"], "NixOS");
        assert_eq!(json["locked"]["rev"], "abc");
    }

    #[test]
    fn test_locked_info_defaults() {
        let info = LockedInfo {
            lock_type: "git".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["type"], "git");
        assert!(json["ref"].is_null());
        assert!(json["rev"].is_null());
    }

    #[test]
    fn test_lock_input_github_fallback() {
        // Without mocking prefetch_flake, this might fail or skip if nix is missing.
        // For now, we test the logic of calling it.
        let _spec = json!({
            "type": "github",
            "owner": "NixOS",
            "repo": "nixpkgs"
        });
        // This test historically checks if we can call lock_input without crashing,
        // even if it might fail network ops in some envs.
        // We skip actual execution here to avoid network dependency in unit tests.
    }
}
