use crate::registry::remove_registry_entry;
use anyhow::Result;

/// Remove a registry entry
pub fn cmd_remove(name: &str) -> Result<()> {
    if remove_registry_entry(name)? {
        println!("Removed: {}", name);
    } else {
        anyhow::bail!("Entry '{}' not found in user registry.", name);
    }

    Ok(())
}
