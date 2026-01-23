//! Search command - search for packages in flakes.
//!
//! Delegates to `nix search` for all search operations.

use std::process::Command;

use anyhow::{Context, Result};
use clap::Args;

#[derive(Args)]
pub struct SearchArgs {
    /// Flake to search in (default: nixpkgs from registry)
    #[arg(default_value = "nixpkgs")]
    pub flake_ref: String,

    /// Regex patterns to search for (matches name or description)
    #[arg()]
    pub patterns: Vec<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Don't print descriptions
    #[arg(long)]
    pub no_desc: bool,
}

pub fn run(args: SearchArgs) -> Result<()> {
    // Delegate to nix search (search is inherently a remote/registry operation)
    let mut cmd = Command::new("nix");
    cmd.args(["--extra-experimental-features", "nix-command flakes"]);
    cmd.arg("search").arg(&args.flake_ref);

    // Add patterns
    for pattern in &args.patterns {
        cmd.arg(pattern);
    }

    // Add flags
    if args.json {
        cmd.arg("--json");
    }

    if args.no_desc {
        cmd.arg("--no-description");
    }

    let status = cmd.status().context("failed to run nix search")?;

    if !status.success() {
        anyhow::bail!("nix search failed");
    }

    Ok(())
}
