//! Flake update command - update flake inputs.
//!
//! This is a user-friendly wrapper around `flake lock --update-all` and
//! `flake lock --update INPUT`.

use std::env;

use anyhow::{Context, Result};
use clap::Args;
use tracing::{debug, instrument};

use super::lock::{run as run_lock, LockArgs};
use crate::flake::resolve_installable;

#[derive(Args)]
pub struct UpdateArgs {
    /// Flake reference (default: .)
    #[arg(default_value = ".")]
    pub flake_ref: String,

    /// Input(s) to update (updates all if not specified)
    #[arg()]
    pub inputs: Vec<String>,

    /// Override an input with a different flake reference
    /// Usage: --override-input INPUT FLAKE_REF
    #[arg(long = "override-input", num_args = 2, value_names = ["INPUT", "FLAKE_REF"], action = clap::ArgAction::Append)]
    pub override_inputs: Vec<String>,
}

#[instrument(level = "debug", skip_all, fields(flake_ref = %args.flake_ref))]
pub fn run(args: UpdateArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;

    // Verify this is a valid flake
    let resolved = resolve_installable(&args.flake_ref, &cwd)?;
    debug!(flake_path = %resolved.path.display(), "updating flake inputs");

    // Parse override inputs into pairs
    let override_inputs: Vec<(String, String)> = args
        .override_inputs
        .chunks(2)
        .filter_map(|chunk| {
            if chunk.len() == 2 {
                Some((chunk[0].clone(), chunk[1].clone()))
            } else {
                None
            }
        })
        .collect();

    // Convert to lock args
    let lock_args = if args.inputs.is_empty() {
        // No specific inputs - update all
        LockArgs {
            flake_ref: args.flake_ref,
            dry_run: false,
            update_all: true,
            update_inputs: vec![],
            override_inputs,
        }
    } else {
        // Update specific inputs
        LockArgs {
            flake_ref: args.flake_ref,
            dry_run: false,
            update_all: false,
            update_inputs: args.inputs,
            override_inputs,
        }
    };

    run_lock(lock_args)
}
