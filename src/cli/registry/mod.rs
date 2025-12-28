use anyhow::Result;
use clap::Subcommand;

#[path = "add/command.rs"]
pub mod add;

#[path = "list/command.rs"]
pub mod list;

#[path = "remove/command.rs"]
pub mod remove;

pub use add::cmd_add;
pub use list::cmd_list;
pub use remove::cmd_remove;

#[derive(Subcommand, Clone, Debug)]
pub enum RegistryCommands {
    /// List all registry entries
    List {
        /// Don't fetch the global registry
        #[arg(long)]
        no_global: bool,
    },

    /// Add or update a registry entry
    Add {
        /// Registry name
        name: String,

        /// Target flake reference
        target: String,
    },

    /// Remove a registry entry
    Remove {
        /// Registry name to remove
        name: String,
    },
}

pub fn cmd_registry(cmd: RegistryCommands) -> Result<()> {
    match cmd {
        RegistryCommands::List { no_global } => cmd_list(no_global),

        RegistryCommands::Add { name, target } => cmd_add(&name, &target),

        RegistryCommands::Remove { name } => cmd_remove(&name),
    }
}
