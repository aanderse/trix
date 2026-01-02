//! trix - Impure flakes wrapper using legacy nix-* commands.

pub mod cli;
pub mod command;
pub mod common;
pub mod flake;
pub mod git;
pub mod lock;
pub mod nix;
pub mod profile;
pub mod registry;

pub use flake::ResolvedInstallable;
