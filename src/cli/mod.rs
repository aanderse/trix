//! CLI module exports and shared utilities.

pub mod common;
pub mod style;

#[path = "build/command.rs"]
pub mod build;

#[path = "copy/command.rs"]
pub mod copy;

#[path = "develop/command.rs"]
pub mod develop;

#[path = "fmt/command.rs"]
pub mod fmt;

#[path = "log/command.rs"]
pub mod log;

#[path = "run/command.rs"]
pub mod run;

#[path = "shell/command.rs"]
pub mod shell;

#[path = "why_depends/command.rs"]
pub mod why_depends;

#[path = "eval/command.rs"]
pub mod eval;

#[path = "repl/command.rs"]
pub mod repl;

pub mod flake;
pub mod hash;
pub mod profile;
pub mod registry;

pub use build::cmd_build;
pub use copy::cmd_copy;
pub use develop::cmd_develop;
pub use eval::cmd_eval;
pub use fmt::cmd_fmt;
pub use log::cmd_log;
pub use repl::cmd_repl;
pub use run::cmd_run;
pub use shell::cmd_shell;
pub use why_depends::cmd_why_depends;
