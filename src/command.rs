use crate::common::Cache;
use crate::nix::get_clean_env;
use anyhow::{Context, Result};
use std::process::Command;

/// Cache for program availability checks
static PROGRAM_AVAILABILITY: Cache<String, bool> = Cache::new();

pub struct NixCommand {
    cmd: Command,
}

impl NixCommand {
    pub fn new(program: &str) -> Self {
        let program = match program {
            "nix" if is_program_available("nom") => "nom",
            "nix-build" if is_program_available("nom-build") => "nom-build",
            _ => program,
        };

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

/// Check if a program is available in PATH
fn is_program_available(program: &str) -> bool {
    if let Some(available) = PROGRAM_AVAILABILITY.get(&program.to_string()) {
        return available;
    }

    let available = check_program_in_path(program);
    PROGRAM_AVAILABILITY.insert(program.to_string(), available);
    available
}

fn check_program_in_path(program: &str) -> bool {
    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            let user_path = path.join(program);
            if user_path.is_file() && is_executable(&user_path) {
                return true;
            }
        }
    }
    false
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(metadata) = path.metadata() {
        return metadata.permissions().mode() & 0o111 != 0;
    }
    false
}

#[cfg(not(unix))]
fn is_executable(_path: &std::path::Path) -> bool {
    true // Assume executable on non-unix for simplicity if it's a file
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

    #[test]
    fn test_is_program_available() {
        use std::fs::File;
        use std::os::unix::fs::PermissionsExt;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let program_name = "test_program_12345";
        let program_path = dir.path().join(program_name);

        File::create(&program_path).unwrap();
        let mut perms = std::fs::metadata(&program_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&program_path, perms).unwrap();

        let path = std::env::var_os("PATH").unwrap();
        let mut paths = std::env::split_paths(&path).collect::<Vec<_>>();
        paths.push(dir.path().to_path_buf());
        let new_path = std::env::join_paths(paths).unwrap();

        // Safety: this is a test, and we are modifying the environment.
        // This might race with other tests if checking PATH, but we are using a unique name.
        unsafe {
            std::env::set_var("PATH", new_path);
        }

        assert!(check_program_in_path(program_name));
        assert!(!check_program_in_path("non_existent_program_98765"));
    }
}
