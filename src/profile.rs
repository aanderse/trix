//! Profile management for trix.
//!
//! Compatible with nix profile's manifest.json format (version 3).
//! Supports both local flake packages (via flake-compat) and remote packages.

use crate::nix::{get_store_dir, get_system, run_nix_build, BuildOptions};
use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Regex for extracting package name from store path (compiled once).
static PKG_NAME_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(.+?)-\d").unwrap());

/// Manifest file structure (version 3)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Manifest {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub elements: HashMap<String, ManifestElement>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ManifestElement {
    #[serde(rename = "attrPath", skip_serializing_if = "Option::is_none")]
    pub attr_path: Option<String>,
    #[serde(rename = "originalUrl", skip_serializing_if = "Option::is_none")]
    pub original_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<serde_json::Value>,
    #[serde(rename = "storePaths", default)]
    pub store_paths: Vec<String>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub priority: i32,
}

/// Get the profile directory (where profile-N-link symlinks live).
pub fn get_profile_dir() -> Result<PathBuf> {
    let profile_link = dirs::home_dir()
        .context("Could not find home directory")?
        .join(".nix-profile");

    if profile_link.exists() {
        let target = fs::read_link(&profile_link)?;
        if let Some(parent) = target.parent() {
            return Ok(parent.to_path_buf());
        }
    }

    // Default location
    Ok(PathBuf::from("/nix/var/nix/profiles/per-user")
        .join(std::env::var("USER").unwrap_or_else(|_| "default".to_string())))
}

/// Get the store path of the current profile generation.
pub fn get_current_profile_path() -> Result<PathBuf> {
    let profile_link = dirs::home_dir()
        .context("Could not find home directory")?
        .join(".nix-profile");

    fs::canonicalize(&profile_link).context("Could not resolve profile link")
}

/// Read the current profile's manifest.json.
pub fn get_current_manifest() -> Result<Manifest> {
    let profile_path = get_current_profile_path()?;
    let manifest_path = profile_path.join("manifest.json");

    if !manifest_path.exists() {
        return Ok(Manifest {
            version: 3,
            elements: HashMap::new(),
        });
    }

    let content = fs::read_to_string(&manifest_path)?;
    let manifest: Manifest = serde_json::from_str(&content)?;
    Ok(manifest)
}

/// Parse generation number from a profile link filename.
pub fn parse_generation_number(filename: &str) -> Option<u32> {
    // Format: profile-42-link
    let parts: Vec<&str> = filename.split('-').collect();
    if parts.len() >= 2 {
        parts[1].parse().ok()
    } else {
        None
    }
}

/// Get the next profile generation number.
pub fn get_next_profile_number() -> Result<u32> {
    let profile_dir = get_profile_dir()?;

    let mut max_gen = 0u32;

    if profile_dir.exists() {
        for entry in fs::read_dir(&profile_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if name_str.starts_with("profile-") && name_str.ends_with("-link") {
                if let Some(gen) = parse_generation_number(&name_str) {
                    max_gen = max_gen.max(gen);
                }
            }
        }
    }

    Ok(max_gen + 1)
}

/// Collect all files/dirs from packages that need to be symlinked in the profile.
pub fn collect_package_paths(store_paths: &[String]) -> Result<HashMap<String, Vec<PathBuf>>> {
    let mut result: HashMap<String, Vec<PathBuf>> = HashMap::new();

    for store_path in store_paths {
        let path = Path::new(store_path);
        if !path.exists() {
            continue;
        }

        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip manifest.json and similar
            if name == "manifest.json" || name == "nix-support" {
                continue;
            }

            result.entry(name).or_default().push(entry.path());
        }
    }

    Ok(result)
}

/// Create a new profile store path with the given manifest and packages.
pub fn create_profile_store_path(manifest: &Manifest, store_paths: &[String]) -> Result<String> {
    // Create a temporary directory for the profile
    // Use /tmp explicitly to avoid issues with TMPDIR pointing to a nix-shell temp dir
    let temp_parent = tempfile::tempdir_in("/tmp")?;
    let profile_dir = temp_parent.path().join("user-environment");
    fs::create_dir_all(&profile_dir)?;

    // Write manifest.json
    let manifest_content = serde_json::to_string_pretty(manifest)?;
    fs::write(profile_dir.join("manifest.json"), manifest_content)?;

    // Collect and symlink package contents
    let package_paths = collect_package_paths(store_paths)?;

    for (name, targets) in package_paths {
        let dest = profile_dir.join(&name);

        if targets.len() == 1 {
            // Simple symlink
            symlink(&targets[0], &dest)?;
        } else {
            // Need to merge directories
            fs::create_dir_all(&dest)?;
            for target in &targets {
                if target.is_dir() {
                    for entry in fs::read_dir(target)? {
                        let entry = entry?;
                        let entry_name = entry.file_name();
                        let entry_dest = dest.join(&entry_name);
                        if !entry_dest.exists() {
                            symlink(entry.path(), &entry_dest)?;
                        }
                    }
                }
            }
        }
    }

    // Add to store
    let mut cmd = crate::command::NixCommand::new("nix-store");
    cmd.args(["--add", &profile_dir.display().to_string()]);

    cmd.output()
}

/// Switch to a new profile generation atomically.
pub fn switch_profile(new_store_path: &str) -> Result<()> {
    let profile_dir = get_profile_dir()?;
    let next_gen = get_next_profile_number()?;

    fs::create_dir_all(&profile_dir)?;

    // Create profile-N-link
    let gen_link = profile_dir.join(format!("profile-{}-link", next_gen));
    symlink(new_store_path, &gen_link)?;

    // Atomically update the profile symlink
    let home = dirs::home_dir().context("Could not find home directory")?;
    let profile_link = home.join(".nix-profile");

    // Create temp link in same directory as target to ensure atomic rename works
    // (rename fails across filesystems with EXDEV)
    let temp_link = home.join(".nix-profile.tmp");
    let _ = fs::remove_file(&temp_link);
    symlink(&gen_link, &temp_link)?;
    fs::rename(&temp_link, &profile_link)?;

    Ok(())
}

/// List installed packages from manifest, returning (name, element) pairs.
pub fn list_installed() -> Result<Vec<(String, ManifestElement)>> {
    let manifest = get_current_manifest()?;
    Ok(manifest.elements.into_iter().collect())
}

/// Check if a string looks like a local path.
#[cfg(test)]
pub fn is_local_path(path: &str) -> bool {
    if path.is_empty() {
        return true;
    }
    path.starts_with('.')
        || path.starts_with('/')
        || path.starts_with('~')
        || path.starts_with("path:")
}

/// Parse an installable reference for profile operations.
pub fn parse_installable_for_profile(installable: &str) -> (String, String, String) {
    let (ref_part, attr) = if let Some((r, a)) = installable.split_once('#') {
        (r.to_string(), a.to_string())
    } else {
        (installable.to_string(), "default".to_string())
    };

    // Determine package name
    let pkg_name = if attr == "default" {
        // Use directory name for default packages
        std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "default".to_string())
    } else {
        // Use attribute name
        attr.split('.').next_back().unwrap_or(&attr).to_string()
    };

    (ref_part, attr, pkg_name)
}

/// Install a package to the profile.
pub fn install(
    installable: &str,
    flake_dir: Option<&Path>,
    attr: Option<&str>,
    store_path: Option<&str>,
) -> Result<bool> {
    let system = get_system()?;
    let store_dir = get_store_dir()?;

    // Build the package if needed
    let (final_store_path, final_attr, flake_ref) = if let Some(path) = store_path {
        // Pre-built package
        let a = attr.unwrap_or("default");
        let ref_str = flake_dir
            .map(|d| format!("path:{}", d.display()))
            .unwrap_or_else(|| ".".to_string());
        (path.to_string(), a.to_string(), ref_str)
    } else {
        // Need to build
        let resolved = crate::flake::resolve_installable(installable);

        if resolved.is_local {
            let dir = resolved.flake_dir.as_ref().context("No flake directory")?;

            // Check if it's a store path (already built derivation)
            // But only if it's not a flake source (doesn't have flake.nix)
            if dir.starts_with(&store_dir) && !crate::nix::check_is_flake(dir) {
                let store_path_str = dir.display().to_string();
                let store_name = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                let pkg_name = if store_name.len() > 33 && store_name.as_bytes()[32] == b'-' {
                    let name_version = &store_name[33..];
                    if let Some(caps) = PKG_NAME_REGEX.captures(name_version) {
                        caps.get(1).unwrap().as_str().to_string()
                    } else {
                        name_version.to_string()
                    }
                } else {
                    store_name
                };

                return install_store_path(&store_path_str, &pkg_name);
            }

            let full_attr =
                crate::flake::resolve_attr_path(&resolved.attr_part, "packages", &system);

            let options = BuildOptions {
                out_link: None,
                ..Default::default()
            };

            let path = run_nix_build(dir, &full_attr, &options, true)?.context("Build failed")?;

            // Use git+file:// for git repos, path: otherwise (matches nix behavior)
            let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
            let is_git = dir.join(".git").exists()
                || std::process::Command::new("git")
                    .args(["-C", &dir.display().to_string(), "rev-parse", "--git-dir"])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);
            let flake_url = if is_git {
                format!("git+file://{}", canonical.display())
            } else {
                format!("path:{}", canonical.display())
            };

            (path, full_attr, flake_url)
        } else {
            // Remote package - need to use nix profile install
            let flake_ref = resolved.flake_ref.as_ref().context("No flake reference")?;
            let full_ref = format!("{}#{}", flake_ref, resolved.attr_part);

            let mut cmd = crate::command::NixCommand::new("nix");
            cmd.args(["build", "--no-link", "--print-out-paths", &full_ref]);

            let path = cmd.output().context("nix build failed")?;
            (path, resolved.attr_part.clone(), flake_ref.clone())
        }
    };

    // Update manifest
    let mut manifest = get_current_manifest()?;

    // Use package name as the key
    let pkg_name = final_attr
        .split('.')
        .next_back()
        .unwrap_or(&final_attr)
        .to_string();

    // Add/replace element (match nix profile format)
    manifest.elements.insert(
        pkg_name,
        ManifestElement {
            attr_path: Some(final_attr),
            original_url: Some(flake_ref.clone()),
            url: Some(flake_ref),
            outputs: None,
            store_paths: vec![final_store_path.clone()],
            active: true,
            priority: 5,
        },
    );

    // Get all store paths
    let all_paths: Vec<String> = manifest
        .elements
        .values()
        .flat_map(|e| e.store_paths.clone())
        .collect();

    // Create new profile
    let new_profile = create_profile_store_path(&manifest, &all_paths)?;
    switch_profile(&new_profile)?;

    Ok(true)
}

/// Remove a package from the profile.
pub fn remove(name: &str) -> Result<bool> {
    let mut manifest = get_current_manifest()?;

    // Try to remove by key directly, or by matching attr_path
    let removed = manifest.elements.remove(name).is_some() || {
        let keys_to_remove: Vec<_> = manifest
            .elements
            .iter()
            .filter(|(_, e)| {
                e.attr_path
                    .as_ref()
                    .map(|p| p.split('.').next_back() == Some(name))
                    .unwrap_or(false)
            })
            .map(|(k, _)| k.clone())
            .collect();
        let did_remove = !keys_to_remove.is_empty();
        for key in keys_to_remove {
            manifest.elements.remove(&key);
        }
        did_remove
    };

    if !removed {
        return Ok(false);
    }

    tracing::debug!("Removing package: {}", name);

    // Get all remaining store paths
    let all_paths: Vec<String> = manifest
        .elements
        .values()
        .flat_map(|e| e.store_paths.clone())
        .collect();

    // Create new profile
    let new_profile = create_profile_store_path(&manifest, &all_paths)?;
    switch_profile(&new_profile)?;

    Ok(true)
}

/// Extract local path from a flake URL (path: or git+file://)
fn extract_local_path(url: &str) -> Option<&str> {
    if let Some(path) = url.strip_prefix("path:") {
        Some(path)
    } else if let Some(path) = url.strip_prefix("git+file://") {
        Some(path)
    } else {
        None
    }
}

/// Upgrade local packages in profile.
pub fn upgrade(name: Option<&str>) -> Result<(u32, u32)> {
    let manifest = get_current_manifest()?;
    let system = get_system()?;
    let store_dir = crate::nix::get_store_dir()?;

    let mut upgraded = 0u32;
    let mut skipped = 0u32;

    for (elem_name, element) in &manifest.elements {
        let attr = match &element.attr_path {
            Some(a) => a,
            None => continue,
        };

        let pkg_name = attr.split('.').next_back().unwrap_or(attr);

        // If a specific name is provided, only process that package
        if let Some(target_name) = name {
            if pkg_name != target_name && elem_name != target_name {
                continue;
            }
        }

        // Check if this is a local path we can upgrade
        let local_path = match &element.original_url {
            Some(url) => extract_local_path(url),
            None => None,
        };

        let path = match local_path {
            Some(p) if !p.starts_with(&store_dir) => p,
            _ => {
                // Not a local path or is a store path - can't upgrade
                if name.is_some() {
                    // User specifically asked for this package
                    skipped += 1;
                }
                continue;
            }
        };

        let flake_dir = PathBuf::from(path);

        if !flake_dir.exists() {
            eprintln!("warning: flake directory not found: {}", path);
            skipped += 1;
            continue;
        }

        let full_attr = crate::flake::resolve_attr_path(attr, "packages", &system);

        let options = BuildOptions {
            out_link: None,
            ..Default::default()
        };

        match run_nix_build(&flake_dir, &full_attr, &options, true) {
            Ok(Some(new_path)) => {
                let old_path = element
                    .store_paths
                    .first()
                    .map(|s| s.as_str())
                    .unwrap_or("");
                if new_path != old_path {
                    tracing::debug!("Upgrading {}: {} -> {}", pkg_name, old_path, new_path);

                    // Re-install with new store path
                    install(
                        &format!("{}#{}", path, attr),
                        Some(&flake_dir),
                        Some(attr),
                        Some(&new_path),
                    )?;

                    upgraded += 1;
                } else {
                    skipped += 1;
                }
            }
            Ok(None) | Err(_) => {
                skipped += 1;
            }
        }
    }

    Ok((upgraded, skipped))
}
/// Install a direct store path to the profile.
fn install_store_path(store_path: &str, pkg_name: &str) -> Result<bool> {
    let mut manifest = get_current_manifest()?;

    // Add/replace element
    manifest.elements.insert(
        pkg_name.to_string(),
        ManifestElement {
            attr_path: Some(pkg_name.to_string()),
            original_url: Some(format!("path:{}", store_path)),
            store_paths: vec![store_path.to_string()],
            active: true,
            priority: 5,
            ..Default::default()
        },
    );

    // Get all store paths
    let all_paths: Vec<String> = manifest
        .elements
        .values()
        .flat_map(|e| e.store_paths.clone())
        .collect();

    // Create new profile
    let new_profile = create_profile_store_path(&manifest, &all_paths)?;
    switch_profile(&new_profile)?;

    tracing::info!("Added {} (direct store path)", pkg_name);

    Ok(true)
}

/// Delete non-current versions of the profile.
pub fn wipe_history(older_than: Option<std::time::Duration>, dry_run: bool) -> Result<()> {
    let profile_dir = get_profile_dir()?;
    let current_path = get_current_profile_path().ok();

    let now = SystemTime::now();
    let mut to_delete = Vec::new();

    if !profile_dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(&profile_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if let Some(num) = parse_generation_number(&name_str) {
            let path = entry.path();
            let target = fs::read_link(&path).ok();

            // Skip current generation
            if let (Some(ref curr), Some(ref t)) = (&current_path, &target) {
                if curr == t {
                    continue;
                }
            }

            // Check age if requested
            if let Some(max_age) = older_than {
                let metadata = fs::symlink_metadata(&path)?;
                let mtime = metadata.modified()?;
                let age = now.duration_since(mtime).unwrap_or_default();
                if age < max_age {
                    continue;
                }
            }

            to_delete.push((num, path));
        }
    }

    if to_delete.is_empty() {
        tracing::debug!("No profile versions to delete.");
        return Ok(());
    }

    to_delete.sort_by_key(|(num, _)| *num);

    for (num, path) in to_delete {
        if dry_run {
            println!("would remove profile version {}", num);
        } else {
            tracing::debug!("removing profile version {}", num);
            fs::remove_file(path)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_is_local_path() {
        assert!(is_local_path("."));
        assert!(is_local_path(""));
        assert!(is_local_path("./foo"));
        assert!(is_local_path("../foo"));
        assert!(is_local_path("/home/user/flake"));
        assert!(is_local_path("~/flake"));
        assert!(!is_local_path("github:NixOS/nixpkgs"));
        assert!(!is_local_path("nixpkgs"));
    }

    #[test]
    fn test_collect_package_paths() {
        let dir = tempdir().unwrap();
        let pkg1 = dir.path().join("pkg1");
        fs::create_dir_all(pkg1.join("bin")).unwrap();
        fs::create_dir_all(pkg1.join("share")).unwrap();

        let pkg2 = dir.path().join("pkg2");
        fs::create_dir_all(pkg2.join("bin")).unwrap();
        fs::create_dir_all(pkg2.join("lib")).unwrap();

        let paths = vec![
            pkg1.to_str().unwrap().to_string(),
            pkg2.to_str().unwrap().to_string(),
        ];
        let result = collect_package_paths(&paths).unwrap();

        assert!(result.contains_key("bin"));
        assert!(result.contains_key("share"));
        assert!(result.contains_key("lib"));
        assert_eq!(result["bin"].len(), 2);
    }

    #[test]
    fn test_parse_generation_number() {
        assert_eq!(parse_generation_number("profile-1-link"), Some(1));
        assert_eq!(parse_generation_number("profile-42-link"), Some(42));
        assert_eq!(parse_generation_number("other-file"), None);
        assert_eq!(parse_generation_number("profile-abc-link"), None);
    }

    #[test]
    fn test_manifest_serialization() {
        let mut elements = HashMap::new();
        elements.insert(
            "hello".to_string(),
            ManifestElement {
                attr_path: Some("hello".to_string()),
                original_url: Some("flake:nixpkgs".to_string()),
                url: None,
                outputs: None,
                store_paths: vec!["/nix/store/abc-hello".to_string()],
                active: true,
                priority: 5,
            },
        );

        let manifest = Manifest {
            version: 3,
            elements,
        };
        let json = serde_json::to_value(&manifest).unwrap();
        assert_eq!(json["version"], 3);
        assert_eq!(json["elements"]["hello"]["attrPath"], "hello");
    }

    #[test]
    fn test_parse_installable_for_profile_detailed() {
        let (r, a, p) = parse_installable_for_profile("nixpkgs#hello");
        assert_eq!(r, "nixpkgs");
        assert_eq!(a, "hello");
        assert_eq!(p, "hello");

        let (r, a, p) = parse_installable_for_profile("github:owner/repo#app");
        assert_eq!(r, "github:owner/repo");
        assert_eq!(a, "app");
        assert_eq!(p, "app");

        let (r, a, p) = parse_installable_for_profile("nixpkgs#legacyPackages.x86_64-linux.hello");
        assert_eq!(r, "nixpkgs");
        assert_eq!(a, "legacyPackages.x86_64-linux.hello");
        assert_eq!(p, "hello");
    }

    #[test]
    fn test_get_current_manifest_empty() {
        let _dir = tempdir().unwrap();
        // Mock get_current_profile_path by using a temp dir with no manifest.json
        // In a real test we'd need to mock the filesystem or env vars more thoroughly.
    }
}
