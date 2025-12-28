use crate::flake::{ensure_lock, resolve_attr_path, resolve_installable};
use crate::nix::{get_derivation_path, get_store_path_from_drv, get_system};
use anyhow::{Context, Result};
use clap::Args;

#[derive(Args, Clone, Debug)]
pub struct CopyArgs {
    /// Installable reference
    #[arg(default_value = ".#default")]
    pub installable: String,

    /// Destination store URL
    #[arg(long)]
    pub to: String,

    /// Don't check signatures
    #[arg(long)]
    pub no_check_sigs: bool,
}

/// Copy a package to another store
/// Copy a package to another store
pub fn cmd_copy(args: CopyArgs) -> Result<()> {
    let resolved = resolve_installable(&args.installable);

    if !resolved.is_local {
        // Passthrough to nix copy
        let flake_ref = resolved.flake_ref.as_deref().unwrap_or("");
        let full_ref = format!("{}#{}", flake_ref, resolved.attr_part);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.args(["copy", "--to", &args.to, &full_ref]);

        if args.no_check_sigs {
            cmd.arg("--no-check-sigs");
        }

        return cmd.run();
    }

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;
    let system = get_system()?;

    // Ensure lock exists
    ensure_lock(flake_dir, None)?;

    // Get attribute
    let attr = resolve_attr_path(&resolved.attr_part, "packages", &system);

    // Get derivation path
    let drv_path = get_derivation_path(flake_dir, &attr)?;
    let store_path = get_store_path_from_drv(&drv_path)?;

    // Copy to destination
    let mut cmd = crate::command::NixCommand::new("nix");
    cmd.args(["copy", "--to", &args.to, &store_path]);

    if args.no_check_sigs {
        cmd.arg("--no-check-sigs");
    }

    cmd.run()
}
