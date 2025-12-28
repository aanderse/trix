use anyhow::{Context, Result};

/// Read manifest.json from a profile generation's store path.
pub fn get_generation_manifest(target: &std::path::Path) -> crate::profile::Manifest {
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
pub fn get_package_versions(
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
        _ => anyhow::bail!(
            "Invalid unit in --older-than: {} (expected s, m, h, d, w)",
            unit
        ),
    }
}

pub fn get_closure(path: &str) -> Result<Vec<String>> {
    let mut cmd = crate::command::NixCommand::new("nix-store");
    cmd.args(["--query", "--requisites", path]);

    let out = cmd.output()?;
    Ok(out.lines().map(|s| s.to_string()).collect())
}

pub fn group_by_package(closure: &[String]) -> std::collections::HashMap<String, (String, String)> {
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

pub fn get_store_path_size(path: &str) -> Result<u64> {
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

pub fn format_size_diff(diff: i64) -> String {
    if diff > 0 {
        // Red+bold for size increases (matches Python _red_bold)
        format!("\x1b[31;1m+{}\x1b[0m", format_size(diff as u64))
    } else if diff < 0 {
        format!("-{}", format_size((-diff) as u64))
    } else {
        "0 B".to_string()
    }
}
