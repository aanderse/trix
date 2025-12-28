use anyhow::Result;
use clap::Subcommand;

pub mod common;

#[path = "add/command.rs"]
pub mod add;

#[path = "diff_closures/command.rs"]
pub mod diff_closures;

#[path = "history/command.rs"]
pub mod history;

#[path = "list/command.rs"]
pub mod list;

#[path = "remove/command.rs"]
pub mod remove;

#[path = "rollback/command.rs"]
pub mod rollback;

#[path = "upgrade/command.rs"]
pub mod upgrade;

#[path = "wipe_history/command.rs"]
pub mod wipe_history;

pub use add::cmd_add;
pub use diff_closures::cmd_diff_closures;
pub use history::cmd_history;
pub use list::cmd_list;
pub use remove::cmd_remove;
pub use rollback::cmd_rollback;
pub use upgrade::cmd_upgrade;
pub use wipe_history::cmd_wipe_history;

#[derive(Subcommand, Clone, Debug)]
pub enum ProfileCommands {
    /// List installed packages
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Add packages to the profile
    Add {
        /// Installable references
        #[arg(required = true)]
        installables: Vec<String>,
    },

    /// Alias for 'add'
    Install {
        /// Installable references
        #[arg(required = true)]
        installables: Vec<String>,
    },

    /// Remove packages from the profile
    Remove {
        /// Package names to remove
        #[arg(required = true)]
        names: Vec<String>,
    },

    /// Upgrade local packages in the profile
    Upgrade {
        /// Specific package to upgrade
        name: Option<String>,
    },

    /// Show profile generation history
    History,

    /// Roll back to the previous profile generation
    Rollback,

    /// Delete non-current versions of the profile
    WipeHistory {
        /// Only delete versions older than this (e.g., 30d)
        #[arg(long)]
        older_than: Option<String>,

        /// Show what would be deleted without actually deleting
        #[arg(long)]
        dry_run: bool,
    },

    /// Show closure difference between profile versions
    DiffClosures,
}

pub fn cmd_profile(cmd: ProfileCommands) -> Result<()> {
    match cmd {
        ProfileCommands::List { json } => cmd_list(json),

        ProfileCommands::Add { installables } | ProfileCommands::Install { installables } => {
            cmd_add(&installables)
        }

        ProfileCommands::Remove { names } => cmd_remove(&names),

        ProfileCommands::Upgrade { name } => cmd_upgrade(name.as_deref()),

        ProfileCommands::History => cmd_history(),

        ProfileCommands::Rollback => cmd_rollback(),

        ProfileCommands::WipeHistory {
            older_than,
            dry_run,
        } => cmd_wipe_history(older_than.as_deref(), dry_run),

        ProfileCommands::DiffClosures => cmd_diff_closures(),
    }
}
