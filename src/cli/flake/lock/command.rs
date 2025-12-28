use crate::flake::resolve_installable;
use crate::lock::sync_inputs;
use anyhow::{Context, Result};

/// Create or update flake.lock without building
pub fn cmd_lock(flake_ref: Option<&str>) -> Result<()> {
    let flake_ref = flake_ref.unwrap_or(".");
    let resolved = resolve_installable(flake_ref);

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;

    sync_inputs(flake_dir, None)?;
    println!("Wrote flake.lock");

    Ok(())
}
