use super::common::run_template_copy;
use anyhow::Result;

/// Create a flake in the current directory from a template
pub fn cmd_init(template_ref: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    run_template_copy(&cwd, template_ref, false)
}
