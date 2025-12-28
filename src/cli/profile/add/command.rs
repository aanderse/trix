use crate::profile::install;
use anyhow::Result;

/// Add packages to the profile
pub fn cmd_add(installables: &[String]) -> Result<()> {
    for installable in installables {
        tracing::debug!("Installing {}...", installable);

        install(installable, None, None, None)?;

        // Extract package name for display (matches Python behavior)
        let (_, _, pkg_name) = crate::profile::parse_installable_for_profile(installable);
        println!("Added {}", pkg_name);
    }

    Ok(())
}
