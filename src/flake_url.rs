//! Flake URL/installable parsing
//!
//! Parses flake references like:
//! - `.` or `.#attr` (current directory)
//! - `./path` or `./path#attr` (relative path)
//! - `/absolute/path#attr` (absolute path)
//! - `github:owner/repo` or `github:owner/repo/ref#attr`
//! - `gitlab:owner/repo`
//! - `sourcehut:~user/repo`
//! - `git+https://example.com/repo?ref=main`
//! - `path:./relative`
//! - `nixpkgs` or `nixpkgs#hello` (indirect/registry)

use std::collections::HashMap;

/// A parsed flake URL/installable
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlakeUrl {
    /// The flake reference (everything before the #)
    pub flake_ref: FlakeRef,
    /// The attribute path (everything after the #), if any
    pub attribute: Option<String>,
}

/// A flake reference (without the attribute part)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlakeRef {
    /// A path-based reference (., ./foo, /absolute, path:./foo)
    Path {
        path: String,
    },
    /// github:owner/repo[/ref]
    GitHub {
        owner: String,
        repo: String,
        ref_or_rev: Option<String>,
    },
    /// gitlab:owner/repo[/ref]
    GitLab {
        owner: String,
        repo: String,
        ref_or_rev: Option<String>,
    },
    /// sourcehut:~owner/repo[/ref]
    Sourcehut {
        owner: String,
        repo: String,
        ref_or_rev: Option<String>,
    },
    /// git+https://... or git+ssh://...
    Git {
        url: String,
        params: HashMap<String, String>,
    },
    /// https://example.com/foo.tar.gz or tarball+https://...
    Tarball {
        url: String,
    },
    /// Indirect/registry reference (nixpkgs, flake:nixpkgs)
    Indirect {
        id: String,
        ref_or_rev: Option<String>,
    },
    /// file:///path or file://localhost/path
    File {
        path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ParseError {}

impl ParseError {
    fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
        }
    }
}

/// Parse a flake URL/installable string
///
/// # Examples
/// ```
/// use trix::flake_url::parse_flake_url;
///
/// let url = parse_flake_url(".#default").unwrap();
/// assert_eq!(url.attribute, Some("default".to_string()));
/// ```
pub fn parse_flake_url(input: &str) -> Result<FlakeUrl, ParseError> {
    if input.is_empty() {
        return Err(ParseError::new("empty flake URL"));
    }

    // Split on # to separate flake ref from attribute
    let (ref_part, attribute) = split_attribute(input);

    let flake_ref = parse_flake_ref(ref_part)?;

    Ok(FlakeUrl {
        flake_ref,
        attribute,
    })
}

/// Split a flake URL into the reference part and attribute part
fn split_attribute(input: &str) -> (&str, Option<String>) {
    // Find the first # that's not part of a URL fragment in git+https etc.
    // For simplicity, we just split on the first #
    if let Some(pos) = input.find('#') {
        let ref_part = &input[..pos];
        let attr_part = &input[pos + 1..];
        if attr_part.is_empty() {
            (ref_part, None)
        } else {
            (ref_part, Some(attr_part.to_string()))
        }
    } else {
        (input, None)
    }
}

/// Parse just the flake reference part (without attribute)
pub fn parse_flake_ref(input: &str) -> Result<FlakeRef, ParseError> {
    if input.is_empty() {
        return Err(ParseError::new("empty flake reference"));
    }

    // Check for URL-like schemes first
    if let Some(rest) = input.strip_prefix("github:") {
        return parse_github_ref(rest);
    }
    if let Some(rest) = input.strip_prefix("gitlab:") {
        return parse_gitlab_ref(rest);
    }
    if let Some(rest) = input.strip_prefix("sourcehut:") {
        return parse_sourcehut_ref(rest);
    }
    if input.starts_with("git+") {
        return parse_git_ref(input);
    }
    if let Some(rest) = input.strip_prefix("path:") {
        return Ok(FlakeRef::Path {
            path: rest.to_string(),
        });
    }
    if let Some(rest) = input.strip_prefix("flake:") {
        return parse_indirect_ref(rest);
    }
    if input.starts_with("tarball+") || is_tarball_url(input) {
        return parse_tarball_ref(input);
    }
    if input.starts_with("file:") {
        return parse_file_ref(input);
    }
    if input.starts_with("http://") || input.starts_with("https://") {
        // Could be a tarball URL
        if is_tarball_url(input) {
            return parse_tarball_ref(input);
        }
        // Otherwise treat as git
        return parse_git_ref(&format!("git+{}", input));
    }

    // Check for path-like references
    if input == "."
        || input.starts_with("./")
        || input.starts_with("../")
        || input.starts_with('/')
    {
        return Ok(FlakeRef::Path {
            path: input.to_string(),
        });
    }

    // Otherwise, treat as indirect/registry reference
    parse_indirect_ref(input)
}

fn parse_github_ref(input: &str) -> Result<FlakeRef, ParseError> {
    // Format: owner/repo[/ref][?params]
    let (path_part, _params) = split_query_params(input);

    let parts: Vec<&str> = path_part.split('/').collect();
    if parts.len() < 2 {
        return Err(ParseError::new(
            "github: requires owner/repo format",
        ));
    }

    let owner = parts[0].to_string();
    let repo = parts[1].to_string();
    let ref_or_rev = if parts.len() > 2 {
        Some(parts[2..].join("/"))
    } else {
        None
    };

    if owner.is_empty() || repo.is_empty() {
        return Err(ParseError::new("github: owner and repo cannot be empty"));
    }

    Ok(FlakeRef::GitHub {
        owner,
        repo,
        ref_or_rev,
    })
}

fn parse_gitlab_ref(input: &str) -> Result<FlakeRef, ParseError> {
    let (path_part, _params) = split_query_params(input);

    let parts: Vec<&str> = path_part.split('/').collect();
    if parts.len() < 2 {
        return Err(ParseError::new(
            "gitlab: requires owner/repo format",
        ));
    }

    let owner = parts[0].to_string();
    let repo = parts[1].to_string();
    let ref_or_rev = if parts.len() > 2 {
        Some(parts[2..].join("/"))
    } else {
        None
    };

    Ok(FlakeRef::GitLab {
        owner,
        repo,
        ref_or_rev,
    })
}

fn parse_sourcehut_ref(input: &str) -> Result<FlakeRef, ParseError> {
    let (path_part, _params) = split_query_params(input);

    let parts: Vec<&str> = path_part.split('/').collect();
    if parts.len() < 2 {
        return Err(ParseError::new(
            "sourcehut: requires ~owner/repo format",
        ));
    }

    let owner = parts[0].to_string();
    let repo = parts[1].to_string();
    let ref_or_rev = if parts.len() > 2 {
        Some(parts[2..].join("/"))
    } else {
        None
    };

    Ok(FlakeRef::Sourcehut {
        owner,
        repo,
        ref_or_rev,
    })
}

fn parse_git_ref(input: &str) -> Result<FlakeRef, ParseError> {
    // Format: git+https://... or git+ssh://...
    let url_part = input.strip_prefix("git+").unwrap_or(input);
    let (url, params) = split_query_params(url_part);

    Ok(FlakeRef::Git {
        url: url.to_string(),
        params,
    })
}

fn parse_tarball_ref(input: &str) -> Result<FlakeRef, ParseError> {
    let url = input
        .strip_prefix("tarball+")
        .unwrap_or(input);

    Ok(FlakeRef::Tarball {
        url: url.to_string(),
    })
}

fn parse_file_ref(input: &str) -> Result<FlakeRef, ParseError> {
    // file:///path or file://localhost/path
    let path = input
        .strip_prefix("file://localhost")
        .or_else(|| input.strip_prefix("file://"))
        .unwrap_or(input);

    Ok(FlakeRef::File {
        path: path.to_string(),
    })
}

fn parse_indirect_ref(input: &str) -> Result<FlakeRef, ParseError> {
    // Format: id[/ref]
    let (path_part, _params) = split_query_params(input);

    let parts: Vec<&str> = path_part.split('/').collect();
    let id = parts[0].to_string();
    let ref_or_rev = if parts.len() > 1 {
        Some(parts[1..].join("/"))
    } else {
        None
    };

    if id.is_empty() {
        return Err(ParseError::new("indirect flake reference cannot be empty"));
    }

    // Validate that it looks like an identifier (not a path)
    if id.contains('.') && !id.contains('/') {
        // Could be a dotted identifier like "nixpkgs" - that's fine
    }

    Ok(FlakeRef::Indirect { id, ref_or_rev })
}

fn is_tarball_url(input: &str) -> bool {
    let lower = input.to_lowercase();
    lower.ends_with(".tar.gz")
        || lower.ends_with(".tar.xz")
        || lower.ends_with(".tar.bz2")
        || lower.ends_with(".tar")
        || lower.ends_with(".zip")
        || lower.ends_with(".tgz")
}

fn split_query_params(input: &str) -> (&str, HashMap<String, String>) {
    if let Some(pos) = input.find('?') {
        let path = &input[..pos];
        let query = &input[pos + 1..];
        let params = parse_query_string(query);
        (path, params)
    } else {
        (input, HashMap::new())
    }
}

fn parse_query_string(query: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    for part in query.split('&') {
        if let Some(pos) = part.find('=') {
            let key = &part[..pos];
            let value = &part[pos + 1..];
            params.insert(key.to_string(), value.to_string());
        } else if !part.is_empty() {
            params.insert(part.to_string(), String::new());
        }
    }
    params
}

impl FlakeUrl {
    /// Returns the flake reference as a string (without the attribute)
    pub fn flake_ref_string(&self) -> String {
        match &self.flake_ref {
            FlakeRef::Path { path } => path.clone(),
            FlakeRef::GitHub {
                owner,
                repo,
                ref_or_rev,
            } => {
                let mut s = format!("github:{}/{}", owner, repo);
                if let Some(r) = ref_or_rev {
                    s.push('/');
                    s.push_str(r);
                }
                s
            }
            FlakeRef::GitLab {
                owner,
                repo,
                ref_or_rev,
            } => {
                let mut s = format!("gitlab:{}/{}", owner, repo);
                if let Some(r) = ref_or_rev {
                    s.push('/');
                    s.push_str(r);
                }
                s
            }
            FlakeRef::Sourcehut {
                owner,
                repo,
                ref_or_rev,
            } => {
                let mut s = format!("sourcehut:{}/{}", owner, repo);
                if let Some(r) = ref_or_rev {
                    s.push('/');
                    s.push_str(r);
                }
                s
            }
            FlakeRef::Git { url, params } => {
                let mut s = format!("git+{}", url);
                if !params.is_empty() {
                    s.push('?');
                    let pairs: Vec<_> = params
                        .iter()
                        .map(|(k, v)| {
                            if v.is_empty() {
                                k.clone()
                            } else {
                                format!("{}={}", k, v)
                            }
                        })
                        .collect();
                    s.push_str(&pairs.join("&"));
                }
                s
            }
            FlakeRef::Tarball { url } => url.clone(),
            FlakeRef::Indirect { id, ref_or_rev } => {
                let mut s = id.clone();
                if let Some(r) = ref_or_rev {
                    s.push('/');
                    s.push_str(r);
                }
                s
            }
            FlakeRef::File { path } => format!("file://{}", path),
        }
    }

    /// Check if this is a local path reference
    pub fn is_local(&self) -> bool {
        matches!(self.flake_ref, FlakeRef::Path { .. } | FlakeRef::File { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== Path-based references ====================

    #[test]
    fn parse_current_dir() {
        let url = parse_flake_url(".").unwrap();
        assert_eq!(url.flake_ref, FlakeRef::Path { path: ".".into() });
        assert_eq!(url.attribute, None);
    }

    #[test]
    fn parse_current_dir_with_attr() {
        let url = parse_flake_url(".#default").unwrap();
        assert_eq!(url.flake_ref, FlakeRef::Path { path: ".".into() });
        assert_eq!(url.attribute, Some("default".into()));
    }

    #[test]
    fn parse_relative_path() {
        let url = parse_flake_url("./subdir").unwrap();
        assert_eq!(url.flake_ref, FlakeRef::Path { path: "./subdir".into() });
        assert_eq!(url.attribute, None);
    }

    #[test]
    fn parse_relative_path_with_attr() {
        let url = parse_flake_url("./foo/bar#mypackage").unwrap();
        assert_eq!(url.flake_ref, FlakeRef::Path { path: "./foo/bar".into() });
        assert_eq!(url.attribute, Some("mypackage".into()));
    }

    #[test]
    fn parse_parent_relative_path() {
        let url = parse_flake_url("../other-project").unwrap();
        assert_eq!(url.flake_ref, FlakeRef::Path { path: "../other-project".into() });
    }

    #[test]
    fn parse_absolute_path() {
        let url = parse_flake_url("/home/user/project").unwrap();
        assert_eq!(url.flake_ref, FlakeRef::Path { path: "/home/user/project".into() });
    }

    #[test]
    fn parse_absolute_path_with_attr() {
        let url = parse_flake_url("/nix/store/abc123#lib").unwrap();
        assert_eq!(url.flake_ref, FlakeRef::Path { path: "/nix/store/abc123".into() });
        assert_eq!(url.attribute, Some("lib".into()));
    }

    #[test]
    fn parse_path_scheme() {
        let url = parse_flake_url("path:./relative").unwrap();
        assert_eq!(url.flake_ref, FlakeRef::Path { path: "./relative".into() });
    }

    #[test]
    fn parse_path_scheme_absolute() {
        let url = parse_flake_url("path:/absolute/path").unwrap();
        assert_eq!(url.flake_ref, FlakeRef::Path { path: "/absolute/path".into() });
    }

    // ==================== GitHub references ====================

    #[test]
    fn parse_github_basic() {
        let url = parse_flake_url("github:NixOS/nixpkgs").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::GitHub {
                owner: "NixOS".into(),
                repo: "nixpkgs".into(),
                ref_or_rev: None,
            }
        );
    }

    #[test]
    fn parse_github_with_ref() {
        let url = parse_flake_url("github:NixOS/nixpkgs/nixos-23.11").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::GitHub {
                owner: "NixOS".into(),
                repo: "nixpkgs".into(),
                ref_or_rev: Some("nixos-23.11".into()),
            }
        );
    }

    #[test]
    fn parse_github_with_attr() {
        let url = parse_flake_url("github:NixOS/nixpkgs#hello").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::GitHub {
                owner: "NixOS".into(),
                repo: "nixpkgs".into(),
                ref_or_rev: None,
            }
        );
        assert_eq!(url.attribute, Some("hello".into()));
    }

    #[test]
    fn parse_github_with_ref_and_attr() {
        let url = parse_flake_url("github:NixOS/nixpkgs/nixos-unstable#legacyPackages.x86_64-linux.hello").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::GitHub {
                owner: "NixOS".into(),
                repo: "nixpkgs".into(),
                ref_or_rev: Some("nixos-unstable".into()),
            }
        );
        assert_eq!(url.attribute, Some("legacyPackages.x86_64-linux.hello".into()));
    }

    #[test]
    fn parse_github_with_deep_ref() {
        // Some repos have branches with slashes
        let url = parse_flake_url("github:owner/repo/feature/branch").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::GitHub {
                owner: "owner".into(),
                repo: "repo".into(),
                ref_or_rev: Some("feature/branch".into()),
            }
        );
    }

    #[test]
    fn parse_github_error_no_repo() {
        let result = parse_flake_url("github:owner");
        assert!(result.is_err());
    }

    #[test]
    fn parse_github_error_empty() {
        let result = parse_flake_url("github:");
        assert!(result.is_err());
    }

    // ==================== GitLab references ====================

    #[test]
    fn parse_gitlab_basic() {
        let url = parse_flake_url("gitlab:inkscape/inkscape").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::GitLab {
                owner: "inkscape".into(),
                repo: "inkscape".into(),
                ref_or_rev: None,
            }
        );
    }

    #[test]
    fn parse_gitlab_with_ref() {
        let url = parse_flake_url("gitlab:inkscape/inkscape/master").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::GitLab {
                owner: "inkscape".into(),
                repo: "inkscape".into(),
                ref_or_rev: Some("master".into()),
            }
        );
    }

    // ==================== Sourcehut references ====================

    #[test]
    fn parse_sourcehut_basic() {
        let url = parse_flake_url("sourcehut:~sircmpwn/aerc").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::Sourcehut {
                owner: "~sircmpwn".into(),
                repo: "aerc".into(),
                ref_or_rev: None,
            }
        );
    }

    // ==================== Git references ====================

    #[test]
    fn parse_git_https() {
        let url = parse_flake_url("git+https://github.com/NixOS/nixpkgs").unwrap();
        if let FlakeRef::Git { url: git_url, params } = url.flake_ref {
            assert_eq!(git_url, "https://github.com/NixOS/nixpkgs");
            assert!(params.is_empty());
        } else {
            panic!("expected Git ref");
        }
    }

    #[test]
    fn parse_git_ssh() {
        let url = parse_flake_url("git+ssh://git@github.com/owner/repo").unwrap();
        if let FlakeRef::Git { url: git_url, .. } = url.flake_ref {
            assert_eq!(git_url, "ssh://git@github.com/owner/repo");
        } else {
            panic!("expected Git ref");
        }
    }

    #[test]
    fn parse_git_with_params() {
        let url = parse_flake_url("git+https://example.com/repo?ref=main&rev=abc123").unwrap();
        if let FlakeRef::Git { url: git_url, params } = url.flake_ref {
            assert_eq!(git_url, "https://example.com/repo");
            assert_eq!(params.get("ref"), Some(&"main".to_string()));
            assert_eq!(params.get("rev"), Some(&"abc123".to_string()));
        } else {
            panic!("expected Git ref");
        }
    }

    #[test]
    fn parse_git_with_dir_param() {
        let url = parse_flake_url("git+https://example.com/repo?dir=subdir").unwrap();
        if let FlakeRef::Git { params, .. } = url.flake_ref {
            assert_eq!(params.get("dir"), Some(&"subdir".to_string()));
        } else {
            panic!("expected Git ref");
        }
    }

    // ==================== Tarball references ====================

    #[test]
    fn parse_tarball_https() {
        let url = parse_flake_url("https://example.com/flake.tar.gz").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::Tarball {
                url: "https://example.com/flake.tar.gz".into()
            }
        );
    }

    #[test]
    fn parse_tarball_explicit() {
        let url = parse_flake_url("tarball+https://example.com/archive.tar.gz").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::Tarball {
                url: "https://example.com/archive.tar.gz".into()
            }
        );
    }

    #[test]
    fn parse_tarball_zip() {
        let url = parse_flake_url("https://example.com/archive.zip").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::Tarball {
                url: "https://example.com/archive.zip".into()
            }
        );
    }

    // ==================== Indirect/registry references ====================

    #[test]
    fn parse_indirect_simple() {
        let url = parse_flake_url("nixpkgs").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::Indirect {
                id: "nixpkgs".into(),
                ref_or_rev: None,
            }
        );
    }

    #[test]
    fn parse_indirect_with_attr() {
        let url = parse_flake_url("nixpkgs#hello").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::Indirect {
                id: "nixpkgs".into(),
                ref_or_rev: None,
            }
        );
        assert_eq!(url.attribute, Some("hello".into()));
    }

    #[test]
    fn parse_indirect_with_ref() {
        let url = parse_flake_url("nixpkgs/nixos-23.11").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::Indirect {
                id: "nixpkgs".into(),
                ref_or_rev: Some("nixos-23.11".into()),
            }
        );
    }

    #[test]
    fn parse_indirect_explicit() {
        let url = parse_flake_url("flake:nixpkgs").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::Indirect {
                id: "nixpkgs".into(),
                ref_or_rev: None,
            }
        );
    }

    #[test]
    fn parse_indirect_home_manager() {
        let url = parse_flake_url("home-manager").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::Indirect {
                id: "home-manager".into(),
                ref_or_rev: None,
            }
        );
    }

    // ==================== File references ====================

    #[test]
    fn parse_file_absolute() {
        let url = parse_flake_url("file:///home/user/flake").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::File {
                path: "/home/user/flake".into()
            }
        );
    }

    #[test]
    fn parse_file_localhost() {
        let url = parse_flake_url("file://localhost/home/user/flake").unwrap();
        assert_eq!(
            url.flake_ref,
            FlakeRef::File {
                path: "/home/user/flake".into()
            }
        );
    }

    // ==================== Attribute parsing ====================

    #[test]
    fn parse_dotted_attribute() {
        let url = parse_flake_url(".#packages.x86_64-linux.default").unwrap();
        assert_eq!(url.attribute, Some("packages.x86_64-linux.default".into()));
    }

    #[test]
    fn parse_complex_attribute() {
        let url = parse_flake_url("nixpkgs#legacyPackages.x86_64-linux.python3Packages.requests").unwrap();
        assert_eq!(
            url.attribute,
            Some("legacyPackages.x86_64-linux.python3Packages.requests".into())
        );
    }

    #[test]
    fn parse_empty_attribute_ignored() {
        let url = parse_flake_url(".#").unwrap();
        assert_eq!(url.attribute, None);
    }

    // ==================== Edge cases ====================

    #[test]
    fn parse_error_empty_string() {
        let result = parse_flake_url("");
        assert!(result.is_err());
    }

    #[test]
    fn is_local_path() {
        assert!(parse_flake_url(".").unwrap().is_local());
        assert!(parse_flake_url("./foo").unwrap().is_local());
        assert!(parse_flake_url("/abs").unwrap().is_local());
        assert!(parse_flake_url("path:./foo").unwrap().is_local());
        assert!(parse_flake_url("file:///foo").unwrap().is_local());
    }

    #[test]
    fn is_not_local() {
        assert!(!parse_flake_url("github:o/r").unwrap().is_local());
        assert!(!parse_flake_url("nixpkgs").unwrap().is_local());
        assert!(!parse_flake_url("git+https://x").unwrap().is_local());
    }

    #[test]
    fn flake_ref_string_roundtrip() {
        let cases = [
            ".",
            "./foo/bar",
            "/absolute/path",
            "github:NixOS/nixpkgs",
            "github:owner/repo/branch",
            "gitlab:owner/repo",
            "nixpkgs",
        ];

        for case in cases {
            let url = parse_flake_url(case).unwrap();
            let s = url.flake_ref_string();
            // Re-parse and compare
            let url2 = parse_flake_url(&s).unwrap();
            assert_eq!(url.flake_ref, url2.flake_ref, "roundtrip failed for {}", case);
        }
    }
}
