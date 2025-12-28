use super::common::build_resolved_attribute;
use anyhow::{Context, Result};
use clap::Args;

#[derive(Args, Clone, Debug)]
pub struct ShellArgs {
    /// Installables references
    #[arg(required = true)]
    pub installables: Vec<String>,

    /// Command to run in shell
    #[arg(short, long)]
    pub command: Option<String>,
}

/// Start a shell with specified packages available
pub fn cmd_shell(args: ShellArgs) -> Result<()> {
    // Check if any installables are remote
    let mut has_remote = false;
    for installable in &args.installables {
        let resolved = crate::flake::resolve_installable(installable);
        if !resolved.is_local {
            has_remote = true;
            break;
        }
    }

    if has_remote {
        // Passthrough to nix shell
        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.args(["shell"]);
        cmd.args(&args.installables);

        if let Some(c) = &args.command {
            cmd.args(["--command", c]);
        }

        return cmd.run();
    }

    // All local - use trix's native handling
    let mut store_paths = Vec::new();
    let options = crate::nix::BuildOptions {
        ..Default::default()
    };

    for installable in &args.installables {
        let resolved = crate::flake::resolve_installable(installable);
        let system = crate::nix::get_system()?;
        let attr = crate::flake::resolve_attr_path(&resolved.attr_part, "packages", &system);

        let store_path = build_resolved_attribute(
            &resolved, &attr, &options, true, // capture_output
        )?
        .context(format!("Failed to build {}", installable))?;

        store_paths.push(store_path);
    }

    // Build PATH with all package bin directories
    let mut bin_paths = Vec::new();
    for store_path in &store_paths {
        let bin_dir = std::path::Path::new(store_path).join("bin");
        if bin_dir.is_dir() {
            bin_paths.push(bin_dir);
        }
    }

    if bin_paths.is_empty() {
        anyhow::bail!("No bin directories found in packages");
    }

    // Prepend to existing PATH
    let mut env = crate::nix::get_clean_env();
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let mut new_path_parts = Vec::new();
    for p in bin_paths {
        new_path_parts.push(p.to_string_lossy().into_owned());
    }
    if !old_path.is_empty() {
        new_path_parts.push(old_path.to_string_lossy().into_owned());
    }
    let new_path = new_path_parts.join(":");
    env.insert("PATH".to_string(), new_path);

    if let Some(cmd_str) = &args.command {
        // Run command and exit
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", cmd_str]);
        cmd.env_clear();
        cmd.envs(env);

        tracing::debug!("+ sh -c {}", cmd_str);

        let status = cmd.status().context("Failed to run sh")?;
        if !status.success() {
            anyhow::bail!(
                "Command failed with exit code: {}",
                status.code().unwrap_or(1)
            );
        }
        Ok(())
    } else {
        // Start interactive shell
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

        let mut cmd = std::process::Command::new(&shell);
        cmd.env_clear();
        cmd.envs(env);

        tracing::debug!("+ {}", shell);

        let status = cmd.status().context(format!("Failed to run {}", shell))?;
        if !status.success() {
            anyhow::bail!(
                "Command failed with exit code: {}",
                status.code().unwrap_or(1)
            );
        }
        Ok(())
    }
}
