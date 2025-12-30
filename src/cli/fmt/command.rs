use super::common::build_resolved_attribute;
use crate::flake::resolve_installable;
use crate::nix::{get_package_main_program, get_system, BuildOptions};
use anyhow::{Context, Result};
use clap::Args;

#[derive(Args, Clone, Debug)]
pub struct FmtArgs {
    /// Installable reference (e.g., '.')
    #[arg(default_value = ".")]
    pub installable: String,

    /// Files to format
    #[arg(last = true)]
    pub args: Vec<String>,

    /// Use specified store URL
    #[arg(long)]
    pub store: Option<String>,
}

pub fn cmd_fmt(args: FmtArgs) -> Result<()> {
    let resolved = resolve_installable(&args.installable);

    if !resolved.is_local {
        // Passthrough to nix fmt
        let flake_ref = resolved.flake_ref.as_deref().unwrap_or("");

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.arg("fmt");

        if !flake_ref.is_empty() {
            cmd.arg(flake_ref);
        }

        if let Some(s) = &args.store {
            cmd.args(["--store", s]);
        }

        if !args.args.is_empty() {
            cmd.arg("--");
            cmd.args(&args.args);
        }

        return cmd.exec();
    }

    let system = get_system()?;

    // Determine attribute to build
    // If attr_part is "default" (from .#default or just .), use formatter.<system>
    let attr = if resolved.attr_part == "default" || resolved.attr_part.is_empty() {
        format!("formatter.{}", system)
    } else {
        resolved.attr_part.clone()
    };

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;

    // Build the formatter
    let build_options = BuildOptions {
        out_link: None,
        store: args.store.clone(),
        ..Default::default()
    };

    let store_path = build_resolved_attribute(&resolved, &attr, &build_options, true)?
        .context("Build failed")?;

    let main_program = get_package_main_program(flake_dir, &attr)?;
    let exe_path = format!("{}/bin/{}", store_path, main_program);

    // Run the executable
    let mut cmd = std::process::Command::new(&exe_path);
    cmd.args(&args.args);

    tracing::debug!("+ {} {}", exe_path, args.args.join(" "));

    let status = cmd
        .status()
        .context(format!("Failed to run {}", exe_path))?;

    if !status.success() {
        anyhow::bail!(
            "Command failed with exit code: {}",
            status.code().unwrap_or(1)
        );
    }

    Ok(())
}
