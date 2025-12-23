//! Eval and repl commands.

use anyhow::{Context, Result};

use crate::flake::{ensure_lock, resolve_installable};
use crate::nix::{run_nix_eval, run_nix_repl, EvalOptions};

/// Options for eval command
pub struct EvalCommandOptions<'a> {
    pub installable: Option<&'a str>,
    pub expr: Option<&'a str>,
    pub output_json: bool,
    pub raw: bool,
    pub apply_fn: Option<&'a str>,
    pub extra_args: Vec<(String, String)>,
    pub extra_argstrs: Vec<(String, String)>,
    pub store: Option<&'a str>,
}

/// Evaluate a flake attribute or Nix expression
pub fn cmd_eval(opts: EvalCommandOptions) -> Result<()> {
    let EvalCommandOptions {
        installable,
        expr,
        output_json,
        raw,
        apply_fn,
        extra_args,
        extra_argstrs,
        store,
    } = opts;
    if let Some(expression) = expr {
        // Raw expression evaluation
        let options = EvalOptions {
            output_json,
            raw,
            apply_fn: apply_fn.map(|s| s.to_string()),
            extra_args,
            extra_argstrs,
            expr: Some(expression.to_string()),
            store: store.map(|s| s.to_string()),
            quiet: false,
        };

        let result = run_nix_eval(None, "", &options)?;
        println!("{}", result);
        return Ok(());
    }

    let installable = installable.unwrap_or(".#");
    let resolved = resolve_installable(installable);

    if !resolved.is_local {
        // Passthrough to nix eval
        let flake_ref = resolved.flake_ref.as_deref().unwrap_or("");
        let full_ref = format!("{}#{}", flake_ref, resolved.attr_part);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.args(["eval", &full_ref]);

        if output_json {
            cmd.arg("--json");
        }

        if raw {
            cmd.arg("--raw");
        }

        if let Some(f) = apply_fn {
            cmd.args(["--apply", f]);
        }

        if let Some(s) = store {
            cmd.args(["--store", s]);
        }

        return cmd.run();
    }

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;

    // Ensure lock exists
    ensure_lock(flake_dir, None)?;

    let options = EvalOptions {
        output_json,
        raw,
        apply_fn: apply_fn.map(|s| s.to_string()),
        extra_args,
        extra_argstrs,
        expr: None,
        store: store.map(|s| s.to_string()),
        quiet: false,
    };

    let result = run_nix_eval(Some(flake_dir), &resolved.attr_part, &options)?;
    println!("{}", result);

    Ok(())
}

/// Start an interactive Nix REPL
pub fn cmd_repl(flake_ref: Option<&str>) -> Result<()> {
    if flake_ref.is_none() {
        // Plain nix repl
        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.arg("repl");

        return cmd.exec();
    }

    let flake_ref = flake_ref.unwrap();
    let resolved = resolve_installable(flake_ref);

    if !resolved.is_local {
        // Passthrough to nix repl
        let full_ref = resolved.flake_ref.as_deref().unwrap_or(flake_ref);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.args(["repl", full_ref]);

        return cmd.exec();
    }

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;

    // Check that flake.nix exists (matches Python behavior)
    if !flake_dir.join("flake.nix").exists() {
        anyhow::bail!("No flake.nix found in {}", flake_dir.display());
    }

    // Ensure lock exists
    ensure_lock(flake_dir, None)?;

    run_nix_repl(flake_dir)
}
