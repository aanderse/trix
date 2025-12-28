//! Flake handling - parsing, URL resolution, lock management.

use anyhow::Result;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::registry::{is_registry_name, registry_entry_to_flake_ref, resolve_registry_name};

/// Cache for flake inputs per directory (canonical path -> inputs JSON)
static FLAKE_INPUTS_CACHE: Lazy<Mutex<HashMap<PathBuf, serde_json::Value>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Result of resolving an installable reference.
///
/// Either local (flake_dir is set) or remote (flake_ref is set).
#[derive(Debug, Clone)]
pub struct ResolvedInstallable {
    pub is_local: bool,
    pub attr_part: String,
    pub flake_dir: Option<PathBuf>, // For local flakes
    pub flake_ref: Option<String>,  // For remote refs (e.g., "github:NixOS/nixpkgs")
}

/// Structured flake source information.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum FlakeSource {
    Github {
        owner: String,
        repo: String,
        #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        flake: Option<bool>,
    },
    Sourcehut {
        owner: String,
        repo: String,
        #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        flake: Option<bool>,
    },
    Gitlab {
        owner: String,
        repo: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        flake: Option<bool>,
    },
    Git {
        url: String,
        #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        flake: Option<bool>,
    },
    Path {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        flake: Option<bool>,
    },
    Follows {
        follows: Vec<String>,
    },
    Unknown {
        url: String,
    },
}

/// Parse a flake URL into structured components.
pub fn parse_flake_url(url: &str) -> FlakeSource {
    // Handle query parameters
    let (url_base, query_params) = if let Some((base, query)) = url.split_once('?') {
        let params: std::collections::HashMap<_, _> = query
            .split('&')
            .filter_map(|part| part.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        (base, params)
    } else {
        (url, std::collections::HashMap::new())
    };

    let git_ref = query_params.get("ref").cloned();
    let rev = query_params.get("rev").cloned();

    // Parse by type
    if let Some(rest) = url_base.strip_prefix("github:") {
        let parts: Vec<&str> = rest.split('/').collect();
        return FlakeSource::Github {
            owner: parts.first().unwrap_or(&"").to_string(),
            repo: parts.get(1).unwrap_or(&"").to_string(),
            git_ref: git_ref.or_else(|| parts.get(2).map(|s| s.to_string())),
            rev,
            flake: None,
        };
    }

    if let Some(rest) = url_base.strip_prefix("sourcehut:") {
        let parts: Vec<&str> = rest.split('/').collect();
        return FlakeSource::Sourcehut {
            owner: parts.first().unwrap_or(&"").to_string(),
            repo: parts.get(1).unwrap_or(&"").to_string(),
            git_ref: git_ref.or_else(|| parts.get(2).map(|s| s.to_string())),
            rev,
            flake: None,
        };
    }

    if let Some(rest) = url_base.strip_prefix("gitlab:") {
        let parts: Vec<&str> = rest.split('/').collect();
        return FlakeSource::Gitlab {
            owner: parts.first().unwrap_or(&"").to_string(),
            repo: parts.get(1).unwrap_or(&"").to_string(),
            host: query_params.get("host").cloned(),
            git_ref: git_ref.or_else(|| parts.get(2).map(|s| s.to_string())),
            rev,
            flake: None,
        };
    }

    if let Some(rest) = url_base.strip_prefix("git+") {
        return FlakeSource::Git {
            url: rest.to_string(),
            git_ref,
            rev,
            flake: None,
        };
    }

    if let Some(rest) = url_base.strip_prefix("path:") {
        return FlakeSource::Path {
            path: rest.to_string(),
            flake: None,
        };
    }

    if url_base.starts_with('/') || url_base.starts_with("./") || url_base.starts_with("../") {
        return FlakeSource::Path {
            path: url_base.to_string(),
            flake: None,
        };
    }

    // Unknown format
    FlakeSource::Unknown {
        url: url_base.to_string(),
    }
}

/// Extract inputs from flake.nix by evaluating with nix-instantiate.
///
/// Returns a map of input names to their specs.
/// Results are cached per canonical path.
pub fn get_flake_inputs(flake_dir: &Path) -> Result<serde_json::Value> {
    // Canonicalize path for cache key
    let canonical = flake_dir
        .canonicalize()
        .unwrap_or_else(|_| flake_dir.to_path_buf());

    // Check cache first
    {
        let cache = FLAKE_INPUTS_CACHE.lock().unwrap();
        if let Some(inputs) = cache.get(&canonical) {
            return Ok(inputs.clone());
        }
    }

    let nix_dir = crate::nix::get_nix_dir()?;
    let expr = format!(
        "import {}/flake_inputs.nix {{ flakePath = {}; }}",
        nix_dir.display(),
        flake_dir.display()
    );

    let mut cmd = crate::command::NixCommand::new("nix-instantiate");
    cmd.args(["--eval", "--json", "--strict", "--expr", &expr]);

    let raw_inputs: Vec<serde_json::Value> = match cmd.json() {
        Ok(inputs) => inputs,
        Err(_) => return Ok(serde_json::json!({})),
    };

    if raw_inputs.is_empty() {
        return Ok(serde_json::json!({}));
    }

    // Convert raw data to parsed format
    let mut parsed = serde_json::Map::new();

    for raw in raw_inputs {
        let name = raw["name"].as_str().unwrap_or("");

        // Check for root-level follows first
        if let Some(follows) = raw["follows"].as_str() {
            let follows_path: Vec<String> = follows.split('/').map(|s| s.to_string()).collect();
            parsed.insert(
                name.to_string(),
                serde_json::to_value(FlakeSource::Follows {
                    follows: follows_path,
                })?,
            );
            continue;
        }

        // Regular input with URL
        if let Some(url) = raw["url"].as_str() {
            let mut source = parse_flake_url(url);

            // Check for flake = false
            let is_flake = raw["flake"].as_bool();

            // Update the source with flake setting if present
            source = match source {
                FlakeSource::Github {
                    owner,
                    repo,
                    git_ref,
                    rev,
                    ..
                } => FlakeSource::Github {
                    owner,
                    repo,
                    git_ref,
                    rev,
                    flake: is_flake,
                },
                FlakeSource::Sourcehut {
                    owner,
                    repo,
                    git_ref,
                    rev,
                    ..
                } => FlakeSource::Sourcehut {
                    owner,
                    repo,
                    git_ref,
                    rev,
                    flake: is_flake,
                },
                FlakeSource::Git {
                    url, git_ref, rev, ..
                } => FlakeSource::Git {
                    url,
                    git_ref,
                    rev,
                    flake: is_flake,
                },
                FlakeSource::Path { path, .. } => FlakeSource::Path {
                    path,
                    flake: is_flake,
                },
                other => other,
            };

            // Add nested follows if present
            let mut spec = serde_json::to_value(source)?;
            if let Some(nested) = raw["nestedFollows"].as_object() {
                if !nested.is_empty() {
                    let mut follows_map = serde_json::Map::new();
                    for (nested_name, follows_value) in nested {
                        if let Some(follows_str) = follows_value.as_str() {
                            let follows_path: Vec<String> =
                                follows_str.split('/').map(|s| s.to_string()).collect();
                            follows_map
                                .insert(nested_name.clone(), serde_json::json!(follows_path));
                        }
                    }
                    spec["follows"] = serde_json::Value::Object(follows_map);
                }
            }

            parsed.insert(name.to_string(), spec);
        }
    }

    let result = serde_json::Value::Object(parsed);

    // Cache the result
    {
        let mut cache = FLAKE_INPUTS_CACHE.lock().unwrap();
        cache.insert(canonical, result.clone());
    }

    Ok(result)
}

/// Extract description from flake.nix.
pub fn get_flake_description(flake_dir: &Path) -> Option<String> {
    let expr = format!(
        "(import {}/flake.nix).description or null",
        flake_dir.display()
    );

    let mut cmd = crate::command::NixCommand::new("nix-instantiate");
    cmd.args(["--eval", "--json", "--expr", &expr]);

    cmd.json::<Option<String>>()
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
}

/// Extract nixConfig from flake.nix.
pub fn get_nix_config(flake_dir: &Path, warn_unsupported: bool) -> serde_json::Value {
    use std::collections::HashSet;

    let supported_options: HashSet<&str> =
        ["bash-prompt", "bash-prompt-prefix", "bash-prompt-suffix"]
            .into_iter()
            .collect();

    // First, get all nixConfig attribute names to check for unsupported options
    let expr_all = format!(
        "builtins.attrNames ((import {}/flake.nix).nixConfig or {{}})",
        flake_dir.display()
    );

    if warn_unsupported {
        let mut cmd_all = crate::command::NixCommand::new("nix-instantiate");
        cmd_all.args(["--eval", "--json", "--expr", &expr_all]);

        if let Ok(all_options) = cmd_all.json::<Vec<String>>() {
            for opt in all_options {
                if !supported_options.contains(opt.as_str()) {
                    eprintln!("warning: nixConfig.{} is not supported by trix", opt);
                }
            }
        }
    }

    // Now get the supported options
    let nix_dir = crate::nix::get_nix_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let expr = format!(
        "import {}/flake_config.nix {{ flakePath = {}; }}",
        nix_dir.display(),
        flake_dir.display()
    );

    let mut cmd = crate::command::NixCommand::new("nix-instantiate");
    cmd.args(["--eval", "--json", "--strict", "--expr", &expr]);

    match cmd.json::<serde_json::Value>() {
        Ok(config) => {
            // Filter out null values
            if let serde_json::Value::Object(map) = config {
                let filtered: serde_json::Map<String, serde_json::Value> =
                    map.into_iter().filter(|(_, v)| !v.is_null()).collect();
                serde_json::Value::Object(filtered)
            } else {
                serde_json::json!({})
            }
        }
        _ => serde_json::json!({}),
    }
}

/// Resolve an installable reference, handling registry lookups.
///
/// This function determines whether an installable is:
/// 1. A local flake (path-based) - handled natively by trix
/// 2. A remote flake (github:, git+, etc.) - passed through to nix
/// 3. A registry name (nixpkgs, home-manager) - resolved via registry
pub fn resolve_installable(installable: &str) -> ResolvedInstallable {
    // Parse the installable to separate path/ref part from attribute
    let (ref_part, attr_part) = if let Some((r, a)) = installable.split_once('#') {
        (r, a.to_string())
    } else {
        (installable, "default".to_string())
    };

    // Case 1: Empty or current directory
    if ref_part.is_empty() || ref_part == "." {
        return ResolvedInstallable {
            is_local: true,
            attr_part,
            flake_dir: Some(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
            flake_ref: None,
        };
    }

    // Case 2: Explicit path (starts with /, ./, ../, ~, or path:)
    if ref_part.starts_with('/')
        || ref_part.starts_with("./")
        || ref_part.starts_with("../")
        || ref_part.starts_with('~')
        || ref_part.starts_with("path:")
    {
        let path = if let Some(rest) = ref_part.strip_prefix("path:") {
            rest
        } else {
            ref_part
        };
        let expanded = shellexpand::tilde(path).to_string();
        let resolved = PathBuf::from(&expanded)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(expanded));

        return ResolvedInstallable {
            is_local: true,
            attr_part,
            flake_dir: Some(resolved),
            flake_ref: None,
        };
    }

    // Case 3: Full flake reference (github:, git+, etc.)
    if ref_part.contains(':') {
        return ResolvedInstallable {
            is_local: false,
            attr_part,
            flake_dir: None,
            flake_ref: Some(ref_part.to_string()),
        };
    }

    // Case 4: Registry name (e.g., "nixpkgs", "home-manager")
    if is_registry_name(ref_part) {
        tracing::debug!("Looking up '{}' in flake registries...", ref_part);
        if let Some(entry) = resolve_registry_name(ref_part, true) {
            tracing::debug!(
                "Found '{}' in registry: type={}, path={:?}",
                ref_part,
                entry.entry_type,
                entry.path
            );
            if entry.entry_type == "path" {
                // Local path from registry - handle natively!
                let path = entry.path.unwrap_or_default();
                let expanded = shellexpand::tilde(&path).to_string();
                let resolved = PathBuf::from(&expanded)
                    .canonicalize()
                    .unwrap_or_else(|_| PathBuf::from(expanded));

                return ResolvedInstallable {
                    is_local: true,
                    attr_part,
                    flake_dir: Some(resolved),
                    flake_ref: None,
                };
            } else {
                // Remote ref from registry - passthrough to nix
                let flake_ref = registry_entry_to_flake_ref(&entry);
                tracing::debug!(
                    "Registry '{}' resolved to remote ref: {}",
                    ref_part,
                    flake_ref
                );
                return ResolvedInstallable {
                    is_local: false,
                    attr_part,
                    flake_dir: None,
                    flake_ref: Some(flake_ref),
                };
            }
        } else {
            // Registry name not found - still try as remote ref
            tracing::debug!("'{}' not found in any registry", ref_part);
            return ResolvedInstallable {
                is_local: false,
                attr_part,
                flake_dir: None,
                flake_ref: Some(ref_part.to_string()),
            };
        }
    }

    // Fallback: treat as local path
    let resolved = PathBuf::from(ref_part)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(ref_part));

    ResolvedInstallable {
        is_local: true,
        attr_part,
        flake_dir: Some(resolved),
        flake_ref: None,
    }
}

/// Check if a string looks like a Nix system identifier (e.g., x86_64-linux).
fn looks_like_system(s: &str) -> bool {
    s.contains('-')
}

/// Build full attribute path with system.
pub fn resolve_attr_path(attr_part: &str, default_category: &str, system: &str) -> String {
    // Known per-system output categories
    let per_system_categories = [
        "packages",
        "devShells",
        "apps",
        "checks",
        "legacyPackages",
        "formatter",
    ];

    // Known top-level (non-system) output categories
    let top_level_categories = [
        "lib",
        "overlays",
        "nixosModules",
        "nixosConfigurations",
        "darwinModules",
        "darwinConfigurations",
        "homeManagerModules",
        "templates",
        "defaultTemplate",
        "self",
    ];

    // Simple name like "hello" or "default" - most common case
    // Empty attr_part (from ".#") defaults to "default"
    if !attr_part.contains('.') {
        let name = if attr_part.is_empty() {
            "default"
        } else {
            attr_part
        };
        return format!("{}.{}.{}", default_category, system, name);
    }

    let parts: Vec<&str> = attr_part.split('.').collect();
    let first = parts[0];

    // Top-level outputs don't need system prefix
    if top_level_categories.contains(&first) {
        return attr_part.to_string();
    }

    // Per-system category (packages, devShells, etc.)
    if per_system_categories.contains(&first) {
        // Check if system is already present
        if parts.len() >= 3 && looks_like_system(parts[1]) {
            return attr_part.to_string();
        }
        // Insert system: "packages.foo" -> "packages.{system}.foo"
        return format!("{}.{}.{}", first, system, parts[1..].join("."));
    }

    // Unknown first component with dots - pass through as-is
    attr_part.to_string()
}

/// Ensure flake.lock exists with locked versions of flake inputs.
pub fn ensure_lock(flake_dir: &Path, inputs: Option<serde_json::Value>) -> Result<()> {
    use crate::lock::ensure_lock as lock_inputs;

    // Get input names from flake.nix if not provided
    let inputs = match inputs {
        Some(i) => i,
        None => get_flake_inputs(flake_dir)?,
    };

    if inputs.as_object().map(|m| m.is_empty()).unwrap_or(true) {
        // No inputs at all - skip entirely
        return Ok(());
    }

    let flake_lock = flake_dir.join("flake.lock");
    if !flake_lock.exists() {
        tracing::warn!("No flake.lock found. Locking flake inputs...");
    }

    lock_inputs(flake_dir, Some(inputs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_flake_url_github() {
        let res = parse_flake_url("github:NixOS/nixpkgs");
        if let FlakeSource::Github { owner, repo, .. } = res {
            assert_eq!(owner, "NixOS");
            assert_eq!(repo, "nixpkgs");
        } else {
            panic!("Expected Github source");
        }

        let res = parse_flake_url("github:NixOS/nixpkgs/nixos-unstable");
        if let FlakeSource::Github { git_ref, .. } = res {
            assert_eq!(git_ref, Some("nixos-unstable".to_string()));
        } else {
            panic!("Expected Github source");
        }

        let res = parse_flake_url("github:NixOS/nixpkgs?ref=nixos-24.05");
        if let FlakeSource::Github { git_ref, .. } = res {
            assert_eq!(git_ref, Some("nixos-24.05".to_string()));
        } else {
            panic!("Expected Github source");
        }

        let res = parse_flake_url("github:NixOS/nixpkgs?rev=abc123def456");
        if let FlakeSource::Github { rev, .. } = res {
            assert_eq!(rev, Some("abc123def456".to_string()));
        } else {
            panic!("Expected Github source");
        }
    }

    #[test]
    fn test_parse_flake_url_git() {
        let res = parse_flake_url("git+https://example.com/repo.git");
        if let FlakeSource::Git { url, .. } = res {
            assert_eq!(url, "https://example.com/repo.git");
        } else {
            panic!("Expected Git source");
        }

        let res = parse_flake_url("git+https://example.com/repo.git?ref=main");
        if let FlakeSource::Git { git_ref, .. } = res {
            assert_eq!(git_ref, Some("main".to_string()));
        } else {
            panic!("Expected Git source");
        }

        let res = parse_flake_url("git+https://example.com/repo.git?rev=abc123");
        if let FlakeSource::Git { rev, .. } = res {
            assert_eq!(rev, Some("abc123".to_string()));
        } else {
            panic!("Expected Git source");
        }
    }

    #[test]
    fn test_parse_flake_url_path() {
        let res = parse_flake_url("path:./local");
        if let FlakeSource::Path { path, .. } = res {
            assert_eq!(path, "./local");
        } else {
            panic!("Expected Path source");
        }

        let res = parse_flake_url("./local");
        if let FlakeSource::Path { path, .. } = res {
            assert_eq!(path, "./local");
        } else {
            panic!("Expected Path source");
        }

        let res = parse_flake_url("/home/user/flake");
        if let FlakeSource::Path { path, .. } = res {
            assert_eq!(path, "/home/user/flake");
        } else {
            panic!("Expected Path source");
        }
    }

    #[test]
    fn test_parse_installable() {
        // Since parse_installable uses current_dir and absolute paths,
        // we test the core logic of splitting by #.
        let (_dir, attr) = parse_installable(".#hello");
        assert_eq!(attr, "hello");

        let (dir, attr) = parse_installable("path/to/flake#pkg");
        assert!(dir.to_str().unwrap().ends_with("path/to/flake"));
        assert_eq!(attr, "pkg");

        let (_dir, attr) = parse_installable(".");
        assert_eq!(attr, "default");
    }

    #[test]
    fn test_resolve_attr_path() {
        assert_eq!(
            resolve_attr_path("hello", "packages", "x86_64-linux"),
            "packages.x86_64-linux.hello"
        );
        assert_eq!(
            resolve_attr_path("default", "devShells", "aarch64-darwin"),
            "devShells.aarch64-darwin.default"
        );
        assert_eq!(
            resolve_attr_path("packages.foo", "packages", "x86_64-linux"),
            "packages.x86_64-linux.foo"
        );
        assert_eq!(
            resolve_attr_path("lib.myFunc", "packages", "x86_64-linux"),
            "lib.myFunc"
        );
        // Test with system already present
        assert_eq!(
            resolve_attr_path("packages.x86_64-linux.foo", "packages", "x86_64-linux"),
            "packages.x86_64-linux.foo"
        );
        // Test unknown category passthrough
        assert_eq!(
            resolve_attr_path("customOutput.bar", "packages", "x86_64-linux"),
            "customOutput.bar"
        );
    }

    #[test]
    fn test_parse_flake_url_sourcehut() {
        let res = parse_flake_url("sourcehut:~user/repo");
        if let FlakeSource::Sourcehut { owner, repo, .. } = res {
            assert_eq!(owner, "~user");
            assert_eq!(repo, "repo");
        } else {
            panic!("Expected Sourcehut source");
        }

        let res = parse_flake_url("sourcehut:~user/repo/main");
        if let FlakeSource::Sourcehut { git_ref, .. } = res {
            assert_eq!(git_ref, Some("main".to_string()));
        } else {
            panic!("Expected Sourcehut source");
        }
    }

    #[test]
    fn test_flake_source_serialization() {
        let source = FlakeSource::Github {
            owner: "NixOS".to_string(),
            repo: "nixpkgs".to_string(),
            git_ref: Some("nixos-unstable".to_string()),
            rev: None,
            flake: None,
        };
        let json = serde_json::to_value(&source).expect("Failed to serialize");
        assert_eq!(json["type"], "github");
        assert_eq!(json["owner"], "NixOS");
        assert_eq!(json["repo"], "nixpkgs");
        assert_eq!(json["ref"], "nixos-unstable");
    }

    #[test]
    fn test_flake_source_deserialization() {
        let json = serde_json::json!({
            "type": "github",
            "owner": "nix-community",
            "repo": "home-manager"
        });
        let source: FlakeSource = serde_json::from_value(json).expect("Failed to deserialize");
        if let FlakeSource::Github { owner, repo, .. } = source {
            assert_eq!(owner, "nix-community");
            assert_eq!(repo, "home-manager");
        } else {
            panic!("Expected Github source");
        }
    }

    #[test]
    fn test_looks_like_system() {
        assert!(looks_like_system("x86_64-linux"));
        assert!(looks_like_system("aarch64-darwin"));
        assert!(looks_like_system("i686-linux"));
        assert!(!looks_like_system("hello"));
        assert!(!looks_like_system("default"));
    }

    #[test]
    fn test_parse_flake_url_unknown() {
        let res = parse_flake_url("https://example.com/flake");
        if let FlakeSource::Unknown { url } = res {
            assert_eq!(url, "https://example.com/flake");
        } else {
            panic!("Expected Unknown source");
        }
    }

    #[test]
    fn test_parse_flake_url_with_multiple_query_params() {
        let res = parse_flake_url("github:owner/repo?ref=main&rev=abc123");
        if let FlakeSource::Github { git_ref, rev, .. } = res {
            assert_eq!(git_ref, Some("main".to_string()));
            assert_eq!(rev, Some("abc123".to_string()));
        } else {
            panic!("Expected Github source");
        }
    }

    pub fn parse_installable(installable: &str) -> (std::path::PathBuf, String) {
        let (path_part, attr_part) = if let Some((p, a)) = installable.split_once('#') {
            (p, a.to_string())
        } else {
            (installable, "default".to_string())
        };

        let flake_dir = if path_part.is_empty() || path_part == "." {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        } else {
            std::path::PathBuf::from(path_part)
                .canonicalize()
                .unwrap_or_else(|_| std::path::PathBuf::from(path_part))
        };

        (flake_dir, attr_part)
    }
}
