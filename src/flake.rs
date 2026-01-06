//! Flake discovery and resolution.
//!
//! This module provides high-level operations for working with flakes:
//! - Discovering flake.nix files in paths
//! - Loading and parsing flake.lock files
//! - Resolving installable references to concrete paths and attributes

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use tracing::{debug, instrument, trace};

use crate::flake_url::{parse_flake_url, FlakeRef};
use crate::lock::FlakeLock;
use crate::registry::{registry_entry_to_flake_ref, resolve_registry_name};

/// Result of resolving an installable reference.
///
/// Either local (path is set) or remote (flake_ref is set).
#[derive(Debug, Clone)]
pub struct ResolvedInstallable {
    /// Whether this is a local flake
    pub is_local: bool,
    /// Attribute part after the # (e.g., "hello" from ".#hello")
    pub attribute: Vec<String>,
    /// For local flakes: path to the flake directory
    pub path: Option<PathBuf>,
    /// For local flakes: parsed flake.lock, if present
    pub lock: Option<FlakeLock>,
    /// For remote refs: the flake reference string (e.g., "github:NixOS/nixpkgs")
    pub flake_ref: Option<String>,
}

impl ResolvedInstallable {
    /// Get the local path, panics if not local
    pub fn local_path(&self) -> &Path {
        self.path.as_ref().expect("not a local flake")
    }

    /// Get the flake.lock, if present
    pub fn get_lock(&self) -> Option<&FlakeLock> {
        self.lock.as_ref()
    }

    /// Build the full installable string for nix commands
    pub fn to_installable_string(&self) -> String {
        let ref_part = if let Some(ref path) = self.path {
            format!("path:{}", path.display())
        } else if let Some(ref flake_ref) = self.flake_ref {
            flake_ref.clone()
        } else {
            ".".to_string()
        };

        if self.attribute.is_empty() {
            ref_part
        } else {
            format!("{}#{}", ref_part, self.attribute.join("."))
        }
    }
}

/// A resolved flake ready for evaluation.
#[derive(Debug)]
pub struct ResolvedFlake {
    /// Absolute path to the flake directory (containing flake.nix)
    pub path: PathBuf,
    /// Parsed flake.lock, if present
    pub lock: Option<FlakeLock>,
    /// Attribute path to evaluate (e.g., ["packages", "x86_64-linux", "default"])
    pub attribute: Vec<String>,
}

impl ResolvedFlake {
    /// Get the path to flake.nix
    pub fn flake_nix_path(&self) -> PathBuf {
        self.path.join("flake.nix")
    }

    /// Get the path to flake.lock
    pub fn flake_lock_path(&self) -> PathBuf {
        self.path.join("flake.lock")
    }
}

/// Find the flake root directory starting from a path.
///
/// If the path is a directory, looks for flake.nix in it.
/// If the path is a file named flake.nix, uses its parent directory.
/// Returns the directory containing flake.nix.
#[instrument(level = "debug", fields(path = %path.display()))]
pub fn find_flake_root(path: &Path) -> Result<PathBuf> {
    trace!("looking for flake root");
    let path = path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize path: {}", path.display()))?;

    if path.is_file() {
        // If it's flake.nix, use its parent
        if path.file_name().map(|n| n == "flake.nix").unwrap_or(false) {
            return path
                .parent()
                .map(|p| p.to_path_buf())
                .ok_or_else(|| anyhow!("flake.nix has no parent directory"));
        }
        bail!("path is a file but not flake.nix: {}", path.display());
    }

    if path.is_dir() {
        let flake_nix = path.join("flake.nix");
        if flake_nix.exists() {
            return Ok(path);
        }
        bail!("no flake.nix found in: {}", path.display());
    }

    bail!("path does not exist: {}", path.display());
}

/// Load a flake.lock file from a flake directory.
///
/// Returns None if flake.lock doesn't exist.
#[instrument(level = "debug", fields(dir = %flake_dir.display()))]
pub fn load_lock(flake_dir: &Path) -> Result<Option<FlakeLock>> {
    let lock_path = flake_dir.join("flake.lock");

    if !lock_path.exists() {
        debug!("no flake.lock found");
        return Ok(None);
    }

    debug!("loading flake.lock");
    let content = std::fs::read_to_string(&lock_path)
        .with_context(|| format!("failed to read flake.lock: {}", lock_path.display()))?;

    let lock = FlakeLock::parse(&content)
        .with_context(|| format!("failed to parse flake.lock: {}", lock_path.display()))?;

    debug!(nodes = lock.nodes.len(), "loaded flake.lock");
    Ok(Some(lock))
}

/// Resolve an installable string to a flake and attribute.
///
/// Handles various forms:
/// - `.` or `.#foo` - current directory
/// - `./path` or `./path#foo` - relative path
/// - `/absolute/path` or `/absolute/path#foo` - absolute path
/// - `github:owner/repo#foo` - (not yet supported for local resolution)
#[instrument(level = "debug", fields(installable = %installable))]
pub fn resolve_installable(installable: &str, cwd: &Path) -> Result<ResolvedFlake> {
    debug!("resolving installable");
    let url = parse_flake_url(installable)
        .map_err(|e| anyhow!("{}", e))
        .with_context(|| format!("failed to parse installable: {}", installable))?;
    trace!(?url, "parsed flake URL");

    // Get the flake path from the reference
    let flake_path = match &url.flake_ref {
        FlakeRef::Path { path, .. } => {
            let path = if path.starts_with('/') {
                PathBuf::from(path)
            } else {
                cwd.join(path)
            };
            find_flake_root(&path)?
        }

        FlakeRef::GitHub { .. }
        | FlakeRef::GitLab { .. }
        | FlakeRef::Sourcehut { .. }
        | FlakeRef::Git { .. }
        | FlakeRef::Tarball { .. }
        | FlakeRef::File { .. }
        | FlakeRef::Indirect { .. } => {
            // These require fetching - not yet implemented
            bail!(
                "remote flake references not yet supported: {}",
                installable
            );
        }
    };

    // Load the lock file
    let lock = load_lock(&flake_path)?;

    // Parse the attribute path
    let attribute: Vec<String> = match &url.attribute {
        Some(attr) if !attr.is_empty() => attr.split('.').map(String::from).collect(),
        _ => Vec::new(),
    };

    debug!(path = %flake_path.display(), ?attribute, "resolved installable");
    Ok(ResolvedFlake {
        path: flake_path,
        lock,
        attribute,
    })
}

/// Resolve an installable string, handling both local and remote refs.
///
/// For local flakes (., ./path, /absolute/path), resolves to path and lock.
/// For remote flakes (github:, nixpkgs, etc.), stores the reference for passthrough.
#[instrument(level = "debug", fields(installable = %installable))]
pub fn resolve_installable_any(installable: &str, cwd: &Path) -> ResolvedInstallable {
    debug!("resolving installable (any)");

    // Parse the installable to separate ref from attribute
    let (ref_part, attr_str) = if let Some((r, a)) = installable.split_once('#') {
        (r, a.to_string())
    } else {
        (installable, String::new())
    };

    // Parse attribute path
    let attribute: Vec<String> = if attr_str.is_empty() {
        Vec::new()
    } else {
        attr_str.split('.').map(String::from).collect()
    };

    // Case 1: Empty or current directory
    if ref_part.is_empty() || ref_part == "." {
        let flake_dir = cwd.to_path_buf();
        let lock = load_lock(&flake_dir).ok().flatten();
        return ResolvedInstallable {
            is_local: true,
            attribute,
            path: Some(flake_dir),
            lock,
            flake_ref: None,
        };
    }

    // Case 2: Explicit path (starts with /, ./, ../, ~, or path:)
    if ref_part.starts_with('/')
        || ref_part.starts_with("./")
        || ref_part.starts_with("../")
        || ref_part.starts_with('~')
        || ref_part.starts_with("path:")
    {
        let path_str = ref_part.strip_prefix("path:").unwrap_or(ref_part);
        let expanded = shellexpand::tilde(path_str).to_string();
        let path = if expanded.starts_with('/') {
            PathBuf::from(&expanded)
        } else {
            cwd.join(&expanded)
        };

        match find_flake_root(&path) {
            Ok(flake_dir) => {
                let lock = load_lock(&flake_dir).ok().flatten();
                return ResolvedInstallable {
                    is_local: true,
                    attribute,
                    path: Some(flake_dir),
                    lock,
                    flake_ref: None,
                };
            }
            Err(e) => {
                debug!("failed to resolve local path: {}", e);
                // Fall through to treat as remote
            }
        }
    }

    // Case 3: Full flake reference (github:, git+, etc.)
    if ref_part.contains(':') {
        return ResolvedInstallable {
            is_local: false,
            attribute,
            path: None,
            lock: None,
            flake_ref: Some(ref_part.to_string()),
        };
    }

    // Case 4: Registry name (e.g., "nixpkgs", "home-manager")
    if let Some(entry) = resolve_registry_name(ref_part, true) {
        debug!("found '{}' in registry: {:?}", ref_part, entry);

        if entry.entry_type == "path" {
            // Local path from registry
            if let Some(ref path_str) = entry.path {
                let expanded = shellexpand::tilde(path_str).to_string();
                let path = PathBuf::from(&expanded);
                if let Ok(flake_dir) = path.canonicalize() {
                    let lock = load_lock(&flake_dir).ok().flatten();
                    return ResolvedInstallable {
                        is_local: true,
                        attribute,
                        path: Some(flake_dir),
                        lock,
                        flake_ref: None,
                    };
                }
            }
        }

        // Remote ref from registry - build the flake reference
        let flake_ref = registry_entry_to_flake_ref(&entry);
        debug!("registry '{}' resolved to remote ref: {}", ref_part, flake_ref);
        return ResolvedInstallable {
            is_local: false,
            attribute,
            path: None,
            lock: None,
            flake_ref: Some(flake_ref),
        };
    }

    // Fallback: treat as indirect/remote reference
    debug!("treating '{}' as indirect reference", ref_part);
    ResolvedInstallable {
        is_local: false,
        attribute,
        path: None,
        lock: None,
        flake_ref: Some(ref_part.to_string()),
    }
}

/// Get the current system identifier (e.g., "x86_64-linux").
pub fn current_system() -> Result<String> {
    // Use Rust's target triple info
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;

    let nix_arch = match arch {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        "x86" => "i686",
        "arm" => "armv7l",
        _ => bail!("unsupported architecture: {}", arch),
    };

    let nix_os = match os {
        "linux" => "linux",
        "macos" => "darwin",
        _ => bail!("unsupported OS: {}", os),
    };

    Ok(format!("{}-{}", nix_arch, nix_os))
}

/// Known output categories that have per-system structure.
const PER_SYSTEM_CATEGORIES: &[&str] = &[
    "packages",
    "devShells",
    "apps",
    "checks",
    "legacyPackages",
    "formatter",
];

/// Known output categories that are NOT per-system.
const TOP_LEVEL_CATEGORIES: &[&str] = &[
    "overlays",
    "nixosModules",
    "nixosConfigurations",
    "darwinModules",
    "darwinConfigurations",
    "homeModules",
    "homeConfigurations",
    "templates",
    "lib",
];

/// Operation context for attribute expansion.
///
/// Different commands have different default output types and search orders.
/// This determines where to look when the user doesn't specify a full path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationContext {
    /// Building packages: tries packages, then legacyPackages
    Build,
    /// Running executables: tries apps, then packages, then legacyPackages
    Run,
    /// Entering dev shells: tries devShells only
    Develop,
    /// Running checks: tries checks only
    Check,
    /// Evaluating arbitrary attributes: no default category, just inserts system if needed
    Eval,
}

impl OperationContext {
    /// Get the ordered list of output categories to search.
    /// Returns None for Eval context (no default categories).
    fn search_categories(&self) -> Option<&'static [&'static str]> {
        match self {
            OperationContext::Build => Some(&["packages", "legacyPackages"]),
            OperationContext::Run => Some(&["apps", "packages", "legacyPackages"]),
            OperationContext::Develop => Some(&["devShells"]),
            OperationContext::Check => Some(&["checks"]),
            OperationContext::Eval => None,
        }
    }
}

/// Check if a string looks like a Nix system identifier (e.g., "x86_64-linux").
pub fn looks_like_system(s: &str) -> bool {
    // System identifiers have the pattern: arch-os (both parts contain no hyphens themselves
    // except for special cases like aarch64-darwin)
    // Simple heuristic: contains exactly one hyphen and matches known patterns
    let parts: Vec<_> = s.split('-').collect();
    if parts.len() != 2 {
        return false;
    }
    // Check if it looks like a known system pattern
    matches!(
        s,
        "x86_64-linux"
            | "aarch64-linux"
            | "x86_64-darwin"
            | "aarch64-darwin"
            | "i686-linux"
            | "armv7l-linux"
    )
}

/// Expand an attribute path based on operation context.
///
/// Returns an ordered list of candidate attribute paths to try. The caller should
/// attempt each in order until one successfully evaluates.
///
/// # Behavior by context
///
/// - **Build**: tries packages, then legacyPackages
/// - **Run**: tries apps, then packages, then legacyPackages
/// - **Develop**: tries devShells
/// - **Check**: tries checks
/// - **Eval**: no default category, just inserts system if the path starts with a per-system category
///
/// # Examples
///
/// ```text
/// // Build context
/// [] -> [["packages", "<system>", "default"], ["legacyPackages", "<system>", "default"]]
/// ["hello"] -> [["packages", "<system>", "hello"], ["legacyPackages", "<system>", "hello"]]
/// ["packages", "hello"] -> [["packages", "<system>", "hello"]]
/// ["packages", "x86_64-linux", "hello"] -> [["packages", "x86_64-linux", "hello"]]
/// ["nixosConfigurations", "myHost"] -> [["nixosConfigurations", "myHost"]]
///
/// // Eval context
/// [] -> [[]]
/// ["hello"] -> [["hello"]]
/// ["packages", "hello"] -> [["packages", "<system>", "hello"]]
/// ```
pub fn expand_attribute(
    attr: &[String],
    context: OperationContext,
    system: &str,
) -> Vec<Vec<String>> {
    // Helper to insert system into a per-system category path
    let insert_system = |category: &str, rest: &[String], add_default: bool| -> Vec<String> {
        let mut result = vec![category.to_string(), system.to_string()];
        result.extend(rest.iter().cloned());
        if add_default && result.len() == 2 {
            result.push("default".to_string());
        }
        result
    };

    // If attr is empty, generate candidates for each search category
    if attr.is_empty() {
        return match context.search_categories() {
            Some(categories) => categories
                .iter()
                .map(|&cat| insert_system(cat, &[], true))
                .collect(),
            None => vec![vec![]], // Eval with empty path
        };
    }

    let first = &attr[0];

    // Check if this is a known per-system category
    let is_per_system = PER_SYSTEM_CATEGORIES
        .iter()
        .any(|&c| c.eq_ignore_ascii_case(first) || c == first);

    // Check if this is a known top-level (non-per-system) category
    let is_top_level = TOP_LEVEL_CATEGORIES
        .iter()
        .any(|&c| c.eq_ignore_ascii_case(first) || c == first);

    // Top-level outputs: return unchanged (no system, no fallbacks)
    if is_top_level {
        return vec![attr.to_vec()];
    }

    // Per-system category specified: insert system if needed, single candidate
    if is_per_system {
        let path = if attr.len() >= 2 && looks_like_system(&attr[1]) {
            // Already has system
            let mut result = attr.to_vec();
            if result.len() == 2 {
                result.push("default".to_string());
            }
            result
        } else {
            // Insert system after category
            insert_system(first, &attr[1..], true)
        };
        return vec![path];
    }

    // Unknown first element (e.g., "hello") - prepend search categories
    // Also include the bare attribute as a final fallback (matches nix behavior)
    match context.search_categories() {
        Some(categories) => {
            let mut candidates: Vec<Vec<String>> = categories
                .iter()
                .map(|&cat| insert_system(cat, attr, false))
                .collect();
            // Add bare attribute as final fallback (like nix does)
            candidates.push(attr.to_vec());
            candidates
        }
        None => {
            // Eval context: return as-is (no category prepending)
            vec![attr.to_vec()]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_flake(dir: &Path) {
        fs::write(
            dir.join("flake.nix"),
            r#"{
                inputs = {};
                outputs = { self }: {
                    packages.x86_64-linux.default = null;
                };
            }"#,
        )
        .unwrap();
    }

    fn create_test_lock(dir: &Path) {
        fs::write(
            dir.join("flake.lock"),
            r#"{
                "nodes": {
                    "root": {}
                },
                "root": "root",
                "version": 7
            }"#,
        )
        .unwrap();
    }

    #[test]
    fn find_flake_root_in_directory() {
        let tmp = TempDir::new().unwrap();
        create_test_flake(tmp.path());

        let root = find_flake_root(tmp.path()).unwrap();
        assert_eq!(root, tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn find_flake_root_from_flake_nix() {
        let tmp = TempDir::new().unwrap();
        create_test_flake(tmp.path());

        let flake_nix = tmp.path().join("flake.nix");
        let root = find_flake_root(&flake_nix).unwrap();
        assert_eq!(root, tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn find_flake_root_missing_flake_nix() {
        let tmp = TempDir::new().unwrap();
        let result = find_flake_root(tmp.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no flake.nix found"));
    }

    #[test]
    fn find_flake_root_nonexistent_path() {
        let result = find_flake_root(Path::new("/nonexistent/path/xyz"));
        assert!(result.is_err());
    }

    #[test]
    fn load_lock_exists() {
        let tmp = TempDir::new().unwrap();
        create_test_flake(tmp.path());
        create_test_lock(tmp.path());

        let lock = load_lock(tmp.path()).unwrap();
        assert!(lock.is_some());
    }

    #[test]
    fn load_lock_missing() {
        let tmp = TempDir::new().unwrap();
        create_test_flake(tmp.path());

        let lock = load_lock(tmp.path()).unwrap();
        assert!(lock.is_none());
    }

    #[test]
    fn resolve_installable_current_dir() {
        let tmp = TempDir::new().unwrap();
        create_test_flake(tmp.path());

        let resolved = resolve_installable(".", tmp.path()).unwrap();
        assert_eq!(resolved.path, tmp.path().canonicalize().unwrap());
        assert!(resolved.attribute.is_empty());
    }

    #[test]
    fn resolve_installable_current_dir_with_attr() {
        let tmp = TempDir::new().unwrap();
        create_test_flake(tmp.path());

        let resolved = resolve_installable(".#hello", tmp.path()).unwrap();
        assert_eq!(resolved.path, tmp.path().canonicalize().unwrap());
        assert_eq!(resolved.attribute, vec!["hello"]);
    }

    #[test]
    fn resolve_installable_dotted_attr() {
        let tmp = TempDir::new().unwrap();
        create_test_flake(tmp.path());

        let resolved = resolve_installable(".#packages.x86_64-linux.hello", tmp.path()).unwrap();
        assert_eq!(
            resolved.attribute,
            vec!["packages", "x86_64-linux", "hello"]
        );
    }

    #[test]
    fn resolve_installable_with_lock() {
        let tmp = TempDir::new().unwrap();
        create_test_flake(tmp.path());
        create_test_lock(tmp.path());

        let resolved = resolve_installable(".", tmp.path()).unwrap();
        assert!(resolved.lock.is_some());
    }

    #[test]
    fn resolve_installable_remote_not_supported() {
        let tmp = TempDir::new().unwrap();
        let result = resolve_installable("github:NixOS/nixpkgs#hello", tmp.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not yet supported"));
    }

    #[test]
    fn current_system_returns_valid_format() {
        let system = current_system().unwrap();
        assert!(system.contains('-'));
        let parts: Vec<&str> = system.split('-').collect();
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn looks_like_system_valid() {
        assert!(looks_like_system("x86_64-linux"));
        assert!(looks_like_system("aarch64-darwin"));
        assert!(looks_like_system("i686-linux"));
    }

    #[test]
    fn looks_like_system_invalid() {
        assert!(!looks_like_system("hello"));
        assert!(!looks_like_system("my-package"));
        assert!(!looks_like_system("x86_64-linux-gnu"));
    }

    // expand_attribute tests - Build context
    #[test]
    fn expand_build_empty() {
        let candidates = expand_attribute(&[], OperationContext::Build, "x86_64-linux");
        assert_eq!(
            candidates,
            vec![
                vec!["packages", "x86_64-linux", "default"],
                vec!["legacyPackages", "x86_64-linux", "default"],
            ]
        );
    }

    #[test]
    fn expand_build_single_name() {
        // nix build .#hello tries: packages.<system>.hello, legacyPackages.<system>.hello, hello
        let candidates =
            expand_attribute(&["hello".to_string()], OperationContext::Build, "x86_64-linux");
        assert_eq!(
            candidates,
            vec![
                vec!["packages", "x86_64-linux", "hello"],
                vec!["legacyPackages", "x86_64-linux", "hello"],
                vec!["hello"], // bare attribute fallback
            ]
        );
    }

    #[test]
    fn expand_build_category_no_system() {
        // packages.hello -> packages.x86_64-linux.hello (single candidate)
        let candidates = expand_attribute(
            &["packages".to_string(), "hello".to_string()],
            OperationContext::Build,
            "x86_64-linux",
        );
        assert_eq!(candidates, vec![vec!["packages", "x86_64-linux", "hello"]]);
    }

    #[test]
    fn expand_build_full_path() {
        // Full path with system -> unchanged
        let attr = vec![
            "packages".to_string(),
            "aarch64-darwin".to_string(),
            "custom".to_string(),
        ];
        let candidates = expand_attribute(&attr, OperationContext::Build, "x86_64-linux");
        assert_eq!(candidates, vec![vec!["packages", "aarch64-darwin", "custom"]]);
    }

    #[test]
    fn expand_build_top_level() {
        // nixosConfigurations.myHost -> unchanged (no system, no fallbacks)
        let candidates = expand_attribute(
            &["nixosConfigurations".to_string(), "myHost".to_string()],
            OperationContext::Build,
            "x86_64-linux",
        );
        assert_eq!(candidates, vec![vec!["nixosConfigurations", "myHost"]]);
    }

    // expand_attribute tests - Run context
    #[test]
    fn expand_run_empty() {
        let candidates = expand_attribute(&[], OperationContext::Run, "x86_64-linux");
        assert_eq!(
            candidates,
            vec![
                vec!["apps", "x86_64-linux", "default"],
                vec!["packages", "x86_64-linux", "default"],
                vec!["legacyPackages", "x86_64-linux", "default"],
            ]
        );
    }

    #[test]
    fn expand_run_single_name() {
        // nix run .#hello tries: apps.<system>.hello, packages.<system>.hello, legacyPackages.<system>.hello, hello
        let candidates =
            expand_attribute(&["hello".to_string()], OperationContext::Run, "x86_64-linux");
        assert_eq!(
            candidates,
            vec![
                vec!["apps", "x86_64-linux", "hello"],
                vec!["packages", "x86_64-linux", "hello"],
                vec!["legacyPackages", "x86_64-linux", "hello"],
                vec!["hello"], // bare attribute fallback
            ]
        );
    }

    // expand_attribute tests - Develop context
    #[test]
    fn expand_develop_empty() {
        let candidates = expand_attribute(&[], OperationContext::Develop, "x86_64-linux");
        assert_eq!(candidates, vec![vec!["devShells", "x86_64-linux", "default"]]);
    }

    #[test]
    fn expand_develop_single_name() {
        // nix develop .#myshell tries: devShells.<system>.myshell, myshell
        let candidates =
            expand_attribute(&["myshell".to_string()], OperationContext::Develop, "x86_64-linux");
        assert_eq!(
            candidates,
            vec![
                vec!["devShells", "x86_64-linux", "myshell"],
                vec!["myshell"], // bare attribute fallback
            ]
        );
    }

    // expand_attribute tests - Check context
    #[test]
    fn expand_check_empty() {
        let candidates = expand_attribute(&[], OperationContext::Check, "x86_64-linux");
        assert_eq!(candidates, vec![vec!["checks", "x86_64-linux", "default"]]);
    }

    // expand_attribute tests - Eval context
    #[test]
    fn expand_eval_empty() {
        // Eval with empty path returns empty candidates
        let candidates = expand_attribute(&[], OperationContext::Eval, "x86_64-linux");
        assert_eq!(candidates, vec![Vec::<String>::new()]);
    }

    #[test]
    fn expand_eval_single_name() {
        // Eval doesn't prepend categories for unknown names
        let candidates =
            expand_attribute(&["hello".to_string()], OperationContext::Eval, "x86_64-linux");
        assert_eq!(candidates, vec![vec!["hello"]]);
    }

    #[test]
    fn expand_eval_category_inserts_system() {
        // Eval inserts system for per-system categories
        let candidates = expand_attribute(
            &["packages".to_string(), "hello".to_string()],
            OperationContext::Eval,
            "x86_64-linux",
        );
        assert_eq!(candidates, vec![vec!["packages", "x86_64-linux", "hello"]]);
    }

    #[test]
    fn expand_eval_top_level_unchanged() {
        // Eval leaves top-level categories unchanged
        let candidates = expand_attribute(
            &["nixosConfigurations".to_string(), "myHost".to_string()],
            OperationContext::Eval,
            "x86_64-linux",
        );
        assert_eq!(candidates, vec![vec!["nixosConfigurations", "myHost"]]);
    }

    // Category-only tests
    #[test]
    fn expand_category_only_adds_default() {
        // devShells -> devShells.x86_64-linux.default
        let candidates =
            expand_attribute(&["devShells".to_string()], OperationContext::Develop, "x86_64-linux");
        assert_eq!(candidates, vec![vec!["devShells", "x86_64-linux", "default"]]);
    }

    #[test]
    fn expand_overlays_no_system() {
        // overlays.default -> overlays.default (no system for top-level)
        let candidates = expand_attribute(
            &["overlays".to_string(), "default".to_string()],
            OperationContext::Build,
            "x86_64-linux",
        );
        assert_eq!(candidates, vec![vec!["overlays", "default"]]);
    }
}
