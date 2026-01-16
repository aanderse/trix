//! Flake.lock parsing and input resolution
//!
//! Handles parsing the flake.lock JSON format and resolving inputs,
//! including `.follows` declarations.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A parsed flake.lock file
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlakeLock {
    /// The nodes in the lock graph
    pub nodes: HashMap<String, LockNode>,
    /// The root node name (usually "root")
    pub root: String,
    /// Lock file version (currently 7)
    pub version: u32,
}

/// A node in the lock graph
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockNode {
    /// Inputs for this node - either a node name or a follows path
    #[serde(default)]
    pub inputs: HashMap<String, InputRef>,
    /// The locked reference (not present for root node)
    #[serde(default)]
    pub locked: Option<LockedRef>,
    /// The original reference (not present for root node)
    #[serde(default)]
    pub original: Option<OriginalRef>,
    /// Whether this node is flake (default true)
    #[serde(default = "default_true")]
    pub flake: bool,
}

fn default_true() -> bool {
    true
}

/// An input reference - either a direct node name or a follows path
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum InputRef {
    /// Direct reference to another node by name
    Direct(String),
    /// Follows path - a list of input names to traverse
    Follows(Vec<String>),
}

/// A locked reference with all the resolved details
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "lowercase")]
pub enum LockedRef {
    GitHub {
        owner: String,
        repo: String,
        rev: String,
        #[serde(rename = "narHash")]
        nar_hash: Option<String>,
        #[serde(rename = "lastModified")]
        last_modified: Option<u64>,
    },
    GitLab {
        owner: String,
        repo: String,
        rev: String,
        #[serde(rename = "narHash")]
        nar_hash: Option<String>,
    },
    Sourcehut {
        owner: String,
        repo: String,
        rev: String,
        #[serde(rename = "narHash")]
        nar_hash: Option<String>,
    },
    Git {
        url: String,
        /// The git revision (may be None for dirty repos)
        rev: Option<String>,
        #[serde(rename = "narHash")]
        nar_hash: Option<String>,
        #[serde(rename = "ref")]
        git_ref: Option<String>,
        /// Dirty revision (for repos with uncommitted changes)
        #[serde(rename = "dirtyRev")]
        dirty_rev: Option<String>,
        #[serde(rename = "dirtyShortRev")]
        dirty_short_rev: Option<String>,
        #[serde(rename = "lastModified")]
        last_modified: Option<u64>,
    },
    Path {
        path: String,
        #[serde(rename = "narHash")]
        nar_hash: Option<String>,
        #[serde(rename = "lastModified")]
        last_modified: Option<u64>,
    },
    Tarball {
        url: String,
        #[serde(rename = "narHash")]
        nar_hash: Option<String>,
    },
    Indirect {
        id: String,
        #[serde(rename = "narHash")]
        nar_hash: Option<String>,
    },
}

/// Original (unlocked) reference
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "lowercase")]
pub enum OriginalRef {
    GitHub {
        owner: String,
        repo: String,
        #[serde(rename = "ref")]
        git_ref: Option<String>,
    },
    GitLab {
        owner: String,
        repo: String,
        #[serde(rename = "ref")]
        git_ref: Option<String>,
    },
    Sourcehut {
        owner: String,
        repo: String,
        #[serde(rename = "ref")]
        git_ref: Option<String>,
    },
    Git {
        url: String,
        #[serde(rename = "ref")]
        git_ref: Option<String>,
    },
    Path {
        path: String,
    },
    Tarball {
        url: String,
    },
    Indirect {
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionError {
    pub message: String,
}

impl std::fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ResolutionError {}

impl ResolutionError {
    fn new(msg: impl Into<String>) -> Self {
        Self { message: msg.into() }
    }
}

/// A resolved input with its locked reference
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedInput {
    /// The node name in the lock file
    pub node_name: String,
    /// The locked reference
    pub locked: LockedRef,
    /// Whether this is a flake
    pub flake: bool,
}

impl FlakeLock {
    /// Parse a flake.lock from JSON
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Get the root node
    pub fn root_node(&self) -> Option<&LockNode> {
        self.nodes.get(&self.root)
    }

    /// Resolve all inputs for a given node
    pub fn resolve_inputs(&self, node_name: &str) -> Result<HashMap<String, ResolvedInput>, ResolutionError> {
        let node = self.nodes.get(node_name).ok_or_else(|| {
            ResolutionError::new(format!("node '{}' not found", node_name))
        })?;

        let mut resolved = HashMap::new();
        for (input_name, input_ref) in &node.inputs {
            let resolved_input = self.resolve_input(node_name, input_name, input_ref)?;
            resolved.insert(input_name.clone(), resolved_input);
        }
        Ok(resolved)
    }

    /// Resolve a single input reference
    pub fn resolve_input(
        &self,
        from_node: &str,
        input_name: &str,
        input_ref: &InputRef,
    ) -> Result<ResolvedInput, ResolutionError> {
        let mut visited = HashSet::new();
        self.resolve_input_inner(from_node, input_name, input_ref, &mut visited)
    }

    fn resolve_input_inner(
        &self,
        from_node: &str,
        input_name: &str,
        input_ref: &InputRef,
        visited: &mut HashSet<String>,
    ) -> Result<ResolvedInput, ResolutionError> {
        match input_ref {
            InputRef::Direct(node_name) => {
                let node = self.nodes.get(node_name).ok_or_else(|| {
                    ResolutionError::new(format!(
                        "input '{}' references non-existent node '{}'",
                        input_name, node_name
                    ))
                })?;

                let locked = node.locked.clone().ok_or_else(|| {
                    ResolutionError::new(format!(
                        "node '{}' has no locked reference",
                        node_name
                    ))
                })?;

                Ok(ResolvedInput {
                    node_name: node_name.clone(),
                    locked,
                    flake: node.flake,
                })
            }
            InputRef::Follows(path) => {
                self.resolve_follows(from_node, input_name, path, visited)
            }
        }
    }

    /// Resolve a follows path
    fn resolve_follows(
        &self,
        from_node: &str,
        input_name: &str,
        path: &[String],
        visited: &mut HashSet<String>,
    ) -> Result<ResolvedInput, ResolutionError> {
        if path.is_empty() {
            return Err(ResolutionError::new(format!(
                "empty follows path for input '{}'",
                input_name
            )));
        }

        // Create a key for cycle detection
        let visit_key = format!("{}:{}", from_node, input_name);
        if visited.contains(&visit_key) {
            return Err(ResolutionError::new(format!(
                "cycle detected in follows: {}",
                visit_key
            )));
        }
        visited.insert(visit_key);

        // Start from root and traverse the path
        let mut current_node = &self.root;

        for (i, segment) in path.iter().enumerate() {
            let node = self.nodes.get(current_node).ok_or_else(|| {
                ResolutionError::new(format!(
                    "follows path references non-existent node '{}'",
                    current_node
                ))
            })?;

            let input_ref = node.inputs.get(segment).ok_or_else(|| {
                ResolutionError::new(format!(
                    "follows path '{}' not found in node '{}' (at segment '{}')",
                    path.join("."),
                    current_node,
                    segment
                ))
            })?;

            // If this is the last segment, resolve it
            if i == path.len() - 1 {
                return self.resolve_input_inner(current_node, segment, input_ref, visited);
            }

            // Otherwise, get the node name and continue traversing
            match input_ref {
                InputRef::Direct(node_name) => {
                    current_node = node_name;
                }
                InputRef::Follows(nested_path) => {
                    // Recursively resolve the follows and use that node
                    let resolved = self.resolve_follows(
                        current_node,
                        segment,
                        nested_path,
                        visited,
                    )?;
                    // We need to continue from the resolved node
                    // For now, we'll use a simplified approach
                    return self.resolve_follows_continue(
                        &resolved.node_name,
                        input_name,
                        &path[i + 1..],
                        visited,
                    );
                }
            }
        }

        Err(ResolutionError::new("unexpected end of follows path"))
    }

    /// Continue resolving a follows path from a specific node
    fn resolve_follows_continue(
        &self,
        start_node: &str,
        input_name: &str,
        remaining_path: &[String],
        visited: &mut HashSet<String>,
    ) -> Result<ResolvedInput, ResolutionError> {
        if remaining_path.is_empty() {
            // We've reached the end, return this node
            let node = self.nodes.get(start_node).ok_or_else(|| {
                ResolutionError::new(format!("node '{}' not found", start_node))
            })?;

            let locked = node.locked.clone().ok_or_else(|| {
                ResolutionError::new(format!("node '{}' has no locked reference", start_node))
            })?;

            return Ok(ResolvedInput {
                node_name: start_node.to_string(),
                locked,
                flake: node.flake,
            });
        }

        let node = self.nodes.get(start_node).ok_or_else(|| {
            ResolutionError::new(format!("node '{}' not found", start_node))
        })?;

        let first = &remaining_path[0];
        let input_ref = node.inputs.get(first).ok_or_else(|| {
            ResolutionError::new(format!(
                "input '{}' not found in node '{}'",
                first, start_node
            ))
        })?;

        if remaining_path.len() == 1 {
            return self.resolve_input_inner(start_node, input_name, input_ref, visited);
        }

        match input_ref {
            InputRef::Direct(node_name) => {
                self.resolve_follows_continue(node_name, input_name, &remaining_path[1..], visited)
            }
            InputRef::Follows(nested_path) => {
                let resolved = self.resolve_follows(start_node, first, nested_path, visited)?;
                self.resolve_follows_continue(
                    &resolved.node_name,
                    input_name,
                    &remaining_path[1..],
                    visited,
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== Lock file parsing ====================

    #[test]
    fn parse_minimal_lock() {
        let json = r#"{
            "nodes": {
                "root": {
                    "inputs": {}
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();
        assert_eq!(lock.version, 7);
        assert_eq!(lock.root, "root");
        assert!(lock.nodes.contains_key("root"));
    }

    #[test]
    fn parse_lock_with_github_input() {
        let json = r#"{
            "nodes": {
                "nixpkgs": {
                    "locked": {
                        "type": "github",
                        "owner": "NixOS",
                        "repo": "nixpkgs",
                        "rev": "abc123",
                        "narHash": "sha256-xyz"
                    },
                    "original": {
                        "type": "github",
                        "owner": "NixOS",
                        "repo": "nixpkgs"
                    }
                },
                "root": {
                    "inputs": {
                        "nixpkgs": "nixpkgs"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();

        // Check nixpkgs node
        let nixpkgs = lock.nodes.get("nixpkgs").unwrap();
        match nixpkgs.locked.as_ref().unwrap() {
            LockedRef::GitHub { owner, repo, rev, .. } => {
                assert_eq!(owner, "NixOS");
                assert_eq!(repo, "nixpkgs");
                assert_eq!(rev, "abc123");
            }
            _ => panic!("expected GitHub locked ref"),
        }

        // Check root inputs
        let root = lock.nodes.get("root").unwrap();
        assert_eq!(
            root.inputs.get("nixpkgs"),
            Some(&InputRef::Direct("nixpkgs".to_string()))
        );
    }

    #[test]
    fn parse_lock_with_follows() {
        let json = r#"{
            "nodes": {
                "home-manager": {
                    "inputs": {
                        "nixpkgs": ["nixpkgs"]
                    },
                    "locked": {
                        "type": "github",
                        "owner": "nix-community",
                        "repo": "home-manager",
                        "rev": "def456"
                    },
                    "original": {
                        "type": "github",
                        "owner": "nix-community",
                        "repo": "home-manager"
                    }
                },
                "nixpkgs": {
                    "locked": {
                        "type": "github",
                        "owner": "NixOS",
                        "repo": "nixpkgs",
                        "rev": "abc123"
                    },
                    "original": {
                        "type": "github",
                        "owner": "NixOS",
                        "repo": "nixpkgs"
                    }
                },
                "root": {
                    "inputs": {
                        "home-manager": "home-manager",
                        "nixpkgs": "nixpkgs"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();

        // Check home-manager has a follows reference
        let hm = lock.nodes.get("home-manager").unwrap();
        assert_eq!(
            hm.inputs.get("nixpkgs"),
            Some(&InputRef::Follows(vec!["nixpkgs".to_string()]))
        );
    }

    #[test]
    fn parse_lock_with_path_input() {
        let json = r#"{
            "nodes": {
                "local": {
                    "locked": {
                        "type": "path",
                        "path": "./subdir"
                    },
                    "original": {
                        "type": "path",
                        "path": "./subdir"
                    }
                },
                "root": {
                    "inputs": {
                        "local": "local"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();
        let local = lock.nodes.get("local").unwrap();
        match local.locked.as_ref().unwrap() {
            LockedRef::Path { path, .. } => {
                assert_eq!(path, "./subdir");
            }
            _ => panic!("expected Path locked ref"),
        }
    }

    #[test]
    fn parse_lock_with_git_input() {
        let json = r#"{
            "nodes": {
                "myrepo": {
                    "locked": {
                        "type": "git",
                        "url": "https://example.com/repo.git",
                        "rev": "abc123",
                        "ref": "main"
                    },
                    "original": {
                        "type": "git",
                        "url": "https://example.com/repo.git",
                        "ref": "main"
                    }
                },
                "root": {
                    "inputs": {
                        "myrepo": "myrepo"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();
        let myrepo = lock.nodes.get("myrepo").unwrap();
        match myrepo.locked.as_ref().unwrap() {
            LockedRef::Git { url, rev, git_ref, .. } => {
                assert_eq!(url, "https://example.com/repo.git");
                assert_eq!(rev, &Some("abc123".to_string()));
                assert_eq!(git_ref, &Some("main".to_string()));
            }
            _ => panic!("expected Git locked ref"),
        }
    }

    #[test]
    fn parse_lock_with_dirty_git_input() {
        // Test parsing a dirty git input (has dirtyRev instead of rev)
        let json = r#"{
            "nodes": {
                "myrepo": {
                    "locked": {
                        "type": "git",
                        "url": "file:///home/user/code/myrepo",
                        "dirtyRev": "abc123-dirty",
                        "dirtyShortRev": "abc123-dirty",
                        "lastModified": 1700000000,
                        "narHash": "sha256-xyz"
                    },
                    "original": {
                        "type": "git",
                        "url": "file:///home/user/code/myrepo"
                    }
                },
                "root": {
                    "inputs": {
                        "myrepo": "myrepo"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();
        let myrepo = lock.nodes.get("myrepo").unwrap();
        match myrepo.locked.as_ref().unwrap() {
            LockedRef::Git { url, rev, dirty_rev, dirty_short_rev, last_modified, nar_hash, .. } => {
                assert_eq!(url, "file:///home/user/code/myrepo");
                assert_eq!(rev, &None);
                assert_eq!(dirty_rev, &Some("abc123-dirty".to_string()));
                assert_eq!(dirty_short_rev, &Some("abc123-dirty".to_string()));
                assert_eq!(last_modified, &Some(1700000000));
                assert_eq!(nar_hash, &Some("sha256-xyz".to_string()));
            }
            _ => panic!("expected Git locked ref"),
        }
    }

    // ==================== Simple input resolution ====================

    #[test]
    fn resolve_simple_input() {
        let json = r#"{
            "nodes": {
                "nixpkgs": {
                    "locked": {
                        "type": "github",
                        "owner": "NixOS",
                        "repo": "nixpkgs",
                        "rev": "abc123"
                    },
                    "original": {
                        "type": "github",
                        "owner": "NixOS",
                        "repo": "nixpkgs"
                    }
                },
                "root": {
                    "inputs": {
                        "nixpkgs": "nixpkgs"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();
        let inputs = lock.resolve_inputs("root").unwrap();

        let nixpkgs = inputs.get("nixpkgs").unwrap();
        assert_eq!(nixpkgs.node_name, "nixpkgs");
        match &nixpkgs.locked {
            LockedRef::GitHub { rev, .. } => assert_eq!(rev, "abc123"),
            _ => panic!("expected GitHub"),
        }
    }

    #[test]
    fn resolve_multiple_inputs() {
        let json = r#"{
            "nodes": {
                "nixpkgs": {
                    "locked": {
                        "type": "github",
                        "owner": "NixOS",
                        "repo": "nixpkgs",
                        "rev": "abc123"
                    },
                    "original": {
                        "type": "github",
                        "owner": "NixOS",
                        "repo": "nixpkgs"
                    }
                },
                "flake-utils": {
                    "locked": {
                        "type": "github",
                        "owner": "numtide",
                        "repo": "flake-utils",
                        "rev": "def456"
                    },
                    "original": {
                        "type": "github",
                        "owner": "numtide",
                        "repo": "flake-utils"
                    }
                },
                "root": {
                    "inputs": {
                        "nixpkgs": "nixpkgs",
                        "flake-utils": "flake-utils"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();
        let inputs = lock.resolve_inputs("root").unwrap();

        assert_eq!(inputs.len(), 2);
        assert!(inputs.contains_key("nixpkgs"));
        assert!(inputs.contains_key("flake-utils"));
    }

    // ==================== Follows resolution ====================

    #[test]
    fn resolve_single_level_follows() {
        // home-manager.inputs.nixpkgs follows root's nixpkgs
        let json = r#"{
            "nodes": {
                "home-manager": {
                    "inputs": {
                        "nixpkgs": ["nixpkgs"]
                    },
                    "locked": {
                        "type": "github",
                        "owner": "nix-community",
                        "repo": "home-manager",
                        "rev": "hm123"
                    },
                    "original": {
                        "type": "github",
                        "owner": "nix-community",
                        "repo": "home-manager"
                    }
                },
                "nixpkgs": {
                    "locked": {
                        "type": "github",
                        "owner": "NixOS",
                        "repo": "nixpkgs",
                        "rev": "abc123"
                    },
                    "original": {
                        "type": "github",
                        "owner": "NixOS",
                        "repo": "nixpkgs"
                    }
                },
                "root": {
                    "inputs": {
                        "home-manager": "home-manager",
                        "nixpkgs": "nixpkgs"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();

        // Resolve home-manager's inputs
        let hm_inputs = lock.resolve_inputs("home-manager").unwrap();
        let hm_nixpkgs = hm_inputs.get("nixpkgs").unwrap();

        // Should resolve to root's nixpkgs
        assert_eq!(hm_nixpkgs.node_name, "nixpkgs");
        match &hm_nixpkgs.locked {
            LockedRef::GitHub { rev, .. } => assert_eq!(rev, "abc123"),
            _ => panic!("expected GitHub"),
        }
    }

    #[test]
    fn resolve_nested_follows() {
        // A.inputs.foo follows B.inputs.bar
        // where B.inputs.bar is a direct reference
        let json = r#"{
            "nodes": {
                "A": {
                    "inputs": {
                        "foo": ["B", "bar"]
                    },
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "A",
                        "rev": "aaa"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "A"
                    }
                },
                "B": {
                    "inputs": {
                        "bar": "C"
                    },
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "B",
                        "rev": "bbb"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "B"
                    }
                },
                "C": {
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "C",
                        "rev": "ccc"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "C"
                    }
                },
                "root": {
                    "inputs": {
                        "A": "A",
                        "B": "B",
                        "C": "C"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();

        let a_inputs = lock.resolve_inputs("A").unwrap();
        let a_foo = a_inputs.get("foo").unwrap();

        // A.foo should resolve to C (via B.bar)
        assert_eq!(a_foo.node_name, "C");
        match &a_foo.locked {
            LockedRef::GitHub { rev, .. } => assert_eq!(rev, "ccc"),
            _ => panic!("expected GitHub"),
        }
    }

    #[test]
    fn resolve_chained_follows() {
        // A.inputs.x follows B
        // B.inputs.y follows C
        // But A.x should get B, not follow B's follows
        let json = r#"{
            "nodes": {
                "A": {
                    "inputs": {
                        "myinput": ["B"]
                    },
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "A",
                        "rev": "aaa"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "A"
                    }
                },
                "B": {
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "B",
                        "rev": "bbb"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "B"
                    }
                },
                "root": {
                    "inputs": {
                        "A": "A",
                        "B": "B"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();

        let a_inputs = lock.resolve_inputs("A").unwrap();
        let a_myinput = a_inputs.get("myinput").unwrap();

        // A.myinput follows root.B, should get B
        assert_eq!(a_myinput.node_name, "B");
    }

    // ==================== Error cases ====================

    #[test]
    fn error_missing_node() {
        let json = r#"{
            "nodes": {
                "root": {
                    "inputs": {
                        "missing": "nonexistent"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();
        let result = lock.resolve_inputs("root");

        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("non-existent"));
    }

    #[test]
    fn error_follows_missing_target() {
        let json = r#"{
            "nodes": {
                "A": {
                    "inputs": {
                        "foo": ["nonexistent"]
                    },
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "A",
                        "rev": "aaa"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "A"
                    }
                },
                "root": {
                    "inputs": {
                        "A": "A"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();
        let result = lock.resolve_inputs("A");

        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("not found"));
    }

    #[test]
    fn error_follows_cycle() {
        // A.x follows B.y, B.y follows A.x
        let json = r#"{
            "nodes": {
                "A": {
                    "inputs": {
                        "x": ["B", "y"]
                    },
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "A",
                        "rev": "aaa"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "A"
                    }
                },
                "B": {
                    "inputs": {
                        "y": ["A", "x"]
                    },
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "B",
                        "rev": "bbb"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "B"
                    }
                },
                "root": {
                    "inputs": {
                        "A": "A",
                        "B": "B"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();
        let result = lock.resolve_inputs("A");

        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("cycle"));
    }

    // ==================== Non-flake inputs ====================

    #[test]
    fn parse_non_flake_input() {
        let json = r#"{
            "nodes": {
                "data": {
                    "flake": false,
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "data",
                        "rev": "abc123"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "data"
                    }
                },
                "root": {
                    "inputs": {
                        "data": "data"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();
        let data = lock.nodes.get("data").unwrap();
        assert!(!data.flake);

        let inputs = lock.resolve_inputs("root").unwrap();
        let data_input = inputs.get("data").unwrap();
        assert!(!data_input.flake);
    }

    // ==================== Additional locked ref types ====================

    #[test]
    fn parse_lock_with_tarball_input() {
        let json = r#"{
            "nodes": {
                "archive": {
                    "locked": {
                        "type": "tarball",
                        "url": "https://example.com/archive.tar.gz",
                        "narHash": "sha256-abc123"
                    },
                    "original": {
                        "type": "tarball",
                        "url": "https://example.com/archive.tar.gz"
                    }
                },
                "root": {
                    "inputs": {
                        "archive": "archive"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();
        let archive = lock.nodes.get("archive").unwrap();
        match archive.locked.as_ref().unwrap() {
            LockedRef::Tarball { url, nar_hash } => {
                assert_eq!(url, "https://example.com/archive.tar.gz");
                assert_eq!(nar_hash, &Some("sha256-abc123".to_string()));
            }
            _ => panic!("expected Tarball locked ref"),
        }
    }

    #[test]
    fn parse_lock_with_sourcehut_input() {
        let json = r#"{
            "nodes": {
                "aerc": {
                    "locked": {
                        "type": "sourcehut",
                        "owner": "~sircmpwn",
                        "repo": "aerc",
                        "rev": "abc123"
                    },
                    "original": {
                        "type": "sourcehut",
                        "owner": "~sircmpwn",
                        "repo": "aerc"
                    }
                },
                "root": {
                    "inputs": {
                        "aerc": "aerc"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();
        let aerc = lock.nodes.get("aerc").unwrap();
        match aerc.locked.as_ref().unwrap() {
            LockedRef::Sourcehut { owner, repo, rev, .. } => {
                assert_eq!(owner, "~sircmpwn");
                assert_eq!(repo, "aerc");
                assert_eq!(rev, "abc123");
            }
            _ => panic!("expected Sourcehut locked ref"),
        }
    }

    #[test]
    fn parse_lock_with_gitlab_input() {
        let json = r#"{
            "nodes": {
                "myproject": {
                    "locked": {
                        "type": "gitlab",
                        "owner": "mygroup",
                        "repo": "myproject",
                        "rev": "def456"
                    },
                    "original": {
                        "type": "gitlab",
                        "owner": "mygroup",
                        "repo": "myproject"
                    }
                },
                "root": {
                    "inputs": {
                        "myproject": "myproject"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();
        let project = lock.nodes.get("myproject").unwrap();
        match project.locked.as_ref().unwrap() {
            LockedRef::GitLab { owner, repo, rev, .. } => {
                assert_eq!(owner, "mygroup");
                assert_eq!(repo, "myproject");
                assert_eq!(rev, "def456");
            }
            _ => panic!("expected GitLab locked ref"),
        }
    }

    // ==================== More edge cases ====================

    #[test]
    fn resolve_empty_inputs() {
        let json = r#"{
            "nodes": {
                "leaf": {
                    "inputs": {},
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "leaf",
                        "rev": "abc123"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "leaf"
                    }
                },
                "root": {
                    "inputs": {
                        "leaf": "leaf"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();
        let leaf_inputs = lock.resolve_inputs("leaf").unwrap();
        assert!(leaf_inputs.is_empty());
    }

    #[test]
    fn resolve_deep_follows_path() {
        // A.foo follows B.bar.baz (3 levels deep)
        // B.bar -> C, C.baz -> D
        let json = r#"{
            "nodes": {
                "A": {
                    "inputs": {
                        "foo": ["B", "bar", "baz"]
                    },
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "A",
                        "rev": "aaa"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "A"
                    }
                },
                "B": {
                    "inputs": {
                        "bar": "C"
                    },
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "B",
                        "rev": "bbb"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "B"
                    }
                },
                "C": {
                    "inputs": {
                        "baz": "D"
                    },
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "C",
                        "rev": "ccc"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "C"
                    }
                },
                "D": {
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "D",
                        "rev": "ddd"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "D"
                    }
                },
                "root": {
                    "inputs": {
                        "A": "A",
                        "B": "B",
                        "C": "C",
                        "D": "D"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();

        let a_inputs = lock.resolve_inputs("A").unwrap();
        let a_foo = a_inputs.get("foo").unwrap();

        // A.foo should resolve to D (via B.bar -> C, C.baz -> D)
        assert_eq!(a_foo.node_name, "D");
        match &a_foo.locked {
            LockedRef::GitHub { rev, .. } => assert_eq!(rev, "ddd"),
            _ => panic!("expected GitHub"),
        }
    }

    #[test]
    fn resolve_follows_through_follows() {
        // A.x follows B.y, and B.y also follows C
        let json = r#"{
            "nodes": {
                "A": {
                    "inputs": {
                        "x": ["B", "y"]
                    },
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "A",
                        "rev": "aaa"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "A"
                    }
                },
                "B": {
                    "inputs": {
                        "y": ["C"]
                    },
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "B",
                        "rev": "bbb"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "B"
                    }
                },
                "C": {
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "C",
                        "rev": "ccc"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "C"
                    }
                },
                "root": {
                    "inputs": {
                        "A": "A",
                        "B": "B",
                        "C": "C"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();

        let a_inputs = lock.resolve_inputs("A").unwrap();
        let a_x = a_inputs.get("x").unwrap();

        // A.x follows B.y, which follows root.C -> should get C
        assert_eq!(a_x.node_name, "C");
        match &a_x.locked {
            LockedRef::GitHub { rev, .. } => assert_eq!(rev, "ccc"),
            _ => panic!("expected GitHub"),
        }
    }

    #[test]
    fn realistic_home_manager_follows() {
        // Real-world pattern: home-manager follows nixpkgs
        let json = r#"{
            "nodes": {
                "home-manager": {
                    "inputs": {
                        "nixpkgs": ["nixpkgs"]
                    },
                    "locked": {
                        "type": "github",
                        "owner": "nix-community",
                        "repo": "home-manager",
                        "rev": "hm-rev-123",
                        "lastModified": 1700000000
                    },
                    "original": {
                        "type": "github",
                        "owner": "nix-community",
                        "repo": "home-manager",
                        "ref": "master"
                    }
                },
                "nixpkgs": {
                    "locked": {
                        "type": "github",
                        "owner": "NixOS",
                        "repo": "nixpkgs",
                        "rev": "nixpkgs-rev-456",
                        "narHash": "sha256-abcdef",
                        "lastModified": 1699999999
                    },
                    "original": {
                        "type": "github",
                        "owner": "NixOS",
                        "repo": "nixpkgs",
                        "ref": "nixos-unstable"
                    }
                },
                "root": {
                    "inputs": {
                        "home-manager": "home-manager",
                        "nixpkgs": "nixpkgs"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();

        // Root should have both inputs
        let root_inputs = lock.resolve_inputs("root").unwrap();
        assert_eq!(root_inputs.len(), 2);

        // home-manager's nixpkgs should follow root's nixpkgs
        let hm_inputs = lock.resolve_inputs("home-manager").unwrap();
        let hm_nixpkgs = hm_inputs.get("nixpkgs").unwrap();
        assert_eq!(hm_nixpkgs.node_name, "nixpkgs");
        match &hm_nixpkgs.locked {
            LockedRef::GitHub { rev, .. } => assert_eq!(rev, "nixpkgs-rev-456"),
            _ => panic!("expected GitHub"),
        }
    }

    #[test]
    fn multiple_inputs_with_mixed_follows() {
        // A has three inputs: one direct, one follows, one follows with path
        let json = r#"{
            "nodes": {
                "A": {
                    "inputs": {
                        "direct": "B",
                        "follows-simple": ["C"],
                        "follows-nested": ["D", "sub"]
                    },
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "A",
                        "rev": "aaa"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "A"
                    }
                },
                "B": {
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "B",
                        "rev": "bbb"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "B"
                    }
                },
                "C": {
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "C",
                        "rev": "ccc"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "C"
                    }
                },
                "D": {
                    "inputs": {
                        "sub": "E"
                    },
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "D",
                        "rev": "ddd"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "D"
                    }
                },
                "E": {
                    "locked": {
                        "type": "github",
                        "owner": "test",
                        "repo": "E",
                        "rev": "eee"
                    },
                    "original": {
                        "type": "github",
                        "owner": "test",
                        "repo": "E"
                    }
                },
                "root": {
                    "inputs": {
                        "A": "A",
                        "B": "B",
                        "C": "C",
                        "D": "D",
                        "E": "E"
                    }
                }
            },
            "root": "root",
            "version": 7
        }"#;

        let lock = FlakeLock::parse(json).unwrap();
        let a_inputs = lock.resolve_inputs("A").unwrap();

        assert_eq!(a_inputs.len(), 3);

        // direct -> B
        let direct = a_inputs.get("direct").unwrap();
        assert_eq!(direct.node_name, "B");

        // follows-simple -> C
        let follows_simple = a_inputs.get("follows-simple").unwrap();
        assert_eq!(follows_simple.node_name, "C");

        // follows-nested -> E (via D.sub)
        let follows_nested = a_inputs.get("follows-nested").unwrap();
        assert_eq!(follows_nested.node_name, "E");
    }
}
