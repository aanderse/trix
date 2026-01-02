use crate::nix::get_clean_env;
use anyhow::{Context, Result};
use std::process::Command;

pub struct NixCommand {
    cmd: Command,
}

impl NixCommand {
    pub fn new(program: &str) -> Self {
        let mut cmd = Command::new(program);
        // Add experimental features flag unconditionally for now
        cmd.args(["--extra-experimental-features", "flakes nix-command"]);
        // Clear inherited env and set clean env (with TMPDIR removed)
        cmd.env_clear();
        cmd.envs(get_clean_env());
        Self { cmd }
    }

    pub fn arg<S: AsRef<std::ffi::OsStr>>(&mut self, arg: S) -> &mut Self {
        self.cmd.arg(arg);
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        self.cmd.args(args);
        self
    }

    pub fn envs<I, K, V>(&mut self, envs: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<std::ffi::OsStr>,
        V: AsRef<std::ffi::OsStr>,
    {
        self.cmd.envs(envs);
        self
    }

    pub fn run(&mut self) -> Result<()> {
        tracing::debug!("+ {}", self.format_command());

        let status = self
            .cmd
            .status()
            .context(format!("Failed to run {}", self.get_program()))?;
        if !status.success() {
            anyhow::bail!(
                "Command failed with exit code: {}",
                status.code().unwrap_or(1)
            );
        }
        Ok(())
    }

    pub fn output(&mut self) -> Result<String> {
        tracing::debug!("+ {}", self.format_command());

        let output = self
            .cmd
            .output()
            .context(format!("Failed to run {}", self.get_program()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Command failed:\n{}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.trim().to_string())
    }

    pub fn json<T: serde::de::DeserializeOwned>(&mut self) -> Result<T> {
        let output = self.output()?;
        serde_json::from_str(&output).context("Failed to parse JSON output")
    }

    #[cfg(unix)]
    pub fn exec(&mut self) -> Result<()> {
        use std::os::unix::process::CommandExt;
        tracing::debug!("+ {}", self.format_command());
        let err = self.cmd.exec();
        anyhow::bail!("Failed to exec {}: {}", self.get_program(), err);
    }

    fn get_program(&self) -> String {
        self.cmd.get_program().to_string_lossy().to_string()
    }

    fn format_command(&self) -> String {
        let program = self.get_program();
        let args: Vec<_> = self.cmd.get_args().map(|a| a.to_string_lossy()).collect();
        format!("{} {}", program, args.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_command() {
        let mut cmd = NixCommand::new("nix");
        cmd.args(["flake", "show"]);
        assert_eq!(
            cmd.format_command(),
            "nix --extra-experimental-features flakes nix-command flake show"
        );
    }

    #[test]
    fn test_all_commands_have_experimental_features() {
        let cmd = NixCommand::new("nix-build");
        // All commands now have experimental features flag unconditionally
        assert!(cmd.format_command().contains("experimental-features"));
    }

    #[test]
    fn test_get_program() {
        let cmd = NixCommand::new("nix-store");
        assert_eq!(cmd.get_program(), "nix-store");
    }
}
