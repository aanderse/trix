pub mod check;
pub mod init;
pub mod lock;
pub mod metadata;
pub mod new;
pub mod show;
pub mod update;

use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct FlakeArgs {
    #[command(subcommand)]
    pub command: FlakeCommands,
}

#[derive(Subcommand)]
pub enum FlakeCommands {
    /// Run flake checks
    Check(check::CheckArgs),
    /// Initialize a new flake in the current directory
    Init(init::InitArgs),
    /// Update and lock flake inputs
    Lock(lock::LockArgs),
    /// Show flake metadata
    Metadata(metadata::MetadataArgs),
    /// Create a new directory with a flake from a template
    New(new::NewArgs),
    /// Show flake outputs
    Show(show::ShowArgs),
    /// Update flake inputs
    Update(update::UpdateArgs),
}

pub fn run(args: FlakeArgs) -> Result<()> {
    match args.command {
        FlakeCommands::Check(args) => check::run(args),
        FlakeCommands::Init(args) => init::run(args),
        FlakeCommands::Lock(args) => lock::run(args),
        FlakeCommands::Metadata(args) => metadata::run(args),
        FlakeCommands::New(args) => new::run(args),
        FlakeCommands::Show(args) => show::run(args),
        FlakeCommands::Update(args) => update::run(args),
    }
}
