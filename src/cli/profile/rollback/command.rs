use anyhow::Result;

/// Roll back to the previous profile generation
pub fn cmd_rollback() -> Result<()> {
    let profile_dir = crate::profile::get_profile_dir()?;
    let current_path = crate::profile::get_current_profile_path()?;

    let mut generations: Vec<(u32, std::path::PathBuf)> = Vec::new();

    for entry in std::fs::read_dir(&profile_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with("profile-") && name_str.ends_with("-link") {
            if let Some(gen) = crate::profile::parse_generation_number(&name_str) {
                generations.push((gen, entry.path()));
            }
        }
    }

    generations.sort_by_key(|(gen, _)| *gen);

    // Find current generation
    let current_gen = generations
        .iter()
        .find(|(_, path)| std::fs::read_link(path).ok() == Some(current_path.clone()));

    if let Some((current_gen_num, _)) = current_gen {
        // Find previous generation
        let prev = generations
            .iter()
            .rev()
            .find(|(gen, _)| gen < current_gen_num);

        if let Some((prev_gen, prev_path)) = prev {
            let prev_target = std::fs::read_link(prev_path)?;
            crate::profile::switch_profile(&prev_target.display().to_string())?;
            println!("Rolled back to generation {}", prev_gen);
            return Ok(());
        }
    }

    anyhow::bail!("No previous generation to roll back to.");
}
