use crate::lock::update_lock;
use anyhow::{Context, Result};

/// Update flake.lock to latest versions
pub fn cmd_update(
    input_name: Option<&str>,
    override_inputs: Option<&std::collections::HashMap<String, String>>,
) -> Result<()> {
    let flake_dir = std::env::current_dir().context("Could not get current directory")?;

    let updates = update_lock(&flake_dir, input_name, override_inputs)?;

    if let Some(updates) = updates {
        if updates.is_empty() {
            if input_name.is_some() {
                println!("Input is already up to date.");
            } else if override_inputs.map(|o| o.is_empty()).unwrap_or(true) {
                println!("All inputs are up to date.");
            }
        } else {
            println!("Updated {} input(s).", updates.len());
        }
    }

    Ok(())
}
