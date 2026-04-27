use anyhow::{Context, Result};

use crate::flake::{ensure_lock, ResolvedInstallable};
use crate::nix::{run_nix_build, BuildOptions, CommonOptions};
use clap::Args;

#[derive(Args, Clone, Debug)]
pub struct CommonArgs {
    /// Pass --arg NAME EXPR to nix
    #[arg(long = "arg", value_names = &["NAME", "EXPR"], num_args = 2)]
    pub extra_args: Vec<String>,

    /// Pass --argstr NAME VALUE to nix
    #[arg(long = "argstr", value_names = &["NAME", "VALUE"], num_args = 2)]
    pub extra_argstrs: Vec<String>,

    /// Use specified store URL
    #[arg(long)]
    pub store: Option<String>,

    /// Set the maximum number of build jobs to run in parallel
    #[arg(short = 'j', long, value_name = "NUMBER")]
    pub max_jobs: Option<i32>,

    /// Set number of cores to use per job
    #[arg(long, value_name = "NUMBER")]
    pub cores: Option<i32>,

    /// Keep going in case of failed builds, to the greatest extent possible
    #[arg(short, long)]
    pub keep_going: bool,

    /// Keep temporary build directory in case of build failure
    #[arg(short = 'K', long)]
    pub keep_failed: bool,

    /// Don’t echo standard output and standard error from builders
    #[arg(long)]
    pub no_build_output: bool,

    /// Decrease verbosity
    #[arg(long)]
    pub quiet: bool,
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

impl CommonArgs {
    pub fn to_common_options(self) -> CommonOptions {
        CommonOptions {
            extra_args: parse_arg_pairs(&self.extra_args),
            extra_argstrs: parse_arg_pairs(&self.extra_argstrs),
            store: self.store,
            max_jobs: self.max_jobs,
            cores: self.cores,
            keep_going: self.keep_going,
            keep_failed: self.keep_failed,
            no_build_output: self.no_build_output,
            quiet: self.quiet,
        }
    }
}

/// Build a resolved flake attribute.
///
/// This helper handles the common logic for local builds:
/// 1. Getting the flake directory
/// 2. Ensuring the lock file exists
/// 3. Running nix-build
pub fn build_resolved_attribute(
    resolved: &ResolvedInstallable,
    attr: &str,
    options: &BuildOptions,
    capture_output: bool,
) -> Result<Option<String>> {
    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;

    // Ensure lock exists
    ensure_lock(flake_dir, None)?;

    run_nix_build(flake_dir, attr, options, capture_output)
}
