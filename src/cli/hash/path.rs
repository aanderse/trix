use crate::command::NixCommand;
use clap::Args;

#[derive(Args, Clone, Debug)]
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

pub fn handle(cmd: &mut NixCommand, args: &PathArgs) {
    cmd.arg("path");
    if args.base16 {
        cmd.arg("--base16");
    }
    if args.base32 {
        cmd.arg("--base32");
    }
    if args.base64 {
        cmd.arg("--base64");
    }
    if args.sri {
        cmd.arg("--sri");
    }
    if let Some(t) = &args.type_ {
        cmd.args(["--type", t]);
    }
    cmd.args(&args.paths);
}
