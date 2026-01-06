//! Hash command - compute and convert cryptographic hashes.

use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct HashArgs {
    #[command(subcommand)]
    pub command: HashCommands,
}

#[derive(Subcommand)]
pub enum HashCommands {
    /// Print cryptographic hash of a regular file
    File(FileArgs),

    /// Print cryptographic hash of the NAR serialisation of a path
    Path(PathArgs),

    /// Convert a hash to base-16 representation (deprecated)
    #[command(name = "to-base16")]
    ToBase16(LegacyArgs),

    /// Convert a hash to base-32 representation (deprecated)
    #[command(name = "to-base32")]
    ToBase32(LegacyArgs),

    /// Convert a hash to base-64 representation (deprecated)
    #[command(name = "to-base64")]
    ToBase64(LegacyArgs),

    /// Convert a hash to SRI representation (deprecated)
    #[command(name = "to-sri")]
    ToSri(LegacyArgs),

    /// Convert between hash formats
    Convert(ConvertArgs),
}

#[derive(Args)]
pub struct FileArgs {
    /// Paths to files
    #[arg(required = true)]
    pub paths: Vec<String>,

    /// Print the hash in base-16 format
    #[arg(long)]
    pub base16: bool,

    /// Print the hash in base-32 (Nix-specific) format
    #[arg(long)]
    pub base32: bool,

    /// Print the hash in base-64 format
    #[arg(long)]
    pub base64: bool,

    /// Print the hash in SRI format
    #[arg(long)]
    pub sri: bool,

    /// Hash algorithm (blake3, md5, sha1, sha256, or sha512)
    #[arg(long = "type")]
    pub type_: Option<String>,
}

#[derive(Args)]
pub struct PathArgs {
    /// Paths to hash
    #[arg(required = true)]
    pub paths: Vec<String>,

    /// Print the hash in base-16 format
    #[arg(long)]
    pub base16: bool,

    /// Print the hash in base-32 (Nix-specific) format
    #[arg(long)]
    pub base32: bool,

    /// Print the hash in base-64 format
    #[arg(long)]
    pub base64: bool,

    /// Print the hash in SRI format
    #[arg(long)]
    pub sri: bool,

    /// Hash algorithm (blake3, md5, sha1, sha256, or sha512)
    #[arg(long = "type")]
    pub type_: Option<String>,
}

#[derive(Args)]
pub struct LegacyArgs {
    /// Hashes to convert
    #[arg(required = true)]
    pub hashes: Vec<String>,

    /// Hash algorithm (blake3, md5, sha1, sha256, or sha512)
    #[arg(long = "type")]
    pub type_: Option<String>,
}

#[derive(Args)]
pub struct ConvertArgs {
    /// Hashes to convert
    #[arg(required = true)]
    pub hashes: Vec<String>,

    /// Source hash format (base16, nix32, base64, sri)
    #[arg(long)]
    pub from: Option<String>,

    /// Target hash format (base16, nix32, base64, sri)
    #[arg(long)]
    pub to: Option<String>,

    /// Hash algorithm (blake3, md5, sha1, sha256, or sha512)
    #[arg(long = "hash-algo")]
    pub hash_algo: Option<String>,
}

pub fn run(args: HashArgs) -> Result<()> {
    let mut cmd = Command::new("nix");
    cmd.arg("hash");

    match args.command {
        HashCommands::File(file_args) => {
            cmd.arg("file");
            if file_args.base16 {
                cmd.arg("--base16");
            }
            if file_args.base32 {
                cmd.arg("--base32");
            }
            if file_args.base64 {
                cmd.arg("--base64");
            }
            if file_args.sri {
                cmd.arg("--sri");
            }
            if let Some(t) = &file_args.type_ {
                cmd.args(["--type", t]);
            }
            cmd.args(&file_args.paths);
        }
        HashCommands::Path(path_args) => {
            cmd.arg("path");
            if path_args.base16 {
                cmd.arg("--base16");
            }
            if path_args.base32 {
                cmd.arg("--base32");
            }
            if path_args.base64 {
                cmd.arg("--base64");
            }
            if path_args.sri {
                cmd.arg("--sri");
            }
            if let Some(t) = &path_args.type_ {
                cmd.args(["--type", t]);
            }
            cmd.args(&path_args.paths);
        }
        HashCommands::ToBase16(legacy_args) => {
            cmd.arg("to-base16");
            if let Some(t) = &legacy_args.type_ {
                cmd.args(["--type", t]);
            }
            cmd.args(&legacy_args.hashes);
        }
        HashCommands::ToBase32(legacy_args) => {
            cmd.arg("to-base32");
            if let Some(t) = &legacy_args.type_ {
                cmd.args(["--type", t]);
            }
            cmd.args(&legacy_args.hashes);
        }
        HashCommands::ToBase64(legacy_args) => {
            cmd.arg("to-base64");
            if let Some(t) = &legacy_args.type_ {
                cmd.args(["--type", t]);
            }
            cmd.args(&legacy_args.hashes);
        }
        HashCommands::ToSri(legacy_args) => {
            cmd.arg("to-sri");
            if let Some(t) = &legacy_args.type_ {
                cmd.args(["--type", t]);
            }
            cmd.args(&legacy_args.hashes);
        }
        HashCommands::Convert(convert_args) => {
            cmd.arg("convert");
            if let Some(f) = &convert_args.from {
                cmd.args(["--from", f]);
            }
            if let Some(t) = &convert_args.to {
                cmd.args(["--to", t]);
            }
            if let Some(algo) = &convert_args.hash_algo {
                cmd.args(["--hash-algo", algo]);
            }
            cmd.args(&convert_args.hashes);
        }
    }

    // Replace current process with nix hash
    let err = cmd.exec();

    // If we get here, exec failed
    Err(err.into())
}
