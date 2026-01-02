use crate::common::Cache;
use anyhow::{Context, Result};
use git2::{Repository, StatusOptions};
use std::path::{Path, PathBuf};

/// Cache for git info per directory (canonical path -> GitInfo)
static GIT_INFO_CACHE: Cache<PathBuf, GitInfo> = Cache::new();

/// Git metadata for an input.
///
/// Matches Nix's behavior:
/// - Clean repo: rev, shortRev, lastModified, lastModifiedDate
/// - Dirty repo: dirtyRev, dirtyShortRev, lastModified, lastModifiedDate
/// - Always: submodules
///
/// Note: We intentionally do NOT compute `revCount`. Computing it requires
/// walking the entire commit history, which takes ~4 seconds even with git's
/// commit-graph optimization (or ~30 seconds with libgit2). Nix caches this
/// per-commit in ~/.cache/nix/fetcher-cache-v4.sqlite, but we don't want to
/// maintain a separate cache. Most flakes don't use revCount anyway, and Nix
/// itself is moving toward not computing it by default for local repos.
use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    /// Full commit hash (only when clean)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// Short commit hash (only when clean)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_rev: Option<String>,
    /// Full commit hash with "-dirty" suffix (only when dirty)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dirty_rev: Option<String>,
    /// Short commit hash with "-dirty" suffix (only when dirty)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dirty_short_rev: Option<String>,
    /// Unix timestamp of last commit
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<i64>,
    /// Formatted date string YYYYMMDDHHMMSS
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified_date: Option<String>,
    /// Whether the repository has submodules
    pub submodules: bool,
}

/// Get git metadata for a directory using libgit2.
///
/// Matches Nix's behavior where clean and dirty repos expose different attributes.
/// Returns default (empty) GitInfo if the directory is not a git repository.
/// Results are cached per canonical path.
pub fn get_git_info(path: &Path) -> Result<GitInfo> {
    // Canonicalize path for cache key
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    // Check cache first
    if let Some(info) = GIT_INFO_CACHE.get(&canonical) {
        tracing::debug!("get_git_info: cache hit");
        return Ok(info);
    }

    tracing::debug!("get_git_info: cache miss, computing...");
    let start = std::time::Instant::now();

    // Try to open the repository; if it fails, it's not a git repo
    let repo = match Repository::discover(path) {
        Ok(r) => r,
        Err(_) => return Ok(GitInfo::default()),
    };
    tracing::debug!("get_git_info: repo open took {:?}", start.elapsed());

    let mut info = GitInfo::default();

    // Get HEAD commit
    let head = repo
        .head()
        .context("Failed to get HEAD reference")?
        .peel_to_commit()
        .context("Failed to peel HEAD to commit")?;

    let rev = head.id().to_string();
    let short_rev: String = rev.chars().take(7).collect();
    tracing::debug!("get_git_info: got HEAD in {:?}", start.elapsed());

    // Check for dirty status (tracked files only, matching Nix behavior)
    let dirty_start = std::time::Instant::now();
    let is_dirty = is_repo_dirty(&repo)?;
    tracing::debug!(
        "get_git_info: is_repo_dirty={} took {:?}",
        is_dirty,
        dirty_start.elapsed()
    );

    if is_dirty {
        // Dirty repo: only dirtyRev and dirtyShortRev
        info.dirty_rev = Some(format!("{}-dirty", rev));
        info.dirty_short_rev = Some(format!("{}-dirty", short_rev));
    } else {
        // Clean repo: rev and shortRev
        info.rev = Some(rev.clone());
        info.short_rev = Some(short_rev);
    }

    // Get last modified time (always included)
    let commit_time = head.time().seconds();
    info.last_modified = Some(commit_time);

    // Format as YYYYMMDDHHMMSS like Nix does
    if let Some(dt) = chrono::DateTime::from_timestamp(commit_time, 0) {
        info.last_modified_date = Some(dt.format("%Y%m%d%H%M%S").to_string());
    }

    // Check for submodules
    info.submodules = has_submodules(&repo);

    // Cache the result
    GIT_INFO_CACHE.insert(canonical, info.clone());

    Ok(info)
}

/// Check if the repository has any submodules.
fn has_submodules(repo: &Repository) -> bool {
    repo.submodules()
        .map(|subs| !subs.is_empty())
        .unwrap_or(false)
}

/// Check if the repository has uncommitted changes to tracked files.
///
/// Tries `git status` first (fast), falls back to libgit2 if git isn't available.
fn is_repo_dirty(repo: &Repository) -> Result<bool> {
    // Try fast path: shell out to git
    if let Some(workdir) = repo.workdir() {
        if let Ok(dirty) = is_repo_dirty_git(workdir) {
            return Ok(dirty);
        }
    }

    // Fallback: use libgit2
    is_repo_dirty_libgit2(repo)
}

/// Check dirty status using `git status` (fast).
fn is_repo_dirty_git(repo_path: &Path) -> Result<bool> {
    let output = std::process::Command::new("git")
        .args([
            "-C",
            &repo_path.display().to_string(),
            "status",
            "--porcelain",
            "--untracked-files=no",
        ])
        .output()
        .context("Failed to run git status")?;

    if !output.status.success() {
        anyhow::bail!("git status failed");
    }

    // If output is empty, repo is clean
    Ok(!output.stdout.is_empty())
}

/// Check dirty status using libgit2 (fallback).
fn is_repo_dirty_libgit2(repo: &Repository) -> Result<bool> {
    let mut opts = StatusOptions::new();
    // Only check tracked files (no untracked), matching Nix behavior
    opts.include_untracked(false)
        .include_ignored(false)
        .include_unmodified(false);

    let statuses = repo
        .statuses(Some(&mut opts))
        .context("Failed to get repository status")?;

    Ok(!statuses.is_empty())
}
