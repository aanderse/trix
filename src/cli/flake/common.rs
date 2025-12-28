use anyhow::{Context, Result};
use std::fs;
use std::os::unix::fs::PermissionsExt;

/// Wrap text in ANSI bold codes.
pub fn bold(text: &str) -> String {
    format!("\x1b[1m{}\x1b[0m", text)
}

/// Format a magenta+bold string (for type labels like "Nixpkgs overlay")
pub fn magenta_bold(text: &str) -> String {
    format!("\x1b[35;1m{}\x1b[0m", text)
}

pub fn run_template_copy(
    target_dir: &std::path::Path,
    template_ref: &str,
    is_new: bool,
) -> Result<()> {
    let (flake_ref, template_name) = if let Some(idx) = template_ref.rfind('#') {
        (&template_ref[..idx], &template_ref[idx + 1..])
    } else {
        (template_ref, "default")
    };

    let flake_ref = if flake_ref == "templates" {
        "github:NixOS/templates"
    } else {
        flake_ref
    };

    tracing::info!("Fetching template from {}#{}", flake_ref, template_name);

    // Prefetch flake
    let mut cmd = crate::command::NixCommand::new("nix");
    cmd.args(["flake", "prefetch", "--json", flake_ref]);

    let prefetch_info: serde_json::Value = cmd.json()?;
    let flake_store_path = prefetch_info["storePath"]
        .as_str()
        .context("Could not determine flake store path")?;

    let flake_path = std::path::Path::new(flake_store_path);
    let flake_nix_path = flake_path.join("flake.nix");

    if !flake_nix_path.exists() {
        anyhow::bail!("No flake.nix found in {}", flake_store_path);
    }

    let nix_dir = crate::nix::get_nix_dir()?;
    let lock_expr = crate::nix::get_lock_expr(flake_path);

    // Evaluate template info
    let template_attr = format!("templates.{}", template_name);
    let template_selector = if template_name == "default" {
        format!("outputs.defaultTemplate or outputs.{}", template_attr)
    } else {
        format!("outputs.{}", template_attr)
    };

    let eval_expr_str = format!(
        r#"
    let
      flake = import {};
      lock = {};
      inputs = import {}/inputs.nix {{
        inherit lock;
        flakeDirPath = {};
        selfInfo = {{}};
      }};
      outputs = flake.outputs (inputs // {{ self = inputs.self // outputs; }});
      template = {};
    in "${{template.path}}@@@${{template.description or ""}}@@@${{template.welcomeText or ""}}"
    "#,
        flake_nix_path.display(),
        lock_expr,
        nix_dir.display(),
        flake_path.display(),
        template_selector
    );

    tracing::debug!("Evaluating template info...");

    let mut cmd = crate::command::NixCommand::new("nix-instantiate");
    cmd.args([
        "--eval",
        "--readonly-mode",
        "--eval-store",
        "dummy://",
        "-E",
        &eval_expr_str,
    ]);

    let result_raw = cmd.output()?;
    // Remove surrounding quotes if any
    let result_raw = if result_raw.starts_with('"') && result_raw.ends_with('"') {
        &result_raw[1..result_raw.len() - 1]
    } else {
        &result_raw
    };

    // Unescape backslashes (nix-instantiate escapes them in output)
    let result_raw = result_raw.replace("\\\\", "\\").replace("\\\"", "\"");

    let parts: Vec<&str> = result_raw.split("@@@").collect();
    if parts.len() < 3 {
        anyhow::bail!("Unexpected template info format: {}", result_raw);
    }

    let template_path_str = parts[0];
    let _template_description = parts[1];
    let template_welcome_text = parts[2];

    let template_path = std::path::Path::new(template_path_str);

    if !template_path.exists() {
        anyhow::bail!("Template path does not exist: {}", template_path_str);
    }

    // Copy files
    let mut copied_count = 0;
    let mut skipped_count = 0;

    for entry in walkdir::WalkDir::new(template_path) {
        let entry = entry?;
        if entry.file_type().is_file() {
            let rel_path = entry.path().strip_prefix(template_path)?;
            let dest_file = target_dir.join(rel_path);

            if dest_file.exists() && !is_new {
                skipped_count += 1;
                continue;
            }

            if let Some(parent) = dest_file.parent() {
                fs::create_dir_all(parent)?;
            }

            fs::copy(entry.path(), &dest_file)?;

            // Make writable
            let mut perms = fs::metadata(&dest_file)?.permissions();
            perms.set_mode(perms.mode() | 0o200);
            fs::set_permissions(&dest_file, perms)?;

            copied_count += 1;
            tracing::debug!("  wrote: {}", rel_path.display());
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

    if !template_welcome_text.is_empty() {
        println!("\n{}", template_welcome_text);
    }

    Ok(())
}
