//! REPL command - start an interactive Nix REPL.
//!
//! Since there's no Rust bindings for the Nix REPL, this command
//! execs nix repl with appropriate arguments.

use std::env;
use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, instrument};

use crate::flake::resolve_installable;

#[derive(Args)]
pub struct ReplArgs {
    /// Flake reference to load (optional)
    pub flake_ref: Option<String>,
}

#[instrument(level = "debug", skip_all)]
pub fn run(args: ReplArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;

    let mut cmd = Command::new("nix");
    cmd.arg("repl");

    if let Some(ref flake_ref) = args.flake_ref {
        // Resolve the flake reference
        let resolved = resolve_installable(flake_ref, &cwd)?;

        // Pass the resolved path to nix repl
        let flake_url = format!("path:{}", resolved.path.display());
        debug!("loading flake: {}", flake_url);

        cmd.arg(&flake_url);
    }

    debug!("executing: nix repl {:?}", args.flake_ref);

    // exec replaces the current process
    let err = cmd.exec();
    Err(anyhow!("failed to exec nix repl: {}", err))
}
