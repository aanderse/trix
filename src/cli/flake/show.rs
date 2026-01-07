//! Flake show command - display flake outputs structure.
//!
//! This module uses a unified tree representation for flake outputs.
//! Both JSON and tree display use the same `build_output_tree` function
//! to ensure consistent output regardless of format.

use std::collections::BTreeMap;
use std::env;

use anyhow::{Context, Result};
use clap::Args;
use owo_colors::{OwoColorize, Stream::Stdout};
use serde_json::json;
use tracing::{debug, instrument};

use crate::eval::Evaluator;
use crate::flake::{current_system, resolve_installable_any};
use crate::progress;

// =============================================================================
// Unified Output Tree Representation
// =============================================================================

/// A node in the flake output tree.
/// This is the single source of truth for what gets displayed.
#[derive(Debug, Clone)]
enum OutputNode {
    /// A derivation (package, devShell, check, formatter, etc.)
    Derivation {
        name: String,
        description: String,
        /// Category for display (e.g., "package", "development environment")
        category: String,
    },
    /// An app (type = "app")
    App {
        name: String,
        description: Option<String>,
    },
    /// An opaque value that shouldn't be enumerated (lib, flakeModule, etc.)
    /// May have an optional description (e.g., templates)
    Opaque {
        /// The output category (e.g., "nixosModules", "templates")
        output_category: String,
        description: Option<String>,
    },
    /// A value omitted due to system filtering (use --all-systems)
    Omitted,
    /// A value omitted because it's legacyPackages (use --legacy)
    OmittedLegacy,
    /// An attribute set containing child nodes
    AttrSet(BTreeMap<String, OutputNode>),
}

#[derive(Args)]
pub struct ShowArgs {
    /// Flake reference (default: .)
    #[arg(default_value = ".")]
    pub flake_ref: String,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Show outputs for all systems
    #[arg(long)]
    pub all_systems: bool,

    /// Show the contents of the legacyPackages output
    #[arg(long)]
    pub legacy: bool,
}

/// Known output categories that have per-system structure
const PER_SYSTEM_OUTPUTS: &[&str] = &[
    "packages",
    "devShells",
    "apps",
    "checks",
    "legacyPackages",
    "formatter",
    // Legacy singular forms
    "defaultPackage",
    "devShell",
    "defaultApp",
];

#[instrument(level = "debug", skip_all, fields(flake_ref = %args.flake_ref, json = args.json))]
pub fn run(args: ShowArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;
    debug!("initializing evaluator");

    // Resolve the flake reference (handles local paths, registry names, and remote refs)
    debug!("resolving flake reference");
    let resolved = resolve_installable_any(&args.flake_ref, &cwd);

    let mut evaluator = Evaluator::new().context("failed to initialize Nix evaluator")?;

    let (flake, flake_url) = if resolved.is_local {
        let flake_path = resolved
            .path
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("local flake must have path"))?;
        let flake_url = format!("path:{}", flake_path.display());

        // Evaluate the local flake
        debug!("evaluating local flake outputs");
        let status = progress::evaluating(&flake_url);
        let flake = evaluator.eval_flake_attr(flake_path, &[])?;
        status.finish_and_clear();

        (flake, flake_url)
    } else {
        // Remote flake - use native flake API
        let flake_ref = resolved
            .flake_ref
            .as_deref()
            .unwrap_or(&args.flake_ref);
        let flake_url = flake_ref.to_string();

        debug!("evaluating remote flake outputs");
        let status = progress::evaluating(&flake_url);
        let flake = evaluator.eval_flake_ref(flake_ref, &cwd)?;
        status.finish_and_clear();

        (flake, flake_url)
    };

    let current_sys = current_system()?;
    debug!(%current_sys, "detected current system");

    // Build the unified output tree - single source of truth
    let output_tree = build_output_tree(
        &mut evaluator,
        &flake,
        &current_sys,
        args.all_systems,
        args.legacy,
    )?;

    if args.json {
        let json_output = render_as_json(&output_tree);
        println!("{}", serde_json::to_string(&json_output)?);
    } else {
        // Print the flake URL
        println!("{}", flake_url.if_supports_color(Stdout, |t| t.bold()));
        render_as_tree(&output_tree, "", true);
    }

    Ok(())
}

/// Check if an output name is a known flake output type
/// Based on: https://github.com/DeterminateSystems/flake-schemas
fn is_known_output(name: &str) -> bool {
    matches!(
        name,
        // Per-system outputs
        "packages"
            | "devShells"
            | "apps"
            | "checks"
            | "legacyPackages"
            | "formatter"
            // Non-per-system outputs
            | "overlays"
            | "nixosModules"
            | "nixosConfigurations"
            | "darwinModules"
            | "darwinConfigurations"
            | "homeModules"
            | "homeConfigurations"
            | "homeManagerModules"
            | "homeManagerModule"
            | "templates"
            | "lib"
            | "library"
            | "dockerImages"
            | "hydraJobs"
            // flake-parts outputs
            | "flakeModule"
            | "flakeModules"
            | "formatterModule"
            // Legacy singular forms
            | "defaultPackage"
            | "devShell"
            | "defaultApp"
            | "overlay"
            | "nixosModule"
            | "darwinModule"
            | "homeModule"
    )
}

/// Check if an output has per-system structure
fn is_per_system(name: &str) -> bool {
    PER_SYSTEM_OUTPUTS.contains(&name)
}

/// Check if an output should be treated as opaque (not enumerate its contents)
/// These outputs are shown as {"type":"unknown"} by nix flake show
fn is_opaque_output(name: &str) -> bool {
    matches!(
        name,
        "lib"
            | "library"
            | "flakeModule"
            | "flakeModules"
            | "formatterModule"
            // Community/unofficial outputs - nix treats as unknown
            | "homeManagerModule"
            | "homeManagerModules"
            | "homeModules"
            | "homeConfigurations"
            | "darwinModules"
            | "darwinConfigurations"
    )
}

/// Check if a string looks like a system name
fn is_system_name(s: &str) -> bool {
    matches!(
        s,
        "x86_64-linux"
            | "aarch64-linux"
            | "x86_64-darwin"
            | "aarch64-darwin"
            | "i686-linux"
            | "armv6l-linux"
            | "armv7l-linux"
            | "riscv64-linux"
            | "powerpc64le-linux"
            | "x86_64-freebsd"
    )
}

// =============================================================================
// Unified Tree Building
// =============================================================================

/// Build the unified output tree for a flake.
/// This is the single source of truth for what gets displayed.
fn build_output_tree(
    evaluator: &mut Evaluator,
    flake: &crate::eval::NixValue,
    current_system: &str,
    all_systems: bool,
    show_legacy: bool,
) -> Result<OutputNode> {
    let mut result = BTreeMap::new();
    let outputs = evaluator.get_attr_names(flake)?;

    for output_name in outputs.iter().filter(|n| is_known_output(n)) {
        if let Some(output_value) = evaluator.get_attr(flake, output_name)? {
            if let Some(node) = build_output_node(
                evaluator,
                &output_value,
                output_name,
                current_system,
                all_systems,
                show_legacy,
                0, // depth
            )? {
                result.insert(output_name.clone(), node);
            }
        }
    }

    Ok(OutputNode::AttrSet(result))
}

/// Build a node for a specific output.
fn build_output_node(
    evaluator: &mut Evaluator,
    value: &crate::eval::NixValue,
    output_name: &str,
    current_system: &str,
    all_systems: bool,
    show_legacy: bool,
    depth: usize,
) -> Result<Option<OutputNode>> {
    // Handle legacyPackages specially - show "omitted (use --legacy to show)" unless --legacy
    if output_name == "legacyPackages" && !show_legacy {
        if evaluator.is_attrs(value)? {
            let mut sys_map = BTreeMap::new();
            let systems = evaluator.get_attr_names(value)?;
            for system in systems.iter().filter(|s| is_system_name(s)) {
                sys_map.insert(system.clone(), OutputNode::OmittedLegacy);
            }
            return Ok(Some(OutputNode::AttrSet(sys_map)));
        }
        return Ok(None);
    }

    // Opaque outputs - don't enumerate contents
    if is_opaque_output(output_name) {
        return Ok(Some(OutputNode::Opaque {
            output_category: output_name.to_string(),
            description: None,
        }));
    }

    // Check if it's an attrset
    if !evaluator.is_attrs(value)? {
        // Direct function/value - get type info
        return Ok(Some(OutputNode::Opaque {
            output_category: output_name.to_string(),
            description: None,
        }));
    }

    // Special handling for hydraJobs
    if output_name == "hydraJobs" && depth == 0 {
        return build_hydra_jobs_node(evaluator, value);
    }

    // Check if this is a derivation
    if is_derivation(evaluator, value)? {
        return Ok(Some(build_derivation_node(evaluator, value, output_name)?));
    }

    // Handle per-system outputs
    if is_per_system(output_name) && depth == 0 {
        return build_per_system_node(evaluator, value, output_name, current_system, all_systems);
    }

    // Regular attrset - enumerate children
    let mut children = BTreeMap::new();
    let attrs = evaluator.get_attr_names(value)?;

    for attr_name in &attrs {
        if let Some(attr_value) = evaluator.get_attr(value, attr_name)? {
            let child_node = build_value_node(evaluator, &attr_value, output_name)?;
            children.insert(attr_name.clone(), child_node);
        }
    }

    Ok(Some(OutputNode::AttrSet(children)))
}

/// Build a node for a per-system output (packages, devShells, etc.)
fn build_per_system_node(
    evaluator: &mut Evaluator,
    value: &crate::eval::NixValue,
    output_name: &str,
    current_system: &str,
    all_systems: bool,
) -> Result<Option<OutputNode>> {
    let mut sys_map = BTreeMap::new();
    let systems = evaluator.get_attr_names(value)?;

    for system in systems.iter().filter(|s| is_system_name(s)) {
        if let Some(sys_value) = evaluator.get_attr(value, system)? {
            // Check if the system value is directly a derivation (like formatter.<system>)
            if is_derivation(evaluator, &sys_value)? {
                if !all_systems && system != current_system {
                    sys_map.insert(system.clone(), OutputNode::Omitted);
                } else {
                    let node = build_derivation_node(evaluator, &sys_value, output_name)?;
                    sys_map.insert(system.clone(), node);
                }
            } else if evaluator.is_attrs(&sys_value)? {
                // Attrset of derivations/values
                let attrs = evaluator.get_attr_names(&sys_value)?;

                // Skip empty systems
                if attrs.is_empty() {
                    continue;
                }

                let mut attr_map = BTreeMap::new();
                let is_cheap_output = output_name == "apps" || output_name == "defaultApp";

                for attr_name in &attrs {
                    if !all_systems && system != current_system && !is_cheap_output {
                        attr_map.insert(attr_name.clone(), OutputNode::Omitted);
                    } else if let Some(attr_value) = evaluator.get_attr(&sys_value, attr_name)? {
                        let node = build_value_node(evaluator, &attr_value, output_name)?;
                        attr_map.insert(attr_name.clone(), node);
                    }
                }

                sys_map.insert(system.clone(), OutputNode::AttrSet(attr_map));
            }
        }
    }

    if sys_map.is_empty() {
        Ok(None)
    } else {
        Ok(Some(OutputNode::AttrSet(sys_map)))
    }
}

/// Build a node for hydraJobs (special nested structure)
/// hydraJobs can have various structures:
/// - hydraJobs.<job>.<system> = derivation (most common)
/// - hydraJobs.<job>.<system>.<crossTarget> = derivation (cross-compilation)
/// - hydraJobs.<job>.<system>.<subJob> = derivation (nested jobs)
fn build_hydra_jobs_node(
    evaluator: &mut Evaluator,
    hydra_jobs: &crate::eval::NixValue,
) -> Result<Option<OutputNode>> {
    let mut result = BTreeMap::new();
    let job_names = evaluator.get_attr_names(hydra_jobs)?;

    for job_name in &job_names {
        if let Some(job_value) = evaluator.get_attr(hydra_jobs, job_name)? {
            if let Some(node) = build_hydra_job_level(evaluator, &job_value)? {
                result.insert(job_name.clone(), node);
            }
        }
    }

    if result.is_empty() {
        Ok(None)
    } else {
        Ok(Some(OutputNode::AttrSet(result)))
    }
}

/// Recursively build nodes for hydraJobs at any nesting level.
/// At each level, check if we have a derivation; if not, recurse into children.
fn build_hydra_job_level(
    evaluator: &mut Evaluator,
    value: &crate::eval::NixValue,
) -> Result<Option<OutputNode>> {
    // Check if this is directly a derivation
    if is_derivation(evaluator, value)? {
        return Ok(Some(build_derivation_node(evaluator, value, "hydraJobs")?));
    }

    // Not a derivation - check if it's an attrset
    if !evaluator.is_attrs(value)? {
        return Ok(None);
    }

    // Recurse into children
    let mut children = BTreeMap::new();
    let attr_names = evaluator.get_attr_names(value)?;

    // Empty attrset - still include it (nix shows as {})
    if attr_names.is_empty() {
        return Ok(Some(OutputNode::AttrSet(children)));
    }

    for attr_name in &attr_names {
        if let Some(attr_value) = evaluator.get_attr(value, attr_name)? {
            if let Some(node) = build_hydra_job_level(evaluator, &attr_value)? {
                children.insert(attr_name.clone(), node);
            }
        }
    }

    if children.is_empty() {
        Ok(None)
    } else {
        Ok(Some(OutputNode::AttrSet(children)))
    }
}

/// Build a node for any value (derivation or other)
fn build_value_node(
    evaluator: &mut Evaluator,
    value: &crate::eval::NixValue,
    output_category: &str,
) -> Result<OutputNode> {
    if !evaluator.is_attrs(value)? {
        return Ok(OutputNode::Opaque {
            output_category: output_category.to_string(),
            description: None,
        });
    }

    if is_derivation(evaluator, value)? {
        return build_derivation_node(evaluator, value, output_category);
    }

    // Check for app (type = "app")
    if is_app(evaluator, value)? {
        return build_app_node(evaluator, value);
    }

    // Non-derivation attrset - use context-aware type
    // For templates, try to extract description
    if output_category == "templates" {
        if let Some(desc_val) = evaluator.get_attr(value, "description")? {
            if let Ok(desc) = evaluator.require_string(&desc_val) {
                return Ok(OutputNode::Opaque {
                    output_category: output_category.to_string(),
                    description: Some(desc),
                });
            }
        }
    }

    Ok(OutputNode::Opaque {
        output_category: output_category.to_string(),
        description: None,
    })
}

/// Check if a value is a derivation
fn is_derivation(evaluator: &mut Evaluator, value: &crate::eval::NixValue) -> Result<bool> {
    if !evaluator.is_attrs(value)? {
        return Ok(false);
    }
    if let Some(type_val) = evaluator.get_attr(value, "type")? {
        Ok(evaluator.require_string(&type_val).ok() == Some("derivation".to_string()))
    } else {
        Ok(false)
    }
}

/// Check if a value is an app (type = "app")
fn is_app(evaluator: &mut Evaluator, value: &crate::eval::NixValue) -> Result<bool> {
    if !evaluator.is_attrs(value)? {
        return Ok(false);
    }
    if let Some(type_val) = evaluator.get_attr(value, "type")? {
        Ok(evaluator.require_string(&type_val).ok() == Some("app".to_string()))
    } else {
        Ok(false)
    }
}

/// Build an app node
fn build_app_node(
    evaluator: &mut Evaluator,
    value: &crate::eval::NixValue,
) -> Result<OutputNode> {
    // Try to get program to extract the name
    let name = if let Some(prog_val) = evaluator.get_attr(value, "program")? {
        if let Ok(prog) = evaluator.require_string(&prog_val) {
            // Extract name from program path (e.g., /nix/store/xxx-hello-2.12/bin/hello -> hello)
            std::path::Path::new(&prog)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Try to get description from meta
    let description = if let Some(meta) = evaluator.get_attr(value, "meta")? {
        if let Some(desc) = evaluator.get_attr(&meta, "description")? {
            evaluator.require_string(&desc).ok()
        } else {
            None
        }
    } else {
        None
    };

    Ok(OutputNode::App { name, description })
}

/// Build a derivation node with name and description
fn build_derivation_node(
    evaluator: &mut Evaluator,
    value: &crate::eval::NixValue,
    output_category: &str,
) -> Result<OutputNode> {
    let name = if let Some(name_val) = evaluator.get_attr(value, "name")? {
        evaluator.require_string(&name_val).unwrap_or_default()
    } else {
        String::new()
    };

    let description = if let Some(meta) = evaluator.get_attr(value, "meta")? {
        if let Some(desc) = evaluator.get_attr(&meta, "description")? {
            evaluator.require_string(&desc).unwrap_or_default()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let category = get_category_label(output_category).to_string();

    Ok(OutputNode::Derivation {
        name,
        description,
        category,
    })
}

/// Get the type name for JSON output (matches nix flake show --json)
fn get_type_for_category(category: &str) -> &'static str {
    match category {
        "overlay" | "overlays" => "nixpkgs-overlay",
        "nixosModule" | "nixosModules" => "nixos-module",
        "nixosConfigurations" => "nixos-configuration",
        // Note: darwinModules, darwinConfigurations, homeManagerModules,
        // homeModules, homeConfigurations are NOT official flake outputs - nix treats them
        // as "unknown". We only give types to official outputs.
        "templates" => "template",
        "apps" | "defaultApp" => "app",
        _ => "unknown",
    }
}

/// Get human-readable type name for tree display
fn get_display_type_for_category(category: &str) -> &'static str {
    match category {
        "overlay" | "overlays" => "Nixpkgs overlay",
        "nixosModule" | "nixosModules" => "NixOS module",
        "nixosConfigurations" => "NixOS configuration",
        "darwinModule" | "darwinModules" => "nix-darwin module",
        "darwinConfigurations" => "nix-darwin configuration",
        "homeModule" | "homeModules" | "homeManagerModules" | "homeManagerModule" => {
            "Home Manager module"
        }
        "homeConfigurations" => "Home Manager configuration",
        "templates" => "template",
        "apps" | "defaultApp" => "app",
        _ => "unknown",
    }
}

/// Get human-readable category label for tree display
fn get_category_label(category: &str) -> &'static str {
    match category {
        "packages" | "legacyPackages" => "package",
        "devShells" => "development environment",
        "apps" | "defaultApp" => "app",
        "checks" => "check",
        "formatter" => "formatter",
        "hydraJobs" => "derivation",
        _ => "derivation",
    }
}

// =============================================================================
// JSON Rendering
// =============================================================================

/// Render the output tree as JSON
fn render_as_json(node: &OutputNode) -> serde_json::Value {
    match node {
        OutputNode::Derivation {
            name,
            description,
            category: _,
        } => {
            json!({
                "type": "derivation",
                "name": name,
                "description": description
            })
        }
        OutputNode::App {
            name: _,
            description,
        } => {
            // nix flake show --json outputs {"type": "app", "description": "..."} if description exists
            if let Some(desc) = description {
                json!({ "type": "app", "description": desc })
            } else {
                json!({ "type": "app" })
            }
        }
        OutputNode::Opaque {
            output_category,
            description,
        } => {
            let type_name = get_type_for_category(output_category);
            if let Some(desc) = description {
                json!({ "type": type_name, "description": desc })
            } else {
                json!({ "type": type_name })
            }
        }
        OutputNode::Omitted | OutputNode::OmittedLegacy => {
            json!({})
        }
        OutputNode::AttrSet(children) => {
            let mut map = serde_json::Map::new();
            for (name, child) in children {
                map.insert(name.clone(), render_as_json(child));
            }
            serde_json::Value::Object(map)
        }
    }
}

// =============================================================================
// Tree Rendering
// =============================================================================

/// Render the output tree as a human-readable tree
fn render_as_tree(node: &OutputNode, prefix: &str, is_root: bool) {
    if let OutputNode::AttrSet(children) = node {
        let entries: Vec<_> = children.iter().collect();
        let last_idx = entries.len().saturating_sub(1);

        for (idx, (name, child)) in entries.iter().enumerate() {
            let is_last = idx == last_idx;
            render_tree_node(name, child, prefix, is_last, is_root);
        }
    }
}

/// Render a single tree node
fn render_tree_node(name: &str, node: &OutputNode, prefix: &str, is_last: bool, is_root: bool) {
    let connector = if is_root {
        if is_last { "└───" } else { "├───" }
    } else {
        if is_last { "└───" } else { "├───" }
    };
    let child_prefix = if is_last { "    " } else { "│   " };

    match node {
        OutputNode::Derivation {
            name: drv_name,
            description: _,
            category,
        } => {
            let type_info = if drv_name.is_empty() {
                format!(": {}", category)
            } else {
                format!(": {} '{}'", category, drv_name)
            };
            println!(
                "{}{}{}{}",
                prefix,
                connector.if_supports_color(Stdout, |t| t.green()),
                name.if_supports_color(Stdout, |t| t.bold()),
                type_info
            );
        }
        OutputNode::App {
            name: _app_name,
            description,
        } => {
            // Match nix format: "app: description" or "app: no description"
            let desc_text = description
                .as_ref()
                .map(|d| d.as_str())
                .unwrap_or("no description");
            println!(
                "{}{}{}: app: {}",
                prefix,
                connector.if_supports_color(Stdout, |t| t.green()),
                name.if_supports_color(Stdout, |t| t.bold()),
                desc_text.if_supports_color(Stdout, |t| t.bold())
            );
        }
        OutputNode::Opaque {
            output_category,
            description,
        } => {
            let display_type = get_display_type_for_category(output_category);
            if let Some(desc) = description {
                // For templates: "template: Description"
                println!(
                    "{}{}{}: {}: {}",
                    prefix,
                    connector.if_supports_color(Stdout, |t| t.green()),
                    name.if_supports_color(Stdout, |t| t.bold()),
                    display_type,
                    desc.if_supports_color(Stdout, |t| t.bold())
                );
            } else {
                // Show the type name for opaque outputs
                println!(
                    "{}{}{}: {}",
                    prefix,
                    connector.if_supports_color(Stdout, |t| t.green()),
                    name.if_supports_color(Stdout, |t| t.bold()),
                    display_type.if_supports_color(Stdout, |t| t.magenta())
                );
            }
        }
        OutputNode::Omitted => {
            println!(
                "{}{}{} {}",
                prefix,
                connector.if_supports_color(Stdout, |t| t.green()),
                name.if_supports_color(Stdout, |t| t.bold()),
                "omitted (use '--all-systems' to show)"
                    .if_supports_color(Stdout, |t| t.magenta())
            );
        }
        OutputNode::OmittedLegacy => {
            println!(
                "{}{}{} {}",
                prefix,
                connector.if_supports_color(Stdout, |t| t.green()),
                name.if_supports_color(Stdout, |t| t.bold()),
                "omitted (use '--legacy' to show)"
                    .if_supports_color(Stdout, |t| t.magenta())
            );
        }
        OutputNode::AttrSet(children) => {
            println!(
                "{}{}{}",
                prefix,
                connector.if_supports_color(Stdout, |t| t.green()),
                name.if_supports_color(Stdout, |t| t.bold())
            );

            let new_prefix = format!("{}{}", prefix, child_prefix);
            let entries: Vec<_> = children.iter().collect();
            let last_idx = entries.len().saturating_sub(1);

            for (idx, (child_name, child_node)) in entries.iter().enumerate() {
                let child_is_last = idx == last_idx;
                render_tree_node(child_name, child_node, &new_prefix, child_is_last, false);
            }
        }
    }
}

