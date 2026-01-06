//! Diff command - compare derivations, store paths, or profile generations.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use owo_colors::{OwoColorize, Stream::Stdout};
use tracing::info;

use crate::eval::Evaluator;
use crate::flake::{current_system, expand_attribute, resolve_installable, OperationContext};
use crate::profile::{get_profile_dir_for, parse_generation_number, parse_store_path, Manifest};

#[derive(Args)]
pub struct DiffArgs {
    /// First argument (installable, store path, or generation number with --generation)
    #[arg(name = "LEFT")]
    pub left: Option<String>,

    /// Second argument (installable, store path, or generation number with --generation)
    #[arg(name = "RIGHT")]
    pub right: Option<String>,

    /// Compare profile generations instead of installables
    #[arg(long)]
    pub generation: bool,

    /// Profile to use (default: ~/.nix-profile)
    #[arg(long)]
    pub profile: Option<PathBuf>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Include full closure diff (transitive dependencies)
    #[arg(long)]
    pub closure: bool,
}

pub fn run(args: DiffArgs) -> Result<()> {
    if args.generation {
        run_generation_diff(args)
    } else {
        run_derivation_diff(args)
    }
}

/// Compare two profile generations.
fn run_generation_diff(args: DiffArgs) -> Result<()> {
    let profile = args.profile.as_deref();
    let profile_dir = get_profile_dir_for(profile)?;

    // Collect all generations
    let generations = collect_generations(&profile_dir)?;
    if generations.is_empty() {
        return Err(anyhow!("No profile generations found"));
    }

    let current_gen = generations.iter().max().copied().unwrap_or(1);

    // Parse left and right generation numbers
    let (left_gen, right_gen) = match (&args.left, &args.right) {
        (None, None) => {
            // Compare previous to current
            if generations.len() < 2 {
                return Err(anyhow!("Need at least 2 generations to compare"));
            }
            let prev = generations
                .iter()
                .filter(|&&g| g < current_gen)
                .max()
                .copied()
                .ok_or_else(|| anyhow!("No previous generation found"))?;
            (prev, current_gen)
        }
        (Some(left), None) => {
            // Compare left to current
            let left: u32 = left
                .parse()
                .context("Expected generation number for LEFT")?;
            (left, current_gen)
        }
        (Some(left), Some(right)) => {
            let left: u32 = left
                .parse()
                .context("Expected generation number for LEFT")?;
            let right: u32 = right
                .parse()
                .context("Expected generation number for RIGHT")?;
            (left, right)
        }
        (None, Some(_)) => {
            return Err(anyhow!("LEFT generation is required if RIGHT is specified"));
        }
    };

    // Validate generations exist
    if !generations.contains(&left_gen) {
        return Err(anyhow!("Generation {} not found", left_gen));
    }
    if !generations.contains(&right_gen) {
        return Err(anyhow!("Generation {} not found", right_gen));
    }

    info!("comparing generations {} -> {}", left_gen, right_gen);

    // Read manifests
    let left_manifest = read_generation_manifest(&profile_dir, left_gen)?;
    let right_manifest = read_generation_manifest(&profile_dir, right_gen)?;

    // Compare manifests
    let diff = compare_manifests(&left_manifest, &right_manifest);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&diff)?);
    } else {
        print_generation_diff(left_gen, right_gen, &diff);
    }

    Ok(())
}

/// Compare two derivations or installables.
fn run_derivation_diff(args: DiffArgs) -> Result<()> {
    let left = args
        .left
        .as_ref()
        .ok_or_else(|| anyhow!("LEFT argument is required"))?;
    let right = args
        .right
        .as_ref()
        .ok_or_else(|| anyhow!("RIGHT argument is required"))?;

    // Resolve to derivation paths
    let left_drv = resolve_to_drv(left)?;
    let right_drv = resolve_to_drv(right)?;

    info!("comparing {} vs {}", left_drv, right_drv);

    if left_drv == right_drv {
        println!(
            "{}",
            "Derivations are identical"
                .if_supports_color(Stdout, |t| t.green())
        );
        return Ok(());
    }

    // Parse both derivations
    let left_parsed = parse_derivation(&left_drv)?;
    let right_parsed = parse_derivation(&right_drv)?;

    // Compare them
    let diff = compare_derivations(&left_parsed, &right_parsed);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&diff)?);
    } else {
        print_derivation_diff(left, right, &diff, args.closure)?;
    }

    Ok(())
}

/// Collect all generation numbers from profile directory.
fn collect_generations(profile_dir: &Path) -> Result<Vec<u32>> {
    let mut generations = Vec::new();

    if !profile_dir.exists() {
        return Ok(generations);
    }

    for entry in fs::read_dir(profile_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if let Some(gen) = parse_generation_number(&name_str) {
            generations.push(gen);
        }
    }

    generations.sort();
    Ok(generations)
}

/// Read manifest from a specific generation.
fn read_generation_manifest(profile_dir: &Path, generation: u32) -> Result<Manifest> {
    let gen_link = profile_dir.join(format!("profile-{}-link", generation));
    let store_path =
        fs::read_link(&gen_link).context(format!("Could not read generation {}", generation))?;

    let manifest_path = store_path.join("manifest.json");
    if !manifest_path.exists() {
        return Ok(Manifest::default());
    }

    let content = fs::read_to_string(&manifest_path)?;
    let manifest: Manifest = serde_json::from_str(&content)?;
    Ok(manifest)
}

/// Diff result for profile generations.
#[derive(Debug, serde::Serialize)]
struct GenerationDiff {
    added: Vec<PackageInfo>,
    removed: Vec<PackageInfo>,
    upgraded: Vec<PackageUpgrade>,
    downgraded: Vec<PackageUpgrade>,
    rebuilt: Vec<PackageInfo>,
}

#[derive(Debug, serde::Serialize)]
struct PackageInfo {
    name: String,
    version: String,
    store_path: String,
}

#[derive(Debug, serde::Serialize)]
struct PackageUpgrade {
    name: String,
    old_version: String,
    new_version: String,
    old_path: String,
    new_path: String,
}

/// Compare two manifests and return the diff.
fn compare_manifests(left: &Manifest, right: &Manifest) -> GenerationDiff {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut upgraded = Vec::new();
    let mut downgraded = Vec::new();
    let mut rebuilt = Vec::new();

    let left_names: HashSet<_> = left.elements.keys().collect();
    let right_names: HashSet<_> = right.elements.keys().collect();

    // Added packages
    for name in right_names.difference(&left_names) {
        if let Some(elem) = right.elements.get(*name) {
            let store_path = elem.store_paths.first().cloned().unwrap_or_default();
            let (_, version) = parse_store_path(&store_path).unwrap_or_default();
            added.push(PackageInfo {
                name: (*name).clone(),
                version,
                store_path,
            });
        }
    }

    // Removed packages
    for name in left_names.difference(&right_names) {
        if let Some(elem) = left.elements.get(*name) {
            let store_path = elem.store_paths.first().cloned().unwrap_or_default();
            let (_, version) = parse_store_path(&store_path).unwrap_or_default();
            removed.push(PackageInfo {
                name: (*name).clone(),
                version,
                store_path,
            });
        }
    }

    // Changed packages
    for name in left_names.intersection(&right_names) {
        let left_elem = left.elements.get(*name).unwrap();
        let right_elem = right.elements.get(*name).unwrap();

        let left_path = left_elem.store_paths.first().cloned().unwrap_or_default();
        let right_path = right_elem.store_paths.first().cloned().unwrap_or_default();

        if left_path != right_path {
            let (_, left_ver) = parse_store_path(&left_path).unwrap_or_default();
            let (_, right_ver) = parse_store_path(&right_path).unwrap_or_default();

            if left_ver != right_ver {
                // Version changed
                let upgrade = PackageUpgrade {
                    name: (*name).clone(),
                    old_version: left_ver.clone(),
                    new_version: right_ver.clone(),
                    old_path: left_path,
                    new_path: right_path,
                };

                // Compare versions to determine if upgrade or downgrade
                if version_cmp(&left_ver, &right_ver).is_lt() {
                    upgraded.push(upgrade);
                } else {
                    downgraded.push(upgrade);
                }
            } else {
                // Same version but different path - rebuilt
                rebuilt.push(PackageInfo {
                    name: (*name).clone(),
                    version: right_ver,
                    store_path: right_path,
                });
            }
        }
    }

    GenerationDiff {
        added,
        removed,
        upgraded,
        downgraded,
        rebuilt,
    }
}

/// Simple version comparison (lexicographic on version segments).
fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let a_parts: Vec<&str> = a.split('.').collect();
    let b_parts: Vec<&str> = b.split('.').collect();

    for (a_part, b_part) in a_parts.iter().zip(b_parts.iter()) {
        // Try numeric comparison first
        if let (Ok(a_num), Ok(b_num)) = (a_part.parse::<u64>(), b_part.parse::<u64>()) {
            match a_num.cmp(&b_num) {
                std::cmp::Ordering::Equal => continue,
                other => return other,
            }
        }
        // Fall back to string comparison
        match a_part.cmp(b_part) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }

    a_parts.len().cmp(&b_parts.len())
}

/// Print generation diff in a nice format.
fn print_generation_diff(left_gen: u32, right_gen: u32, diff: &GenerationDiff) {
    println!(
        "Comparing generation {} {} {}",
        left_gen.to_string().if_supports_color(Stdout, |t| t.bold()),
        "→".if_supports_color(Stdout, |t| t.dimmed()),
        right_gen.to_string().if_supports_color(Stdout, |t| t.bold())
    );
    println!();

    let has_changes = !diff.added.is_empty()
        || !diff.removed.is_empty()
        || !diff.upgraded.is_empty()
        || !diff.downgraded.is_empty()
        || !diff.rebuilt.is_empty();

    if !has_changes {
        println!(
            "{}",
            "No changes"
                .if_supports_color(Stdout, |t| t.dimmed())
        );
        return;
    }

    // Summary line
    let mut parts = Vec::new();
    if !diff.added.is_empty() {
        parts.push(format!(
            "{} added",
            diff.added
                .len()
                .to_string()
                .if_supports_color(Stdout, |t| t.green())
        ));
    }
    if !diff.removed.is_empty() {
        parts.push(format!(
            "{} removed",
            diff.removed
                .len()
                .to_string()
                .if_supports_color(Stdout, |t| t.red())
        ));
    }
    if !diff.upgraded.is_empty() {
        parts.push(format!(
            "{} upgraded",
            diff.upgraded
                .len()
                .to_string()
                .if_supports_color(Stdout, |t| t.cyan())
        ));
    }
    if !diff.downgraded.is_empty() {
        parts.push(format!(
            "{} downgraded",
            diff.downgraded
                .len()
                .to_string()
                .if_supports_color(Stdout, |t| t.yellow())
        ));
    }
    if !diff.rebuilt.is_empty() {
        parts.push(format!(
            "{} rebuilt",
            diff.rebuilt
                .len()
                .to_string()
                .if_supports_color(Stdout, |t| t.dimmed())
        ));
    }
    println!("{}", parts.join(", "));
    println!();

    // Added
    if !diff.added.is_empty() {
        println!(
            "{} {}",
            "+".if_supports_color(Stdout, |t| t.green()),
            "Added:".if_supports_color(Stdout, |t| t.bold())
        );
        for pkg in &diff.added {
            let ver_str = if pkg.version.is_empty() {
                String::new()
            } else {
                format!(" {}", pkg.version.if_supports_color(Stdout, |t| t.dimmed()))
            };
            println!(
                "  {} {}{}",
                "+".if_supports_color(Stdout, |t| t.green()),
                pkg.name,
                ver_str
            );
        }
        println!();
    }

    // Removed
    if !diff.removed.is_empty() {
        println!(
            "{} {}",
            "-".if_supports_color(Stdout, |t| t.red()),
            "Removed:".if_supports_color(Stdout, |t| t.bold())
        );
        for pkg in &diff.removed {
            let ver_str = if pkg.version.is_empty() {
                String::new()
            } else {
                format!(" {}", pkg.version.if_supports_color(Stdout, |t| t.dimmed()))
            };
            println!(
                "  {} {}{}",
                "-".if_supports_color(Stdout, |t| t.red()),
                pkg.name,
                ver_str
            );
        }
        println!();
    }

    // Upgraded
    if !diff.upgraded.is_empty() {
        println!(
            "{} {}",
            "↑".if_supports_color(Stdout, |t| t.cyan()),
            "Upgraded:".if_supports_color(Stdout, |t| t.bold())
        );
        for pkg in &diff.upgraded {
            println!(
                "  {} {} {} {}",
                pkg.name,
                pkg.old_version.if_supports_color(Stdout, |t| t.dimmed()),
                "→".if_supports_color(Stdout, |t| t.dimmed()),
                pkg.new_version.if_supports_color(Stdout, |t| t.cyan())
            );
        }
        println!();
    }

    // Downgraded
    if !diff.downgraded.is_empty() {
        println!(
            "{} {}",
            "↓".if_supports_color(Stdout, |t| t.yellow()),
            "Downgraded:".if_supports_color(Stdout, |t| t.bold())
        );
        for pkg in &diff.downgraded {
            println!(
                "  {} {} {} {}",
                pkg.name,
                pkg.old_version.if_supports_color(Stdout, |t| t.dimmed()),
                "→".if_supports_color(Stdout, |t| t.yellow()),
                pkg.new_version.if_supports_color(Stdout, |t| t.yellow())
            );
        }
        println!();
    }

    // Rebuilt
    if !diff.rebuilt.is_empty() {
        println!(
            "{} {}",
            "~".if_supports_color(Stdout, |t| t.dimmed()),
            "Rebuilt (same version, different derivation):".if_supports_color(Stdout, |t| t.bold())
        );
        for pkg in &diff.rebuilt {
            let ver_str = if pkg.version.is_empty() {
                String::new()
            } else {
                format!(" {}", pkg.version.if_supports_color(Stdout, |t| t.dimmed()))
            };
            println!(
                "  {} {}{}",
                "~".if_supports_color(Stdout, |t| t.dimmed()),
                pkg.name,
                ver_str
            );
        }
    }
}

/// Resolve an argument to a derivation path.
fn resolve_to_drv(arg: &str) -> Result<String> {
    // Check if it's already a store path
    if arg.starts_with("/nix/store/") {
        if arg.ends_with(".drv") {
            return Ok(arg.to_string());
        }
        // Get the derivation for this output
        return get_drv_for_output(arg);
    }

    // Try to resolve as local installable first
    let cwd = env::current_dir().context("failed to get current directory")?;
    if let Ok(resolved) = resolve_installable(arg, &cwd) {
        let is_local = resolved.path.exists() && resolved.path.join("flake.nix").exists();

        if is_local {
            let system = current_system()?;
            let candidates = expand_attribute(&resolved.attribute, OperationContext::Build, &system);
            let attr_path = &candidates[0];
            let mut eval = Evaluator::new().context("failed to initialize evaluator")?;
            let value = eval
                .eval_flake_attr(&resolved.path, attr_path)
                .context("failed to evaluate derivation")?;
            return eval.get_drv_path(&value);
        }
    }

    // Fall back to nix for remote references - use path-info --derivation
    let output = Command::new("nix")
        .args(["path-info", "--derivation", arg])
        .output()
        .context("failed to run nix path-info")?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        // Try nix eval approach as last resort
        let output = Command::new("nix")
            .args([
                "eval",
                "--raw",
                &format!("{}#.drvPath", arg),
            ])
            .output()
            .context("failed to run nix eval")?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(anyhow!("Could not resolve {} to derivation", arg))
        }
    }
}

/// Get the derivation path for a store output.
fn get_drv_for_output(output_path: &str) -> Result<String> {
    let output = Command::new("nix-store")
        .args(["--query", "--deriver", output_path])
        .output()
        .context("failed to run nix-store --query")?;

    if !output.status.success() {
        return Err(anyhow!(
            "Could not find deriver for {}",
            output_path
        ));
    }

    let drv = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if drv == "unknown-deriver" {
        return Err(anyhow!("Deriver unknown for {}", output_path));
    }

    Ok(drv)
}

/// Parsed derivation structure.
#[derive(Debug, Clone)]
struct ParsedDerivation {
    input_drvs: HashMap<String, Vec<String>>,
    input_srcs: Vec<String>,
    platform: String,
    builder: String,
    args: Vec<String>,
    env: HashMap<String, String>,
}

/// Parse a .drv file.
fn parse_derivation(drv_path: &str) -> Result<ParsedDerivation> {
    // Use nix derivation show for JSON output
    let output = Command::new("nix")
        .args(["derivation", "show", drv_path])
        .output()
        .context("failed to run nix derivation show")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("nix derivation show failed: {}", stderr));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;

    // The output is a map with the drv path as key
    let drv_data = json
        .as_object()
        .and_then(|m| m.values().next())
        .ok_or_else(|| anyhow!("Invalid derivation show output"))?;

    let input_drvs = drv_data
        .get("inputDrvs")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    let outputs = v
                        .get("outputs")
                        .and_then(|o| o.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|s| s.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    (k.clone(), outputs)
                })
                .collect()
        })
        .unwrap_or_default();

    let input_srcs = drv_data
        .get("inputSrcs")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let platform = drv_data
        .get("system")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let builder = drv_data
        .get("builder")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let args = drv_data
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let env = drv_data
        .get("env")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect()
        })
        .unwrap_or_default();

    Ok(ParsedDerivation {
        input_drvs,
        input_srcs,
        platform,
        builder,
        args,
        env,
    })
}

/// Diff result for derivations.
#[derive(Debug, serde::Serialize)]
struct DerivationDiff {
    platform_changed: Option<(String, String)>,
    builder_changed: Option<(String, String)>,
    args_changed: bool,
    added_inputs: Vec<String>,
    removed_inputs: Vec<String>,
    changed_inputs: Vec<InputChange>,
    added_srcs: Vec<String>,
    removed_srcs: Vec<String>,
    env_added: Vec<String>,
    env_removed: Vec<String>,
    env_changed: Vec<EnvChange>,
}

#[derive(Debug, serde::Serialize)]
struct InputChange {
    name: String,
    old_drv: String,
    new_drv: String,
}

#[derive(Debug, serde::Serialize)]
struct EnvChange {
    key: String,
    old_value: String,
    new_value: String,
}

/// Compare two parsed derivations.
fn compare_derivations(left: &ParsedDerivation, right: &ParsedDerivation) -> DerivationDiff {
    let platform_changed = if left.platform != right.platform {
        Some((left.platform.clone(), right.platform.clone()))
    } else {
        None
    };

    let builder_changed = if left.builder != right.builder {
        Some((left.builder.clone(), right.builder.clone()))
    } else {
        None
    };

    let args_changed = left.args != right.args;

    // Compare input derivations
    let left_inputs: HashSet<_> = left.input_drvs.keys().collect();
    let right_inputs: HashSet<_> = right.input_drvs.keys().collect();

    let added_inputs: Vec<String> = right_inputs
        .difference(&left_inputs)
        .map(|s| extract_input_name(s))
        .collect();

    let removed_inputs: Vec<String> = left_inputs
        .difference(&right_inputs)
        .map(|s| extract_input_name(s))
        .collect();

    // For inputs in both, compare by package name
    let mut changed_inputs = Vec::new();
    let left_by_name = group_inputs_by_name(&left.input_drvs);
    let right_by_name = group_inputs_by_name(&right.input_drvs);

    for (name, left_drv) in &left_by_name {
        if let Some(right_drv) = right_by_name.get(name) {
            if left_drv != right_drv {
                changed_inputs.push(InputChange {
                    name: name.clone(),
                    old_drv: left_drv.clone(),
                    new_drv: right_drv.clone(),
                });
            }
        }
    }

    // Compare input sources
    let left_srcs: HashSet<_> = left.input_srcs.iter().collect();
    let right_srcs: HashSet<_> = right.input_srcs.iter().collect();

    let added_srcs: Vec<String> = right_srcs
        .difference(&left_srcs)
        .map(|s| (*s).clone())
        .collect();
    let removed_srcs: Vec<String> = left_srcs
        .difference(&right_srcs)
        .map(|s| (*s).clone())
        .collect();

    // Compare env vars (filter out noisy ones)
    let ignore_keys = ["out", "outputs", "drvPath", "builder"];
    let left_env: HashMap<_, _> = left
        .env
        .iter()
        .filter(|(k, _)| !ignore_keys.contains(&k.as_str()))
        .collect();
    let right_env: HashMap<_, _> = right
        .env
        .iter()
        .filter(|(k, _)| !ignore_keys.contains(&k.as_str()))
        .collect();

    let left_keys: HashSet<_> = left_env.keys().collect();
    let right_keys: HashSet<_> = right_env.keys().collect();

    let env_added: Vec<String> = right_keys
        .difference(&left_keys)
        .map(|k| (**k).clone())
        .collect();
    let env_removed: Vec<String> = left_keys
        .difference(&right_keys)
        .map(|k| (**k).clone())
        .collect();

    let mut env_changed = Vec::new();
    for key in left_keys.intersection(&right_keys) {
        let left_val = left_env.get(*key).unwrap();
        let right_val = right_env.get(*key).unwrap();
        if left_val != right_val {
            env_changed.push(EnvChange {
                key: (**key).clone(),
                old_value: (**left_val).clone(),
                new_value: (**right_val).clone(),
            });
        }
    }

    DerivationDiff {
        platform_changed,
        builder_changed,
        args_changed,
        added_inputs,
        removed_inputs,
        changed_inputs,
        added_srcs,
        removed_srcs,
        env_added,
        env_removed,
        env_changed,
    }
}

/// Extract package name from derivation path.
fn extract_input_name(drv_path: &str) -> String {
    // /nix/store/abc123-name-1.2.3.drv -> name-1.2.3
    Path::new(drv_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.split_once('-').map(|(_, rest)| rest))
        .unwrap_or(drv_path)
        .to_string()
}

/// Group input derivations by package name.
fn group_inputs_by_name(inputs: &HashMap<String, Vec<String>>) -> HashMap<String, String> {
    let mut result = HashMap::new();
    for (drv_path, _) in inputs {
        let name = extract_input_name(drv_path);
        // Split name from version for grouping
        let base_name = name
            .rfind('-')
            .and_then(|pos| {
                if name[pos + 1..]
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_digit())
                    .unwrap_or(false)
                {
                    Some(&name[..pos])
                } else {
                    None
                }
            })
            .unwrap_or(&name);
        result.insert(base_name.to_string(), drv_path.clone());
    }
    result
}

/// Print derivation diff in a nice format.
fn print_derivation_diff(
    left_name: &str,
    right_name: &str,
    diff: &DerivationDiff,
    _show_closure: bool,
) -> Result<()> {
    println!(
        "Comparing {} {} {}",
        left_name.if_supports_color(Stdout, |t| t.bold()),
        "→".if_supports_color(Stdout, |t| t.dimmed()),
        right_name.if_supports_color(Stdout, |t| t.bold())
    );
    println!();

    let has_changes = diff.platform_changed.is_some()
        || diff.builder_changed.is_some()
        || diff.args_changed
        || !diff.added_inputs.is_empty()
        || !diff.removed_inputs.is_empty()
        || !diff.changed_inputs.is_empty()
        || !diff.added_srcs.is_empty()
        || !diff.removed_srcs.is_empty()
        || !diff.env_added.is_empty()
        || !diff.env_removed.is_empty()
        || !diff.env_changed.is_empty();

    if !has_changes {
        println!(
            "{}",
            "No significant differences"
                .if_supports_color(Stdout, |t| t.dimmed())
        );
        return Ok(());
    }

    // Platform
    if let Some((old, new)) = &diff.platform_changed {
        println!(
            "{} {} {} {}",
            "Platform:".if_supports_color(Stdout, |t| t.bold()),
            old.if_supports_color(Stdout, |t| t.red()),
            "→".if_supports_color(Stdout, |t| t.dimmed()),
            new.if_supports_color(Stdout, |t| t.green())
        );
    }

    // Builder
    if let Some((old, new)) = &diff.builder_changed {
        println!(
            "{} {} {} {}",
            "Builder:".if_supports_color(Stdout, |t| t.bold()),
            old.if_supports_color(Stdout, |t| t.red()),
            "→".if_supports_color(Stdout, |t| t.dimmed()),
            new.if_supports_color(Stdout, |t| t.green())
        );
    }

    if diff.args_changed {
        println!(
            "{}",
            "Builder arguments changed"
                .if_supports_color(Stdout, |t| t.yellow())
        );
    }

    // Input changes
    if !diff.added_inputs.is_empty() || !diff.removed_inputs.is_empty() || !diff.changed_inputs.is_empty() {
        println!();
        println!(
            "{}",
            "Input derivations:".if_supports_color(Stdout, |t| t.bold())
        );

        for input in &diff.added_inputs {
            println!(
                "  {} {}",
                "+".if_supports_color(Stdout, |t| t.green()),
                input
            );
        }
        for input in &diff.removed_inputs {
            println!(
                "  {} {}",
                "-".if_supports_color(Stdout, |t| t.red()),
                input
            );
        }
        for change in &diff.changed_inputs {
            let old_ver = extract_input_name(&change.old_drv);
            let new_ver = extract_input_name(&change.new_drv);
            println!(
                "  {} {} {} {}",
                "~".if_supports_color(Stdout, |t| t.yellow()),
                change.name,
                old_ver.if_supports_color(Stdout, |t| t.dimmed()),
                format!("→ {}", new_ver).if_supports_color(Stdout, |t| t.yellow())
            );
        }
    }

    // Source changes
    if !diff.added_srcs.is_empty() || !diff.removed_srcs.is_empty() {
        println!();
        println!(
            "{}",
            "Input sources:".if_supports_color(Stdout, |t| t.bold())
        );
        for src in &diff.added_srcs {
            println!(
                "  {} {}",
                "+".if_supports_color(Stdout, |t| t.green()),
                src
            );
        }
        for src in &diff.removed_srcs {
            println!(
                "  {} {}",
                "-".if_supports_color(Stdout, |t| t.red()),
                src
            );
        }
    }

    // Env changes (limit output)
    let total_env_changes = diff.env_added.len() + diff.env_removed.len() + diff.env_changed.len();
    if total_env_changes > 0 {
        println!();
        println!(
            "{}",
            format!("Environment variables ({} changes):", total_env_changes)
                .if_supports_color(Stdout, |t| t.bold())
        );

        // Only show first few to avoid noise
        let max_show = 5;
        let mut shown = 0;

        for key in &diff.env_added {
            if shown >= max_show {
                break;
            }
            println!(
                "  {} {}",
                "+".if_supports_color(Stdout, |t| t.green()),
                key
            );
            shown += 1;
        }
        for key in &diff.env_removed {
            if shown >= max_show {
                break;
            }
            println!(
                "  {} {}",
                "-".if_supports_color(Stdout, |t| t.red()),
                key
            );
            shown += 1;
        }
        for change in &diff.env_changed {
            if shown >= max_show {
                break;
            }
            println!(
                "  {} {}",
                "~".if_supports_color(Stdout, |t| t.yellow()),
                change.key
            );
            shown += 1;
        }

        if total_env_changes > max_show {
            println!(
                "  {}",
                format!("... and {} more", total_env_changes - max_show)
                    .if_supports_color(Stdout, |t| t.dimmed())
            );
        }
    }

    Ok(())
}
