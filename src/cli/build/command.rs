use super::common::build_resolved_attribute;
use crate::cli::common::CommonArgs;
use crate::flake::{resolve_attr_path, resolve_installable};
use crate::nix::{apply_common_args, get_system, BuildOptions, CommonOptions};
use anyhow::Result;
use clap::Args;

enum BuildSource {
    File(String),
    Expr(String),
}

#[derive(Args, Clone, Debug)]
pub struct BuildArgs {
    /// Installable reference (e.g., '.#hello', 'nixpkgs#cowsay')
    #[arg(default_value = ".#default")]
    pub installable: String,

    /// Name for result symlink
    #[arg(short, long, default_value = "result")]
    pub out_link: String,

    /// Do not create a result symlink
    #[arg(long)]
    pub no_link: bool,

    /// Build from a Nix file instead of flake.nix
    #[arg(short = 'f', long = "file")]
    pub nix_file: Option<String>,

    #[command(flatten)]
    pub common: CommonArgs,
}

pub fn cmd_build(args: BuildArgs) -> Result<()> {
    let common = args.common.to_common_options();

    // If -f is specified, bypass flake machinery entirely
    if let Some(ref file) = args.nix_file {
        return cmd_build_legacy(
            BuildSource::File(file.clone()),
            &args.installable,
            if args.no_link {
                None
            } else {
                Some(&args.out_link)
            },
            &common,
        );
    }

    let out_link = if args.no_link {
        None
    } else {
        Some(args.out_link.as_str())
    };

    let resolved = resolve_installable(&args.installable);

    if !resolved.is_local {
        let flake_ref = resolved.flake_ref.as_deref().unwrap_or("");

        // If it looks like a flake, use nix build
        if crate::nix::check_is_flake(std::path::Path::new(flake_ref)) {
            // Passthrough to nix build
            let full_ref = format!("{}#{}", flake_ref, resolved.attr_part);

            let mut cmd = crate::command::NixCommand::new("nix");
            cmd.arg("build").arg(&full_ref);

            if args.no_link {
                cmd.arg("--no-link");
            } else if let Some(link) = out_link {
                cmd.args(["-o", link]);
            }

            apply_common_args(&mut cmd, &common);

            return cmd.run();
        } else {
            // Not a flake, try legacy build with fetchTree
            tracing::info!("Repository does not appear to be a flake, attempting legacy build...");

            // Use builtins.fetchTree with the provided URL string directly
            // This works with either github:owner/repo or https://github.com/owner/repo
            let expr = format!("import (builtins.fetchTree {:?})", flake_ref);

            return cmd_build_legacy(
                BuildSource::Expr(expr),
                &resolved.attr_part,
                out_link,
                &common,
            );
        }
    }

    let system = get_system()?;

    // Resolve attribute path
    let attr = resolve_attr_path(&resolved.attr_part, "packages", &system);

    let options = BuildOptions {
        out_link: if args.no_link {
            None
        } else {
            Some(args.out_link.clone())
        },
        common,
    };

    build_resolved_attribute(&resolved, &attr, &options, false)?;

    Ok(())
}

/// Build from a plain Nix file (bypasses flake machinery).
fn cmd_build_legacy(
    source: BuildSource,
    attr: &str,
    out_link: Option<&str>,
    common: &CommonOptions,
) -> Result<()> {
    let mut cmd = crate::command::NixCommand::new("nix-build");

    match source {
        BuildSource::File(path) => {
            cmd.arg(path);
        }
        BuildSource::Expr(expr) => {
            cmd.args(["-E", &expr]);
        }
    }

    // Attribute becomes -A when using -f or -E
    if attr != ".#default" && attr != "." && !attr.is_empty() {
        // Strip any .# prefix if present
        let attr = attr.strip_prefix(".#").unwrap_or(attr);
        if attr != "default" {
            cmd.args(["-A", attr]);
        }
    }

    apply_common_args(&mut cmd, &common);

    match out_link {
        Some(link) => {
            cmd.args(["-o", link]);
        }
        None => {
            cmd.arg("--no-link");
        }
    }

    cmd.run()
}
