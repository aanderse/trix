//! Test fixtures for conformance testing.
//!
//! Provides test flakes that can be materialized to temporary directories
//! for testing both trix and nix against.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// A test fixture representing a flake scenario.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// Name of the fixture (for identification in test output).
    pub name: &'static str,

    /// Description of what this fixture tests.
    pub description: &'static str,

    /// Contents of flake.nix.
    pub flake_nix: &'static str,

    /// Optional pre-existing flake.lock content.
    pub flake_lock: Option<&'static str>,

    /// Additional files to create (path, content).
    pub extra_files: &'static [(&'static str, &'static str)],

    /// Whether to initialize a git repository (required for flakes).
    pub needs_git: bool,
}

impl Fixture {
    /// Materialize this fixture to a temporary directory.
    pub fn setup(&self) -> Result<TempDir, String> {
        let temp_dir =
            TempDir::new().map_err(|e| format!("failed to create temp dir: {}", e))?;

        // Write flake.nix
        fs::write(temp_dir.path().join("flake.nix"), self.flake_nix)
            .map_err(|e| format!("failed to write flake.nix: {}", e))?;

        // Write flake.lock if provided
        if let Some(lock_content) = self.flake_lock {
            fs::write(temp_dir.path().join("flake.lock"), lock_content)
                .map_err(|e| format!("failed to write flake.lock: {}", e))?;
        }

        // Write extra files
        for (path, content) in self.extra_files {
            let file_path = temp_dir.path().join(path);
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create dir for {}: {}", path, e))?;
            }
            fs::write(&file_path, content)
                .map_err(|e| format!("failed to write {}: {}", path, e))?;
        }

        // Initialize git repo if needed
        if self.needs_git {
            init_git_repo(temp_dir.path())?;
        }

        Ok(temp_dir)
    }
}

/// Initialize a git repository and add all files.
fn init_git_repo(dir: &Path) -> Result<(), String> {
    let init = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to run git init: {}", e))?;

    if !init.status.success() {
        return Err(format!(
            "git init failed: {}",
            String::from_utf8_lossy(&init.stderr)
        ));
    }

    // Configure git user for commits
    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir)
        .output()
        .ok();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir)
        .output()
        .ok();

    // Add all files
    let add = Command::new("git")
        .args(["add", "."])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to run git add: {}", e))?;

    if !add.status.success() {
        return Err(format!(
            "git add failed: {}",
            String::from_utf8_lossy(&add.stderr)
        ));
    }

    // Commit
    let commit = Command::new("git")
        .args(["commit", "-m", "initial", "--quiet"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to run git commit: {}", e))?;

    if !commit.status.success() {
        return Err(format!(
            "git commit failed: {}",
            String::from_utf8_lossy(&commit.stderr)
        ));
    }

    Ok(())
}

// =============================================================================
// Simple Fixtures
// =============================================================================

/// A minimal flake with no inputs and a simple package.
pub const SIMPLE_PACKAGE: Fixture = Fixture {
    name: "simple_package",
    description: "Minimal flake with a single package output",
    flake_nix: r#"{
  inputs = {};
  outputs = { self }: {
    packages.x86_64-linux.default = derivation {
      name = "simple";
      system = "x86_64-linux";
      builder = "/bin/sh";
      args = ["-c" "echo hello > $out"];
    };
  };
}"#,
    flake_lock: None,
    extra_files: &[],
    needs_git: true,
};

/// A flake with only lib outputs (no packages).
pub const LIB_ONLY: Fixture = Fixture {
    name: "lib_only",
    description: "Flake with only lib attribute (no packages)",
    flake_nix: r#"{
  inputs = {};
  outputs = { self }: {
    lib = {
      hello = "world";
      add = x: y: x + y;
      nested = {
        value = 42;
        list = [1 2 3];
      };
    };
  };
}"#,
    flake_lock: None,
    extra_files: &[],
    needs_git: true,
};

/// A flake with empty outputs.
pub const EMPTY_OUTPUTS: Fixture = Fixture {
    name: "empty_outputs",
    description: "Flake with empty outputs",
    flake_nix: r#"{
  inputs = {};
  outputs = { self }: {};
}"#,
    flake_lock: None,
    extra_files: &[],
    needs_git: true,
};

/// A flake with multiple output types.
pub const MULTI_OUTPUT: Fixture = Fixture {
    name: "multi_output",
    description: "Flake with packages, apps, lib, and overlays",
    flake_nix: r#"{
  inputs = {};
  outputs = { self }: {
    lib = {
      greet = name: "Hello, ${name}!";
    };

    packages.x86_64-linux = {
      default = derivation {
        name = "hello";
        system = "x86_64-linux";
        builder = "/bin/sh";
        args = ["-c" "echo hello > $out"];
      };
      other = derivation {
        name = "other";
        system = "x86_64-linux";
        builder = "/bin/sh";
        args = ["-c" "echo other > $out"];
      };
    };

    overlays.default = final: prev: {
      hello = prev.hello or null;
    };
  };
}"#,
    flake_lock: None,
    extra_files: &[],
    needs_git: true,
};

// =============================================================================
// Fixtures with Inputs
// =============================================================================

/// A flake with a single nixpkgs input.
pub const WITH_NIXPKGS: Fixture = Fixture {
    name: "with_nixpkgs",
    description: "Flake with nixpkgs input",
    flake_nix: r#"{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
  };
  outputs = { self, nixpkgs }: {
    lib.pkgs = nixpkgs;
  };
}"#,
    flake_lock: None,
    extra_files: &[],
    needs_git: true,
};

/// A flake with flake-utils input.
pub const WITH_FLAKE_UTILS: Fixture = Fixture {
    name: "with_flake_utils",
    description: "Flake with flake-utils input",
    flake_nix: r#"{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
  };
  outputs = { self, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system: {
      packages.default = derivation {
        name = "test-${system}";
        inherit system;
        builder = "/bin/sh";
        args = ["-c" "echo ${system} > $out"];
      };
    });
}"#,
    flake_lock: None,
    extra_files: &[],
    needs_git: true,
};

/// A flake with follows directive.
pub const WITH_FOLLOWS: Fixture = Fixture {
    name: "with_follows",
    description: "Flake with follows directive",
    flake_nix: r#"{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
    flake-utils = {
      url = "github:numtide/flake-utils";
      inputs.systems.follows = "";
    };
  };
  outputs = { self, nixpkgs, flake-utils }: {
    lib = {};
  };
}"#,
    flake_lock: None,
    extra_files: &[],
    needs_git: true,
};

// =============================================================================
// Edge Case Fixtures
// =============================================================================

/// A flake with special characters in strings.
pub const SPECIAL_CHARS: Fixture = Fixture {
    name: "special_chars",
    description: "Flake with special characters in values",
    flake_nix: r#"{
  inputs = {};
  outputs = { self }: {
    lib = {
      withQuotes = "hello \"world\"";
      withNewlines = "line1\nline2";
      withTabs = "col1\tcol2";
      withBackslash = "path\\to\\file";
      withUnicode = "Hello, \u4e16\u754c!";
    };
  };
}"#,
    flake_lock: None,
    extra_files: &[],
    needs_git: true,
};

/// A flake with deeply nested attributes.
pub const DEEP_NESTING: Fixture = Fixture {
    name: "deep_nesting",
    description: "Flake with deeply nested attribute structure",
    flake_nix: r#"{
  inputs = {};
  outputs = { self }: {
    lib = {
      a = {
        b = {
          c = {
            d = {
              e = {
                value = "deep";
              };
            };
          };
        };
      };
    };
  };
}"#,
    flake_lock: None,
    extra_files: &[],
    needs_git: true,
};

/// A flake with various Nix types.
pub const ALL_TYPES: Fixture = Fixture {
    name: "all_types",
    description: "Flake with various Nix value types for eval testing",
    flake_nix: r#"{
  inputs = {};
  outputs = { self }: {
    lib = {
      aString = "hello";
      anInt = 42;
      aFloat = 3.14;
      aBool = true;
      aNull = null;
      aList = [1 2 3 "four" true];
      anAttrSet = { x = 1; y = 2; };
      aPath = ./flake.nix;
    };
  };
}"#,
    flake_lock: None,
    extra_files: &[],
    needs_git: true,
};

// =============================================================================
// Helper Functions
// =============================================================================

/// Fetch a GitHub flake to a temporary directory at a specific commit.
/// Uses shallow fetch for speed.
pub fn fetch_github_flake(owner: &str, repo: &str, rev: &str) -> Result<TempDir, String> {
    let temp_dir =
        TempDir::new().map_err(|e| format!("failed to create temp dir: {}", e))?;

    let repo_url = format!("https://github.com/{}/{}.git", owner, repo);
    let dir = temp_dir.path();

    // Initialize empty repo
    let init = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to run git init: {}", e))?;

    if !init.status.success() {
        return Err(format!(
            "git init failed: {}",
            String::from_utf8_lossy(&init.stderr)
        ));
    }

    // Add remote
    let remote = Command::new("git")
        .args(["remote", "add", "origin", &repo_url])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to add remote: {}", e))?;

    if !remote.status.success() {
        return Err(format!(
            "git remote add failed: {}",
            String::from_utf8_lossy(&remote.stderr)
        ));
    }

    // Fetch the specific commit (shallow)
    let fetch = Command::new("git")
        .args(["fetch", "--depth", "1", "--quiet", "origin", rev])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to fetch: {}", e))?;

    if !fetch.status.success() {
        return Err(format!(
            "git fetch failed: {}",
            String::from_utf8_lossy(&fetch.stderr)
        ));
    }

    // Checkout FETCH_HEAD
    let checkout = Command::new("git")
        .args(["checkout", "--quiet", "FETCH_HEAD"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to checkout: {}", e))?;

    if !checkout.status.success() {
        return Err(format!(
            "git checkout failed: {}",
            String::from_utf8_lossy(&checkout.stderr)
        ));
    }

    // Verify flake.nix exists
    if !dir.join("flake.nix").exists() {
        return Err("flake.nix not found in cloned repository".to_string());
    }

    Ok(temp_dir)
}
