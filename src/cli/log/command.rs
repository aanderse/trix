use crate::flake::{resolve_attr_path, resolve_installable};
use crate::nix::{get_derivation_path, get_system};
use anyhow::{Context, Result};
use clap::Args;

#[derive(Args, Clone, Debug)]
pub struct LogArgs {
    /// Installable reference
    #[arg(default_value = ".#default")]
    pub installable: String,
}

/// Show build log for a package
/// Show build log for a package
pub fn cmd_log(args: LogArgs) -> Result<()> {
    let resolved = resolve_installable(&args.installable);

    if !resolved.is_local {
        // Passthrough to nix log
        let flake_ref = resolved.flake_ref.as_deref().unwrap_or("");
        let full_ref = format!("{}#{}", flake_ref, resolved.attr_part);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.args(["log", &full_ref]);

        return cmd.run();
    }

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;
    let system = get_system()?;

    let attr = resolve_attr_path(&resolved.attr_part, "packages", &system);
    let drv_path = get_derivation_path(flake_dir, &attr)?;

    if let Some(log) = crate::nix::get_build_log(&drv_path) {
        print!("{}", log);
    } else {
        anyhow::bail!("No build log available for {}", drv_path);
    }

    Ok(())
}
