//! NixOS system management commands.
//!
//! This module provides:
//! - `trix os rebuild` - build and activate NixOS configurations
//! - `trix os repl` - interactive REPL for exploring NixOS configurations
//!
//! Both commands evaluate flakes without copying them to the store.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};
use tempfile::TempDir;
use tracing::{debug, info};

use crate::eval::generate_flake_eval_expr;
use crate::flake::find_flake_root;
use crate::lock::FlakeLock;

#[derive(Args)]
pub struct OsArgs {
    #[command(subcommand)]
    pub command: OsCommands,
}

#[derive(Subcommand)]
pub enum OsCommands {
    /// Build and/or activate a NixOS configuration
    ///
    /// This command evaluates your flake using trix (avoiding the store copy),
    /// generates a temporary Nix expression, and passes it to nixos-rebuild.
    Rebuild(RebuildArgs),

    /// Open an interactive REPL for exploring a NixOS configuration
    ///
    /// In flake mode (default), this evaluates the flake without copying it to
    /// the store. In --file mode, it passes through to nixos-rebuild repl.
    Repl(ReplArgs),
}

#[derive(Args)]
pub struct RebuildArgs {
    /// Action to perform
    #[arg(value_enum, default_value = "switch")]
    pub action: RebuildAction,

    /// Flake reference with optional hostname (e.g., .#myhost, /path/to/flake#hostname)
    /// Defaults to current directory with system hostname
    #[arg(long = "flake", short = 'f')]
    pub flake_ref: Option<String>,

    /// Prefix activation commands with sudo (for remote deployment as non-root)
    #[arg(long)]
    pub sudo: bool,

    /// Ask for sudo password for remote activation (implies --sudo)
    #[arg(long, short = 'S')]
    pub ask_sudo_password: bool,

    /// Build on a remote host
    #[arg(long)]
    pub build_host: Option<String>,

    /// Activate on a remote host
    #[arg(long)]
    pub target_host: Option<String>,

    /// Use a named system profile instead of /nix/var/nix/profiles/system
    #[arg(long = "profile-name", short = 'p')]
    pub profile_name: Option<String>,

    /// Show Nix evaluation trace on error
    #[arg(long)]
    pub show_trace: bool,

    /// Print the generated Nix expression (for debugging)
    #[arg(long)]
    pub print_expr: bool,

    /// Verbose output from nixos-rebuild
    #[arg(long = "nixos-verbose")]
    pub nixos_verbose: bool,

    /// Additional arguments to pass through to nixos-rebuild
    #[arg(last = true)]
    pub passthrough: Vec<OsString>,
}

#[derive(Args)]
pub struct ReplArgs {
    /// Flake reference with optional hostname (e.g., .#myhost, /path/to/flake#hostname)
    /// Defaults to current directory with system hostname
    #[arg(long = "flake", short = 'f', conflicts_with_all = ["file", "attr"])]
    pub flake_ref: Option<String>,

    /// Path to a NixOS configuration file (non-flake mode, passes through to nixos-rebuild)
    #[arg(long = "file", short = 'F', requires = "attr")]
    pub file: Option<String>,

    /// Attribute path in the file (non-flake mode, passes through to nixos-rebuild)
    #[arg(long = "attr", short = 'A')]
    pub attr: Option<String>,

    /// Show Nix evaluation trace on error
    #[arg(long)]
    pub show_trace: bool,

    /// Print the generated Nix expression (for debugging, flake mode only)
    #[arg(long, conflicts_with_all = ["file", "attr"])]
    pub print_expr: bool,

    /// Additional arguments to pass through to nix repl
    #[arg(last = true)]
    pub passthrough: Vec<OsString>,
}

#[derive(Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum RebuildAction {
    /// Build and activate the new configuration
    Switch,
    /// Build and set for next boot (don't activate now)
    Boot,
    /// Build and activate, but don't add to bootloader
    Test,
    /// Build only, print store path
    Build,
    /// Show what would be built without building
    DryBuild,
    /// Build a VM for testing the configuration
    BuildVm,
    /// Build a VM with persistent state
    BuildVmWithBootloader,
}

impl RebuildAction {
    fn as_nixos_rebuild_arg(&self) -> &'static str {
        match self {
            RebuildAction::Switch => "switch",
            RebuildAction::Boot => "boot",
            RebuildAction::Test => "test",
            RebuildAction::Build => "build",
            RebuildAction::DryBuild => "dry-build",
            RebuildAction::BuildVm => "build-vm",
            RebuildAction::BuildVmWithBootloader => "build-vm-with-bootloader",
        }
    }
}

pub fn run(args: OsArgs) -> Result<()> {
    match args.command {
        OsCommands::Rebuild(rebuild_args) => run_rebuild(rebuild_args),
        OsCommands::Repl(repl_args) => run_repl(repl_args),
    }
}

fn run_rebuild(args: RebuildArgs) -> Result<()> {
    // Parse the flake reference
    let (flake_path, hostname) = parse_flake_ref(&args.flake_ref)?;

    info!(
        flake = %flake_path.display(),
        hostname = %hostname,
        action = ?args.action,
        "preparing NixOS rebuild"
    );

    // Read the flake.lock
    let lock_path = flake_path.join("flake.lock");
    let lock: FlakeLock = if lock_path.exists() {
        let content =
            std::fs::read_to_string(&lock_path).context("failed to read flake.lock")?;
        serde_json::from_str(&content).context("failed to parse flake.lock")?
    } else {
        return Err(anyhow!(
            "flake.lock not found at {}. Run 'trix flake lock' first.",
            lock_path.display()
        ));
    };

    // Generate the Nix expression
    let flake_dir_str = flake_path
        .to_str()
        .ok_or_else(|| anyhow!("flake path is not valid UTF-8"))?;

    // Build attribute path to nixosConfigurations.<hostname>
    // We return the full nixosConfiguration, and nixos-rebuild will access
    // config.system.build.toplevel from it
    let attr_path = vec![
        "nixosConfigurations".to_string(),
        hostname.clone(),
    ];

    let expr = generate_flake_eval_expr(flake_dir_str, &lock, &attr_path)?;

    // Add timestamp header for debugging
    let expr_with_header = format!(
        r#"# Generated by trix for NixOS rebuild
# Flake: {}
# Host: {}
# Generated: {}
{}
"#,
        flake_path.display(),
        hostname,
        chrono::Utc::now().to_rfc3339(),
        expr
    );

    if args.print_expr {
        println!("{}", expr_with_header);
        return Ok(());
    }

    // Create a temporary directory that will be cleaned up automatically
    let temp_dir = TempDir::new().context("failed to create temporary directory")?;
    let expr_file = temp_dir.path().join("nixos-config.nix");

    debug!(expr_file = %expr_file.display(), "writing expression to temp file");
    std::fs::write(&expr_file, &expr_with_header).context("failed to write expression file")?;

    // Build the nixos-rebuild command
    let mut cmd = Command::new("nixos-rebuild");

    // Add the action
    cmd.arg(args.action.as_nixos_rebuild_arg());

    // Add --file pointing to our generated expression
    cmd.arg("--file").arg(&expr_file);

    // nixos-rebuild --file expects to find config.system.build.toplevel
    // Our expression returns nixosConfigurations.<hostname> which has this structure

    // Add optional flags
    if args.sudo || args.ask_sudo_password {
        cmd.arg("--sudo");
    }

    if args.ask_sudo_password {
        cmd.arg("--ask-sudo-password");
    }

    if let Some(ref host) = args.build_host {
        cmd.arg("--build-host").arg(host);
    }

    if let Some(ref host) = args.target_host {
        cmd.arg("--target-host").arg(host);
    }

    if let Some(ref profile) = args.profile_name {
        cmd.arg("--profile-name").arg(profile);
    }

    if args.show_trace {
        cmd.arg("--show-trace");
    }

    if args.nixos_verbose {
        cmd.arg("--verbose");
    }

    // Add passthrough arguments
    for arg in &args.passthrough {
        cmd.arg(arg);
    }

    info!(cmd = ?cmd, "running nixos-rebuild");

    // Run nixos-rebuild
    let status = cmd
        .status()
        .context("failed to run nixos-rebuild - is it installed?")?;

    handle_exit_status(status)
}

fn run_repl(args: ReplArgs) -> Result<()> {
    // Non-flake mode: passthrough to nixos-rebuild repl
    if args.file.is_some() || args.attr.is_some() {
        info!("using nixos-rebuild repl for non-flake mode");

        let mut cmd = Command::new("nixos-rebuild");
        cmd.arg("repl");

        if let Some(ref file) = args.file {
            cmd.args(["--file", file]);
        }

        if let Some(ref attr) = args.attr {
            cmd.args(["--attr", attr]);
        }

        if args.show_trace {
            cmd.arg("--show-trace");
        }

        for arg in &args.passthrough {
            cmd.arg(arg);
        }

        debug!(cmd = ?cmd, "executing nixos-rebuild repl");

        // exec replaces the current process
        let err = cmd.exec();
        return Err(anyhow!("failed to exec nixos-rebuild: {}", err));
    }

    // Flake mode: use our implementation (no store copy)
    let (flake_path, hostname) = parse_flake_ref(&args.flake_ref)?;

    info!(
        flake = %flake_path.display(),
        hostname = %hostname,
        "preparing NixOS REPL"
    );

    // Read the flake.lock
    let lock_path = flake_path.join("flake.lock");
    let lock: FlakeLock = if lock_path.exists() {
        let content =
            std::fs::read_to_string(&lock_path).context("failed to read flake.lock")?;
        serde_json::from_str(&content).context("failed to parse flake.lock")?
    } else {
        return Err(anyhow!(
            "flake.lock not found at {}. Run 'trix flake lock' first.",
            lock_path.display()
        ));
    };

    // Generate the Nix expression for the NixOS configuration
    let flake_dir_str = flake_path
        .to_str()
        .ok_or_else(|| anyhow!("flake path is not valid UTF-8"))?;

    // Build attribute path to nixosConfigurations.<hostname>
    let attr_path = vec![
        "nixosConfigurations".to_string(),
        hostname.clone(),
    ];

    let base_expr = generate_flake_eval_expr(flake_dir_str, &lock, &attr_path)?;

    // Wrap the expression to expose config, options, pkgs, lib like nixos-rebuild repl does
    let repl_expr = format!(
        r#"# NixOS REPL - generated by trix
# Flake: {flake_path}
# Host: {hostname}
#
# Available variables:
#   config  - Evaluated NixOS configuration options
#   options - NixOS option definitions and metadata
#   pkgs    - Nixpkgs package set
#   lib     - Nixpkgs library functions
#
let
  nixosConfig = {base_expr};
in {{
  inherit (nixosConfig) config options;
  pkgs = nixosConfig.pkgs or nixosConfig._module.args.pkgs;
  lib = nixosConfig.lib or nixosConfig._module.args.lib or nixosConfig.pkgs.lib;
}}"#,
        flake_path = flake_path.display(),
        hostname = hostname,
        base_expr = base_expr,
    );

    if args.print_expr {
        println!("{}", repl_expr);
        return Ok(());
    }

    // Build nix repl command
    let mut cmd = Command::new("nix");
    cmd.args(["repl", "--expr", &repl_expr]);

    if args.show_trace {
        cmd.arg("--show-trace");
    }

    for arg in &args.passthrough {
        cmd.arg(arg);
    }

    info!("starting NixOS REPL for {}", hostname);
    debug!(cmd = ?cmd, "executing nix repl");

    // exec replaces the current process
    let err = cmd.exec();
    Err(anyhow!("failed to exec nix repl: {}", err))
}

/// Parse a flake reference into (flake_path, hostname).
///
/// Formats:
/// - None or "." -> (current dir, system hostname)
/// - ".#myhost" -> (current dir, "myhost")
/// - "/path/to/flake" -> (/path/to/flake, system hostname)
/// - "/path/to/flake#myhost" -> (/path/to/flake, "myhost")
fn parse_flake_ref(flake_ref: &Option<String>) -> Result<(PathBuf, String)> {
    let (ref_part, hostname_part) = match flake_ref {
        Some(r) => {
            if let Some((path, host)) = r.split_once('#') {
                (path.to_string(), Some(host.to_string()))
            } else {
                (r.clone(), None)
            }
        }
        None => (".".to_string(), None),
    };

    // Resolve the flake path
    let raw_path = if ref_part.is_empty() || ref_part == "." {
        std::env::current_dir().context("failed to get current directory")?
    } else {
        PathBuf::from(&ref_part)
    };

    // Find the actual flake root (in case we're in a subdirectory)
    let flake_path = find_flake_root(&raw_path).unwrap_or(raw_path);

    // Determine hostname (fall back to system hostname if empty or not specified)
    let hostname = match hostname_part {
        Some(h) if !h.is_empty() => h,
        _ => {
            // Use system hostname
            hostname::get()
                .context("failed to get system hostname")?
                .into_string()
                .map_err(|_| anyhow!("hostname is not valid UTF-8"))?
        }
    };

    Ok((flake_path, hostname))
}

fn handle_exit_status(status: ExitStatus) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        match status.code() {
            Some(code) => Err(anyhow!("nixos-rebuild exited with code {}", code)),
            None => Err(anyhow!("nixos-rebuild was terminated by signal")),
        }
    }
}
