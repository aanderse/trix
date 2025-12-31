//! Shebang script support.
//!
//! Allows trix to be used as a script interpreter, similar to nix-shell.
//!
//! Example script:
//! ```bash
//! #!/usr/bin/env trix
//! #!trix develop -i python3 .#devShell
//!
//! print("Hello from Python!")
//! ```

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Result of parsing a shebang script.
#[derive(Debug)]
pub struct ShebangScript {
    /// The path to the script file.
    pub script_path: String,
    /// Arguments extracted from #!trix lines.
    pub args: Vec<String>,
    /// Global arguments (like -v) that appeared before the script path.
    pub global_args: Vec<String>,
    /// Index of the script in the original args.
    pub script_index: usize,
}

/// Detect if we're being invoked as a shebang interpreter.
///
/// Returns Some(ShebangScript) if an argument is a file that contains
/// `#!trix` directive lines. Also returns the index of the script in the args.
/// Returns None otherwise.
pub fn detect_shebang(args: &[String]) -> Option<ShebangScript> {
    // In shebang mode, args look like: ["trix", "script.sh", "arg1", "arg2", ...]
    // Or with flags: ["trix", "-v", "script.sh", "arg1", "arg2", ...]
    // We need at least the program name and the script path
    if args.len() < 2 {
        return None;
    }

    // Check if the first argument looks like a subcommand rather than a file
    // Common subcommands that shouldn't be treated as scripts
    let subcommands = [
        "build",
        "develop",
        "eval",
        "run",
        "copy",
        "log",
        "repl",
        "why-depends",
        "shell",
        "flake",
        "profile",
        "registry",
        "hash",
        "fmt",
        "completion",
        "-h",
        "--help",
        "-V",
        "--version",
    ];

    // Global flags that can appear before the script
    let global_flags = ["-v", "--verbose"];

    // Find the first non-flag argument that could be a script
    let mut script_index = None;
    let mut global_flag_indices = Vec::new();

    for (i, arg) in args.iter().enumerate().skip(1) {
        if subcommands.contains(&arg.as_str()) {
            // Found a subcommand, not shebang mode
            return None;
        }
        if global_flags.contains(&arg.as_str()) {
            global_flag_indices.push(i);
            continue;
        }
        // This could be the script path
        script_index = Some(i);
        break;
    }

    let script_idx = script_index?;
    let potential_script = &args[script_idx];

    // Check if it's a file path
    let path = Path::new(potential_script);
    if !path.is_file() {
        return None;
    }

    // Try to parse shebang directives from the file
    match parse_shebang_directives(path) {
        Some(directive_args) if !directive_args.is_empty() => {
            // Collect global flags that were specified
            let global_args: Vec<String> = global_flag_indices
                .iter()
                .map(|&i| args[i].clone())
                .collect();

            Some(ShebangScript {
                script_path: potential_script.clone(),
                args: directive_args,
                global_args,
                script_index: script_idx,
            })
        }
        _ => None,
    }
}

/// Parse #!trix directive lines from a script file.
///
/// Reads the file and extracts arguments from lines matching:
/// - `#!trix ...`
/// - `#! trix ...`
///
/// Multiple directive lines are concatenated.
fn parse_shebang_directives(path: &Path) -> Option<Vec<String>> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut all_args = Vec::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let trimmed = line.trim();

        // Stop at first non-comment, non-empty line (actual script content)
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            break;
        }

        // Check for trix directive lines
        // Match: #!trix, #! trix, # !trix, # ! trix
        let directive = if let Some(rest) = trimmed.strip_prefix("#!trix") {
            Some(rest)
        } else if let Some(rest) = trimmed.strip_prefix("#! trix") {
            Some(rest)
        } else if let Some(rest) = trimmed.strip_prefix("# !trix") {
            Some(rest)
        } else { trimmed.strip_prefix("# ! trix") };

        if let Some(rest) = directive {
            // Parse the rest of the line as shell-like arguments
            let args = parse_args(rest.trim());
            all_args.extend(args);
        }
    }

    if all_args.is_empty() {
        None
    } else {
        Some(all_args)
    }
}

/// Parse a string into shell-like arguments.
///
/// Handles basic quoting with single and double quotes.
fn parse_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
            }
            '\\' if in_double_quote => {
                // Handle escape in double quotes
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ' ' | '\t' if !in_single_quote && !in_double_quote => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => {
                current.push(c);
            }
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_args_simple() {
        assert_eq!(parse_args("develop -i python3"), vec!["develop", "-i", "python3"]);
    }

    #[test]
    fn test_parse_args_quoted() {
        assert_eq!(
            parse_args("shell -c 'echo hello'"),
            vec!["shell", "-c", "echo hello"]
        );
        assert_eq!(
            parse_args("shell -c \"echo hello\""),
            vec!["shell", "-c", "echo hello"]
        );
    }

    #[test]
    fn test_parse_args_empty() {
        assert!(parse_args("").is_empty());
        assert!(parse_args("   ").is_empty());
    }

    #[test]
    fn test_parse_shebang_directives() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "#!/usr/bin/env trix").unwrap();
        writeln!(file, "#!trix develop -i python3").unwrap();
        writeln!(file, "#!trix --pure").unwrap();
        writeln!(file, "").unwrap();
        writeln!(file, "print('hello')").unwrap();
        file.flush().unwrap();

        let args = parse_shebang_directives(file.path()).unwrap();
        assert_eq!(args, vec!["develop", "-i", "python3", "--pure"]);
    }

    #[test]
    fn test_parse_shebang_with_space() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "#!/usr/bin/env trix").unwrap();
        writeln!(file, "#! trix shell nixpkgs#hello").unwrap();
        file.flush().unwrap();

        let args = parse_shebang_directives(file.path()).unwrap();
        assert_eq!(args, vec!["shell", "nixpkgs#hello"]);
    }

    #[test]
    fn test_no_shebang_directives() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "#!/bin/bash").unwrap();
        writeln!(file, "echo hello").unwrap();
        file.flush().unwrap();

        let args = parse_shebang_directives(file.path());
        assert!(args.is_none());
    }

    #[test]
    fn test_detect_shebang_with_script() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "#!/usr/bin/env trix").unwrap();
        writeln!(file, "#!trix develop -i bash").unwrap();
        file.flush().unwrap();

        let args = vec![
            "trix".to_string(),
            file.path().to_string_lossy().to_string(),
        ];
        let result = detect_shebang(&args);
        assert!(result.is_some());
        let shebang = result.unwrap();
        assert_eq!(shebang.args, vec!["develop", "-i", "bash"]);
    }

    #[test]
    fn test_detect_shebang_with_subcommand() {
        let args = vec!["trix".to_string(), "build".to_string()];
        assert!(detect_shebang(&args).is_none());
    }

    #[test]
    fn test_detect_shebang_with_global_flag() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "#!/usr/bin/env trix").unwrap();
        writeln!(file, "#!trix develop -i bash").unwrap();
        file.flush().unwrap();

        let args = vec![
            "trix".to_string(),
            "-v".to_string(),
            file.path().to_string_lossy().to_string(),
            "arg1".to_string(),
        ];
        let result = detect_shebang(&args);
        assert!(result.is_some());
        let shebang = result.unwrap();
        assert_eq!(shebang.args, vec!["develop", "-i", "bash"]);
        assert_eq!(shebang.global_args, vec!["-v"]);
        assert_eq!(shebang.script_index, 2);
    }

    #[test]
    fn test_detect_shebang_verbose_before_subcommand() {
        // Ensure -v before a subcommand doesn't trigger shebang detection
        let args = vec!["trix".to_string(), "-v".to_string(), "build".to_string()];
        assert!(detect_shebang(&args).is_none());
    }
}
