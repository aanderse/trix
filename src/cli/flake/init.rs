//! Flake init command - initialize a new flake in the current directory.

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use tracing::{debug, info};
use walkdir::WalkDir;

// No longer need Evaluator - using nix eval subprocess

#[derive(Args)]
pub struct InitArgs {
    /// Template reference (e.g., templates#trivial, github:NixOS/templates#rust)
    #[arg(short, long, default_value = "templates#trivial")]
    pub template: String,
}

pub fn run(args: InitArgs) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;
    run_template_copy(&cwd, &args.template, false)
}

/// Copy template files to the target directory.
/// If `is_new` is true, this is for `flake new` and files won't be skipped.
/// If `is_new` is false, this is for `flake init` and existing files are skipped.
pub fn run_template_copy(target_dir: &Path, template_ref: &str, is_new: bool) -> Result<()> {
    // Parse template reference (flake_ref#template_name)
    let (flake_ref, template_name) = if let Some(idx) = template_ref.rfind('#') {
        (&template_ref[..idx], &template_ref[idx + 1..])
    } else {
        (template_ref, "default")
    };

    // Expand "templates" shorthand to NixOS/templates
    let flake_ref = if flake_ref == "templates" {
        "github:NixOS/templates"
    } else {
        flake_ref
    };

    info!("Fetching template from {}#{}", flake_ref, template_name);

    // Prefetch the flake to get its store path
    let output = Command::new("nix")
        .args(["flake", "prefetch", "--json", flake_ref])
        .output()
        .context("failed to run nix flake prefetch")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("failed to prefetch flake: {}", stderr.trim()));
    }

    let prefetch_info: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("failed to parse prefetch output")?;

    let flake_store_path = prefetch_info["storePath"]
        .as_str()
        .context("could not determine flake store path")?;

    let flake_path = Path::new(flake_store_path);
    let flake_nix_path = flake_path.join("flake.nix");

    if !flake_nix_path.exists() {
        return Err(anyhow!("no flake.nix found in {}", flake_store_path));
    }

    debug!(store_path = %flake_store_path, "prefetched flake");

    // Build the attribute path for the template
    let template_attr = if template_name == "default" {
        vec!["defaultTemplate".to_string()]
    } else {
        vec!["templates".to_string(), template_name.to_string()]
    };

    debug!(attr = %template_attr.join("."), "evaluating template");

    // Evaluate the template path using nix eval
    let template_path_expr = format!("{}#{}.path", flake_store_path, template_attr.join("."));
    let path_output = Command::new("nix")
        .args(["eval", "--raw", &template_path_expr])
        .output()
        .context("failed to run nix eval for template path")?;

    if !path_output.status.success() {
        let stderr = String::from_utf8_lossy(&path_output.stderr);
        return Err(anyhow!(
            "template '{}' not found: {}",
            template_name,
            stderr.trim()
        ));
    }

    let template_path_str = String::from_utf8_lossy(&path_output.stdout).to_string();
    let template_path = Path::new(&template_path_str);

    debug!(template_path = %template_path_str, "got template path");

    if !template_path.exists() {
        return Err(anyhow!("template path does not exist: {}", template_path_str));
    }

    // Get optional description using nix eval
    let desc_expr = format!("{}#{}.description or null", flake_store_path, template_attr.join("."));
    let description = Command::new("nix")
        .args(["eval", "--raw", &desc_expr])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).to_string();
            if s.is_empty() || s == "null" {
                None
            } else {
                Some(s)
            }
        });

    // Get optional welcomeText using nix eval
    let welcome_expr = format!("{}#{}.welcomeText or null", flake_store_path, template_attr.join("."));
    let welcome_text = Command::new("nix")
        .args(["eval", "--raw", &welcome_expr])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).to_string();
            if s.is_empty() || s == "null" {
                None
            } else {
                Some(s)
            }
        });

    if let Some(ref desc) = description {
        debug!(description = %desc, "template description");
    }

    // Copy files from template
    let mut copied_count = 0;
    let mut skipped_count = 0;

    for entry in WalkDir::new(template_path) {
        let entry = entry.context("failed to walk template directory")?;
        if entry.file_type().is_file() {
            let rel_path = entry
                .path()
                .strip_prefix(template_path)
                .context("failed to get relative path")?;
            let dest_file = target_dir.join(rel_path);

            if dest_file.exists() && !is_new {
                skipped_count += 1;
                continue;
            }

            if let Some(parent) = dest_file.parent() {
                fs::create_dir_all(parent).context("failed to create parent directory")?;
            }

            fs::copy(entry.path(), &dest_file).context("failed to copy file")?;

            // Make writable (files from store are read-only)
            let mut perms = fs::metadata(&dest_file)?.permissions();
            perms.set_mode(perms.mode() | 0o200);
            fs::set_permissions(&dest_file, perms)?;

            copied_count += 1;
            debug!("wrote: {}", rel_path.display());
        }
    }

    if copied_count > 0 {
        if is_new {
            println!("Created {} in {}", template_ref, target_dir.display());
        } else {
            println!("Initialized {} in current directory", template_ref);
        }
    }

    if skipped_count > 0 {
        println!("(skipped {} existing files)", skipped_count);
    }

    if let Some(text) = welcome_text {
        if !text.is_empty() {
            println!("\n{}", text);
        }
    }

    Ok(())
}
