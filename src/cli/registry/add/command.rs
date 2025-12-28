use crate::registry::{add_registry_entry, registry_entry_to_flake_ref};
use anyhow::Result;

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
