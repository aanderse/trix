use crate::flake::{ensure_lock, resolve_installable};
use crate::nix::run_nix_repl;
use anyhow::{Context, Result};
use clap::Args;

#[derive(Args, Clone, Debug)]
pub struct ReplArgs {
    /// Flake reference to load
    pub flake_ref: Option<String>,
}

/// Start an interactive Nix REPL
/// Start an interactive Nix REPL
pub fn cmd_repl(args: ReplArgs) -> Result<()> {
    if args.flake_ref.is_none() {
        // Plain nix repl
        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.arg("repl");

        return cmd.exec();
    }

    let flake_ref = args.flake_ref.as_deref().unwrap();
    let resolved = resolve_installable(flake_ref);

    if !resolved.is_local {
        // Passthrough to nix repl
        let full_ref = resolved.flake_ref.as_deref().unwrap_or(flake_ref);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.args(["repl", full_ref]);

        return cmd.exec();
    }

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;

    // Check that flake.nix exists (matches Python behavior)
    if !flake_dir.join("flake.nix").exists() {
        anyhow::bail!("No flake.nix found in {}", flake_dir.display());
    }

    // Ensure lock exists
    ensure_lock(flake_dir, None)?;

    run_nix_repl(flake_dir)
}
