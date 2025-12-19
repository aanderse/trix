//! Flake registry resolution.
//!
//! Reads nix flake registries to resolve short names like 'nixpkgs' to their
//! full flake references. Supports:
//! - User registry: ~/.config/nix/registry.json
//! - System registry: /etc/nix/registry.json
//! - Global registry: https://channels.nixos.org/flake-registry.json (cached)

use anyhow::Result;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const GLOBAL_REGISTRY_URL: &str = "https://channels.nixos.org/flake-registry.json";
const CACHE_TTL: Duration = Duration::from_secs(3600); // 1 hour

/// Cache for global registry
static GLOBAL_REGISTRY_CACHE: Lazy<Mutex<Option<(RegistryFile, Instant)>>> =
    Lazy::new(|| Mutex::new(None));

/// A resolved registry entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntry {
    #[serde(rename = "type")]
    pub entry_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Registry file structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RegistryFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    flakes: Vec<RegistryFlakeEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegistryFlakeEntry {
    from: RegistryFrom,
    to: RegistryTo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegistryFrom {
    #[serde(rename = "type")]
    from_type: String,
    #[serde(default)]
    id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegistryTo {
    #[serde(rename = "type")]
    to_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    git_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rev: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

/// Get the user registry path.
fn get_user_registry_path() -> PathBuf {
    let config_home = env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
        dirs::home_dir()
            .map(|h| h.join(".config").display().to_string())
            .unwrap_or_else(|| "~/.config".to_string())
    });
    PathBuf::from(config_home).join("nix").join("registry.json")
}

/// Get the system registry path.
fn get_system_registry_path() -> PathBuf {
    PathBuf::from("/etc/nix/registry.json")
}

/// Load a registry file, returning empty registry if not found.
fn load_registry_file(path: &PathBuf) -> RegistryFile {
    if !path.exists() {
        return RegistryFile::default();
    }

    match fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => RegistryFile::default(),
    }
}

/// Fetch and cache the global registry.
fn fetch_global_registry() -> RegistryFile {
    // Check cache first
    {
        let cache = GLOBAL_REGISTRY_CACHE.lock().unwrap();
        if let Some((ref registry, ref time)) = *cache {
            if time.elapsed() < CACHE_TTL {
                return registry.clone();
            }
        }
    }

    // Fetch from network
    let registry = match reqwest::blocking::Client::new()
        .get(GLOBAL_REGISTRY_URL)
        .timeout(Duration::from_secs(5))
        .send()
    {
        Ok(response) => match response.json::<RegistryFile>() {
            Ok(data) => data,
            Err(_) => {
                let cache = GLOBAL_REGISTRY_CACHE.lock().unwrap();
                return cache.as_ref().map(|(r, _)| r.clone()).unwrap_or_default();
            }
        },
        Err(_) => {
            let cache = GLOBAL_REGISTRY_CACHE.lock().unwrap();
            return cache.as_ref().map(|(r, _)| r.clone()).unwrap_or_default();
        }
    };

    // Update cache
    {
        let mut cache = GLOBAL_REGISTRY_CACHE.lock().unwrap();
        *cache = Some((registry.clone(), Instant::now()));
    }

    registry
}

/// Parse a registry 'to' entry into a RegistryEntry.
fn parse_registry_entry(entry: &RegistryFlakeEntry) -> Option<RegistryEntry> {
    let to = &entry.to;

    match to.to_type.as_str() {
        "path" => Some(RegistryEntry {
            entry_type: "path".to_string(),
            path: to.path.clone(),
            owner: None,
            repo: None,
            git_ref: None,
            rev: None,
            url: None,
        }),
        "github" => Some(RegistryEntry {
            entry_type: "github".to_string(),
            path: None,
            owner: to.owner.clone(),
            repo: to.repo.clone(),
            git_ref: to.git_ref.clone(),
            rev: to.rev.clone(),
            url: None,
        }),
        "git" => Some(RegistryEntry {
            entry_type: "git".to_string(),
            path: None,
            owner: None,
            repo: None,
            git_ref: to.git_ref.clone(),
            rev: to.rev.clone(),
            url: to.url.clone(),
        }),
        _ => None,
    }
}

/// Search a registry for a name, return the resolved entry.
fn search_registry(registry: &RegistryFile, name: &str) -> Option<RegistryEntry> {
    for entry in &registry.flakes {
        if entry.from.from_type == "indirect" && entry.from.id == name {
            return parse_registry_entry(entry);
        }
    }
    None
}

/// Resolve a registry name to its target.
///
/// Searches in order:
/// 1. User registry (~/.config/nix/registry.json)
/// 2. System registry (/etc/nix/registry.json)
/// 3. Global registry (https://channels.nixos.org/flake-registry.json)
pub fn resolve_registry_name(name: &str, use_global: bool) -> Option<RegistryEntry> {
    // Check user registry first
    let user_registry = load_registry_file(&get_user_registry_path());
    if let Some(result) = search_registry(&user_registry, name) {
        return Some(result);
    }

    // Check system registry
    let system_registry = load_registry_file(&get_system_registry_path());
    if let Some(result) = search_registry(&system_registry, name) {
        return Some(result);
    }

    // Check global registry
    if use_global {
        let global_registry = fetch_global_registry();
        if let Some(result) = search_registry(&global_registry, name) {
            return Some(result);
        }
    }

    None
}

/// Convert a registry entry to a flake reference string.
pub fn registry_entry_to_flake_ref(entry: &RegistryEntry) -> String {
    match entry.entry_type.as_str() {
        "path" => entry.path.clone().unwrap_or_default(),
        "github" => {
            let owner = entry.owner.as_deref().unwrap_or("");
            let repo = entry.repo.as_deref().unwrap_or("");
            let mut flake_ref = format!("github:{}/{}", owner, repo);

            if let Some(ref rev) = entry.rev {
                flake_ref.push('/');
                flake_ref.push_str(rev);
            } else if let Some(ref git_ref) = entry.git_ref {
                flake_ref.push('/');
                flake_ref.push_str(git_ref);
            }
            flake_ref
        }
        "git" => {
            let url = entry.url.as_deref().unwrap_or("");
            let mut flake_ref = format!("git+{}", url);

            let mut params = Vec::new();
            if let Some(ref git_ref) = entry.git_ref {
                params.push(format!("ref={}", git_ref));
            }
            if let Some(ref rev) = entry.rev {
                params.push(format!("rev={}", rev));
            }
            if !params.is_empty() {
                flake_ref.push('?');
                flake_ref.push_str(&params.join("&"));
            }
            flake_ref
        }
        _ => String::new(),
    }
}

/// Check if a reference looks like a registry name (not a path or full ref).
///
/// Registry names are simple identifiers like 'nixpkgs', 'home-manager'.
/// Not paths (., ./, /, ~) or full refs (github:, git+, path:).
pub fn is_registry_name(ref_str: &str) -> bool {
    // Empty or path-like
    if ref_str.is_empty()
        || ref_str.starts_with('.')
        || ref_str.starts_with('/')
        || ref_str.starts_with('~')
    {
        return false;
    }

    // Full flake reference (has colon like github: or git+)
    if ref_str.contains(':') {
        return false;
    }

    // Contains # (has attribute part) - get base
    let base = ref_str.split('#').next().unwrap_or("");

    // Check if base is a simple identifier (alphanumeric + hyphen + underscore)
    !base.is_empty()
        && base
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

/// Parse a flake reference string into a registry 'to' entry.
fn parse_flake_ref_to_entry(ref_str: &str) -> RegistryTo {
    // Local path
    if ref_str.starts_with('/')
        || ref_str.starts_with('~')
        || ref_str.starts_with("./")
        || ref_str.starts_with("../")
    {
        let path = shellexpand::tilde(ref_str).to_string();
        let path = std::fs::canonicalize(&path)
            .map(|p| p.display().to_string())
            .unwrap_or(path);
        return RegistryTo {
            to_type: "path".to_string(),
            path: Some(path),
            owner: None,
            repo: None,
            git_ref: None,
            rev: None,
            url: None,
        };
    }

    // path: prefix
    if let Some(rest) = ref_str.strip_prefix("path:") {
        let path = shellexpand::tilde(rest).to_string();
        let path = std::fs::canonicalize(&path)
            .map(|p| p.display().to_string())
            .unwrap_or(path);
        return RegistryTo {
            to_type: "path".to_string(),
            path: Some(path),
            owner: None,
            repo: None,
            git_ref: None,
            rev: None,
            url: None,
        };
    }

    // github: reference
    if let Some(rest) = ref_str.strip_prefix("github:") {
        let (rest, query_params) = parse_query_params(rest);
        let parts: Vec<&str> = rest.split('/').collect();

        let owner = parts.first().unwrap_or(&"").to_string();
        let repo = parts.get(1).unwrap_or(&"").to_string();
        let mut git_ref = parts.get(2).map(|s| s.to_string());
        let mut rev = None;

        if let Some(r) = query_params.get("ref") {
            git_ref = Some(r.clone());
        }
        if let Some(r) = query_params.get("rev") {
            rev = Some(r.clone());
        }

        return RegistryTo {
            to_type: "github".to_string(),
            path: None,
            owner: Some(owner),
            repo: Some(repo),
            git_ref,
            rev,
            url: None,
        };
    }

    // git+ reference
    if let Some(rest) = ref_str.strip_prefix("git+") {
        let (url, query_params) = parse_query_params(rest);

        return RegistryTo {
            to_type: "git".to_string(),
            path: None,
            owner: None,
            repo: None,
            git_ref: query_params.get("ref").cloned(),
            rev: query_params.get("rev").cloned(),
            url: Some(url.to_string()),
        };
    }

    // Fallback: treat as path
    let path = shellexpand::tilde(ref_str).to_string();
    let path = std::fs::canonicalize(&path)
        .map(|p| p.display().to_string())
        .unwrap_or(path);
    RegistryTo {
        to_type: "path".to_string(),
        path: Some(path),
        owner: None,
        repo: None,
        git_ref: None,
        rev: None,
        url: None,
    }
}

fn parse_query_params(s: &str) -> (&str, HashMap<String, String>) {
    let mut params = HashMap::new();
    if let Some((base, query)) = s.split_once('?') {
        for part in query.split('&') {
            if let Some((k, v)) = part.split_once('=') {
                params.insert(k.to_string(), v.to_string());
            }
        }
        (base, params)
    } else {
        (s, params)
    }
}

/// Save the user registry file.
fn save_user_registry(registry: &RegistryFile) -> Result<()> {
    let path = get_user_registry_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(registry)?;
    fs::write(&path, format!("{}\n", content))?;
    Ok(())
}

/// List all registry entries from all sources.
///
/// Returns a list of (name, source, entry) tuples where source is "user", "system", or "global".
pub fn list_all_registries(use_global: bool) -> Vec<(String, String, RegistryEntry)> {
    let mut results = Vec::new();

    // User registry
    let user_registry = load_registry_file(&get_user_registry_path());
    for entry in &user_registry.flakes {
        if entry.from.from_type == "indirect" {
            if let Some(parsed) = parse_registry_entry(entry) {
                results.push((entry.from.id.clone(), "user".to_string(), parsed));
            }
        }
    }

    // System registry
    let system_registry = load_registry_file(&get_system_registry_path());
    for entry in &system_registry.flakes {
        if entry.from.from_type == "indirect" {
            if let Some(parsed) = parse_registry_entry(entry) {
                results.push((entry.from.id.clone(), "system".to_string(), parsed));
            }
        }
    }

    // Global registry
    if use_global {
        let global_registry = fetch_global_registry();
        for entry in &global_registry.flakes {
            if entry.from.from_type == "indirect" {
                if let Some(parsed) = parse_registry_entry(entry) {
                    results.push((entry.from.id.clone(), "global".to_string(), parsed));
                }
            }
        }
    }

    results
}

/// Add an entry to the user registry.
pub fn add_registry_entry(name: &str, target: &str) -> Result<()> {
    let mut user_registry = load_registry_file(&get_user_registry_path());

    // Ensure structure
    if user_registry.version == 0 {
        user_registry.version = 2;
    }

    // Remove existing entry with same name
    user_registry
        .flakes
        .retain(|e| !(e.from.from_type == "indirect" && e.from.id == name));

    // Add new entry
    user_registry.flakes.push(RegistryFlakeEntry {
        from: RegistryFrom {
            from_type: "indirect".to_string(),
            id: name.to_string(),
        },
        to: parse_flake_ref_to_entry(target),
    });

    save_user_registry(&user_registry)
}

/// Remove an entry from the user registry.
///
/// Returns true if entry was found and removed, false otherwise.
pub fn remove_registry_entry(name: &str) -> Result<bool> {
    let mut user_registry = load_registry_file(&get_user_registry_path());

    let original_count = user_registry.flakes.len();

    // Filter out the entry
    user_registry
        .flakes
        .retain(|e| !(e.from.from_type == "indirect" && e.from.id == name));

    if user_registry.flakes.len() < original_count {
        save_user_registry(&user_registry)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_registry_name() {
        assert!(is_registry_name("nixpkgs"));
        assert!(is_registry_name("home-manager"));
        assert!(!is_registry_name("."));
        assert!(!is_registry_name("./foo"));
        assert!(!is_registry_name("/foo"));
        assert!(!is_registry_name("~/foo"));
        assert!(!is_registry_name("github:NixOS/nixpkgs"));
        assert!(!is_registry_name("path:/foo"));
    }

    #[test]
    fn test_parse_query_params() {
        let (base, params) = parse_query_params("foo?ref=master&rev=123");
        assert_eq!(base, "foo");
        assert_eq!(params.get("ref").unwrap(), "master");
        assert_eq!(params.get("rev").unwrap(), "123");

        let (base, params) = parse_query_params("bar");
        assert_eq!(base, "bar");
        assert!(params.is_empty());
    }

    #[test]
    fn test_registry_entry_with_git_ref() {
        let entry = RegistryEntry {
            entry_type: "github".to_string(),
            path: None,
            owner: Some("NixOS".to_string()),
            repo: Some("nixpkgs".to_string()),
            git_ref: Some("nixos-unstable".to_string()),
            rev: None,
            url: None,
        };
        assert_eq!(
            registry_entry_to_flake_ref(&entry),
            "github:NixOS/nixpkgs/nixos-unstable"
        );
    }

    #[test]
    fn test_parse_flake_ref_to_entry() {
        let entry = parse_flake_ref_to_entry("github:owner/repo?ref=main");
        assert_eq!(entry.to_type, "github");
        assert_eq!(entry.owner.unwrap(), "owner");
        assert_eq!(entry.repo.unwrap(), "repo");
        assert_eq!(entry.git_ref.unwrap(), "main");

        let entry = parse_flake_ref_to_entry("path:/tmp/foo");
        assert_eq!(entry.to_type, "path");
        assert!(entry.path.unwrap().ends_with("foo"));
    }
}
