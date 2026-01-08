//! REPL command - start an interactive Nix REPL.
//!
//! For local flakes, this generates an evaluation expression that evaluates
//! the flake in-place (without copying to the Nix store) and passes it to
//! `nix repl --expr`. For remote flakes, it delegates to `nix repl` directly.

use std::collections::HashMap;
use std::env;
use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, info, instrument};

use crate::eval::generate_flake_eval_expr;
use crate::flake::resolve_installable_any;
use crate::lock::FlakeLock;

#[derive(Args)]
pub struct ReplArgs {
    /// Flake reference to load (optional)
    pub flake_ref: Option<String>,
}

#[instrument(level = "debug", skip_all)]
pub fn run(args: ReplArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;

    let mut cmd = Command::new("nix");
    cmd.arg("repl");

    if let Some(ref flake_ref) = args.flake_ref {
        // Resolve the flake reference
        let resolved = resolve_installable_any(flake_ref, &cwd);

        if resolved.is_local {
            // Local flake - generate evaluation expression to avoid store copy
            let flake_path = resolved
                .path
                .as_ref()
                .ok_or_else(|| anyhow!("local flake must have path"))?;

            let path_str = flake_path
                .to_str()
                .ok_or_else(|| anyhow!("invalid flake path"))?;

            // Load and parse the lock file
            let lock_path = flake_path.join("flake.lock");
            let lock = if lock_path.exists() {
                debug!("reading flake.lock");
                let content =
                    std::fs::read_to_string(&lock_path).context("failed to read flake.lock")?;
                let lock: FlakeLock =
                    serde_json::from_str(&content).context("failed to parse flake.lock")?;
                debug!(nodes = lock.nodes.len(), "parsed flake.lock");
                lock
            } else {
                debug!("no flake.lock found, using empty lock");
                FlakeLock {
                    nodes: HashMap::new(),
                    root: "root".to_string(),
                    version: 7,
                }
            };

            // Generate the evaluation expression for the whole flake outputs
            // We pass an empty attr_path to get the full outputs attrset
            let expr = generate_flake_eval_expr(path_str, &lock, &[])?;

            info!("starting repl for {}", flake_path.display());
            debug!("using --expr to evaluate flake in-place (no store copy)");

            cmd.args(["--expr", &expr]);
        } else {
            // Remote flake - delegate to nix repl directly
            // Remote flakes are already in the store or will be fetched
            info!("starting repl for {} (remote)", flake_ref);
            cmd.arg(flake_ref);
        }
    }

    debug!("executing: nix repl");

    // exec replaces the current process
    let err = cmd.exec();
    Err(anyhow!("failed to exec nix repl: {}", err))
}
