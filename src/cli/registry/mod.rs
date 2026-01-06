//! Registry subcommands for managing flake registries.

use anyhow::Result;
use clap::{Args, Subcommand};
use owo_colors::{OwoColorize, Stream::Stdout};

use crate::registry::{
    add_registry_entry, list_all_registries, pin_registry_entry, registry_entry_to_flake_ref,
    remove_registry_entry, resolve_registry_name,
};

#[derive(Args)]
pub struct RegistryArgs {
    #[command(subcommand)]
    command: RegistryCommands,
}

#[derive(Subcommand)]
enum RegistryCommands {
    /// List all registry entries
    List {
        /// Don't fetch the global registry
        #[arg(long)]
        no_global: bool,
    },

    /// Add or update a registry entry
    Add {
        /// Registry name
        name: String,

        /// Target flake reference
        target: String,
    },

    /// Remove a registry entry
    Remove {
        /// Registry name to remove
        name: String,
    },

    /// Pin a registry entry to a specific revision
    Pin {
        /// Registry name to pin
        name: String,

        /// Commit/revision to pin to
        #[arg(long)]
        to: Option<String>,
    },
}

pub fn run(args: RegistryArgs) -> Result<()> {
    match args.command {
        RegistryCommands::List { no_global } => cmd_list(no_global),
        RegistryCommands::Add { name, target } => cmd_add(&name, &target),
        RegistryCommands::Remove { name } => cmd_remove(&name),
        RegistryCommands::Pin { name, to } => cmd_pin(&name, to),
    }
}

/// List all registry entries
fn cmd_list(no_global: bool) -> Result<()> {
    let entries = list_all_registries(!no_global);

    if entries.is_empty() {
        println!("No registry entries found.");
        return Ok(());
    }

    // Print in nix registry list format: "source flake:name target"
    for (name, source, entry) in &entries {
        let flake_ref = registry_entry_to_flake_ref(entry);
        println!(
            "{:<6} {} {}",
            source.if_supports_color(Stdout, |t| t.dimmed()),
            format!("flake:{}", name).if_supports_color(Stdout, |t| t.bold()),
            flake_ref
        );
    }

    Ok(())
}

/// Add or update a registry entry
fn cmd_add(name: &str, target: &str) -> Result<()> {
    add_registry_entry(name, target)?;

    // Show what was added
    if let Some(entry) = resolve_registry_name(name, false) {
        let flake_ref = registry_entry_to_flake_ref(&entry);
        println!(
            "Added: {} -> {}",
            format!("flake:{}", name).if_supports_color(Stdout, |t| t.bold()),
            flake_ref
        );
    }

    Ok(())
}

/// Remove a registry entry
fn cmd_remove(name: &str) -> Result<()> {
    if remove_registry_entry(name)? {
        println!(
            "Removed: {}",
            format!("flake:{}", name).if_supports_color(Stdout, |t| t.bold())
        );
    } else {
        anyhow::bail!("Entry '{}' not found in user registry.", name);
    }

    Ok(())
}

/// Pin a registry entry to a specific revision
fn cmd_pin(name: &str, to: Option<String>) -> Result<()> {
    // If no revision specified, we'd need to fetch the current revision
    // For now, require --to
    let rev = to.ok_or_else(|| anyhow::anyhow!("--to revision is required"))?;

    pin_registry_entry(name, &rev)?;

    println!(
        "Pinned: {} to {}",
        format!("flake:{}", name).if_supports_color(Stdout, |t| t.bold()),
        rev.if_supports_color(Stdout, |t| t.cyan())
    );

    Ok(())
}
