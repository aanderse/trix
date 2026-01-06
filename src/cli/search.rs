//! Search command - search for packages in flakes.
//!
//! Similar to `nix search`, this command searches for packages in a flake
//! by evaluating packages/legacyPackages and matching against regex patterns.

use std::env;

use anyhow::{Context, Result};
use clap::Args;
use regex::Regex;

use crate::eval::Evaluator;
use crate::flake::{current_system, resolve_installable_any};
use crate::progress;

#[derive(Args)]
pub struct SearchArgs {
    /// Flake to search in (default: nixpkgs from registry)
    #[arg(default_value = "nixpkgs")]
    pub flake_ref: String,

    /// Regex patterns to search for (matches name or description)
    #[arg()]
    pub patterns: Vec<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Don't print descriptions
    #[arg(long)]
    pub no_desc: bool,
}

#[derive(serde::Serialize)]
struct SearchResult {
    attr_path: String,
    name: String,
    version: String,
    description: String,
}

pub fn run(args: SearchArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;
    let system = current_system()?;

    // Resolve the flake reference (handles local paths, registry names, and remote refs)
    let resolved = resolve_installable_any(&args.flake_ref, &cwd);

    let status = progress::evaluating(&format!("{}#legacyPackages.{}", args.flake_ref, system));

    let mut evaluator = Evaluator::new().context("failed to initialize Nix evaluator")?;

    // Compile regex patterns
    let regexes: Vec<Regex> = args
        .patterns
        .iter()
        .map(|p| Regex::new(p))
        .collect::<Result<Vec<_>, _>>()
        .context("invalid regex pattern")?;

    // Try legacyPackages first, then packages
    let attr_bases = [
        format!("legacyPackages.{}", system),
        format!("packages.{}", system),
    ];

    let mut results = Vec::new();
    let mut found_base = false;

    // Helper to evaluate a flake attribute (works for both local and remote)
    let eval_attr = |evaluator: &mut Evaluator, attr_path: &[String]| -> Result<crate::eval::NixValue> {
        if resolved.is_local {
            let flake_path = resolved.path.as_ref().unwrap();
            evaluator.eval_flake_attr(flake_path, attr_path)
        } else {
            let flake_ref = resolved.flake_ref.as_deref().unwrap_or(&args.flake_ref);
            let full_ref = format!("{}#{}", flake_ref, attr_path.join("."));
            evaluator.eval_flake_ref(&full_ref, &cwd)
        }
    };

    for attr_base in &attr_bases {
        let base_path: Vec<String> = attr_base.split('.').map(String::from).collect();

        // Try to evaluate the base attribute
        match eval_attr(&mut evaluator, &base_path) {
            Ok(base_value) => {
                found_base = true;
                status.finish_and_clear();

                // Get attribute names
                let attr_names = match evaluator.get_attr_names(&base_value) {
                    Ok(names) => names,
                    Err(_) => continue,
                };

                for attr_name in attr_names {
                    // Skip internal attributes
                    if attr_name.starts_with('_') {
                        continue;
                    }

                    // Try to get package info
                    let mut path = base_path.clone();
                    path.push(attr_name.clone());

                    if let Ok(pkg_value) = eval_attr(&mut evaluator, &path) {
                        // Try to get meta.description and name/pname
                        let name = get_package_name(&mut evaluator, &pkg_value)
                            .unwrap_or_else(|| attr_name.clone());
                        let version =
                            get_package_version(&mut evaluator, &pkg_value).unwrap_or_default();
                        let description =
                            get_package_description(&mut evaluator, &pkg_value).unwrap_or_default();

                        // Check if matches any pattern (or show all if no patterns)
                        let matches = if regexes.is_empty() {
                            true
                        } else {
                            regexes.iter().any(|re| {
                                re.is_match(&attr_name)
                                    || re.is_match(&name)
                                    || re.is_match(&description)
                            })
                        };

                        if matches {
                            results.push(SearchResult {
                                attr_path: format!("{}.{}", attr_base, attr_name),
                                name,
                                version,
                                description,
                            });
                        }
                    }
                }
                break; // Found a valid base, don't try others
            }
            Err(_) => continue,
        }
    }

    status.finish_and_clear();

    if !found_base {
        anyhow::bail!(
            "flake '{}' does not have packages.{} or legacyPackages.{}",
            args.flake_ref,
            system,
            system
        );
    }

    // Output results
    if args.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        for result in &results {
            // Format: * attr_path (version)
            //   description
            let version_str = if result.version.is_empty() {
                String::new()
            } else {
                format!(" ({})", result.version)
            };

            println!(
                "* \x1b[1m{}\x1b[0m{}",
                result.attr_path, version_str
            );

            if !args.no_desc && !result.description.is_empty() {
                println!("  {}", result.description);
            }
            println!();
        }

        if results.is_empty() {
            if args.patterns.is_empty() {
                println!("No packages found.");
            } else {
                println!(
                    "No packages matching '{}' found.",
                    args.patterns.join(" ")
                );
            }
        }
    }

    Ok(())
}

fn get_package_name(evaluator: &mut Evaluator, pkg_value: &crate::eval::NixValue) -> Option<String> {
    // Try pname first, then name
    if let Ok(Some(pname_val)) = evaluator.get_attr(pkg_value, "pname") {
        if let Ok(s) = evaluator.require_string(&pname_val) {
            return Some(s);
        }
    }
    if let Ok(Some(name_val)) = evaluator.get_attr(pkg_value, "name") {
        if let Ok(s) = evaluator.require_string(&name_val) {
            // Strip version suffix from name (e.g., "hello-2.10" -> "hello")
            if let Some(pos) = s.rfind('-') {
                let (name, version) = s.split_at(pos);
                if version[1..].chars().next().map_or(false, |c| c.is_ascii_digit()) {
                    return Some(name.to_string());
                }
            }
            return Some(s);
        }
    }
    None
}

fn get_package_version(evaluator: &mut Evaluator, pkg_value: &crate::eval::NixValue) -> Option<String> {
    if let Ok(Some(version_val)) = evaluator.get_attr(pkg_value, "version") {
        if let Ok(s) = evaluator.require_string(&version_val) {
            return Some(s);
        }
    }
    // Try extracting from name
    if let Ok(Some(name_val)) = evaluator.get_attr(pkg_value, "name") {
        if let Ok(s) = evaluator.require_string(&name_val) {
            if let Some(pos) = s.rfind('-') {
                let version = &s[pos + 1..];
                if version.chars().next().map_or(false, |c| c.is_ascii_digit()) {
                    return Some(version.to_string());
                }
            }
        }
    }
    None
}

fn get_package_description(
    evaluator: &mut Evaluator,
    pkg_value: &crate::eval::NixValue,
) -> Option<String> {
    // Try meta.description
    if let Ok(Some(meta_val)) = evaluator.get_attr(pkg_value, "meta") {
        if let Ok(Some(desc_val)) = evaluator.get_attr(&meta_val, "description") {
            if let Ok(s) = evaluator.require_string(&desc_val) {
                return Some(s);
            }
        }
    }
    None
}
