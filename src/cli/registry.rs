//! Registry subcommands.

use anyhow::Result;

use crate::registry::{
    add_registry_entry, list_all_registries, registry_entry_to_flake_ref, remove_registry_entry,
};

/// List all registry entries
pub fn cmd_list(no_global: bool) -> Result<()> {
    let entries = list_all_registries(!no_global);

    if entries.is_empty() {
        println!("No registry entries found.");
        return Ok(());
    }

    // Group by source
    let mut by_source: std::collections::HashMap<
        &str,
        Vec<(&str, &crate::registry::RegistryEntry)>,
    > = std::collections::HashMap::new();

    for (name, source, entry) in &entries {
        by_source
            .entry(source.as_str())
            .or_default()
            .push((name.as_str(), entry));
    }

    for source in ["user", "system", "global"] {
        if let Some(entries) = by_source.get(source) {
            if !entries.is_empty() {
                println!("\n{} registry:", source.to_uppercase());
                let mut entries: Vec<_> = entries.clone();
                entries.sort_by_key(|(name, _)| *name);
                for (name, entry) in entries {
                    let flake_ref = registry_entry_to_flake_ref(entry);
                    if entry.entry_type == "path" {
                        println!("  {} -> {} (local)", name, flake_ref);
                    } else {
                        println!("  {} -> {}", name, flake_ref);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Add or update a registry entry
pub fn cmd_add(name: &str, target: &str) -> Result<()> {
    add_registry_entry(name, target)?;

    // Show what was added
    if let Some(entry) = crate::registry::resolve_registry_name(name, false) {
        let flake_ref = registry_entry_to_flake_ref(&entry);
        if entry.entry_type == "path" {
            println!(
                "Added: {} -> {} (local, handled natively by trix)",
                name, flake_ref
            );
        } else {
            println!(
                "Added: {} -> {} (remote, passthrough to nix)",
                name, flake_ref
            );
        }
    }

    Ok(())
}

/// Remove a registry entry
pub fn cmd_remove(name: &str) -> Result<()> {
    if remove_registry_entry(name)? {
        println!("Removed: {}", name);
    } else {
        anyhow::bail!("Entry '{}' not found in user registry.", name);
    }

    Ok(())
}
