use crate::profile::upgrade;
use anyhow::Result;

/// Upgrade local packages in the profile
pub fn cmd_upgrade(name: Option<&str>) -> Result<()> {
    let (upgraded, skipped) = upgrade(name)?;

    if upgraded > 0 {
        println!("Upgraded {} package(s)", upgraded);
    } else if skipped > 0 {
        println!("All {} package(s) up to date", skipped);
    } else {
        println!("No local packages to upgrade");
    }

    Ok(())
}
