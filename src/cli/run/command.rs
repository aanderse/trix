use super::common::build_resolved_attribute;
use crate::flake::{ensure_lock, resolve_attr_path, resolve_installable};
use crate::nix::{get_system, BuildOptions};
use anyhow::{Context, Result};
use clap::Args;

#[derive(Args, Clone, Debug)]
pub struct RunArgs {
    /// Installable reference (e.g., '.#hello', 'nixpkgs#cowsay')
    #[arg(default_value = ".#default")]
    pub installable: String,

    /// Arguments to pass to the program
    #[arg(last = true)]
    pub args: Vec<String>,

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

/// Build and run a package from flake.nix
/// Build and run a package from flake.nix
pub fn cmd_run(args: RunArgs) -> Result<()> {
    let resolved = resolve_installable(&args.installable);

    if !resolved.is_local {
        // Passthrough to nix run
        let flake_ref = resolved.flake_ref.as_deref().unwrap_or("");
        let full_ref = format!("{}#{}", flake_ref, resolved.attr_part);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.args(["run", &full_ref]);

        if let Some(s) = &args.store {
            cmd.args(["--store", s]);
        }

        if !args.args.is_empty() {
            cmd.arg("--");
            cmd.args(&args.args);
        }

        return cmd.exec();
    }

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;
    let system = get_system()?;

    // Ensure lock exists
    ensure_lock(flake_dir, None)?;

    // Try apps first, then packages
    // Empty attr_part (from ".#") defaults to "default"
    let attr_name = if resolved.attr_part.is_empty() {
        "default"
    } else {
        &resolved.attr_part
    };
    let app_attr = format!("apps.{}.{}", system, attr_name);
    let pkg_attr = resolve_attr_path(&resolved.attr_part, "packages", &system);

    // Check if it's an app
    let exe_path = if crate::nix::flake_has_attr(flake_dir, &app_attr)? {
        // It's an app - get the program path
        let options = crate::nix::EvalOptions {
            output_json: true,
            ..Default::default()
        };
        let result =
            crate::nix::run_nix_eval(Some(flake_dir), &format!("{}.program", app_attr), &options)?;
        let program: String = serde_json::from_str(&result)?;
        program
    } else {
        // It's a package - build and get the executable
        let options = BuildOptions {
            out_link: None,
            extra_args: parse_arg_pairs(&args.extra_args),
            extra_argstrs: parse_arg_pairs(&args.extra_argstrs),
            store: args.store.clone(),
        };

        let store_path = build_resolved_attribute(&resolved, &pkg_attr, &options, true)?
            .context("Build failed")?;

        // Get the main program name from meta.mainProgram, pname, or name
        let main_program = crate::nix::get_package_main_program(flake_dir, &pkg_attr)?;
        format!("{}/bin/{}", store_path, main_program)
    };

    // Run the executable
    let mut cmd = std::process::Command::new(&exe_path);
    cmd.args(&args.args);

    tracing::debug!("+ {} {}", exe_path, args.args.join(" "));

    let status = cmd
        .status()
        .context(format!("Failed to run {}", exe_path))?;

    if !status.success() {
        // Exit silently with the same code - the application already printed its error
        std::process::exit(status.code().unwrap_or(1))
    }

    Ok(())
}
