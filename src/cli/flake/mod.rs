use anyhow::Result;
use clap::Subcommand;

pub mod common;

#[path = "check/command.rs"]
pub mod check;

#[path = "init/command.rs"]
pub mod init;

#[path = "lock/command.rs"]
pub mod lock;

#[path = "metadata/command.rs"]
pub mod metadata;

#[path = "new/command.rs"]
pub mod new;

#[path = "show/command.rs"]
pub mod show;

#[path = "update/command.rs"]
pub mod update;

pub use check::cmd_check;
pub use init::cmd_init;
pub use lock::cmd_lock;
pub use metadata::cmd_metadata;
pub use new::cmd_new;
pub use show::cmd_show;
pub use update::cmd_update;

#[derive(Subcommand, Clone, Debug)]
pub enum FlakeCommands {
    /// Show flake metadata
    Metadata {
        /// Flake reference
        #[arg(default_value = ".")]
        flake_ref: Option<String>,
    },

    /// Show flake output attributes
    Show {
        /// Flake reference
        #[arg(default_value = ".")]
        flake_ref: Option<String>,

        /// Show all systems
        #[arg(long)]
        all_systems: bool,

        /// Use legacy nix command behavior if true
        #[arg(long, hide = true)]
        legacy: bool,
    },

    /// Update flake inputs
    Update {
        /// Specific input to update
        input_name: Option<String>,

        /// Override input (e.g. nixpkgs=github:NixOS/nixpkgs/nixos-unstable)
        #[arg(long, num_args = 2, value_names = ["INPUT", "REF"])]
        override_input: Vec<String>,
    },

    /// Check flake health
    Check {
        /// Flake reference
        #[arg(default_value = ".")]
        flake_ref: Option<String>,
    },

    /// Create or update flake.lock
    Lock {
        /// Flake reference
        #[arg(default_value = ".")]
        flake_ref: Option<String>,
    },

    /// Initialize a new flake in the current directory
    Init {
        /// Template reference
        #[arg(short, long, default_value = "templates#default")]
        template: String,
    },

    /// Create a new directory with a flake from a template
    New {
        /// Directory for the new flake
        path: String,
        /// Template reference
        #[arg(short, long, default_value = "templates#default")]
        template: String,
    },
}

pub fn cmd_flake(cmd: FlakeCommands) -> Result<()> {
    match cmd {
        FlakeCommands::Show {
            flake_ref,
            all_systems,
            legacy,
        } => cmd_show(flake_ref.as_deref(), all_systems, legacy),

        FlakeCommands::Metadata { flake_ref } => cmd_metadata(flake_ref.as_deref()),

        FlakeCommands::Update {
            input_name,
            override_input,
        } => {
            let override_inputs: std::collections::HashMap<String, String> = override_input
                .chunks(2)
                .filter_map(|chunk| {
                    if chunk.len() == 2 {
                        Some((chunk[0].clone(), chunk[1].clone()))
                    } else {
                        None
                    }
                })
                .collect();
            let override_ref = if override_inputs.is_empty() {
                None
            } else {
                Some(&override_inputs)
            };
            cmd_update(input_name.as_deref(), override_ref)
        }

        FlakeCommands::Lock { flake_ref } => cmd_lock(flake_ref.as_deref()),

        FlakeCommands::Check { flake_ref } => cmd_check(flake_ref.as_deref(), false),

        FlakeCommands::Init { template } => cmd_init(&template),

        FlakeCommands::New { path, template } => cmd_new(&path, &template),
    }
}
