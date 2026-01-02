use crate::common::Cache;
use crate::nix::get_clean_env;
use anyhow::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::process::Command;

/// Cache for program availability checks
static PROGRAM_AVAILABILITY: Cache<String, bool> = Cache::new();

#[derive(Debug)]
pub struct NixCommand {
    program: String,
    args: Vec<OsString>,
    envs: Vec<(OsString, OsString)>,
}

impl NixCommand {
    pub fn new(program: &str) -> Self {
        // Start with clean environment
        let envs = get_clean_env()
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();

        let mut cmd = Self {
            program: program.to_string(),
            args: Vec::new(),
            envs,
        };

        // Add experimental features flag unconditionally for now
        cmd.args(["--extra-experimental-features", "flakes nix-command"]);
        cmd
    }

    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.arg(arg);
        }
        self
    }

    pub fn envs<I, K, V>(&mut self, envs: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        for (k, v) in envs {
            self.envs
                .push((k.as_ref().to_os_string(), v.as_ref().to_os_string()));
        }
        self
    }

    fn construct_command(&self) -> Command {
        // Check for nom availability and substitutions
        let mut program = self.program.clone();
        let mut args = self.args.clone();

        if program == "nix" && is_program_available("nom") {
            // Check if "build" is in the arguments using OsStr comparison
            let build_arg = OsString::from("build");

            // Find position of "build" argument
            let build_pos = args.iter().position(|arg| arg == &build_arg);

            if let Some(pos) = build_pos {
                program = "nom".to_string();
                // Remove "build" from its current position and insert at the front
                // This ensures `nom build ...` structure which is preferred
                let arg = args.remove(pos);
                args.insert(0, arg);
            }
        } else if program == "nix-build" && is_program_available("nom-build") {
            program = "nom-build".to_string();
        }

        let mut cmd = Command::new(&program);
        cmd.args(&args);
        cmd.env_clear();
        cmd.envs(self.envs.clone());
        cmd
    }

    pub fn run(&mut self) -> Result<()> {
        let mut cmd = self.construct_command();
        tracing::debug!("+ {}", self.format_command());

        let status = cmd
            .status()
            .context(format!("Failed to run {}", self.program))?;
        if !status.success() {
            anyhow::bail!(
                "Command failed with exit code: {}",
                status.code().unwrap_or(1)
            );
        }
        Ok(())
    }

    pub fn output(&mut self) -> Result<String> {
        let mut cmd = self.construct_command();
        tracing::debug!("+ {}", self.format_command());

        let output = cmd
            .output()
            .context(format!("Failed to run {}", self.program))?;
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
        let mut cmd = self.construct_command();
        tracing::debug!("+ {}", self.format_command());
        let err = cmd.exec();
        anyhow::bail!("Failed to exec {}: {}", self.program, err);
    }

    pub fn format_command(&self) -> String {
        let cmd = self.construct_command();
        let program = cmd.get_program().to_string_lossy();
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy()).collect();
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

        // We also need to clear specific cache entries or rely on new queries
        // Since we cannot clear the static cache easily from here (it's private),
        // we hope this program name hasn't been cached yet.
        assert!(check_program_in_path(program_name));
        assert!(!check_program_in_path("non_existent_program_98765"));
    }

    #[test]
    fn test_nom_available_build_substitution() {
        PROGRAM_AVAILABILITY.insert("nom".to_string(), true);

        let mut cmd = NixCommand::new("nix");
        cmd.arg("build");

        let formatted = cmd.format_command();
        // Check exact start to verify order: "nom build"
        assert!(
            formatted.starts_with("nom build"),
            "Expected 'nom build' at start but got '{}'",
            formatted
        );
        assert!(
            formatted.contains("--extra-experimental-features"),
            "Should contain experimental features"
        );
    }

    #[test]
    fn test_nom_not_available_no_substitution() {
        PROGRAM_AVAILABILITY.insert("nom".to_string(), false);

        let mut cmd = NixCommand::new("nix");
        cmd.arg("build");

        let formatted = cmd.format_command();
        assert!(
            formatted.starts_with("nix"),
            "Expected 'nix' but got '{}'",
            formatted
        );
    }

    #[test]
    fn test_nom_available_no_build_command() {
        PROGRAM_AVAILABILITY.insert("nom".to_string(), true);

        let mut cmd = NixCommand::new("nix");
        cmd.arg("flake"); // Not "build"

        let formatted = cmd.format_command();
        assert!(
            formatted.starts_with("nix"),
            "Expected 'nix' but got '{}'",
            formatted
        );
    }

    #[test]
    fn test_nix_build_nom_build_substitution() {
        PROGRAM_AVAILABILITY.insert("nom-build".to_string(), true);
        let cmd = NixCommand::new("nix-build");
        assert!(cmd.format_command().starts_with("nom-build"));

        PROGRAM_AVAILABILITY.insert("nom-build".to_string(), false);
        let cmd2 = NixCommand::new("nix-build");
        assert!(cmd2.format_command().starts_with("nix-build"));
    }
}
