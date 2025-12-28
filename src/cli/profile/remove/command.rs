use crate::profile::remove;
use anyhow::Result;

/// Remove packages from the profile
pub fn cmd_remove(names: &[String]) -> Result<()> {
    for name in names {
        if remove(name)? {
            println!("Removed: {}", name);
        } else {
            eprintln!("Package not found: {}", name);
        }
    }

    Ok(())
}
