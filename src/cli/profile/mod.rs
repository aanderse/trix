//! Profile commands - manage Nix profiles.
//!
//! Profiles are collections of packages that can be installed, upgraded,
//! and rolled back independently. This implementation builds profiles natively
//! without copying local flakes to the store.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::time::Duration;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Local};
use clap::{Args, Subcommand};
use tracing::debug;

use crate::cli::build::parse_override_inputs;
use crate::profile::{
    self, extract_version, format_size, format_size_diff, get_closure,
    get_current_profile_path_for, get_profile_dir_for, get_store_path_size, group_by_package,
    list_installed_for, parse_generation_number, parse_older_than, switch_profile, Manifest,
};

#[derive(Args)]
pub struct ProfileArgs {
    /// The profile to operate on (default: ~/.nix-profile)
    #[arg(long, global = true)]
    profile: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: ProfileCommand,
}

#[derive(Subcommand)]
enum ProfileCommand {
    /// List packages in a profile
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Add a package to a profile
    Add {
        /// Installable references (e.g., 'nixpkgs#hello', '.#myapp')
        #[arg(required = true)]
        installables: Vec<String>,

        /// Priority for conflict resolution (lower = higher priority)
        #[arg(long, default_value = "5")]
        priority: i32,

        /// Consider all previously downloaded files out-of-date
        #[arg(long)]
        refresh: bool,

        /// Override a flake input with a local path (avoids store copy for the override)
        /// Usage: --override-input nixpkgs ~/nixpkgs
        #[arg(long = "override-input", num_args = 2, value_names = ["INPUT", "PATH"], action = clap::ArgAction::Append)]
        override_input: Vec<String>,
    },
    /// Alias for 'add'
    Install {
        /// Installable references
        #[arg(required = true)]
        installables: Vec<String>,

        /// Priority for conflict resolution
        #[arg(long, default_value = "5")]
        priority: i32,

        /// Consider all previously downloaded files out-of-date
        #[arg(long)]
        refresh: bool,

        /// Override a flake input with a local path (avoids store copy for the override)
        /// Usage: --override-input nixpkgs ~/nixpkgs
        #[arg(long = "override-input", num_args = 2, value_names = ["INPUT", "PATH"], action = clap::ArgAction::Append)]
        override_input: Vec<String>,
    },
    /// Remove packages from a profile
    Remove {
        /// Package names to remove
        #[arg(required = true)]
        packages: Vec<String>,
    },
    /// Upgrade packages using their most recent flake
    Upgrade {
        /// Specific package to upgrade (upgrades all packages if omitted)
        package: Option<String>,

        /// Consider all previously downloaded files out-of-date
        #[arg(long)]
        refresh: bool,

        /// Override a flake input with a local path (avoids store copy for the override)
        /// Usage: --override-input nixpkgs ~/nixpkgs
        #[arg(long = "override-input", num_args = 2, value_names = ["INPUT", "PATH"], action = clap::ArgAction::Append)]
        override_input: Vec<String>,
    },
    /// Show all versions of a profile
    History,
    /// Roll back to the previous version or a specified version
    Rollback {
        /// Version to roll back to (defaults to previous)
        #[arg(long)]
        to: Option<u32>,
    },
    /// Delete non-current versions of a profile
    WipeHistory {
        /// Remove generations older than this (e.g., '30d', '7d')
        #[arg(long)]
        older_than: Option<String>,

        /// Show what would be deleted without actually deleting
        #[arg(long)]
        dry_run: bool,
    },
    /// Show the closure difference between profile versions
    DiffClosures,
}

pub fn run(args: ProfileArgs) -> Result<()> {
    let profile = args.profile.as_deref();
    match args.command {
        ProfileCommand::List { json } => run_list(json, profile),
        ProfileCommand::Add {
            installables,
            priority,
            refresh,
            override_input,
        }
        | ProfileCommand::Install {
            installables,
            priority,
            refresh,
            override_input,
        } => run_add(&installables, priority, refresh, &override_input, profile),
        ProfileCommand::Remove { packages } => run_remove(&packages, profile),
        ProfileCommand::Upgrade { package, refresh, override_input } => run_upgrade(package.as_deref(), refresh, &override_input, profile),
        ProfileCommand::History => run_history(profile),
        ProfileCommand::Rollback { to } => run_rollback(to, profile),
        ProfileCommand::WipeHistory {
            older_than,
            dry_run,
        } => run_wipe_history(older_than.as_deref(), dry_run, profile),
        ProfileCommand::DiffClosures => run_diff_closures(profile),
    }
}

fn run_list(output_json: bool, profile: Option<&std::path::Path>) -> Result<()> {
    let mut elements = list_installed_for(profile)?;

    // Sort alphabetically by name
    elements.sort_by(|(a, _), (b, _)| a.cmp(b));

    if output_json {
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

fn run_add(installables: &[String], priority: i32, refresh: bool, override_input: &[String], _profile: Option<&std::path::Path>) -> Result<()> {
    // TODO: Pass profile to install function when profile::install_for is implemented
    let input_overrides = parse_override_inputs(override_input);

    for installable in installables {
        debug!("installing {}...", installable);

        let pkg_name = profile::install(installable, priority, refresh, &input_overrides)?;
        println!("Added {}", pkg_name);
    }

    Ok(())
}

fn run_remove(packages: &[String], _profile: Option<&std::path::Path>) -> Result<()> {
    // TODO: Pass profile to remove function when profile::remove_for is implemented
    for name in packages {
        if profile::remove(name)? {
            println!("Removed: {}", name);
        } else {
            eprintln!("Package not found: {}", name);
        }
    }

    Ok(())
}

fn run_upgrade(name: Option<&str>, refresh: bool, override_input: &[String], _profile: Option<&std::path::Path>) -> Result<()> {
    // TODO: Pass profile to upgrade function when profile::upgrade_for is implemented
    let input_overrides = parse_override_inputs(override_input);
    let (upgraded, skipped) = profile::upgrade(name, refresh, &input_overrides)?;

    if upgraded > 0 {
        println!("Upgraded {} package(s)", upgraded);
    } else if skipped > 0 {
        println!("All {} package(s) up to date", skipped);
    } else {
        println!("No packages to upgrade");
    }

    Ok(())
}

fn run_history(profile: Option<&std::path::Path>) -> Result<()> {
    let profile_dir = get_profile_dir_for(profile)?;

    if !profile_dir.exists() {
        println!("No profile generations found");
        return Ok(());
    }

    // Collect generations with their link path, target, and mtime
    let mut generations: Vec<(u32, std::path::PathBuf, std::path::PathBuf, i64)> = Vec::new();

    for entry in fs::read_dir(&profile_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with("profile-") && name_str.ends_with("-link") {
            if let Some(gen_number) = parse_generation_number(&name_str) {
                if let Ok(target) = fs::read_link(entry.path()) {
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

    let current = get_current_profile_path_for(profile).ok();

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

        // Format version number with ANSI codes
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
        let mut all_packages: BTreeSet<&String> = prev_versions.keys().collect();
        all_packages.extend(curr_versions.keys());

        let mut changes = Vec::new();

        for pkg in all_packages {
            let old_ver = prev_versions.get(pkg);
            let new_ver = curr_versions.get(pkg);

            match (old_ver, new_ver) {
                (None, Some(new)) => {
                    changes.push(format!("  {}: ∅ -> {}", pkg, new));
                }
                (Some(old), None) => {
                    changes.push(format!("  {}: {} -> ∅", pkg, old));
                }
                (Some(old), Some(new)) if old != new => {
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
fn get_generation_manifest(target: &std::path::Path) -> Manifest {
    let manifest_path = target.join("manifest.json");
    if manifest_path.exists() {
        if let Ok(content) = fs::read_to_string(&manifest_path) {
            if let Ok(manifest) = serde_json::from_str(&content) {
                return manifest;
            }
        }
    }
    Manifest {
        version: 3,
        elements: HashMap::new(),
    }
}

/// Extract package name -> version mapping from manifest.
fn get_package_versions(manifest: &Manifest) -> HashMap<String, String> {
    let mut versions = HashMap::new();
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

fn run_rollback(to: Option<u32>, profile: Option<&std::path::Path>) -> Result<()> {
    let profile_dir = get_profile_dir_for(profile)?;
    let current_path = get_current_profile_path_for(profile)?;

    let mut generations: Vec<(u32, std::path::PathBuf)> = Vec::new();

    for entry in fs::read_dir(&profile_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with("profile-") && name_str.ends_with("-link") {
            if let Some(gen) = parse_generation_number(&name_str) {
                generations.push((gen, entry.path()));
            }
        }
    }

    generations.sort_by_key(|(gen, _)| *gen);

    if let Some(target_gen) = to {
        // Roll back to specific version
        if let Some((_, path)) = generations.iter().find(|(gen, _)| *gen == target_gen) {
            let target = fs::read_link(path)?;
            switch_profile(&target.display().to_string())?;
            println!("Rolled back to generation {}", target_gen);
            return Ok(());
        }
        return Err(anyhow!("Generation {} not found", target_gen));
    }

    // Find current generation
    let current_gen = generations
        .iter()
        .find(|(_, path)| fs::read_link(path).ok() == Some(current_path.clone()));

    if let Some((current_gen_num, _)) = current_gen {
        // Find previous generation
        let prev = generations
            .iter()
            .rev()
            .find(|(gen, _)| gen < current_gen_num);

        if let Some((prev_gen, prev_path)) = prev {
            let prev_target = fs::read_link(prev_path)?;
            switch_profile(&prev_target.display().to_string())?;
            println!("Rolled back to generation {}", prev_gen);
            return Ok(());
        }
    }

    Err(anyhow!("No previous generation to roll back to."))
}

fn run_wipe_history(older_than: Option<&str>, dry_run: bool, _profile: Option<&std::path::Path>) -> Result<()> {
    // TODO: Pass profile to wipe_history function when profile::wipe_history_for is implemented
    let older_than_duration = if let Some(ot) = older_than {
        Some(Duration::from_secs(parse_older_than(ot)?))
    } else {
        None
    };

    let count = profile::wipe_history(older_than_duration, dry_run)?;

    if count == 0 {
        println!("No profile versions to delete.");
    } else if dry_run {
        println!("Would delete {} version(s)", count);
    } else {
        println!("Deleted {} version(s)", count);
    }

    Ok(())
}

fn run_diff_closures(profile: Option<&std::path::Path>) -> Result<()> {
    let profile_dir = get_profile_dir_for(profile)?;

    let mut generations = Vec::new();
    for entry in fs::read_dir(&profile_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if let Some(num) = parse_generation_number(&name_str) {
            if let Ok(target) = fs::read_link(entry.path()) {
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
        let mut all_names: BTreeSet<_> = prev_packages.keys().collect();
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
                                "  {}: {} -> {}, {}",
                                name, prev_ver, curr_ver, size_str
                            ));
                        } else {
                            changes.push(format!("  {}: {}", name, size_str));
                        }
                    }
                }
                (None, Some((curr_ver, curr_path))) => {
                    let size = get_store_path_size(curr_path).unwrap_or(0);
                    let size_str = format!("\x1b[31;1m+{}\x1b[0m", format_size(size));
                    changes.push(format!("  {}: ∅ -> {}, {}", name, curr_ver, size_str));
                }
                (Some((prev_ver, prev_path)), None) => {
                    let size = get_store_path_size(prev_path).unwrap_or(0);
                    changes.push(format!(
                        "  {}: {} -> ∅, -{}",
                        name,
                        prev_ver,
                        format_size(size)
                    ));
                }
                (None, None) => {}
            }
        }

        if !changes.is_empty() {
            println!("Version {} -> {}:", prev_num, curr_num);
            for change in changes {
                println!("{}", change);
            }
            println!();
        }
    }

    Ok(())
}
