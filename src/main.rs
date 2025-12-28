//! trix - trick yourself into flakes
//!
//! Provides a flake-like experience using legacy nix commands (nix-build, nix-shell, nix-instantiate)
//! without requiring the experimental flakes feature to be enabled.

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};

mod cli;
mod command;
mod flake;
mod git;
mod lock;
mod nix;
mod profile;
mod registry;

/// trix - trick yourself into flakes
#[derive(Parser)]
#[command(name = "trix")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build a package from flake.nix or a Nix file
    /// Build a package from flake.nix or a Nix file
    Build(cli::build::BuildArgs),

    /// Enter a development shell from flake.nix
    Develop(cli::develop::DevelopArgs),

    /// Evaluate a flake attribute or Nix expression
    Eval(cli::eval::EvalArgs),

    /// Build and run a package or app from flake.nix
    /// Build and run a package or app from flake.nix
    Run(cli::run::RunArgs),

    /// Copy a package to another store
    Copy(cli::copy::CopyArgs),

    /// Show build log for a package
    Log(cli::log::LogArgs),

    /// Start an interactive Nix REPL
    Repl(cli::repl::ReplArgs),

    /// Show why a package depends on another
    WhyDepends(cli::why_depends::WhyDependsArgs),

    /// Start a shell with specified packages available
    Shell(cli::shell::ShellArgs),

    /// Manage flake inputs and outputs
    #[command(subcommand)]
    Flake(cli::flake::FlakeCommands),

    /// Manage Nix profiles
    #[command(subcommand)]
    Profile(cli::profile::ProfileCommands),

    /// Manage flake registries
    #[command(subcommand)]
    Registry(cli::registry::RegistryCommands),

    /// Compute and convert cryptographic hashes
    #[command(subcommand)]
    Hash(cli::hash::HashCommands),

    /// Format files using the flake's formatter
    #[command(name = "fmt")]
    Fmt(cli::fmt::FmtArgs),

    /// Generate shell completion script
    Completion {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
}

fn main() {
    let cli = Cli::parse();

    // Initialize tracing
    // Default to INFO unless verbose is set (then DEBUG), or RUST_LOG overrides it.
    let default_level = if cli.verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(default_level.into())
                .from_env_lossy(),
        )
        .with_target(false) // cleaner output for simple CLI tools
        .with_writer(std::io::stderr)
        .init();

    if let Err(e) = run(cli) {
        tracing::error!("Error: {:#}", e); // Use {:#} for alternate view (causal chain)
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Build(args) => cli::cmd_build(args),

        Commands::Develop(args) => cli::cmd_develop(args),

        Commands::Eval(args) => cli::cmd_eval(args),

        Commands::Run(args) => cli::cmd_run(args),

        Commands::Copy(args) => cli::cmd_copy(args),

        Commands::Log(args) => cli::cmd_log(args),

        Commands::Repl(args) => cli::cmd_repl(args),

        Commands::WhyDepends(args) => cli::cmd_why_depends(args),

        Commands::Shell(args) => cli::cmd_shell(args),

        Commands::Flake(flake_cmd) => cli::flake::cmd_flake(flake_cmd),

        Commands::Profile(profile_cmd) => cli::profile::cmd_profile(profile_cmd),

        Commands::Registry(registry_cmd) => cli::registry::cmd_registry(registry_cmd),

        Commands::Hash(hash_cmd) => cli::hash::cmd_hash(hash_cmd),

        Commands::Fmt(args) => cli::cmd_fmt(args),

        Commands::Completion { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "trix", &mut std::io::stdout());
            Ok(())
        }
    }
}
