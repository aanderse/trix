use super::common::{get_generation_manifest, get_package_versions};
use crate::profile::parse_generation_number;
use anyhow::Result;
use chrono::{DateTime, Local};
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;

/// Show profile generation history
pub fn cmd_history() -> Result<()> {
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
            if let Some(gen_number) = parse_generation_number(&name_str) {
                if let Ok(target) = std::fs::read_link(entry.path()) {
                    // Get mtime from the symlink itself (lstat)
                    if let Ok(metadata) = entry.path().symlink_metadata() {
                        let mtime = metadata.mtime();
                        generations.push((gen_number, entry.path(), target, mtime));
                    }
                }
            }
        }
    }

    if generations.is_empty() {
        println!("No profile generations found");
        return Ok(());
    }

    generations.sort_by_key(|(gen_number, _, _, _)| *gen_number);

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
