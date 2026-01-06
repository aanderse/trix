//! Flake new command - create a new directory with a flake from a template.

use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use clap::Args;

use super::init::run_template_copy;

#[derive(Args)]
pub struct NewArgs {
    /// Directory for the new flake
    pub path: String,

    /// Template reference (e.g., templates#trivial, github:NixOS/templates#rust)
    #[arg(short, long, default_value = "templates#trivial")]
    pub template: String,
}

pub fn run(args: NewArgs) -> Result<()> {
    let target_dir = Path::new(&args.path);

    if target_dir.exists() {
        return Err(anyhow!("directory already exists: {}", args.path));
    }

    // Create the target directory
    fs::create_dir_all(target_dir).context("failed to create directory")?;

    // Run template copy, cleaning up on failure
    match run_template_copy(target_dir, &args.template, true) {
        Ok(_) => Ok(()),
        Err(e) => {
            // Clean up on failure
            let _ = fs::remove_dir_all(target_dir);
            Err(e)
        }
    }
}
