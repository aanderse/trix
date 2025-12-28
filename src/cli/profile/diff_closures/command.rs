use super::common::{
    format_size, format_size_diff, get_closure, get_store_path_size, group_by_package,
};
use anyhow::Result;

/// Show closure difference between profile versions
pub fn cmd_diff_closures() -> Result<()> {
    let profile_dir = crate::profile::get_profile_dir()?;

    let mut generations = Vec::new();
    for entry in std::fs::read_dir(&profile_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if let Some(num) = crate::profile::parse_generation_number(&name_str) {
            if let Ok(target) = std::fs::read_link(entry.path()) {
                generations.push((num, target));
            }
        }
    }

    if generations.len() < 2 {
        println!("Need at least 2 generations to show differences.");
        return Ok(());
    }

    generations.sort_by_key(|(num, _)| *num);

    for i in 1..generations.len() {
        let (prev_num, prev_target) = &generations[i - 1];
        let (curr_num, curr_target) = &generations[i];

        let prev_closure = get_closure(&prev_target.to_string_lossy())?;
        let curr_closure = get_closure(&curr_target.to_string_lossy())?;

        let prev_packages = group_by_package(&prev_closure);
        let curr_packages = group_by_package(&curr_closure);

        let mut changes = Vec::new();
        let mut all_names: std::collections::BTreeSet<_> = prev_packages.keys().collect();
        all_names.extend(curr_packages.keys());

        for name in all_names {
            if name == "profile" || name == "user-environment" {
                continue;
            }

            let prev_info = prev_packages.get(name);
            let curr_info = curr_packages.get(name);

            match (prev_info, curr_info) {
                (Some((prev_ver, prev_path)), Some((curr_ver, curr_path))) => {
                    if prev_path != curr_path {
                        let prev_size = get_store_path_size(prev_path).unwrap_or(0);
                        let curr_size = get_store_path_size(curr_path).unwrap_or(0);
                        let diff = curr_size as i64 - prev_size as i64;
                        let size_str = format_size_diff(diff);

                        if prev_ver != curr_ver {
                            changes.push(format!(
                                "  {}: {} → {}, {}",
                                name, prev_ver, curr_ver, size_str
                            ));
                        } else {
                            changes.push(format!("  {}: {}", name, size_str));
                        }
                    }
                }
                (None, Some((curr_ver, curr_path))) => {
                    let size = get_store_path_size(curr_path).unwrap_or(0);
                    // Red+bold for size of added packages (matches Python)
                    let size_str = format!("\x1b[31;1m+{}\x1b[0m", format_size(size));
                    changes.push(format!("  {}: ∅ → {}, {}", name, curr_ver, size_str));
                }
                (Some((prev_ver, prev_path)), None) => {
                    let size = get_store_path_size(prev_path).unwrap_or(0);
                    changes.push(format!(
                        "  {}: {} → ∅, -{}",
                        name,
                        prev_ver,
                        format_size(size)
                    ));
                }
                (None, None) => {}
            }
        }

        if !changes.is_empty() {
            println!("Version {} → {}:", prev_num, curr_num);
            for change in changes {
                println!("{}", change);
            }
            println!();
        }
    }

    Ok(())
}
