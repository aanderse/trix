use super::common::build_resolved_attribute;
use crate::flake::{resolve_attr_path, resolve_installable};
use crate::nix::{get_system, BuildOptions};
use anyhow::Result;
use clap::Args;

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

/// Build a package from flake.nix
/// Build a package from flake.nix
pub fn cmd_build(args: BuildArgs) -> Result<()> {
    // If -f is specified, bypass flake machinery entirely
    if let Some(ref file) = args.nix_file {
        return cmd_build_file(
            file,
            &args.installable,
            if args.no_link {
                None
            } else {
                Some(&args.out_link)
            },
            parse_arg_pairs(&args.extra_args),
            parse_arg_pairs(&args.extra_argstrs),
            args.store.as_deref(),
        );
    }

    let out_link = if args.no_link {
        None
    } else {
        Some(args.out_link.as_str())
    };

    let resolved = resolve_installable(&args.installable);

    if !resolved.is_local {
        // Passthrough to nix build
        let flake_ref = resolved.flake_ref.as_deref().unwrap_or("");
        let full_ref = format!("{}#{}", flake_ref, resolved.attr_part);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.arg("build").arg(&full_ref);

        if args.no_link {
            cmd.arg("--no-link");
        } else if let Some(link) = out_link {
            cmd.args(["-o", link]);
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

        return cmd.run();
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
        extra_args: parse_arg_pairs(&args.extra_args),
        extra_argstrs: parse_arg_pairs(&args.extra_argstrs),
        store: args.store.clone(),
    };

    build_resolved_attribute(&resolved, &attr, &options, false)?;

    Ok(())
}

/// Build from a plain Nix file (bypasses flake machinery).
fn cmd_build_file(
    nix_file: &str,
    attr: &str,
    out_link: Option<&str>,
    extra_args: Vec<(String, String)>,
    extra_argstrs: Vec<(String, String)>,
    store: Option<&str>,
) -> Result<()> {
    let mut cmd = crate::command::NixCommand::new("nix-build");
    cmd.arg(nix_file);

    // Attribute becomes -A when using -f
    if attr != ".#default" && attr != "." && !attr.is_empty() {
        // Strip any .# prefix if present
        let attr = attr.strip_prefix(".#").unwrap_or(attr);
        if attr != "default" {
            cmd.args(["-A", attr]);
        }
    }

    for (name, expr) in &extra_args {
        cmd.args(["--arg", name, expr]);
    }

    for (name, value) in &extra_argstrs {
        cmd.args(["--argstr", name, value]);
    }

    if let Some(s) = store {
        cmd.args(["--store", s]);
    }

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
