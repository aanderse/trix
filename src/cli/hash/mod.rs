pub mod convert;
pub mod file;
pub mod path;

use self::convert::{ConvertArgs, LegacyArgs};
use self::file::FileArgs;
use self::path::PathArgs;
use crate::command::NixCommand;
use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand, Clone, Debug)]
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

pub fn cmd_hash(cmd: HashCommands) -> Result<()> {
    let mut command = NixCommand::new("nix");
    command.arg("hash");

    match cmd {
        HashCommands::File(args) => file::handle(&mut command, &args),
        HashCommands::Path(args) => path::handle(&mut command, &args),
        HashCommands::ToBase16(args) => convert::handle_legacy(&mut command, &args, "to-base16"),
        HashCommands::ToBase32(args) => convert::handle_legacy(&mut command, &args, "to-base32"),
        HashCommands::ToBase64(args) => convert::handle_legacy(&mut command, &args, "to-base64"),
        HashCommands::ToSri(args) => convert::handle_legacy(&mut command, &args, "to-sri"),
        HashCommands::Convert(args) => convert::handle_convert(&mut command, &args),
    }

    // Interactive command, replaces current process
    // Actually NixCommand uses Command::exec on unix which replaces process
    command.exec()
}
