use crate::command::NixCommand;
use clap::Args;

#[derive(Args, Clone, Debug)]
pub struct LegacyArgs {
    /// Hashes to convert
    #[arg(required = true)]
    pub hashes: Vec<String>,

    /// Hash algorithm (blake3, md5, sha1, sha256, or sha512)
    #[arg(long = "type")]
    pub type_: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct ConvertArgs {
    /// Hashes to convert
    #[arg(required = true)]
    pub hashes: Vec<String>,

    /// Hash format (base16, nix32, base64, sri)
    #[arg(long)]
    pub from: Option<String>,

    /// Hash format (base16, nix32, base64, sri)
    #[arg(long)]
    pub to: Option<String>,

    /// Hash algorithm (blake3, md5, sha1, sha256, or sha512)
    #[arg(long = "hash-algo")]
    pub hash_algo: Option<String>,
}

pub fn handle_legacy(cmd: &mut NixCommand, args: &LegacyArgs, subcommand: &str) {
    cmd.arg(subcommand);
    if let Some(t) = &args.type_ {
        cmd.args(["--type", t]);
    }
    cmd.args(&args.hashes);
}

pub fn handle_convert(cmd: &mut NixCommand, args: &ConvertArgs) {
    cmd.arg("convert");
    if let Some(f) = &args.from {
        cmd.args(["--from", f]);
    }
    if let Some(t) = &args.to {
        cmd.args(["--to", t]);
    }
    if let Some(algo) = &args.hash_algo {
        cmd.args(["--hash-algo", algo]);
    }
    cmd.args(&args.hashes);
}
