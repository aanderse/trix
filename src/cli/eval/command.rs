use crate::cli::common::CommonArgs;
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

    #[command(flatten)]
    pub common: CommonArgs,
}

/// Evaluate a flake attribute or Nix expression
/// Evaluate a flake attribute or Nix expression
pub fn cmd_eval(args: EvalArgs) -> Result<()> {
    let common = args.common.to_common_options();

    if let Some(expression) = &args.expr {
        // Raw expression evaluation
        let options = EvalOptions {
            output_json: args.json,
            raw: args.raw,
            apply_fn: args.apply.clone(),
            common,
            expr: Some(expression.clone()),
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

        if let Some(s) = &common.store {
            cmd.args(["--store", s]);
        }

        for (name, expr) in &common.extra_args {
            cmd.args(["--arg", &name, &expr]);
        }

        for (name, value) in &common.extra_argstrs {
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
        expr: None,
        common,
    };

    let result = run_nix_eval(Some(flake_dir), &resolved.attr_part, &options)?;
    println!("{}", result);

    Ok(())
}
