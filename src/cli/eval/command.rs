use crate::flake::{ensure_lock, resolve_installable};
use crate::nix::{run_nix_eval, EvalOptions};
use anyhow::{Context, Result};
use clap::Args;

#[derive(Args, Clone, Debug)]
pub struct EvalArgs {
    /// Installable reference
    #[arg(default_value = ".#")]
    pub installable: Option<String>,

    /// Nix expression to evaluate
    #[arg(long)]
    pub expr: Option<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Output raw string without quotes
    #[arg(long)]
    pub raw: bool,

    /// Apply function to result
    #[arg(long)]
    pub apply: Option<String>,

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

/// Evaluate a flake attribute or Nix expression
/// Evaluate a flake attribute or Nix expression
pub fn cmd_eval(args: EvalArgs) -> Result<()> {
    if let Some(expression) = &args.expr {
        // Raw expression evaluation
        let options = EvalOptions {
            output_json: args.json,
            raw: args.raw,
            apply_fn: args.apply.clone(),
            extra_args: parse_arg_pairs(&args.extra_args),
            extra_argstrs: parse_arg_pairs(&args.extra_argstrs),
            expr: Some(expression.clone()),
            store: args.store.clone(),
            quiet: false,
        };

        let result = run_nix_eval(None, "", &options)?;
        println!("{}", result);
        return Ok(());
    }

    let installable = args.installable.as_deref().unwrap_or(".#");
    let resolved = resolve_installable(installable);

    if !resolved.is_local {
        // Passthrough to nix eval
        let flake_ref = resolved.flake_ref.as_deref().unwrap_or("");
        let full_ref = format!("{}#{}", flake_ref, resolved.attr_part);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.args(["eval", &full_ref]);

        if args.json {
            cmd.arg("--json");
        }

        if args.raw {
            cmd.arg("--raw");
        }

        if let Some(f) = &args.apply {
            cmd.args(["--apply", f]);
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

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;

    // Ensure lock exists
    ensure_lock(flake_dir, None)?;

    let options = EvalOptions {
        output_json: args.json,
        raw: args.raw,
        apply_fn: args.apply.clone(),
        extra_args: parse_arg_pairs(&args.extra_args),
        extra_argstrs: parse_arg_pairs(&args.extra_argstrs),
        expr: None,
        store: args.store.clone(),
        quiet: false,
    };

    let result = run_nix_eval(Some(flake_dir), &resolved.attr_part, &options)?;
    println!("{}", result);

    Ok(())
}
