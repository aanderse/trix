use std::io;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use tracing::Level;
use tracing_subscriber::EnvFilter;

mod cli;
pub mod eval;
pub mod flake;
pub mod flake_url;
pub mod lock;
pub mod profile;
pub mod progress;
pub mod registry;
mod shebang;

#[derive(Parser)]
#[command(name = "trix")]
#[command(about = "A Nix flake CLI that never copies your project to the store")]
#[command(version)]
struct Cli {
    /// Increase verbosity (-v info, -vv debug, -vvv trace)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build a flake output
    Build(cli::build::BuildArgs),
    /// Generate shell completion scripts
    Completion {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Copy a store path to another store
    Copy(cli::copy::CopyArgs),
    /// Enter a development shell
    Develop(cli::develop::DevelopArgs),
    /// Compare derivations or profile generations
    Diff(cli::diff::DiffArgs),
    /// Evaluate a Nix expression
    Eval(cli::eval::EvalArgs),
    /// Flake-related commands
    Flake(cli::flake::FlakeArgs),
    /// Format Nix files using the flake's formatter
    Fmt(cli::fmt::FmtArgs),
    /// Compute and convert cryptographic hashes
    Hash(cli::hash::HashArgs),
    /// Show build logs for a package
    Log(cli::log::LogArgs),
    /// Manage Nix profiles
    Profile(cli::profile::ProfileArgs),
    /// Manage flake registries
    Registry(cli::registry::RegistryArgs),
    /// Search for packages in a flake
    Search(cli::search::SearchArgs),
    /// Start an interactive Nix REPL
    Repl(cli::repl::ReplArgs),
    /// Build and run a package
    Run(cli::run::RunArgs),
    /// Start a shell with specified packages
    Shell(cli::shell::ShellArgs),
    /// Show why a package depends on another
    WhyDepends(cli::why_depends::WhyDependsArgs),
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Check for shebang mode before normal CLI parsing
    let (cli, shebang_info) = if let Some(shebang_script) = shebang::detect_shebang(&args) {
        // Build the argument list for shebang mode
        let mut new_args = vec![args[0].clone()];

        // Add global flags (like -v) first
        new_args.extend(shebang_script.global_args.clone());

        // Add the directive arguments
        new_args.extend(shebang_script.args.clone());

        // Add hidden arguments for script path and script arguments
        new_args.push("--script".to_string());
        new_args.push(shebang_script.script_path.clone());

        // Add script arguments (everything after the script path in original args)
        let script_args_start = shebang_script.script_index + 1;
        if args.len() > script_args_start {
            new_args.push("--script-args".to_string());
            new_args.extend(args[script_args_start..].iter().cloned());
        }

        let cli = Cli::parse_from(&new_args);
        (cli, Some(shebang_script))
    } else {
        (Cli::parse(), None)
    };

    // Initialize tracing based on verbosity level
    // -v = INFO, -vv = DEBUG, -vvv = TRACE
    // Can be overridden with TRIX_LOG or RUST_LOG env vars
    let default_level = match cli.verbose {
        0 => Level::WARN,
        1 => Level::INFO,
        2 => Level::DEBUG,
        _ => Level::TRACE,
    };

    let env_filter = EnvFilter::builder()
        .with_default_directive(default_level.into())
        .with_env_var("TRIX_LOG")
        .from_env_lossy();

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(cli.verbose >= 2) // Show target module at debug+
        .with_writer(std::io::stderr)
        .init();

    if shebang_info.is_some() {
        tracing::debug!("Running in shebang mode");
    }

    if let Err(e) = run(cli) {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Build(args) => cli::build::run(args),
        Commands::Completion { shell } => {
            generate(shell, &mut Cli::command(), "trix", &mut io::stdout());
            Ok(())
        }
        Commands::Copy(args) => cli::copy::run(args),
        Commands::Develop(args) => cli::develop::run(args),
        Commands::Diff(args) => cli::diff::run(args),
        Commands::Eval(args) => cli::eval::run(args),
        Commands::Flake(args) => cli::flake::run(args),
        Commands::Fmt(args) => cli::fmt::run(args),
        Commands::Hash(args) => cli::hash::run(args),
        Commands::Log(args) => cli::log::run(args),
        Commands::Profile(args) => cli::profile::run(args),
        Commands::Registry(args) => cli::registry::run(args),
        Commands::Search(args) => cli::search::run(args),
        Commands::Repl(args) => cli::repl::run(args),
        Commands::Run(args) => cli::run::run(args),
        Commands::Shell(args) => cli::shell::run(args),
        Commands::WhyDepends(args) => cli::why_depends::run(args),
    }
}
