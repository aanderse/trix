//! Profile subcommands.

use anyhow::{Context, Result};

use crate::profile::{install, list_installed, remove, upgrade, wipe_history};

/// List installed packages
pub fn cmd_list(output_json: bool) -> Result<()> {
    let mut elements = list_installed()?;

    // Sort alphabetically by name (matches nix profile list)
    elements.sort_by(|(a, _), (b, _)| a.cmp(b));

    if output_json {
        // For JSON output, just output the elements without names
        let elems: Vec<_> = elements.iter().map(|(_, e)| e).collect();
        let json = serde_json::to_string_pretty(&elems)?;
        println!("{}", json);
        return Ok(());
    }

    if elements.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }

    for (i, (name, elem)) in elements.iter().enumerate() {
        if i > 0 {
            println!();
        }

        // Name in bold
        println!("Name:               \x1b[1m{}\x1b[0m", name);

        if let Some(ref attr_path) = elem.attr_path {
            println!("Flake attribute:    {}", attr_path);
        }

        if let Some(ref original_url) = elem.original_url {
            println!("Original flake URL: {}", original_url);
        }

        if let Some(ref url) = elem.url {
            println!("Locked flake URL:   {}", url);
        }

        if !elem.store_paths.is_empty() {
            println!("Store paths:        {}", elem.store_paths[0]);
            for path in &elem.store_paths[1..] {
                println!("                    {}", path);
            }
        }
    }

    Ok(())
}

/// Add packages to the profile
pub fn cmd_add(installables: &[String]) -> Result<()> {
    for installable in installables {
        tracing::debug!("Installing {}...", installable);

        install(installable, None, None, None)?;

        // Extract package name for display (matches Python behavior)
        let (_, _, pkg_name) = crate::profile::parse_installable_for_profile(installable);
        println!("Added {}", pkg_name);
    }

    Ok(())
}

/// Remove packages from the profile
pub fn cmd_remove(names: &[String]) -> Result<()> {
    for name in names {
        if remove(name)? {
            println!("Removed: {}", name);
        } else {
            eprintln!("Package not found: {}", name);
        }
    }

    Ok(())
}

/// Upgrade local packages in the profile
pub fn cmd_upgrade(name: Option<&str>) -> Result<()> {
    let (upgraded, skipped) = upgrade(name)?;

    if upgraded > 0 {
        println!("Upgraded {} package(s)", upgraded);
    } else if skipped > 0 {
        println!("All {} package(s) up to date", skipped);
    } else {
        println!("No local packages to upgrade");
    }

    Ok(())
}

/// Show profile generation history
pub fn cmd_history() -> Result<()> {
    use chrono::{DateTime, Local};
    use std::collections::HashMap;
    use std::os::unix::fs::MetadataExt;

    let profile_dir = crate::profile::get_profile_dir()?;

    if !profile_dir.exists() {
        println!("No profile generations found");
        return Ok(());
    }

    // Collect generations with their link path, target, and mtime
    let mut generations: Vec<(u32, std::path::PathBuf, std::path::PathBuf, i64)> = Vec::new();

    for entry in std::fs::read_dir(&profile_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with("profile-") && name_str.ends_with("-link") {
            if let Some(gen) = crate::profile::parse_generation_number(&name_str) {
                if let Ok(target) = std::fs::read_link(entry.path()) {
                    // Get mtime from the symlink itself (lstat)
                    if let Ok(metadata) = entry.path().symlink_metadata() {
                        let mtime = metadata.mtime();
                        generations.push((gen, entry.path(), target, mtime));
                    }
                }
            }
        }
    }

    if generations.is_empty() {
        println!("No profile generations found");
        return Ok(());
    }

    generations.sort_by_key(|(gen, _, _, _)| *gen);

    let current = crate::profile::get_current_profile_path().ok();

    // Track previous versions for diff
    let mut prev_versions: HashMap<String, String> = HashMap::new();

    for (i, (num, _link, target, mtime)) in generations.iter().enumerate() {
        // Format date
        let datetime = DateTime::from_timestamp(*mtime, 0)
            .map(|dt| dt.with_timezone(&Local))
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Check if this is the current generation
        let is_current = current.as_ref() == Some(target);

        // Format version number with ANSI codes (green+bold for current, bold for others)
        let version_str = if is_current {
            format!("\x1b[32;1m{}\x1b[0m", num)
        } else {
            format!("\x1b[1m{}\x1b[0m", num)
        };

        // Build header with parent reference
        let header = if i == 0 {
            format!("Version {} ({}):", version_str, datetime)
        } else {
            let prev_num = generations[i - 1].0;
            format!("Version {} ({}) <- {}:", version_str, datetime, prev_num)
        };

        println!("{}", header);

        // Get manifest and extract package versions
        let manifest = get_generation_manifest(target);
        let curr_versions = get_package_versions(&manifest);

        // Find changes
        let mut all_packages: std::collections::BTreeSet<&String> = prev_versions.keys().collect();
        all_packages.extend(curr_versions.keys());

        let mut changes = Vec::new();

        for pkg in all_packages {
            let old_ver = prev_versions.get(pkg);
            let new_ver = curr_versions.get(pkg);

            match (old_ver, new_ver) {
                (None, Some(new)) => {
                    // Added
                    changes.push(format!("  {}: ∅ -> {}", pkg, new));
                }
                (Some(old), None) => {
                    // Removed
                    changes.push(format!("  {}: {} -> ∅", pkg, old));
                }
                (Some(old), Some(new)) if old != new => {
                    // Changed
                    changes.push(format!("  {}: {} -> {}", pkg, old, new));
                }
                _ => {}
            }
        }

        if changes.is_empty() {
            println!("  No changes.");
        } else {
            for change in changes {
                println!("{}", change);
            }
        }

        println!();

        prev_versions = curr_versions;
    }

    Ok(())
}

/// Read manifest.json from a profile generation's store path.
fn get_generation_manifest(target: &std::path::Path) -> crate::profile::Manifest {
    let manifest_path = target.join("manifest.json");
    if manifest_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&manifest_path) {
            if let Ok(manifest) = serde_json::from_str(&content) {
                return manifest;
            }
        }
    }
    crate::profile::Manifest {
        version: 3,
        elements: std::collections::HashMap::new(),
    }
}

/// Extract package name -> version mapping from manifest.
fn get_package_versions(
    manifest: &crate::profile::Manifest,
) -> std::collections::HashMap<String, String> {
    let mut versions = std::collections::HashMap::new();
    for (name, element) in &manifest.elements {
        if element.active {
            if let Some(store_path) = element.store_paths.first() {
                versions.insert(name.clone(), extract_version(store_path));
            } else {
                versions.insert(name.clone(), "unknown".to_string());
            }
        }
    }
    versions
}

/// Extract version from a store path like /nix/store/xxx-name-1.2.3
fn extract_version(store_path: &str) -> String {
    let basename = std::path::Path::new(store_path)
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
        // If no clear version, use the whole name-version part
        return name_version.to_string();
    }
    store_path.to_string()
}

/// Roll back to the previous profile generation
pub fn cmd_rollback() -> Result<()> {
    let profile_dir = crate::profile::get_profile_dir()?;
    let current_path = crate::profile::get_current_profile_path()?;

    let mut generations: Vec<(u32, std::path::PathBuf)> = Vec::new();

    for entry in std::fs::read_dir(&profile_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with("profile-") && name_str.ends_with("-link") {
            if let Some(gen) = crate::profile::parse_generation_number(&name_str) {
                generations.push((gen, entry.path()));
            }
        }
    }

    generations.sort_by_key(|(gen, _)| *gen);

    // Find current generation
    let current_gen = generations
        .iter()
        .find(|(_, path)| std::fs::read_link(path).ok() == Some(current_path.clone()));

    if let Some((current_gen_num, _)) = current_gen {
        // Find previous generation
        let prev = generations
            .iter()
            .rev()
            .find(|(gen, _)| gen < current_gen_num);

        if let Some((prev_gen, prev_path)) = prev {
            let prev_target = std::fs::read_link(prev_path)?;
            crate::profile::switch_profile(&prev_target.display().to_string())?;
            println!("Rolled back to generation {}", prev_gen);
            return Ok(());
        }
    }

    anyhow::bail!("No previous generation to roll back to.");
}

/// Delete non-current versions of the profile
pub fn cmd_wipe_history(older_than: Option<&str>, dry_run: bool) -> Result<()> {
    let older_than_duration = if let Some(ot) = older_than {
        Some(std::time::Duration::from_secs(parse_older_than(ot)?))
    } else {
        None
    };

    wipe_history(older_than_duration, dry_run)
}

fn parse_older_than(s: &str) -> Result<u64> {
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
        _ => anyhow::bail!(
            "Invalid unit in --older-than: {} (expected s, m, h, d, w)",
            unit
        ),
    }
}

/// Show closure difference between profile versions
pub fn cmd_diff_closures() -> Result<()> {
    let profile_dir = crate::profile::get_profile_dir()?;

    let mut generations = Vec::new();
    for entry in std::fs::read_dir(&profile_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if let Some(num) = crate::profile::parse_generation_number(&name_str) {
            if let Ok(target) = std::fs::read_link(entry.path()) {
                generations.push((num, target));
            }
        }
    }

    if generations.len() < 2 {
        println!("Need at least 2 generations to show differences.");
        return Ok(());
    }

    generations.sort_by_key(|(num, _)| *num);

    for i in 1..generations.len() {
        let (prev_num, prev_target) = &generations[i - 1];
        let (curr_num, curr_target) = &generations[i];

        let prev_closure = get_closure(&prev_target.to_string_lossy())?;
        let curr_closure = get_closure(&curr_target.to_string_lossy())?;

        let prev_packages = group_by_package(&prev_closure);
        let curr_packages = group_by_package(&curr_closure);

        let mut changes = Vec::new();
        let mut all_names: std::collections::BTreeSet<_> = prev_packages.keys().collect();
        all_names.extend(curr_packages.keys());

        for name in all_names {
            if name == "profile" || name == "user-environment" {
                continue;
            }

            let prev_info = prev_packages.get(name);
            let curr_info = curr_packages.get(name);

            match (prev_info, curr_info) {
                (Some((prev_ver, prev_path)), Some((curr_ver, curr_path))) => {
                    if prev_path != curr_path {
                        let prev_size = get_store_path_size(prev_path).unwrap_or(0);
                        let curr_size = get_store_path_size(curr_path).unwrap_or(0);
                        let diff = curr_size as i64 - prev_size as i64;
                        let size_str = format_size_diff(diff);

                        if prev_ver != curr_ver {
                            changes.push(format!(
                                "  {}: {} → {}, {}",
                                name, prev_ver, curr_ver, size_str
                            ));
                        } else {
                            changes.push(format!("  {}: {}", name, size_str));
                        }
                    }
                }
                (None, Some((curr_ver, curr_path))) => {
                    let size = get_store_path_size(curr_path).unwrap_or(0);
                    // Red+bold for size of added packages (matches Python)
                    let size_str = format!("\x1b[31;1m+{}\x1b[0m", format_size(size));
                    changes.push(format!("  {}: ∅ → {}, {}", name, curr_ver, size_str));
                }
                (Some((prev_ver, prev_path)), None) => {
                    let size = get_store_path_size(prev_path).unwrap_or(0);
                    changes.push(format!(
                        "  {}: {} → ∅, -{}",
                        name,
                        prev_ver,
                        format_size(size)
                    ));
                }
                (None, None) => {}
            }
        }

        if !changes.is_empty() {
            println!("Version {} → {}:", prev_num, curr_num);
            for change in changes {
                println!("{}", change);
            }
            println!();
        }
    }

    Ok(())
}

fn get_closure(path: &str) -> Result<Vec<String>> {
    let mut cmd = crate::command::NixCommand::new("nix-store");
    cmd.args(["--query", "--requisites", path]);

    let out = cmd.output()?;
    Ok(out.lines().map(|s| s.to_string()).collect())
}

fn group_by_package(closure: &[String]) -> std::collections::HashMap<String, (String, String)> {
    let mut map = std::collections::HashMap::new();
    for path in closure {
        if let Some((name, version)) = parse_store_path(path) {
            map.insert(name.to_string(), (version.to_string(), path.to_string()));
        }
    }
    map
}

fn parse_store_path(path: &str) -> Option<(&str, &str)> {
    // /nix/store/hash-name-version
    let filename = path.split('/').next_back()?;
    let name_part = filename.split_once('-')?.1;

    // Try to split name and version. This is heuristic.
    // Versions usually start with a digit.
    if let Some(idx) = name_part.find(|c: char| c.is_ascii_digit()) {
        if idx > 0 && name_part.as_bytes()[idx - 1] == b'-' {
            return Some((&name_part[..idx - 1], &name_part[idx..]));
        }
    }

    Some((name_part, ""))
}

fn get_store_path_size(path: &str) -> Result<u64> {
    // Use nix path-info for accurate size
    let mut cmd = crate::command::NixCommand::new("nix");
    cmd.args(["path-info", "--json", path]);

    let info: serde_json::Value = cmd.json().unwrap_or(serde_json::json!([]));
    if let Some(arr) = info.as_array() {
        if let Some(first) = arr.first() {
            return Ok(first["narSize"].as_u64().unwrap_or(0));
        }
    }

    Ok(0)
}

fn format_size(size: u64) -> String {
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

fn format_size_diff(diff: i64) -> String {
    if diff > 0 {
        // Red+bold for size increases (matches Python _red_bold)
        format!("\x1b[31;1m+{}\x1b[0m", format_size(diff as u64))
    } else if diff < 0 {
        format!("-{}", format_size((-diff) as u64))
    } else {
        "0 B".to_string()
    }
}
