use super::common::build_resolved_attribute;
use anyhow::{Context, Result};
use clap::Args;

#[derive(Args, Clone, Debug)]
pub struct WhyDependsArgs {
    /// Package to check
    pub package: String,
    /// Dependency to trace
    pub dependency: String,
}

/// Show why a package depends on another
/// Show why a package depends on another
pub fn cmd_why_depends(args: WhyDependsArgs) -> Result<()> {
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

    let pkg_path = resolve_to_store_path(&args.package)?;
    let dep_path = resolve_to_store_path(&args.dependency)?;

    let mut cmd = crate::command::NixCommand::new("nix");
    cmd.args(["why-depends", &pkg_path, &dep_path]);

    cmd.run()
}
