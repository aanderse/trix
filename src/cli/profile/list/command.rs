use crate::profile::list_installed;
use anyhow::Result;

/// List installed packages
pub fn cmd_list(output_json: bool) -> Result<()> {
    let mut elements = list_installed()?;

    // Sort alphabetically by name (matches nix profile list)
    elements.sort_by(|(a, _), (b, _)| a.cmp(b));

    if output_json {
        // For JSON output, just output the elements without names
        let elems: Vec<_> = elements.iter().map(|(_, e)| e).collect();
        let json = serde_json::to_string_pretty(&elems)?;
        println!("{}", json);
        return Ok(());
    }

    if elements.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }

    for (i, (name, elem)) in elements.iter().enumerate() {
        if i > 0 {
            println!();
        }

        // Name in bold
        println!("Name:               \x1b[1m{}\x1b[0m", name);

        if let Some(ref attr_path) = elem.attr_path {
            println!("Flake attribute:    {}", attr_path);
        }

        if let Some(ref original_url) = elem.original_url {
            println!("Original flake URL: {}", original_url);
        }

        if let Some(ref url) = elem.url {
            println!("Locked flake URL:   {}", url);
        }

        if !elem.store_paths.is_empty() {
            println!("Store paths:        {}", elem.store_paths[0]);
            for path in &elem.store_paths[1..] {
                println!("                    {}", path);
            }
        }
    }

    Ok(())
}
