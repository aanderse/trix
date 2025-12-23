//! Build, develop, run, and related commands.

use anyhow::{Context, Result};

use crate::flake::{ensure_lock, resolve_attr_path, resolve_installable, ResolvedInstallable};
use crate::nix::{
    get_derivation_path, get_package_main_program, get_store_path_from_drv, get_system,
    run_nix_build, run_nix_shell, BuildOptions, ShellOptions,
};

/// Build a resolved flake attribute.
///
/// This helper handles the common logic for local builds:
/// 1. Getting the flake directory
/// 2. Ensuring the lock file exists
/// 3. Running nix-build
fn build_resolved_attribute(
    resolved: &ResolvedInstallable,
    attr: &str,
    options: &BuildOptions,
    capture_output: bool,
) -> Result<Option<String>> {
    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;

    // Ensure lock exists
    ensure_lock(flake_dir, None)?;

    run_nix_build(flake_dir, attr, options, capture_output)
}

/// Build a package from flake.nix
pub fn cmd_build(
    installable: &str,
    out_link: Option<&str>,
    no_link: bool,
    extra_args: Vec<(String, String)>,
    extra_argstrs: Vec<(String, String)>,
    store: Option<&str>,
) -> Result<()> {
    let resolved = resolve_installable(installable);

    if !resolved.is_local {
        // Passthrough to nix build
        let flake_ref = resolved.flake_ref.as_deref().unwrap_or("");
        let full_ref = format!("{}#{}", flake_ref, resolved.attr_part);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.arg("build").arg(&full_ref);

        if no_link {
            cmd.arg("--no-link");
        } else if let Some(link) = out_link {
            cmd.args(["-o", link]);
        }

        if let Some(s) = store {
            cmd.args(["--store", s]);
        }

        for (name, expr) in &extra_args {
            cmd.args(["--arg", name, expr]);
        }

        for (name, value) in &extra_argstrs {
            cmd.args(["--argstr", name, value]);
        }

        return cmd.run();
    }

    let system = get_system()?;

    // Resolve attribute path
    let attr = resolve_attr_path(&resolved.attr_part, "packages", &system);

    let options = BuildOptions {
        out_link: if no_link {
            None
        } else {
            Some(out_link.unwrap_or("result").to_string())
        },
        extra_args,
        extra_argstrs,
        store: store.map(|s| s.to_string()),
    };

    build_resolved_attribute(&resolved, &attr, &options, false)?;

    Ok(())
}

/// Enter a development shell from flake.nix
pub fn cmd_develop(
    installable: &str,
    run_cmd: Option<&str>,
    extra_args: Vec<(String, String)>,
    extra_argstrs: Vec<(String, String)>,
    store: Option<&str>,
) -> Result<()> {
    let resolved = resolve_installable(installable);

    if !resolved.is_local {
        // Passthrough to nix develop
        let flake_ref = resolved.flake_ref.as_deref().unwrap_or("");
        let full_ref = format!("{}#{}", flake_ref, resolved.attr_part);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.arg("develop").arg(&full_ref);

        if let Some(c) = run_cmd {
            cmd.args(["--command", c]);
        }

        if let Some(s) = store {
            cmd.args(["--store", s]);
        }

        return cmd.exec();
    }

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;
    let system = get_system()?;

    // Ensure lock exists
    ensure_lock(flake_dir, None)?;

    // Resolve attribute path for devShells
    let attr = resolve_attr_path(&resolved.attr_part, "devShells", &system);

    // Get nixConfig
    let nix_config = crate::flake::get_nix_config(flake_dir, true);

    let options = ShellOptions {
        command: run_cmd.map(|s| s.to_string()),
        extra_args,
        extra_argstrs,
        store: store.map(|s| s.to_string()),
        bash_prompt: nix_config["bash-prompt"].as_str().map(|s| s.to_string()),
        bash_prompt_prefix: nix_config["bash-prompt-prefix"]
            .as_str()
            .map(|s| s.to_string()),
        bash_prompt_suffix: nix_config["bash-prompt-suffix"]
            .as_str()
            .map(|s| s.to_string()),
    };

    run_nix_shell(flake_dir, &attr, &options)
}

/// Build and run a package from flake.nix
pub fn cmd_run(
    installable: &str,
    args: &[String],
    extra_args: Vec<(String, String)>,
    extra_argstrs: Vec<(String, String)>,
    store: Option<&str>,
) -> Result<()> {
    let resolved = resolve_installable(installable);

    if !resolved.is_local {
        // Passthrough to nix run
        let flake_ref = resolved.flake_ref.as_deref().unwrap_or("");
        let full_ref = format!("{}#{}", flake_ref, resolved.attr_part);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.args(["run", &full_ref]);

        if let Some(s) = store {
            cmd.args(["--store", s]);
        }

        if !args.is_empty() {
            cmd.arg("--");
            cmd.args(args);
        }

        return cmd.exec();
    }

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;
    let system = get_system()?;

    // Ensure lock exists
    ensure_lock(flake_dir, None)?;

    // Try apps first, then packages
    // Empty attr_part (from ".#") defaults to "default"
    let attr_name = if resolved.attr_part.is_empty() { "default" } else { &resolved.attr_part };
    let app_attr = format!("apps.{}.{}", system, attr_name);
    let pkg_attr = resolve_attr_path(&resolved.attr_part, "packages", &system);

    // Check if it's an app
    let exe_path = if crate::nix::flake_has_attr(flake_dir, &app_attr)? {
        // It's an app - get the program path
        let options = crate::nix::EvalOptions {
            output_json: true,
            ..Default::default()
        };
        let result =
            crate::nix::run_nix_eval(Some(flake_dir), &format!("{}.program", app_attr), &options)?;
        let program: String = serde_json::from_str(&result)?;
        program
    } else {
        // It's a package - build and get the executable
        let options = BuildOptions {
            out_link: None,
            extra_args: extra_args.clone(),
            extra_argstrs: extra_argstrs.clone(),
            store: store.map(|s| s.to_string()),
        };

        let store_path = build_resolved_attribute(&resolved, &pkg_attr, &options, true)?
            .context("Build failed")?;

        // Get the main program name from meta.mainProgram, pname, or name
        let main_program = crate::nix::get_package_main_program(flake_dir, &pkg_attr)?;
        format!("{}/bin/{}", store_path, main_program)
    };

    // Run the executable
    let mut cmd = std::process::Command::new(&exe_path);
    cmd.args(args);

    tracing::debug!("+ {} {}", exe_path, args.join(" "));

    let status = cmd
        .status()
        .context(format!("Failed to run {}", exe_path))?;
    if !status.success() {
        anyhow::bail!(
            "Command failed with exit code: {}",
            status.code().unwrap_or(1)
        );
    }
    Ok(())
}

/// Copy a package to another store
pub fn cmd_copy(installable: &str, to: &str, no_check_sigs: bool) -> Result<()> {
    let resolved = resolve_installable(installable);

    if !resolved.is_local {
        // Passthrough to nix copy
        let flake_ref = resolved.flake_ref.as_deref().unwrap_or("");
        let full_ref = format!("{}#{}", flake_ref, resolved.attr_part);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.args(["copy", "--to", to, &full_ref]);

        if no_check_sigs {
            cmd.arg("--no-check-sigs");
        }

        return cmd.run();
    }

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;
    let system = get_system()?;

    // Ensure lock exists
    ensure_lock(flake_dir, None)?;

    // Get attribute
    let attr = resolve_attr_path(&resolved.attr_part, "packages", &system);

    // Get derivation path
    let drv_path = get_derivation_path(flake_dir, &attr)?;
    let store_path = get_store_path_from_drv(&drv_path)?;

    // Copy to destination
    let mut cmd = crate::command::NixCommand::new("nix");
    cmd.args(["copy", "--to", to, &store_path]);

    if no_check_sigs {
        cmd.arg("--no-check-sigs");
    }

    cmd.run()
}

/// Show build log for a package
pub fn cmd_log(installable: &str) -> Result<()> {
    let resolved = resolve_installable(installable);

    if !resolved.is_local {
        // Passthrough to nix log
        let flake_ref = resolved.flake_ref.as_deref().unwrap_or("");
        let full_ref = format!("{}#{}", flake_ref, resolved.attr_part);

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.args(["log", &full_ref]);

        return cmd.run();
    }

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;
    let system = get_system()?;

    let attr = resolve_attr_path(&resolved.attr_part, "packages", &system);
    let drv_path = get_derivation_path(flake_dir, &attr)?;

    if let Some(log) = crate::nix::get_build_log(&drv_path) {
        print!("{}", log);
    } else {
        anyhow::bail!("No build log available for {}", drv_path);
    }

    Ok(())
}

/// Show why a package depends on another
pub fn cmd_why_depends(package: &str, dependency: &str) -> Result<()> {
    fn resolve_to_store_path(ref_str: &str) -> Result<String> {
        if ref_str.starts_with("/nix/store/") {
            return Ok(ref_str.to_string());
        }

        let resolved = crate::flake::resolve_installable(ref_str);
        if !resolved.is_local {
            // For remote refs, we need to build first then copy the store path
            let full_ref = if resolved.attr_part != "default" {
                format!(
                    "{}#{}",
                    resolved.flake_ref.as_deref().unwrap_or(""),
                    resolved.attr_part
                )
            } else {
                resolved.flake_ref.as_deref().unwrap_or("").to_string()
            };

            let mut cmd = crate::command::NixCommand::new("nix");
            cmd.args(["build", "--no-link", "--print-out-paths", &full_ref]);

            return cmd.output();
        }

        let system = crate::nix::get_system()?;
        let attr = crate::flake::resolve_attr_path(&resolved.attr_part, "packages", &system);

        // Build to get store path
        let options = crate::nix::BuildOptions {
            ..Default::default()
        };
        let store_path = build_resolved_attribute(
            &resolved, &attr, &options, true, // capture_output
        )?
        .context(format!("Failed to build {}", ref_str))?;

        Ok(store_path)
    }

    let pkg_path = resolve_to_store_path(package)?;
    let dep_path = resolve_to_store_path(dependency)?;

    let mut cmd = crate::command::NixCommand::new("nix");
    cmd.args(["why-depends", &pkg_path, &dep_path]);

    cmd.run()
}

/// Start a shell with specified packages available
pub fn cmd_shell(installables: &[String], command: Option<&str>) -> Result<()> {
    // Check if any installables are remote
    let mut has_remote = false;
    for installable in installables {
        let resolved = crate::flake::resolve_installable(installable);
        if !resolved.is_local {
            has_remote = true;
            break;
        }
    }

    if has_remote {
        // Passthrough to nix shell
        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.args(["shell"]);
        cmd.args(installables);

        if let Some(c) = command {
            cmd.args(["--command", c]);
        }

        return cmd.run();
    }

    // All local - use trix's native handling
    let mut store_paths = Vec::new();
    let options = crate::nix::BuildOptions {
        ..Default::default()
    };
    for installable in installables {
        let resolved = crate::flake::resolve_installable(installable);
        let system = crate::nix::get_system()?;
        let attr = crate::flake::resolve_attr_path(&resolved.attr_part, "packages", &system);

        let store_path = build_resolved_attribute(
            &resolved, &attr, &options, true, // capture_output
        )?
        .context(format!("Failed to build {}", installable))?;

        store_paths.push(store_path);
    }

    // Build PATH with all package bin directories
    let mut bin_paths = Vec::new();
    for store_path in &store_paths {
        let bin_dir = std::path::Path::new(store_path).join("bin");
        if bin_dir.is_dir() {
            bin_paths.push(bin_dir);
        }
    }

    if bin_paths.is_empty() {
        anyhow::bail!("No bin directories found in packages");
    }

    // Prepend to existing PATH
    let mut env = crate::nix::get_clean_env();
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let mut new_path_parts = Vec::new();
    for p in bin_paths {
        new_path_parts.push(p.to_string_lossy().into_owned());
    }
    if !old_path.is_empty() {
        new_path_parts.push(old_path.to_string_lossy().into_owned());
    }
    let new_path = new_path_parts.join(":");
    env.insert("PATH".to_string(), new_path);

    if let Some(cmd_str) = command {
        // Run command and exit
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", cmd_str]);
        cmd.env_clear();
        cmd.envs(env);

        tracing::debug!("+ sh -c {}", cmd_str);

        let status = cmd.status().context("Failed to run sh")?;
        if !status.success() {
            anyhow::bail!(
                "Command failed with exit code: {}",
                status.code().unwrap_or(1)
            );
        }
        Ok(())
    } else {
        // Start interactive shell
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

        let mut cmd = std::process::Command::new(&shell);
        cmd.env_clear();
        cmd.envs(env);

        tracing::debug!("+ {}", shell);

        let status = cmd.status().context(format!("Failed to run {}", shell))?;
        if !status.success() {
            anyhow::bail!(
                "Command failed with exit code: {}",
                status.code().unwrap_or(1)
            );
        }
        Ok(())
    }
}

/// Format files using the flake's formatter
pub fn cmd_fmt(installable: &str, args: &[String], store: Option<&str>) -> Result<()> {
    let resolved = resolve_installable(installable);

    if !resolved.is_local {
        // Passthrough to nix fmt
        let flake_ref = resolved.flake_ref.as_deref().unwrap_or("");

        let mut cmd = crate::command::NixCommand::new("nix");
        cmd.arg("fmt");

        if !flake_ref.is_empty() {
            cmd.arg(flake_ref);
        }

        if let Some(s) = store {
            cmd.args(["--store", s]);
        }

        if !args.is_empty() {
            cmd.arg("--");
            cmd.args(args);
        }

        return cmd.exec();
    }

    let system = get_system()?;

    // Determine attribute to build
    // If attr_part is "default" (from .#default or just .), use formatter.<system>
    let attr = if resolved.attr_part == "default" || resolved.attr_part.is_empty() {
        format!("formatter.{}", system)
    } else {
        resolved.attr_part.clone()
    };

    let flake_dir = resolved.flake_dir.as_ref().context("No flake directory")?;

    // Build the formatter first to ensure the store path exists
    let build_options = BuildOptions {
        out_link: None,
        store: store.map(|s| s.to_string()),
        ..Default::default()
    };

    let store_path = build_resolved_attribute(&resolved, &attr, &build_options, true)?
        .context("Build failed")?;

    let main_program = get_package_main_program(flake_dir, &attr)?;
    let exe_path = format!("{}/bin/{}", store_path, main_program);

    // Run the executable
    let mut cmd = std::process::Command::new(&exe_path);
    cmd.args(args);

    tracing::debug!("+ {} {}", exe_path, args.join(" "));

    let status = cmd
        .status()
        .context(format!("Failed to run {}", exe_path))?;

    if !status.success() {
        anyhow::bail!(
            "Command failed with exit code: {}",
            status.code().unwrap_or(1)
        );
    }

    Ok(())
}
