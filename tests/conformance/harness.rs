//! Test harness for conformance testing.
//!
//! Provides infrastructure to run commands against both trix and nix,
//! and compare the results using various strategies.

use regex::Regex;
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// Result of running a command.
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub duration: Duration,
}

impl CommandResult {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Strategy for comparing trix and nix outputs.
#[derive(Debug, Clone)]
pub enum ComparisonStrategy {
    /// Exact string match (rarely used).
    Exact,

    /// JSON semantic comparison - parses JSON and compares values,
    /// ignoring formatting and key order.
    JsonSemantic {
        /// Fields to ignore during comparison (e.g., "path", "lastModified").
        ignore_fields: Vec<&'static str>,
    },

    /// Text comparison with normalization.
    TextNormalized {
        /// Replace store paths with normalized placeholders.
        normalize_store_paths: bool,
        /// Additional regex patterns to remove before comparison.
        ignore_patterns: Vec<&'static str>,
    },

    /// Only compare exit codes.
    ExitCodeOnly,

    /// Both must succeed or both must fail.
    SuccessMatch,
}

/// Result of comparing trix and nix outputs.
#[derive(Debug)]
pub enum ComparisonResult {
    /// Outputs match according to the strategy.
    Match,

    /// Outputs differ.
    Mismatch {
        field: String,
        trix_value: String,
        nix_value: String,
    },

    /// Trix succeeded but nix failed.
    TrixSucceededNixFailed {
        trix_stdout: String,
        nix_stderr: String,
    },

    /// Nix succeeded but trix failed.
    NixSucceededTrixFailed {
        nix_stdout: String,
        trix_stderr: String,
    },
}

impl ComparisonResult {
    pub fn is_match(&self) -> bool {
        matches!(self, ComparisonResult::Match)
    }
}

/// Test harness for conformance testing.
pub struct ConformanceHarness {
    trix_bin: String,
}

impl Default for ConformanceHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl ConformanceHarness {
    /// Create a new test harness.
    pub fn new() -> Self {
        let trix_bin =
            std::env::var("CARGO_BIN_EXE_trix").unwrap_or_else(|_| "target/debug/trix".to_string());
        Self { trix_bin }
    }

    /// Run a command with trix.
    pub fn run_trix(&self, args: &[&str]) -> CommandResult {
        self.run_command(&self.trix_bin, args)
    }

    /// Run a command with nix.
    pub fn run_nix(&self, args: &[&str]) -> CommandResult {
        self.run_command("nix", args)
    }

    /// Run a command and capture the result.
    fn run_command(&self, program: &str, args: &[&str]) -> CommandResult {
        let start = Instant::now();
        let output = Command::new(program)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("Failed to run {}: {}", program, e));

        CommandResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            duration: start.elapsed(),
        }
    }

    /// Run the same command against both trix and nix.
    pub fn run_both(&self, args: &[&str]) -> (CommandResult, CommandResult) {
        let trix_result = self.run_trix(args);
        let nix_result = self.run_nix(args);
        (trix_result, nix_result)
    }

    /// Compare trix and nix results using the specified strategy.
    pub fn compare(
        &self,
        trix: &CommandResult,
        nix: &CommandResult,
        strategy: &ComparisonStrategy,
    ) -> ComparisonResult {
        match strategy {
            ComparisonStrategy::Exact => self.compare_exact(trix, nix),
            ComparisonStrategy::JsonSemantic { ignore_fields } => {
                self.compare_json_semantic(trix, nix, ignore_fields)
            }
            ComparisonStrategy::TextNormalized {
                normalize_store_paths,
                ignore_patterns,
            } => self.compare_text_normalized(trix, nix, *normalize_store_paths, ignore_patterns),
            ComparisonStrategy::ExitCodeOnly => self.compare_exit_code(trix, nix),
            ComparisonStrategy::SuccessMatch => self.compare_success_match(trix, nix),
        }
    }

    /// Run command against both and compare, returning the comparison result.
    pub fn test_conformance(
        &self,
        args: &[&str],
        strategy: &ComparisonStrategy,
    ) -> (CommandResult, CommandResult, ComparisonResult) {
        let (trix, nix) = self.run_both(args);
        let result = self.compare(&trix, &nix, strategy);
        (trix, nix, result)
    }

    /// Run command against both, compare, and assert they match.
    /// Panics with detailed output if they don't match.
    pub fn assert_conformance(&self, args: &[&str], strategy: &ComparisonStrategy) {
        let (trix, nix, result) = self.test_conformance(args, strategy);

        if !result.is_match() {
            panic!(
                "\n\
                 ========== CONFORMANCE FAILURE ==========\n\
                 Command: {}\n\
                 Strategy: {:?}\n\
                 \n\
                 --- Trix (exit {}) ---\n\
                 stdout:\n{}\n\
                 stderr:\n{}\n\
                 \n\
                 --- Nix (exit {}) ---\n\
                 stdout:\n{}\n\
                 stderr:\n{}\n\
                 \n\
                 --- Comparison Result ---\n\
                 {:?}\n\
                 ==========================================",
                args.join(" "),
                strategy,
                trix.exit_code,
                trix.stdout.trim(),
                trix.stderr.trim(),
                nix.exit_code,
                nix.stdout.trim(),
                nix.stderr.trim(),
                result,
            );
        }
    }

    // --- Comparison implementations ---

    fn compare_exact(&self, trix: &CommandResult, nix: &CommandResult) -> ComparisonResult {
        // First check success/failure
        if trix.success() != nix.success() {
            return self.success_failure_mismatch(trix, nix);
        }

        if trix.stdout.trim() == nix.stdout.trim() {
            ComparisonResult::Match
        } else {
            ComparisonResult::Mismatch {
                field: "stdout".to_string(),
                trix_value: trix.stdout.clone(),
                nix_value: nix.stdout.clone(),
            }
        }
    }

    fn compare_json_semantic(
        &self,
        trix: &CommandResult,
        nix: &CommandResult,
        ignore_fields: &[&str],
    ) -> ComparisonResult {
        // First check success/failure
        if trix.success() != nix.success() {
            return self.success_failure_mismatch(trix, nix);
        }

        // If both failed, that's a match
        if !trix.success() && !nix.success() {
            return ComparisonResult::Match;
        }

        // Parse JSON
        let trix_json: Value = match serde_json::from_str(trix.stdout.trim()) {
            Ok(v) => v,
            Err(e) => {
                return ComparisonResult::Mismatch {
                    field: "json_parse".to_string(),
                    trix_value: format!("parse error: {}", e),
                    nix_value: "valid json".to_string(),
                }
            }
        };

        let nix_json: Value = match serde_json::from_str(nix.stdout.trim()) {
            Ok(v) => v,
            Err(e) => {
                return ComparisonResult::Mismatch {
                    field: "json_parse".to_string(),
                    trix_value: "valid json".to_string(),
                    nix_value: format!("parse error: {}", e),
                }
            }
        };

        // Compare JSON values
        self.compare_json_values(&trix_json, &nix_json, "", ignore_fields)
    }

    fn compare_json_values(
        &self,
        trix: &Value,
        nix: &Value,
        path: &str,
        ignore_fields: &[&str],
    ) -> ComparisonResult {
        // Check if this field should be ignored
        let field_name = path.split('.').last().unwrap_or("");
        if ignore_fields.contains(&field_name) {
            return ComparisonResult::Match;
        }

        match (trix, nix) {
            (Value::Object(trix_obj), Value::Object(nix_obj)) => {
                // Check for missing keys in trix
                for key in nix_obj.keys() {
                    if ignore_fields.contains(&key.as_str()) {
                        continue;
                    }
                    if !trix_obj.contains_key(key) {
                        return ComparisonResult::Mismatch {
                            field: format!("{}.{}", path, key),
                            trix_value: "<missing>".to_string(),
                            nix_value: nix_obj[key].to_string(),
                        };
                    }
                }

                // Check for extra keys in trix
                for key in trix_obj.keys() {
                    if ignore_fields.contains(&key.as_str()) {
                        continue;
                    }
                    if !nix_obj.contains_key(key) {
                        return ComparisonResult::Mismatch {
                            field: format!("{}.{}", path, key),
                            trix_value: trix_obj[key].to_string(),
                            nix_value: "<missing>".to_string(),
                        };
                    }
                }

                // Compare values recursively
                for (key, trix_val) in trix_obj {
                    if ignore_fields.contains(&key.as_str()) {
                        continue;
                    }
                    let nix_val = &nix_obj[key];
                    let new_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", path, key)
                    };
                    let result = self.compare_json_values(trix_val, nix_val, &new_path, ignore_fields);
                    if !result.is_match() {
                        return result;
                    }
                }
                ComparisonResult::Match
            }
            (Value::Array(trix_arr), Value::Array(nix_arr)) => {
                if trix_arr.len() != nix_arr.len() {
                    return ComparisonResult::Mismatch {
                        field: format!("{} (length)", path),
                        trix_value: trix_arr.len().to_string(),
                        nix_value: nix_arr.len().to_string(),
                    };
                }
                for (i, (trix_val, nix_val)) in trix_arr.iter().zip(nix_arr.iter()).enumerate() {
                    let new_path = format!("{}[{}]", path, i);
                    let result = self.compare_json_values(trix_val, nix_val, &new_path, ignore_fields);
                    if !result.is_match() {
                        return result;
                    }
                }
                ComparisonResult::Match
            }
            _ => {
                if trix == nix {
                    ComparisonResult::Match
                } else {
                    ComparisonResult::Mismatch {
                        field: path.to_string(),
                        trix_value: trix.to_string(),
                        nix_value: nix.to_string(),
                    }
                }
            }
        }
    }

    fn compare_text_normalized(
        &self,
        trix: &CommandResult,
        nix: &CommandResult,
        normalize_store_paths: bool,
        ignore_patterns: &[&str],
    ) -> ComparisonResult {
        // First check success/failure
        if trix.success() != nix.success() {
            return self.success_failure_mismatch(trix, nix);
        }

        // If both failed, that's a match
        if !trix.success() && !nix.success() {
            return ComparisonResult::Match;
        }

        let mut trix_text = trix.stdout.clone();
        let mut nix_text = nix.stdout.clone();

        // Strip ANSI escape codes
        let ansi_re = Regex::new(r"\x1b\[[0-9;]*m").unwrap();
        trix_text = ansi_re.replace_all(&trix_text, "").to_string();
        nix_text = ansi_re.replace_all(&nix_text, "").to_string();

        // Normalize store paths
        if normalize_store_paths {
            let store_path_re = Regex::new(r"/nix/store/[a-z0-9]{32}-").unwrap();
            trix_text = store_path_re
                .replace_all(&trix_text, "/nix/store/HASH-")
                .to_string();
            nix_text = store_path_re
                .replace_all(&nix_text, "/nix/store/HASH-")
                .to_string();
        }

        // Apply ignore patterns
        for pattern in ignore_patterns {
            if let Ok(re) = Regex::new(pattern) {
                trix_text = re.replace_all(&trix_text, "").to_string();
                nix_text = re.replace_all(&nix_text, "").to_string();
            }
        }

        // Normalize whitespace
        trix_text = trix_text.trim().to_string();
        nix_text = nix_text.trim().to_string();

        if trix_text == nix_text {
            ComparisonResult::Match
        } else {
            ComparisonResult::Mismatch {
                field: "normalized_text".to_string(),
                trix_value: trix_text,
                nix_value: nix_text,
            }
        }
    }

    fn compare_exit_code(&self, trix: &CommandResult, nix: &CommandResult) -> ComparisonResult {
        if trix.exit_code == nix.exit_code {
            ComparisonResult::Match
        } else {
            ComparisonResult::Mismatch {
                field: "exit_code".to_string(),
                trix_value: trix.exit_code.to_string(),
                nix_value: nix.exit_code.to_string(),
            }
        }
    }

    fn compare_success_match(&self, trix: &CommandResult, nix: &CommandResult) -> ComparisonResult {
        if trix.success() == nix.success() {
            ComparisonResult::Match
        } else {
            self.success_failure_mismatch(trix, nix)
        }
    }

    fn success_failure_mismatch(
        &self,
        trix: &CommandResult,
        nix: &CommandResult,
    ) -> ComparisonResult {
        if trix.success() && !nix.success() {
            ComparisonResult::TrixSucceededNixFailed {
                trix_stdout: trix.stdout.clone(),
                nix_stderr: nix.stderr.clone(),
            }
        } else {
            ComparisonResult::NixSucceededTrixFailed {
                nix_stdout: nix.stdout.clone(),
                trix_stderr: trix.stderr.clone(),
            }
        }
    }
}

/// Helper to run trix on a specific directory/flake.
pub fn trix_command_in(dir: &Path, args: &[&str]) -> CommandResult {
    let harness = ConformanceHarness::new();
    let mut full_args: Vec<&str> = args.to_vec();

    // If the command takes a path argument, we need to append it
    // This is a simplified helper - for more complex cases, build args manually
    full_args.push(dir.to_str().unwrap());

    harness.run_trix(&full_args)
}

/// Helper to run nix on a specific directory/flake.
pub fn nix_command_in(dir: &Path, args: &[&str]) -> CommandResult {
    let harness = ConformanceHarness::new();
    let mut full_args: Vec<&str> = args.to_vec();
    full_args.push(dir.to_str().unwrap());
    harness.run_nix(&full_args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_comparison_equal() {
        let harness = ConformanceHarness::new();
        let trix = CommandResult {
            stdout: r#"{"a": 1, "b": 2}"#.to_string(),
            stderr: String::new(),
            exit_code: 0,
            duration: Duration::ZERO,
        };
        let nix = CommandResult {
            stdout: r#"{"b": 2, "a": 1}"#.to_string(), // Different order
            stderr: String::new(),
            exit_code: 0,
            duration: Duration::ZERO,
        };

        let result = harness.compare(
            &trix,
            &nix,
            &ComparisonStrategy::JsonSemantic {
                ignore_fields: vec![],
            },
        );
        assert!(result.is_match());
    }

    #[test]
    fn test_json_comparison_different() {
        let harness = ConformanceHarness::new();
        let trix = CommandResult {
            stdout: r#"{"a": 1}"#.to_string(),
            stderr: String::new(),
            exit_code: 0,
            duration: Duration::ZERO,
        };
        let nix = CommandResult {
            stdout: r#"{"a": 2}"#.to_string(),
            stderr: String::new(),
            exit_code: 0,
            duration: Duration::ZERO,
        };

        let result = harness.compare(
            &trix,
            &nix,
            &ComparisonStrategy::JsonSemantic {
                ignore_fields: vec![],
            },
        );
        assert!(!result.is_match());
    }

    #[test]
    fn test_json_comparison_ignore_field() {
        let harness = ConformanceHarness::new();
        let trix = CommandResult {
            stdout: r#"{"a": 1, "path": "/foo"}"#.to_string(),
            stderr: String::new(),
            exit_code: 0,
            duration: Duration::ZERO,
        };
        let nix = CommandResult {
            stdout: r#"{"a": 1, "path": "/bar"}"#.to_string(),
            stderr: String::new(),
            exit_code: 0,
            duration: Duration::ZERO,
        };

        let result = harness.compare(
            &trix,
            &nix,
            &ComparisonStrategy::JsonSemantic {
                ignore_fields: vec!["path"],
            },
        );
        assert!(result.is_match());
    }

    #[test]
    fn test_store_path_normalization() {
        let harness = ConformanceHarness::new();
        let trix = CommandResult {
            stdout: "/nix/store/abc123def456abc123def456abc123de-foo\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
            duration: Duration::ZERO,
        };
        let nix = CommandResult {
            stdout: "/nix/store/xyz789xyz789xyz789xyz789xyz789xy-foo\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
            duration: Duration::ZERO,
        };

        let result = harness.compare(
            &trix,
            &nix,
            &ComparisonStrategy::TextNormalized {
                normalize_store_paths: true,
                ignore_patterns: vec![],
            },
        );
        assert!(result.is_match());
    }
}
