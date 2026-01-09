//! Profile management for trix.
//!
//! Compatible with nix profile's manifest.json format (version 3).
//! Supports both local flake packages (without copying to store) and remote packages.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::eval::Evaluator;
use crate::flake::{current_system, expand_attribute, format_attribute_not_found_error, resolve_installable_any, OperationContext};
use crate::progress;

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
    #[serde(default = "default_priority")]
    pub priority: i32,
}

fn default_priority() -> i32 {
    5
}

/// Get the default profile link path (~/.nix-profile).
pub fn default_profile_link() -> Result<PathBuf> {
    dirs::home_dir()
        .context("Could not find home directory")
        .map(|h| h.join(".nix-profile"))
}

/// Get the profile directory (where profile-N-link symlinks live).
pub fn get_profile_dir() -> Result<PathBuf> {
    get_profile_dir_for(None)
}

/// Get the profile directory for a specific profile path.
pub fn get_profile_dir_for(profile: Option<&Path>) -> Result<PathBuf> {
    let profile_link = match profile {
        Some(p) => p.to_path_buf(),
        None => default_profile_link()?,
    };

    if profile_link.exists() {
        let target = fs::read_link(&profile_link)?;
        if let Some(parent) = target.parent() {
            return Ok(parent.to_path_buf());
        }
    }

    // For custom profiles, use the parent directory
    if let Some(p) = profile {
        if let Some(parent) = p.parent() {
            return Ok(parent.to_path_buf());
        }
    }

    // Default location
    Ok(PathBuf::from("/nix/var/nix/profiles/per-user")
        .join(std::env::var("USER").unwrap_or_else(|_| "default".to_string())))
}

/// Get the store path of the current profile generation.
pub fn get_current_profile_path() -> Result<PathBuf> {
    get_current_profile_path_for(None)
}

/// Get the store path of the current profile generation for a specific profile.
pub fn get_current_profile_path_for(profile: Option<&Path>) -> Result<PathBuf> {
    let profile_link = match profile {
        Some(p) => p.to_path_buf(),
        None => default_profile_link()?,
    };

    fs::canonicalize(&profile_link).with_context(|| {
        format!(
            "Could not resolve profile link: {}. Try running 'trix profile install <package>' to create a profile.",
            profile_link.display()
        )
    })
}

/// Read the current profile's manifest.json.
pub fn get_current_manifest() -> Result<Manifest> {
    get_current_manifest_for(None)
}

/// Read the current profile's manifest.json for a specific profile.
pub fn get_current_manifest_for(profile: Option<&Path>) -> Result<Manifest> {
    let profile_path = get_current_profile_path_for(profile)?;
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

/// Get the current manifest or an empty one if no profile exists.
/// Used for install which needs to work on first run.
fn get_or_create_manifest() -> Manifest {
    get_current_manifest().unwrap_or(Manifest {
        version: 3,
        elements: HashMap::new(),
    })
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
    get_next_profile_number_for(None)
}

/// Get the next profile generation number for a specific profile.
pub fn get_next_profile_number_for(profile: Option<&Path>) -> Result<u32> {
    let profile_dir = get_profile_dir_for(profile)?;

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

/// Entry for a package path with its priority.
#[derive(Debug)]
struct PrioritizedPath {
    path: PathBuf,
    priority: i32,
}

/// Collect all files/dirs from packages that need to be symlinked in the profile.
/// Each entry includes the priority from the manifest for conflict resolution.
fn collect_package_paths_with_priority(
    manifest: &Manifest,
) -> Result<HashMap<String, Vec<PrioritizedPath>>> {
    let mut result: HashMap<String, Vec<PrioritizedPath>> = HashMap::new();

    // Build a map from store path to priority
    let mut store_path_priorities: HashMap<&str, i32> = HashMap::new();
    for element in manifest.elements.values() {
        if element.active {
            for sp in &element.store_paths {
                store_path_priorities.insert(sp.as_str(), element.priority);
            }
        }
    }

    // Collect paths from all active elements
    for element in manifest.elements.values() {
        if !element.active {
            continue;
        }

        for store_path in &element.store_paths {
            let path = Path::new(store_path);
            if !path.exists() {
                continue;
            }

            let priority = store_path_priorities
                .get(store_path.as_str())
                .copied()
                .unwrap_or(5);

            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().to_string();

                // Skip manifest.json and nix-support
                if name == "manifest.json" || name == "nix-support" {
                    continue;
                }

                result.entry(name).or_default().push(PrioritizedPath {
                    path: entry.path(),
                    priority,
                });
            }
        }
    }

    // Sort each entry by priority (lower priority number wins)
    for paths in result.values_mut() {
        paths.sort_by_key(|p| p.priority);
    }

    Ok(result)
}

/// Create a new profile store path with the given manifest and packages.
pub fn create_profile_store_path(manifest: &Manifest, _store_paths: &[String]) -> Result<String> {
    // Create a temporary directory for the profile
    // Use /tmp explicitly to avoid issues with TMPDIR pointing to a nix-shell temp dir
    let temp_parent = tempfile::tempdir_in("/tmp")?;
    let profile_dir = temp_parent.path().join("user-environment");
    fs::create_dir_all(&profile_dir)?;

    // Write manifest.json
    let manifest_content = serde_json::to_string_pretty(manifest)?;
    fs::write(profile_dir.join("manifest.json"), manifest_content)?;

    // Collect and symlink package contents, respecting priorities
    // Lower priority numbers win in case of conflicts (like nix profile)
    let package_paths = collect_package_paths_with_priority(manifest)?;

    for (name, targets) in package_paths {
        let dest = profile_dir.join(&name);

        if targets.len() == 1 {
            // Simple symlink
            symlink(&targets[0].path, &dest)?;
        } else {
            // Check if any target is not a directory - if so, use highest priority (first)
            let all_dirs = targets.iter().all(|t| t.path.is_dir());

            if !all_dirs {
                // Not all are directories - symlink to the highest priority one
                // (targets are already sorted by priority, so first wins)
                symlink(&targets[0].path, &dest)?;
            } else {
                // All are directories - merge them, with priority determining winner for conflicts
                fs::create_dir_all(&dest)?;

                // Process in priority order (already sorted), first entry wins
                for target in &targets {
                    if target.path.is_dir() {
                        for entry in fs::read_dir(&target.path)? {
                            let entry = entry?;
                            let entry_name = entry.file_name();
                            let entry_dest = dest.join(&entry_name);
                            if !entry_dest.exists() {
                                // First (highest priority) wins
                                symlink(entry.path(), &entry_dest)?;
                            }
                        }
                    }
                }
            }
        }
    }

    // Add to store using nix-store --add
    let output = Command::new("nix-store")
        .args(["--add", &profile_dir.display().to_string()])
        .output()
        .context("failed to run nix-store --add")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("nix-store --add failed: {}", stderr));
    }

    let store_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(store_path)
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
    let temp_link = home.join(".nix-profile.tmp");
    let _ = fs::remove_file(&temp_link);
    symlink(&gen_link, &temp_link)?;
    fs::rename(&temp_link, &profile_link)?;

    Ok(())
}

/// List installed packages from manifest, returning (name, element) pairs.
pub fn list_installed() -> Result<Vec<(String, ManifestElement)>> {
    list_installed_for(None)
}

pub fn list_installed_for(profile: Option<&Path>) -> Result<Vec<(String, ManifestElement)>> {
    let manifest = get_current_manifest_for(profile)?;
    Ok(manifest.elements.into_iter().collect())
}

/// Check if a string looks like a local path.
pub fn is_local_path(path: &str) -> bool {
    if path.is_empty() {
        return true;
    }
    path.starts_with('.')
        || path.starts_with('/')
        || path.starts_with('~')
        || path.starts_with("path:")
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

/// Build a package and return its store path.
/// Uses native evaluation.
fn build_package(flake_dir: &Path, attr_path: &[String]) -> Result<String> {
    let eval_target = format!("{}#{}", flake_dir.display(), attr_path.join("."));
    info!("evaluating {}", eval_target);

    let status = progress::evaluating(&eval_target);

    let mut eval = Evaluator::new().context("failed to initialize evaluator")?;
    let value = eval
        .eval_flake_attr(flake_dir, attr_path)
        .context("failed to evaluate derivation")?;

    status.finish_and_clear();

    let drv_path = eval.get_drv_path(&value)?;
    debug!(drv = %drv_path, "got derivation path");

    // Build it
    info!("building {}", drv_path);
    let build_status = progress::building(&drv_path);

    let store_path = eval.build_value(&value)?;

    build_status.finish_and_clear();

    Ok(store_path)
}

/// Install a package to the profile.
pub fn install(installable: &str, priority: i32, refresh: bool) -> Result<String> {
    let system = current_system()?;
    let cwd = std::env::current_dir().context("failed to get current directory")?;

    // Resolve the installable (handles local paths, registry names, and remote refs)
    debug!("resolving installable: {}", installable);
    let resolved = resolve_installable_any(installable, &cwd);

    let (store_path, attr_path_str, flake_url) = if resolved.is_local {
        // Local flake - use our evaluator
        let flake_path = resolved.path.as_ref().expect("local flake must have path");
        let candidates = expand_attribute(&resolved.attribute, OperationContext::Build, &system);

        // Compute flake URL early so we can use it in error messages
        let canonical = flake_path
            .canonicalize()
            .unwrap_or_else(|_| flake_path.clone());
        let is_git = git2::Repository::discover(flake_path).is_ok();

        let flake_url = if is_git {
            format!("git+file://{}", canonical.display())
        } else {
            format!("path:{}", canonical.display())
        };

        // Try each candidate until one works (like build.rs does)
        let (attr_path, store_path) = {
            let mut found = None;

            for candidate in &candidates {
                match build_package(flake_path, candidate) {
                    Ok(path) => {
                        found = Some((candidate.clone(), path));
                        break;
                    }
                    Err(e) => {
                        debug!("candidate {} failed: {}", candidate.join("."), e);
                    }
                }
            }

            found.ok_or_else(|| {
                anyhow!(format_attribute_not_found_error(&flake_url, &candidates))
            })?
        };

        (store_path, attr_path.join("."), flake_url)
    } else {
        // Remote flake - use nix build
        let attr = if resolved.attribute.is_empty() {
            "default".to_string()
        } else {
            resolved.attribute.join(".")
        };

        // Build the flake ref from the resolved reference
        let ref_part = resolved
            .flake_ref
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or_else(|| installable.split('#').next().unwrap_or("."));
        let flake_ref = format!("{}#{}", ref_part, attr);
        info!("building remote package: {}", flake_ref);

        let build_status = progress::building(&flake_ref);

        let mut cmd = Command::new("nix");
        cmd.args(["build", "--no-link", "--print-out-paths"]);
        if refresh {
            cmd.arg("--refresh");
        }
        cmd.arg(&flake_ref);

        let output = cmd.output().context("failed to run nix build")?;

        build_status.finish_and_clear();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("nix build failed: {}", stderr));
        }

        let store_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let flake_url = ref_part.to_string();

        (store_path, attr, flake_url)
    };

    debug!(store_path = %store_path, "built package");

    // Update manifest (create empty if no profile exists yet)
    let mut manifest = get_or_create_manifest();

    // Use package name as the key
    // If attribute is "default", extract name from store path instead
    let attr_name = attr_path_str
        .split('.')
        .next_back()
        .unwrap_or(&attr_path_str);

    let pkg_name = if attr_name == "default" {
        // Extract name from store path (e.g., /nix/store/xxx-hello-2.12 -> "hello")
        parse_store_path(&store_path)
            .map(|(name, _)| name)
            .unwrap_or_else(|| attr_name.to_string())
    } else {
        attr_name.to_string()
    };

    // Add/replace element
    manifest.elements.insert(
        pkg_name.clone(),
        ManifestElement {
            attr_path: Some(attr_path_str),
            original_url: Some(flake_url.clone()),
            url: Some(flake_url),
            outputs: None,
            store_paths: vec![store_path.clone()],
            active: true,
            priority,
        },
    );

    // Get all store paths
    let all_paths: Vec<String> = manifest
        .elements
        .values()
        .flat_map(|e| e.store_paths.clone())
        .collect();

    // Create new profile
    info!("updating profile");
    let new_profile = create_profile_store_path(&manifest, &all_paths)?;
    switch_profile(&new_profile)?;

    Ok(pkg_name)
}

/// Install a direct store path to the profile.
pub fn install_store_path(store_path: &str, pkg_name: &str) -> Result<()> {
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

    info!("added {} (direct store path)", pkg_name);

    Ok(())
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

    debug!("removing package: {}", name);

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

/// Upgrade packages in profile (both local and remote).
pub fn upgrade(name: Option<&str>, refresh: bool) -> Result<(u32, u32)> {
    let manifest = get_current_manifest()?;
    let system = current_system()?;

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

        // Check if this is a local path we can upgrade natively
        let local_path = match &element.original_url {
            Some(url) => extract_local_path(url),
            None => None,
        };

        let old_path = element
            .store_paths
            .first()
            .map(|s| s.as_str())
            .unwrap_or("");

        match local_path {
            Some(path) if !path.starts_with("/nix/store") => {
                // Local flake - upgrade natively
                let flake_dir = PathBuf::from(&path);

                if !flake_dir.exists() {
                    eprintln!("warning: flake directory not found: {}", path);
                    skipped += 1;
                    continue;
                }

                // Build the attribute path
                let candidates = expand_attribute(
                    &attr.split('.').map(|s| s.to_string()).collect::<Vec<_>>(),
                    OperationContext::Build,
                    &system,
                );
                let attr_path = &candidates[0];

                match build_package(&flake_dir, attr_path) {
                    Ok(new_path) => {
                        if new_path != old_path {
                            debug!("upgrading {}: {} -> {}", pkg_name, old_path, new_path);

                            // Re-install with new store path (refresh doesn't matter for local)
                            let installable = format!("{}#{}", path, attr);
                            install(&installable, element.priority, false)?;

                            upgraded += 1;
                        } else {
                            skipped += 1;
                        }
                    }
                    Err(e) => {
                        eprintln!("warning: failed to build {}: {}", pkg_name, e);
                        skipped += 1;
                    }
                }
            }
            _ => {
                // Remote flake - upgrade via nix build
                let original_url = match &element.original_url {
                    Some(url) => url,
                    None => {
                        debug!("skipping {} - no original_url", elem_name);
                        skipped += 1;
                        continue;
                    }
                };

                // Build the flake reference for nix build
                let flake_ref = format!("{}#{}", original_url, attr);
                info!("upgrading remote package: {}", flake_ref);

                let build_status = progress::building(&flake_ref);

                let mut cmd = Command::new("nix");
                cmd.args(["build", "--no-link", "--print-out-paths"]);
                if refresh {
                    cmd.arg("--refresh");
                }
                cmd.arg(&flake_ref);

                let output = cmd.output().context("failed to run nix build")?;

                build_status.finish_and_clear();

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    eprintln!("warning: failed to build {}: {}", pkg_name, stderr.trim());
                    skipped += 1;
                    continue;
                }

                let new_path = String::from_utf8_lossy(&output.stdout).trim().to_string();

                if new_path != old_path {
                    debug!("upgrading {}: {} -> {}", pkg_name, old_path, new_path);

                    // Re-install with new store path
                    install(&flake_ref, element.priority, refresh)?;
                    upgraded += 1;
                } else {
                    skipped += 1;
                }
            }
        }
    }

    Ok((upgraded, skipped))
}

/// Delete non-current versions of the profile.
pub fn wipe_history(older_than: Option<std::time::Duration>, dry_run: bool) -> Result<u32> {
    let profile_dir = get_profile_dir()?;
    let current_path = get_current_profile_path().ok();

    let now = SystemTime::now();
    let mut to_delete = Vec::new();

    if !profile_dir.exists() {
        return Ok(0);
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
        return Ok(0);
    }

    to_delete.sort_by_key(|(num, _)| *num);

    let count = to_delete.len() as u32;

    for (num, path) in to_delete {
        if dry_run {
            println!("would remove profile version {}", num);
        } else {
            debug!("removing profile version {}", num);
            fs::remove_file(path)?;
        }
    }

    Ok(count)
}

/// Get the closure of a store path.
pub fn get_closure(path: &str) -> Result<Vec<String>> {
    let output = Command::new("nix-store")
        .args(["--query", "--requisites", path])
        .output()
        .context("failed to run nix-store --query")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("nix-store --query failed: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().map(|s| s.to_string()).collect())
}

/// Get the size of a store path.
pub fn get_store_path_size(path: &str) -> Result<u64> {
    let output = Command::new("nix")
        .args(["path-info", "--json", path])
        .output()
        .context("failed to run nix path-info")?;

    if !output.status.success() {
        return Ok(0);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let info: serde_json::Value = serde_json::from_str(&stdout).unwrap_or(serde_json::json!([]));

    if let Some(arr) = info.as_array() {
        if let Some(first) = arr.first() {
            return Ok(first["narSize"].as_u64().unwrap_or(0));
        }
    }

    Ok(0)
}

/// Format a byte size for display.
pub fn format_size(size: u64) -> String {
    if size < 1024 {
        format!("{} B", size)
    } else if size < 1024 * 1024 {
        format!("{:.1} KiB", size as f64 / 1024.0)
    } else if size < 1024 * 1024 * 1024 {
        format!("{:.1} MiB", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GiB", size as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Format a size difference for display.
pub fn format_size_diff(diff: i64) -> String {
    if diff > 0 {
        format!("\x1b[31;1m+{}\x1b[0m", format_size(diff as u64))
    } else if diff < 0 {
        format!("-{}", format_size((-diff) as u64))
    } else {
        "0 B".to_string()
    }
}

/// Parse an --older-than duration string (e.g., "30d", "7d", "1w").
pub fn parse_older_than(s: &str) -> Result<u64> {
    let mut num_str = String::new();
    let mut unit = 'd';

    for c in s.chars() {
        if c.is_ascii_digit() {
            num_str.push(c);
        } else {
            unit = c;
            break;
        }
    }

    let num: u64 = num_str.parse().context("Invalid number in --older-than")?;

    match unit {
        's' => Ok(num),
        'm' => Ok(num * 60),
        'h' => Ok(num * 3600),
        'd' => Ok(num * 86400),
        'w' => Ok(num * 604800),
        _ => Err(anyhow!(
            "Invalid unit in --older-than: {} (expected s, m, h, d, w)",
            unit
        )),
    }
}

/// Extract version from a store path like /nix/store/xxx-name-1.2.3
pub fn extract_version(store_path: &str) -> String {
    let basename = Path::new(store_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    // Remove the hash prefix (32 chars + dash)
    if basename.len() > 33 && basename.as_bytes()[32] == b'-' {
        let name_version = &basename[33..];
        // Try to find version at end (after last dash followed by digit)
        if let Some(pos) = name_version.rfind('-') {
            let after_dash = &name_version[pos + 1..];
            if after_dash
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
            {
                return after_dash.to_string();
            }
        }
        return name_version.to_string();
    }
    store_path.to_string()
}

/// Parse a store path into (name, version).
pub fn parse_store_path(path: &str) -> Option<(String, String)> {
    let filename = path.split('/').next_back()?;
    let name_part = filename.split_once('-')?.1;

    // Try to split name and version
    if let Some(idx) = name_part.find(|c: char| c.is_ascii_digit()) {
        if idx > 0 && name_part.as_bytes()[idx - 1] == b'-' {
            return Some((
                name_part[..idx - 1].to_string(),
                name_part[idx..].to_string(),
            ));
        }
    }

    Some((name_part.to_string(), String::new()))
}

/// Group closure paths by package name.
pub fn group_by_package(closure: &[String]) -> HashMap<String, (String, String)> {
    let mut map = HashMap::new();
    for path in closure {
        if let Some((name, version)) = parse_store_path(path) {
            map.insert(name, (version, path.clone()));
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_parse_generation_number() {
        assert_eq!(parse_generation_number("profile-1-link"), Some(1));
        assert_eq!(parse_generation_number("profile-42-link"), Some(42));
        assert_eq!(parse_generation_number("other-file"), None);
        assert_eq!(parse_generation_number("profile-abc-link"), None);
    }

    #[test]
    fn test_parse_installable_for_profile() {
        let (r, a, p) = parse_installable_for_profile("nixpkgs#hello");
        assert_eq!(r, "nixpkgs");
        assert_eq!(a, "hello");
        assert_eq!(p, "hello");

        let (r, a, p) = parse_installable_for_profile("github:owner/repo#app");
        assert_eq!(r, "github:owner/repo");
        assert_eq!(a, "app");
        assert_eq!(p, "app");
    }

    #[test]
    fn test_extract_version() {
        // Real nix store paths have 32-char hashes
        assert_eq!(
            extract_version("/nix/store/abcdefghijklmnopqrstuvwxyz012345-hello-2.10"),
            "2.10"
        );
        // Paths without valid hash prefix return the store path
        assert_eq!(
            extract_version("/nix/store/abc123-hello-2.10"),
            "/nix/store/abc123-hello-2.10"
        );
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(2048), "2.0 KiB");
        assert_eq!(format_size(1024 * 1024), "1.0 MiB");
    }

    #[test]
    fn test_parse_older_than() {
        assert_eq!(parse_older_than("30d").unwrap(), 30 * 86400);
        assert_eq!(parse_older_than("1w").unwrap(), 604800);
        assert_eq!(parse_older_than("24h").unwrap(), 24 * 3600);
    }
}
