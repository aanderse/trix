use crate::flake::{ensure_lock, resolve_attr_path, resolve_installable};
use crate::nix::{get_system, run_nix_shell, ShellOptions};
use anyhow::{Context, Result};
use clap::Args;

#[derive(Args, Clone, Debug)]
pub struct DevelopArgs {
    /// Installable reference (e.g., '.#default', '.#myshell')
    #[arg(default_value = ".#default")]
    pub installable: String,

    /// Command to run in shell
    #[arg(short, long)]
    pub command: Option<String>,

    /// Pass --arg NAME EXPR to nix
    #[arg(long = "arg", value_names = &["NAME", "EXPR"], num_args = 2)]
    pub extra_args: Vec<String>,

    /// Pass --argstr NAME VALUE to nix
    #[arg(long = "argstr", value_names = &["NAME", "VALUE"], num_args = 2)]
    pub extra_argstrs: Vec<String>,

    /// Use specified store URL
    #[arg(long)]
    pub store: Option<String>,
}

fn parse_arg_pairs(args: &[String]) -> Vec<(String, String)> {
    args.chunks(2)
        .filter_map(|chunk| {
            if chunk.len() == 2 {
                Some((chunk[0].clone(), chunk[1].clone()))
            } else {
                None
            }
        })
        .collect()
}

/// Enter a development shell from flake.nix
/// Enter a development shell from flake.nix
pub fn cmd_develop(args: DevelopArgs) -> Result<()> {
    let resolved = resolve_installable(&args.installable);

    if !resolved.is_local {
        // Passthrough to nix develop
        let flake_ref = resolved.flake_ref.as_deref().unwrap_or("");
        let full_ref = format!("{}#{}", flake_ref, resolved.attr_part);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.arg("develop").arg(&full_ref);

        if let Some(c) = &args.command {
            cmd.args(["--command", c]);
        }

        if let Some(s) = &args.store {
            cmd.args(["--store", s]);
        }

        for (name, expr) in parse_arg_pairs(&args.extra_args) {
            cmd.args(["--arg", &name, &expr]);
        }

        for (name, value) in parse_arg_pairs(&args.extra_argstrs) {
            cmd.args(["--argstr", &name, &value]);
        }

        return cmd.exec();
    }

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;
    let system = get_system()?;

    // Ensure lock exists
    ensure_lock(flake_dir, None)?;

    // Resolve attribute path for devShells
    let attr = resolve_attr_path(&resolved.attr_part, "devShells", &system);

    // Get nixConfig
    let nix_config = crate::flake::get_nix_config(flake_dir, true);

    let options = ShellOptions {
        command: args.command.clone(),
        extra_args: parse_arg_pairs(&args.extra_args),
        extra_argstrs: parse_arg_pairs(&args.extra_argstrs),
        store: args.store.clone(),
        bash_prompt: nix_config["bash-prompt"].as_str().map(|s| s.to_string()),
        bash_prompt_prefix: nix_config["bash-prompt-prefix"]
            .as_str()
            .map(|s| s.to_string()),
        bash_prompt_suffix: nix_config["bash-prompt-suffix"]
            .as_str()
            .map(|s| s.to_string()),
    };

    run_nix_shell(flake_dir, &attr, &options)
}
