//! trix - trick yourself into flakes
//!
//! Provides a flake-like experience using legacy nix commands (nix-build, nix-shell, nix-instantiate)
//! without requiring the experimental flakes feature to be enabled.

use anyhow::{Context, Result};
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
use cli::eval::EvalCommandOptions;

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
    Build {
        /// Installable reference (e.g., '.#hello', 'nixpkgs#cowsay')
        #[arg(default_value = ".#default")]
        installable: String,

        /// Name for result symlink
        #[arg(short, long, default_value = "result")]
        out_link: String,

        /// Do not create a result symlink
        #[arg(long)]
        no_link: bool,

        /// Build from a Nix file instead of flake.nix
        #[arg(short = 'f', long = "file")]
        nix_file: Option<String>,

        /// Pass --arg NAME EXPR to nix
        #[arg(long = "arg", value_names = &["NAME", "EXPR"], num_args = 2)]
        extra_args: Vec<String>,

        /// Pass --argstr NAME VALUE to nix
        #[arg(long = "argstr", value_names = &["NAME", "VALUE"], num_args = 2)]
        extra_argstrs: Vec<String>,

        /// Use specified store URL
        #[arg(long)]
        store: Option<String>,
    },

    /// Enter a development shell from flake.nix
    Develop {
        /// Installable reference (e.g., '.#default', '.#myshell')
        #[arg(default_value = ".#default")]
        installable: String,

        /// Command to run in shell
        #[arg(short, long)]
        command: Option<String>,

        /// Pass --arg NAME EXPR to nix
        #[arg(long = "arg", value_names = &["NAME", "EXPR"], num_args = 2)]
        extra_args: Vec<String>,

        /// Pass --argstr NAME VALUE to nix
        #[arg(long = "argstr", value_names = &["NAME", "VALUE"], num_args = 2)]
        extra_argstrs: Vec<String>,

        /// Use specified store URL
        #[arg(long)]
        store: Option<String>,
    },

    /// Evaluate a flake attribute or Nix expression
    Eval {
        /// Installable reference
        #[arg(default_value = ".#")]
        installable: Option<String>,

        /// Nix expression to evaluate
        #[arg(long)]
        expr: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Output raw string without quotes
        #[arg(long)]
        raw: bool,

        /// Apply function to result
        #[arg(long)]
        apply: Option<String>,

        /// Pass --arg NAME EXPR to nix
        #[arg(long = "arg", value_names = &["NAME", "EXPR"], num_args = 2)]
        extra_args: Vec<String>,

        /// Pass --argstr NAME VALUE to nix
        #[arg(long = "argstr", value_names = &["NAME", "VALUE"], num_args = 2)]
        extra_argstrs: Vec<String>,

        /// Use specified store URL
        #[arg(long)]
        store: Option<String>,
    },

    /// Build and run a package or app from flake.nix
    Run {
        /// Installable reference (e.g., '.#hello', 'nixpkgs#cowsay')
        #[arg(default_value = ".#default")]
        installable: String,

        /// Arguments to pass to the program
        #[arg(last = true)]
        args: Vec<String>,

        /// Pass --arg NAME EXPR to nix
        #[arg(long = "arg", value_names = &["NAME", "EXPR"], num_args = 2)]
        extra_args: Vec<String>,

        /// Pass --argstr NAME VALUE to nix
        #[arg(long = "argstr", value_names = &["NAME", "VALUE"], num_args = 2)]
        extra_argstrs: Vec<String>,

        /// Use specified store URL
        #[arg(long)]
        store: Option<String>,
    },

    /// Copy a package to another store
    Copy {
        /// Installable reference
        #[arg(default_value = ".#default")]
        installable: String,

        /// Destination store URL
        #[arg(long)]
        to: String,

        /// Don't check signatures
        #[arg(long)]
        no_check_sigs: bool,
    },

    /// Show build log for a package
    Log {
        /// Installable reference
        #[arg(default_value = ".#default")]
        installable: String,
    },

    /// Start an interactive Nix REPL
    Repl {
        /// Flake reference to load
        flake_ref: Option<String>,
    },

    /// Show why a package depends on another
    WhyDepends {
        /// Package to check
        package: String,
        /// Dependency to trace
        dependency: String,
    },

    /// Start a shell with specified packages available
    Shell {
        /// Installable references
        #[arg(required = true)]
        installables: Vec<String>,

        /// Command to run in shell
        #[arg(short, long)]
        command: Option<String>,
    },

    /// Manage flake inputs and outputs
    #[command(subcommand)]
    Flake(FlakeCommands),

    /// Manage installed packages (nix profile compatible)
    #[command(subcommand)]
    Profile(ProfileCommands),

    /// Manage flake registries
    #[command(subcommand)]
    Registry(RegistryCommands),

    /// Format files using the flake's formatter
    #[command(name = "fmt")]
    Fmt {
        /// Installable reference (e.g., '.')
        #[arg(default_value = ".")]
        installable: String,

        /// Files to format
        #[arg(last = true)]
        args: Vec<String>,

        /// Use specified store URL
        #[arg(long)]
        store: Option<String>,
    },

    /// Generate shell completion script
    Completion {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Subcommand)]
enum FlakeCommands {
    /// Show flake outputs structure
    Show {
        /// Flake reference
        #[arg(default_value = ".")]
        flake_ref: Option<String>,

        /// Show outputs for all systems
        #[arg(long)]
        all_systems: bool,

        /// Show legacy packages
        #[arg(long)]
        legacy: bool,
    },

    /// Show flake metadata and inputs
    Metadata {
        /// Flake reference
        #[arg(default_value = ".")]
        flake_ref: Option<String>,
    },

    /// Update flake.lock to latest versions
    Update {
        /// Specific input to update
        input_name: Option<String>,
        /// Override a specific input to a flake reference (can be repeated)
        #[arg(long = "override-input", value_names = ["INPUT", "FLAKE_REF"], num_args = 2, action = clap::ArgAction::Append)]
        override_input: Vec<String>,
    },

    /// Create or update flake.lock without building
    Lock {
        /// Flake reference
        #[arg(default_value = ".")]
        flake_ref: Option<String>,
    },

    /// Run flake checks
    Check {
        /// Flake reference
        #[arg(default_value = ".")]
        flake_ref: Option<String>,
    },

    /// Create a flake in the current directory from a template
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

#[derive(Subcommand)]
enum ProfileCommands {
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

#[derive(Subcommand)]
enum RegistryCommands {
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
        Commands::Build {
            installable,
            out_link,
            no_link,
            nix_file,
            extra_args,
            extra_argstrs,
            store,
        } => {
            // If -f is specified, bypass flake machinery entirely
            if let Some(ref file) = nix_file {
                return cmd_build_file(
                    file,
                    &installable,
                    if no_link { None } else { Some(&out_link) },
                    parse_arg_pairs(&extra_args),
                    parse_arg_pairs(&extra_argstrs),
                    store.as_deref(),
                );
            }

            let out_link = if no_link {
                None
            } else {
                Some(out_link.as_str())
            };
            cli::cmd_build(
                &installable,
                out_link,
                no_link,
                parse_arg_pairs(&extra_args),
                parse_arg_pairs(&extra_argstrs),
                store.as_deref(),
            )
        }

        Commands::Develop {
            installable,
            command,
            extra_args,
            extra_argstrs,
            store,
        } => cli::cmd_develop(
            &installable,
            command.as_deref(),
            parse_arg_pairs(&extra_args),
            parse_arg_pairs(&extra_argstrs),
            store.as_deref(),
        ),

        Commands::Eval {
            installable,
            expr,
            json,
            raw,
            apply,
            extra_args,
            extra_argstrs,
            store,
        } => cli::cmd_eval(EvalCommandOptions {
            installable: installable.as_deref(),
            expr: expr.as_deref(),
            output_json: json,
            raw,
            apply_fn: apply.as_deref(),
            extra_args: parse_arg_pairs(&extra_args),
            extra_argstrs: parse_arg_pairs(&extra_argstrs),
            store: store.as_deref(),
        }),

        Commands::Run {
            installable,
            args,
            extra_args,
            extra_argstrs,
            store,
        } => cli::cmd_run(
            &installable,
            &args,
            parse_arg_pairs(&extra_args),
            parse_arg_pairs(&extra_argstrs),
            store.as_deref(),
        ),

        Commands::Copy {
            installable,
            to,
            no_check_sigs,
        } => cli::cmd_copy(&installable, &to, no_check_sigs),

        Commands::Log { installable } => cli::cmd_log(&installable),

        Commands::Repl { flake_ref } => cli::cmd_repl(flake_ref.as_deref()),

        Commands::WhyDepends {
            package,
            dependency,
        } => cli::cmd_why_depends(&package, &dependency),

        Commands::Shell {
            installables,
            command,
        } => cli::cmd_shell(&installables, command.as_deref()),

        Commands::Flake(flake_cmd) => match flake_cmd {
            FlakeCommands::Show {
                flake_ref,
                all_systems,
                legacy,
            } => cli::flake::cmd_show(flake_ref.as_deref(), all_systems, legacy),

            FlakeCommands::Metadata { flake_ref } => cli::flake::cmd_metadata(flake_ref.as_deref()),

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
                cli::flake::cmd_update(input_name.as_deref(), override_ref)
            }

            FlakeCommands::Lock { flake_ref } => cli::flake::cmd_lock(flake_ref.as_deref()),

            FlakeCommands::Check { flake_ref } => {
                cli::flake::cmd_check(flake_ref.as_deref(), false)
            }

            FlakeCommands::Init { template } => cli::flake::cmd_init(&template),

            FlakeCommands::New { path, template } => cli::flake::cmd_new(&path, &template),
        },

        Commands::Profile(profile_cmd) => match profile_cmd {
            ProfileCommands::List { json } => cli::profile::cmd_list(json),

            ProfileCommands::Add { installables } | ProfileCommands::Install { installables } => {
                cli::profile::cmd_add(&installables)
            }

            ProfileCommands::Remove { names } => cli::profile::cmd_remove(&names),

            ProfileCommands::Upgrade { name } => cli::profile::cmd_upgrade(name.as_deref()),

            ProfileCommands::History => cli::profile::cmd_history(),

            ProfileCommands::Rollback => cli::profile::cmd_rollback(),

            ProfileCommands::WipeHistory {
                older_than,
                dry_run,
            } => cli::profile::cmd_wipe_history(older_than.as_deref(), dry_run),

            ProfileCommands::DiffClosures => cli::profile::cmd_diff_closures(),
        },

        Commands::Registry(registry_cmd) => match registry_cmd {
            RegistryCommands::List { no_global } => cli::registry::cmd_list(no_global),

            RegistryCommands::Add { name, target } => cli::registry::cmd_add(&name, &target),

            RegistryCommands::Remove { name } => cli::registry::cmd_remove(&name),
        },

        Commands::Fmt {
            installable,
            args,
            store,
        } => cli::cmd_fmt(&installable, &args, store.as_deref()),

        Commands::Completion { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "trix", &mut std::io::stdout());
            Ok(())
        }
    }
}

/// Build from a plain Nix file (bypasses flake machinery).
fn cmd_build_file(
    nix_file: &str,
    attr: &str,
    out_link: Option<&str>,
    extra_args: Vec<(String, String)>,
    extra_argstrs: Vec<(String, String)>,
    store: Option<&str>,
) -> Result<()> {
    let mut cmd = std::process::Command::new("nix-build");
    cmd.arg(nix_file);

    // Attribute becomes -A when using -f
    if attr != ".#default" && attr != "." && !attr.is_empty() {
        // Strip any .# prefix if present
        let attr = attr.strip_prefix(".#").unwrap_or(attr);
        if attr != "default" {
            cmd.args(["-A", attr]);
        }
    }

    for (name, expr) in &extra_args {
        cmd.args(["--arg", name, expr]);
    }

    for (name, value) in &extra_argstrs {
        cmd.args(["--argstr", name, value]);
    }

    if let Some(s) = store {
        cmd.args(["--store", s]);
    }

    match out_link {
        Some(link) => {
            cmd.args(["-o", link]);
        }
        None => {
            cmd.arg("--no-link");
        }
    }

    let args: Vec<_> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    tracing::debug!("+ nix-build {}", args.join(" "));

    // Clear inherited env and set clean env (with TMPDIR removed)
    cmd.env_clear();
    cmd.envs(nix::get_clean_env());

    let status = cmd.status().context("Failed to run nix-build")?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}
