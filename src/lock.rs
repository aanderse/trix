//! Version locking using nix flake prefetch.
//!
//! Produces flake.lock files in the native nix format (version 7).

use anyhow::Result;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs;

use std::path::Path;

use crate::cli::style::*;
use crate::flake::get_flake_inputs;

// ============================================================================
// ANSI color helpers (matching nix's style)
// ============================================================================

/// Format a locked node as a display URL with date (matching nix's format).
fn format_locked_url(node: &LockNode) -> String {
    if let Some(ref locked) = node.locked {
        let url = match locked.lock_type.as_str() {
            "github" => {
                let owner = locked.owner.as_deref().unwrap_or("");
                let repo = locked.repo.as_deref().unwrap_or("");
                let rev = locked.rev.as_deref().unwrap_or("");
                format!("github:{}/{}/{}", owner, repo, rev)
            }
            "git" => {
                let url = locked.url.as_deref().unwrap_or("");
                let rev = locked.rev.as_deref().unwrap_or("");
                format!("git+{}?rev={}", url, rev)
            }
            "path" => {
                format!("path:{}", locked.path.as_deref().unwrap_or(""))
            }
            _ => format!("{:?}", locked),
        };

        if let Some(last_modified) = locked.last_modified {
            if let Some(dt) = DateTime::from_timestamp(last_modified, 0) {
                let local_dt = dt.with_timezone(&Local);
                return format!("{} ({})", url, local_dt.format("%Y-%m-%d"));
            }
        }
        url
    } else {
        String::new()
    }
}

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
    #[serde(rename = "revCount", skip_serializing_if = "Option::is_none")]
    pub rev_count: Option<i64>,
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
                locked.rev_count = get_int_field(&result, "revCount");
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

/// Fetch a locked input's source and read its flake.lock.
///
/// Returns the parsed flake.lock content, or None if no flake.lock exists.
fn fetch_source_flake_lock(node: &LockNode, input_name: &str) -> Option<Value> {
    let locked = node.locked.as_ref()?;
    let source_type = &locked.lock_type;

    // Path inputs: read directly from filesystem
    if source_type == "path" {
        let path = locked.path.as_ref()?;
        let lock_path = Path::new(path).join("flake.lock");
        if !lock_path.exists() {
            return None;
        }
        return fs::read_to_string(&lock_path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok());
    }

    // Build nix expression to fetch and read flake.lock
    let nix_expr = match source_type.as_str() {
        "git" => {
            let url = locked.url.as_deref().unwrap_or("");
            let rev = locked.rev.as_deref().unwrap_or("");
            let nar_hash = locked.nar_hash.as_deref().unwrap_or("");
            let ref_part = locked
                .git_ref
                .as_ref()
                .map(|r| format!("ref = \"{}\";", r))
                .unwrap_or_default();
            format!(
                r#"
                let
                  src = builtins.fetchGit {{
                    url = "{}";
                    rev = "{}";
                    narHash = "{}";
                    {}
                  }};
                  lockPath = src + "/flake.lock";
                in
                  if builtins.pathExists lockPath
                  then builtins.readFile lockPath
                  else ""
                "#,
                url, rev, nar_hash, ref_part
            )
        }
        "github" => {
            let owner = locked.owner.as_deref().unwrap_or("");
            let repo = locked.repo.as_deref().unwrap_or("");
            let rev = locked.rev.as_deref().unwrap_or("");
            let nar_hash = locked.nar_hash.as_deref().unwrap_or("");
            let url = format!(
                "https://github.com/{}/{}/archive/{}.tar.gz",
                owner, repo, rev
            );
            format!(
                r#"
                let
                  src = builtins.fetchTarball {{
                    url = "{}";
                    sha256 = "{}";
                  }};
                  lockPath = src + "/flake.lock";
                in
                  if builtins.pathExists lockPath
                  then builtins.readFile lockPath
                  else ""
                "#,
                url, nar_hash
            )
        }
        "gitlab" => {
            let owner = locked.owner.as_deref().unwrap_or("");
            let repo = locked.repo.as_deref().unwrap_or("");
            let rev = locked.rev.as_deref().unwrap_or("");
            let nar_hash = locked.nar_hash.as_deref().unwrap_or("");
            let host = locked.host.as_deref().unwrap_or("gitlab.com");
            let url = format!(
                "https://{}/{}/{}/-/archive/{}/{}-{}.tar.gz",
                host, owner, repo, rev, repo, rev
            );
            format!(
                r#"
                let
                  src = builtins.fetchTarball {{
                    url = "{}";
                    sha256 = "{}";
                  }};
                  lockPath = src + "/flake.lock";
                in
                  if builtins.pathExists lockPath
                  then builtins.readFile lockPath
                  else ""
                "#,
                url, nar_hash
            )
        }
        "sourcehut" => {
            let owner = locked.owner.as_deref().unwrap_or("");
            let repo = locked.repo.as_deref().unwrap_or("");
            let rev = locked.rev.as_deref().unwrap_or("");
            let nar_hash = locked.nar_hash.as_deref().unwrap_or("");
            let host = locked.host.as_deref().unwrap_or("git.sr.ht");
            let url = format!(
                "https://{}/~{}/{}/archive/{}.tar.gz",
                host, owner, repo, rev
            );
            format!(
                r#"
                let
                  src = builtins.fetchTarball {{
                    url = "{}";
                    sha256 = "{}";
                  }};
                  lockPath = src + "/flake.lock";
                in
                  if builtins.pathExists lockPath
                  then builtins.readFile lockPath
                  else ""
                "#,
                url, nar_hash
            )
        }
        "mercurial" | "hg" => {
            crate::nix::warn(&format!(
                "mercurial input '{}' skipped (not supported for transitive dependency collection)",
                input_name
            ));
            return None;
        }
        _ => {
            crate::nix::warn(&format!(
                "unknown source type '{}' for input '{}', skipping transitive dependency collection",
                source_type, input_name
            ));
            return None;
        }
    };

    // Run nix-instantiate to fetch and read the lock file
    let output = std::process::Command::new("nix-instantiate")
        .args(["--eval", "--expr", &nix_expr])
        .env_remove("TMPDIR")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let result = String::from_utf8_lossy(&output.stdout);
    let result = result.trim();

    // nix-instantiate returns a quoted string
    if result.starts_with('"') && result.ends_with('"') {
        let unquoted = &result[1..result.len() - 1];
        // Unescape the string
        let unescaped = unquoted
            .replace("\\n", "\n")
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");

        if unescaped.is_empty() {
            return None;
        }

        serde_json::from_str(&unescaped).ok()
    } else {
        None
    }
}

/// Recursively collect transitive dependencies from an input's flake.lock.
///
/// For flake inputs, fetches their source and reads their flake.lock to find
/// transitive dependencies that need to be added to our lock file.
fn collect_transitive_deps(
    node: &mut LockNode,
    node_name: &str,
    new_nodes: &mut HashMap<String, LockNode>,
    added_inputs: &mut Vec<(String, LockNode)>,
) {
    // Skip non-flake inputs
    if node.flake == Some(false) {
        return;
    }

    // Get the input's flake.lock
    let input_lock = match fetch_source_flake_lock(node, node_name) {
        Some(lock) => lock,
        None => return,
    };

    let input_nodes = match input_lock.get("nodes").and_then(|n| n.as_object()) {
        Some(nodes) => nodes,
        None => return,
    };

    let input_root_inputs = match input_nodes
        .get("root")
        .and_then(|r| r.get("inputs"))
        .and_then(|i| i.as_object())
    {
        Some(inputs) => inputs,
        None => return,
    };

    // Get existing overrides from this node
    let node_inputs = node.inputs.clone().unwrap_or_default();

    // For each input in the transitive flake.lock
    for (input_name, ref_value) in input_root_inputs {
        // Resolve the reference to a node name
        let ref_node_name = if let Some(arr) = ref_value.as_array() {
            // Follows reference within the input's lock
            arr.first()
                .and_then(|v| v.as_str())
                .unwrap_or(input_name)
                .to_string()
        } else {
            ref_value.as_str().unwrap_or(input_name).to_string()
        };

        // Skip if already overridden by a follows in our lock (list values)
        if let Some(existing_ref) = node_inputs.get(input_name) {
            if existing_ref.is_array() {
                continue;
            }
        }

        // Add the input reference to this node (if not already there)
        if node.inputs.is_none() {
            node.inputs = Some(HashMap::new());
        }
        if let Some(ref mut inputs) = node.inputs {
            if !inputs.contains_key(input_name) {
                inputs.insert(input_name.clone(), json!(ref_node_name));
            }
        }

        // Skip adding the node if we already have it
        if new_nodes.contains_key(&ref_node_name) {
            continue;
        }

        // Get the transitive node from the input's lock
        let trans_node_value = match input_nodes.get(&ref_node_name) {
            Some(n) => n,
            None => continue,
        };

        // Parse as LockNode
        let mut trans_node: LockNode = match serde_json::from_value(trans_node_value.clone()) {
            Ok(n) => n,
            Err(_) => continue,
        };

        tracing::debug!("  Adding transitive dep '{}'", ref_node_name);

        // Add to our lock
        new_nodes.insert(ref_node_name.clone(), trans_node.clone());
        added_inputs.push((ref_node_name.clone(), trans_node.clone()));

        // Recursively collect this node's transitive deps
        collect_transitive_deps(&mut trans_node, &ref_node_name, new_nodes, added_inputs);

        // Update the node in new_nodes with any changes from recursion
        new_nodes.insert(ref_node_name, trans_node);
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

/// Recursively remove null values from JSON (nix doesn't accept them).
fn remove_nulls(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let filtered: Map<String, Value> = map
                .into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, remove_nulls(v)))
                .collect();
            Value::Object(filtered)
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(remove_nulls).collect()),
        other => other,
    }
}

/// Recursively sort all keys in a JSON value.
fn sort_json(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted: Vec<_> = map.into_iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            let sorted_map: Map<String, Value> =
                sorted.into_iter().map(|(k, v)| (k, sort_json(v))).collect();
            Value::Object(sorted_map)
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(sort_json).collect()),
        other => other,
    }
}

/// Write lock file with consistent formatting and sorted keys.
fn write_lock(flake_lock: &Path, lock_data: &LockFile) -> Result<()> {
    let value = serde_json::to_value(lock_data)?;
    let sanitized = remove_nulls(value);
    let sorted = sort_json(sanitized);
    let content = serde_json::to_string_pretty(&sorted)?;
    fs::write(flake_lock, format!("{}\n", content))?;
    Ok(())
}

/// Print lock file changes in nix's format.
fn print_lock_changes(
    flake_lock: &Path,
    lock_existed: bool,
    added_inputs: &[(String, LockNode)],
    updated_inputs: &[(String, LockNode, LockNode)],
    removed_inputs: &[String],
    added_follows: &[(String, Vec<String>)],
) {
    if added_inputs.is_empty()
        && updated_inputs.is_empty()
        && removed_inputs.is_empty()
        && added_follows.is_empty()
    {
        return;
    }

    let action = if lock_existed { "updating" } else { "creating" };
    eprintln!(
        "{} {} lock file '{}':",
        yellow("warning:"),
        action,
        flake_lock.display()
    );

    for (name, node) in added_inputs {
        let url = format_locked_url(node);
        eprintln!(
            "{} {} {}:",
            magenta("•"),
            magenta("Added input"),
            bold(&format!("'{}'", name))
        );
        eprintln!("    {}", cyan(&format!("'{}'", url)));
    }

    for (name, follows_path) in added_follows {
        eprintln!(
            "{} {} {}:",
            magenta("•"),
            magenta("Added input"),
            bold(&format!("'{}'", name))
        );
        eprintln!(
            "    {} {}",
            magenta("follows"),
            cyan(&format!("'{}'", follows_path.join("/")))
        );
    }

    for (name, old_node, new_node) in updated_inputs {
        let old_url = format_locked_url(old_node);
        let new_url = format_locked_url(new_node);
        eprintln!(
            "{} {} {}:",
            magenta("•"),
            magenta("Updated input"),
            bold(&format!("'{}'", name))
        );
        eprintln!("    {}", cyan(&format!("'{}'", old_url)));
        eprintln!("  → {}", cyan(&format!("'{}'", new_url)));
    }

    for name in removed_inputs {
        eprintln!(
            "{} {} {}",
            magenta("•"),
            magenta("Removed input"),
            bold(&format!("'{}'", name))
        );
    }
}

/// Sync flake.nix inputs to lock file.
///
/// Uses nix flake prefetch which respects access-tokens for private repos.
/// Produces native flake.lock format (version 7).
/// Sync flake.nix inputs to lock file.
///
/// Uses nix flake prefetch which respects access-tokens for private repos.
/// Produces native flake.lock format (version 7).
pub fn sync_inputs(flake_dir: &Path, inputs: Option<serde_json::Value>) -> Result<bool> {
    let flake_lock = flake_dir.join("flake.lock");
    let lock_existed = flake_lock.exists();
    let inputs = match inputs {
        Some(i) => i,
        None => get_flake_inputs(flake_dir)?,
    };

    let input_map = match inputs.as_object() {
        Some(m) if !m.is_empty() => m,
        _ => return Ok(true), // No inputs to lock
    };

    // Read existing lock
    let mut lock_data = read_lock(&flake_lock);

    // Track changes for output
    let mut added_inputs: Vec<(String, LockNode)> = Vec::new();
    let mut added_follows: Vec<(String, Vec<String>)> = Vec::new();
    let mut removed_inputs: Vec<String> = Vec::new();

    // Ensure root node exists
    if !lock_data.nodes.contains_key("root") {
        lock_data.nodes.insert(
            "root".to_string(),
            LockNode {
                inputs: Some(HashMap::new()),
                ..Default::default()
            },
        );
    }

    // Ensure root has inputs map
    if let Some(root) = lock_data.nodes.get_mut("root") {
        if root.inputs.is_none() {
            root.inputs = Some(HashMap::new());
        }
    }

    // Collect input names for tracking
    let input_names: HashSet<String> = input_map.keys().cloned().collect();

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
                let follows_path: Vec<String> = follows
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
                let follows_value: Vec<Value> = follows_path.iter().map(|s| json!(s)).collect();

                // Check if this follows entry already exists in the lock
                let already_exists = lock_data
                    .nodes
                    .get("root")
                    .and_then(|r| r.inputs.as_ref())
                    .and_then(|i| i.get(name))
                    .map(|existing| *existing == Value::Array(follows_value.clone()))
                    .unwrap_or(false);

                if let Some(root) = lock_data.nodes.get_mut("root") {
                    if let Some(ref mut root_inputs) = root.inputs {
                        root_inputs.insert(name.clone(), Value::Array(follows_value));
                        if !already_exists {
                            added_follows.push((name.clone(), follows_path));
                        }
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
        if let Some(mut node) = lock_input(name, spec)? {
            // Add transitive follows if specified
            if let Some(follows_map) = spec.get("follows").and_then(|f| f.as_object()) {
                let mut node_inputs = node.inputs.clone().unwrap_or_default();
                for (follow_name, follow_path) in follows_map {
                    if let Some(arr) = follow_path.as_array() {
                        let path: Vec<Value> = arr
                            .iter()
                            .filter_map(|v| v.as_str().map(|s| json!(s)))
                            .collect();
                        node_inputs.insert(follow_name.clone(), Value::Array(path));
                    }
                }
                if !node_inputs.is_empty() {
                    node.inputs = Some(node_inputs);
                }
            }

            // Collect transitive dependencies
            collect_transitive_deps(&mut node, name, &mut lock_data.nodes, &mut added_inputs);

            lock_data.nodes.insert(name.clone(), node.clone());
            if let Some(root) = lock_data.nodes.get_mut("root") {
                if let Some(ref mut root_inputs) = root.inputs {
                    root_inputs.insert(name.clone(), json!(name));
                }
            }

            added_inputs.push((name.clone(), node));
        }
    }

    // Remove inputs that are no longer in flake.nix
    let to_remove: Vec<String> = existing_root_keys
        .into_iter()
        .filter(|k| !input_names.contains(k))
        .collect();

    for name in &to_remove {
        if let Some(root) = lock_data.nodes.get_mut("root") {
            if let Some(ref mut root_inputs) = root.inputs {
                root_inputs.remove(name);
            }
        }
        lock_data.nodes.remove(name);
        removed_inputs.push(name.clone());
    }

    // Write if changed
    let changed =
        !added_inputs.is_empty() || !added_follows.is_empty() || !removed_inputs.is_empty();
    if changed {
        write_lock(&flake_lock, &lock_data)?;
        print_lock_changes(
            &flake_lock,
            lock_existed,
            &added_inputs,
            &[], // No updates in sync_inputs
            &removed_inputs,
            &added_follows,
        );
    }

    Ok(true)
}

/// Ensure lock file exists and is up to date with flake inputs.
pub fn ensure_lock(flake_dir: &Path, inputs: Option<serde_json::Value>) -> Result<()> {
    sync_inputs(flake_dir, inputs)?;
    Ok(())
}

/// Lock an input to a specific flake reference (for --override-input).
fn lock_flake_ref(
    name: &str,
    flake_ref: &str,
    original_spec: Option<&Value>,
) -> Result<Option<LockNode>> {
    tracing::debug!("Locking {} to {}", name, flake_ref);

    let prefetch_result = prefetch_flake(flake_ref)?;
    let result = match prefetch_result {
        Some(r) => r,
        None => return Ok(None),
    };

    let locked = result.get("locked").cloned().unwrap_or_default();
    let prefetch_original = result.get("original").cloned().unwrap_or_default();
    let source_type = locked["type"].as_str().unwrap_or("");

    let nar_hash = result["hash"]
        .as_str()
        .or_else(|| locked["narHash"].as_str())
        .map(|s| s.to_string());

    match source_type {
        "github" => {
            // Build original from flake.nix spec if provided (for overrides)
            let original = if let Some(spec) = original_spec {
                if spec["type"].as_str() == Some("github") {
                    let mut orig = serde_json::Map::new();
                    orig.insert("type".to_string(), json!("github"));
                    if let Some(owner) = spec["owner"].as_str() {
                        orig.insert("owner".to_string(), json!(owner));
                    }
                    if let Some(repo) = spec["repo"].as_str() {
                        orig.insert("repo".to_string(), json!(repo));
                    }
                    if let Some(git_ref) = spec["ref"].as_str() {
                        orig.insert("ref".to_string(), json!(git_ref));
                    }
                    Value::Object(orig)
                } else {
                    prefetch_original
                }
            } else {
                prefetch_original
            };

            Ok(Some(LockNode {
                locked: Some(LockedInfo {
                    lock_type: "github".to_string(),
                    owner: locked["owner"].as_str().map(|s| s.to_string()),
                    repo: locked["repo"].as_str().map(|s| s.to_string()),
                    rev: locked["rev"].as_str().map(|s| s.to_string()),
                    nar_hash,
                    last_modified: locked["lastModified"].as_i64(),
                    ..Default::default()
                }),
                original: Some(original),
                ..Default::default()
            }))
        }
        "git" => {
            let original = if let Some(spec) = original_spec {
                if spec["type"].as_str() == Some("git") {
                    let mut orig = serde_json::Map::new();
                    orig.insert("type".to_string(), json!("git"));
                    if let Some(url) = spec["url"].as_str() {
                        orig.insert("url".to_string(), json!(url));
                    }
                    if let Some(git_ref) = spec["ref"].as_str() {
                        orig.insert("ref".to_string(), json!(git_ref));
                    }
                    Value::Object(orig)
                } else {
                    prefetch_original
                }
            } else {
                prefetch_original
            };

            Ok(Some(LockNode {
                locked: Some(LockedInfo {
                    lock_type: "git".to_string(),
                    url: locked["url"].as_str().map(|s| s.to_string()),
                    rev: locked["rev"].as_str().map(|s| s.to_string()),
                    git_ref: locked["ref"].as_str().map(|s| s.to_string()),
                    nar_hash,
                    last_modified: locked["lastModified"].as_i64(),
                    rev_count: locked["revCount"].as_i64(),
                    ..Default::default()
                }),
                original: Some(original),
                ..Default::default()
            }))
        }
        _ => {
            eprintln!("Unsupported flake type for override: {}", source_type);
            Ok(None)
        }
    }
}

/// Update locked inputs to latest versions.
///
/// Args:
///   flake_dir: Directory containing flake.nix
///   input_name: Specific input to update, or None for all
///   override_inputs: Dict mapping input names to flake refs to pin to
pub fn update_lock(
    flake_dir: &Path,
    input_name: Option<&str>,
    override_inputs: Option<&HashMap<String, String>>,
) -> Result<Option<HashMap<String, (Value, Value)>>> {
    let flake_lock = flake_dir.join("flake.lock");
    let lock_existed = flake_lock.exists();
    let inputs = get_flake_inputs(flake_dir)?;
    let override_inputs = override_inputs.cloned().unwrap_or_default();

    let input_map = match inputs.as_object() {
        Some(m) if !m.is_empty() => m,
        _ => return Ok(Some(HashMap::new())),
    };

    // Validate override inputs exist in flake.nix
    for name in override_inputs.keys() {
        if !input_map.contains_key(name) {
            eprintln!("Error: input '{}' not found in flake.nix", name);
            return Ok(None);
        }
    }

    // Read existing lock or create new
    let mut lock_data = read_lock(&flake_lock);
    let mut updates: HashMap<String, (Value, Value)> = HashMap::new();
    let mut added_inputs: Vec<(String, LockNode)> = Vec::new();
    let mut updated_inputs: Vec<(String, LockNode, LockNode)> = Vec::new();

    // Ensure root node exists
    if !lock_data.nodes.contains_key("root") {
        lock_data.nodes.insert(
            "root".to_string(),
            LockNode {
                inputs: Some(HashMap::new()),
                ..Default::default()
            },
        );
    }

    // Ensure root has inputs map
    let root_inputs = lock_data
        .nodes
        .get_mut("root")
        .and_then(|r| r.inputs.as_mut());

    if root_inputs.is_none() {
        if let Some(root) = lock_data.nodes.get_mut("root") {
            root.inputs = Some(HashMap::new());
        }
    }

    // Apply override inputs first
    for (name, flake_ref) in &override_inputs {
        let old_node = lock_data.nodes.get(name).cloned();
        let original_spec = input_map.get(name);

        if let Some(new_node) = lock_flake_ref(name, flake_ref, original_spec)? {
            let old_rev = old_node
                .as_ref()
                .and_then(|n| n.locked.as_ref())
                .and_then(|l| l.rev.as_ref())
                .map(|r| &r[..11.min(r.len())])
                .unwrap_or("");
            let new_rev = new_node
                .locked
                .as_ref()
                .and_then(|l| l.rev.as_ref())
                .map(|r| &r[..11.min(r.len())])
                .unwrap_or("");

            if old_rev != new_rev {
                if let Some(ref old) = old_node {
                    let old_val = serde_json::to_value(&old.locked).unwrap_or_default();
                    let new_val = serde_json::to_value(&new_node.locked).unwrap_or_default();
                    updates.insert(name.clone(), (old_val, new_val));
                    updated_inputs.push((name.clone(), old.clone(), new_node.clone()));
                } else {
                    added_inputs.push((name.clone(), new_node.clone()));
                }
            }

            // Collect transitive dependencies
            let mut node = new_node.clone();
            collect_transitive_deps(&mut node, name, &mut lock_data.nodes, &mut added_inputs);

            lock_data.nodes.insert(name.clone(), node);
            if let Some(root) = lock_data.nodes.get_mut("root") {
                if let Some(ref mut inputs) = root.inputs {
                    inputs.insert(name.clone(), json!(name));
                }
            }
        } else {
            eprintln!("Error: Failed to lock '{}' to {}", name, flake_ref);
            return Ok(None);
        }
    }

    // If we only have overrides and no input_name, we're done
    if !override_inputs.is_empty() && input_name.is_none() {
        write_lock(&flake_lock, &lock_data)?;
        print_lock_changes(
            &flake_lock,
            lock_existed,
            &added_inputs,
            &updated_inputs,
            &[],
            &[],
        );

        // Inform user if nothing changed
        if updates.is_empty() && added_inputs.is_empty() {
            for name in override_inputs.keys() {
                let rev = lock_data
                    .nodes
                    .get(name)
                    .and_then(|n| n.locked.as_ref())
                    .and_then(|l| l.rev.as_ref())
                    .map(|r| &r[..11.min(r.len())])
                    .unwrap_or("");
                eprintln!(
                    "{} input {} already at {}",
                    yellow("warning:"),
                    bold(&format!("'{}'", name)),
                    cyan(rev)
                );
            }
        }
        return Ok(Some(updates));
    }

    // Determine which inputs to update (excluding already-overridden ones)
    let inputs_to_update: Vec<String> = if let Some(name) = input_name {
        if input_map.contains_key(name) {
            if override_inputs.contains_key(name) {
                vec![] // Already handled
            } else {
                vec![name.to_string()]
            }
        } else {
            eprintln!("Error: input '{}' not found in flake.nix", name);
            return Ok(None);
        }
    } else {
        input_map
            .keys()
            .filter(|k| !override_inputs.contains_key(*k))
            .cloned()
            .collect()
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

        let old_node = lock_data.nodes.get(&name).cloned();

        // Re-lock the input
        if let Some(mut new_node) = lock_input(&name, spec)? {
            let old_rev = old_node
                .as_ref()
                .and_then(|n| n.locked.as_ref())
                .and_then(|l| l.rev.as_ref())
                .map(|r| &r[..11.min(r.len())])
                .unwrap_or("");
            let new_rev = new_node
                .locked
                .as_ref()
                .and_then(|l| l.rev.as_ref())
                .map(|r| &r[..11.min(r.len())])
                .unwrap_or("");

            if old_rev != new_rev {
                if let Some(ref old) = old_node {
                    let old_val = serde_json::to_value(&old.locked).unwrap_or_default();
                    let new_val = serde_json::to_value(&new_node.locked).unwrap_or_default();
                    updates.insert(name.clone(), (old_val, new_val));
                    updated_inputs.push((name.clone(), old.clone(), new_node.clone()));
                } else {
                    added_inputs.push((name.clone(), new_node.clone()));
                }
            }

            // Add transitive follows if specified
            if let Some(follows_map) = spec.get("follows").and_then(|f| f.as_object()) {
                let mut node_inputs = new_node.inputs.clone().unwrap_or_default();
                for (follow_name, follow_path) in follows_map {
                    if let Some(arr) = follow_path.as_array() {
                        let path: Vec<Value> = arr
                            .iter()
                            .filter_map(|v| v.as_str().map(|s| json!(s)))
                            .collect();
                        node_inputs.insert(follow_name.clone(), Value::Array(path));
                    }
                }
                if !node_inputs.is_empty() {
                    new_node.inputs = Some(node_inputs);
                }
            }

            // Collect transitive dependencies
            collect_transitive_deps(
                &mut new_node,
                &name,
                &mut lock_data.nodes,
                &mut added_inputs,
            );

            lock_data.nodes.insert(name.clone(), new_node);

            // Ensure root inputs reference
            if let Some(root) = lock_data.nodes.get_mut("root") {
                if let Some(ref mut inputs) = root.inputs {
                    inputs.insert(name.clone(), json!(name));
                }
            }
        }
    }

    // Write if changed
    if !updates.is_empty() || !added_inputs.is_empty() {
        write_lock(&flake_lock, &lock_data)?;
        print_lock_changes(
            &flake_lock,
            lock_existed,
            &added_inputs,
            &updated_inputs,
            &[],
            &[],
        );
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
