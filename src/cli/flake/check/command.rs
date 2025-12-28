use crate::flake::{ensure_lock, resolve_installable};
use crate::nix::{eval_flake_outputs, get_system};
use anyhow::{Context, Result};
use rayon::prelude::*;

/// Run flake checks
pub fn cmd_check(flake_ref: Option<&str>, all_systems: bool) -> Result<()> {
    let flake_ref = flake_ref.unwrap_or(".");
    let resolved = resolve_installable(flake_ref);

    if !resolved.is_local {
        // Passthrough to nix flake check
        let full_ref = resolved.flake_ref.as_deref().unwrap_or(flake_ref);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.args(["flake", "check", full_ref]);

        return cmd.run();
    }

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;
    let system = get_system()?;

    // Ensure lock exists
    ensure_lock(flake_dir, None)?;

    // Get checks for current system
    let checks_attr = format!("checks.{}", system);

    // Build all checks
    let outputs = eval_flake_outputs(flake_dir, all_systems, false)?;

    if let Some(ref outputs) = outputs {
        if let Some(checks) = outputs.get("checks").and_then(|c| c.get(&system)) {
            if let Some(check_names) = checks.as_object() {
                let mut passed = 0;
                let mut failed = 0;

                let names: Vec<String> = check_names.keys().cloned().collect();
                let results: Vec<(String, Result<()>)> = names
                    .into_par_iter()
                    .map(|name| {
                        let attr = format!("{}.{}", checks_attr, name);
                        let options = crate::nix::BuildOptions {
                            out_link: None,
                            ..Default::default()
                        };

                        let res = crate::nix::run_nix_build(flake_dir, &attr, &options, true);
                        (name, res.map(|_| ()))
                    })
                    .collect();

                for (name, res) in results {
                    print!("checking {}: ", name);
                    match res {
                        Ok(_) => {
                            println!("ok");
                            passed += 1;
                        }
                        Err(e) => {
                            println!("FAILED");
                            tracing::debug!("  Error: {}", e);
                            failed += 1;
                        }
                    }
                }

                println!();
                println!("{} passed, {} failed", passed, failed);

                if failed > 0 {
                    anyhow::bail!("{} test(s) failed", failed);
                }

                return Ok(());
            }
        }
    }

    println!("No checks found for {}", system);

    Ok(())
}
