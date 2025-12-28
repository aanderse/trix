use super::common::run_template_copy;
use anyhow::{Context, Result};

/// Create a new directory with a flake from a template
pub fn cmd_new(path: &str, template_ref: &str) -> Result<()> {
    let target_dir = std::path::Path::new(path);
    if target_dir.exists() {
        anyhow::bail!("Directory already exists: {}", path);
    }

    std::fs::create_dir_all(target_dir).context("Failed to create directory")?;

    match run_template_copy(target_dir, template_ref, true) {
        Ok(_) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_dir(target_dir);
            Err(e)
        }
    }
}
