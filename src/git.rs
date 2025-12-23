use anyhow::{Context, Result};
use git2::{Repository, StatusOptions};
use std::path::Path;

/// Git metadata for an input.
///
/// Matches Nix's behavior:
/// - Clean repo: rev, shortRev, lastModified, lastModifiedDate, revCount
/// - Dirty repo: dirtyRev, dirtyShortRev, lastModified, lastModifiedDate
#[derive(Debug, Clone, Default)]
pub struct GitInfo {
    /// Full commit hash (only when clean)
    pub rev: Option<String>,
    /// Short commit hash (only when clean)
    pub short_rev: Option<String>,
    /// Full commit hash with "-dirty" suffix (only when dirty)
    pub dirty_rev: Option<String>,
    /// Short commit hash with "-dirty" suffix (only when dirty)
    pub dirty_short_rev: Option<String>,
    /// Unix timestamp of last commit
    pub last_modified: Option<i64>,
    /// Formatted date string YYYYMMDDHHMMSS
    pub last_modified_date: Option<String>,
    /// Number of commits (only when clean)
    pub rev_count: Option<i64>,
}

/// Get git metadata for a directory using libgit2.
///
/// Matches Nix's behavior where clean and dirty repos expose different attributes.
/// Returns default (empty) GitInfo if the directory is not a git repository.
pub fn get_git_info(path: &Path) -> Result<GitInfo> {
    // Try to open the repository; if it fails, it's not a git repo
    let repo = match Repository::discover(path) {
        Ok(r) => r,
        Err(_) => return Ok(GitInfo::default()),
    };

    let mut info = GitInfo::default();

    // Get HEAD commit
    let head = repo
        .head()
        .context("Failed to get HEAD reference")?
        .peel_to_commit()
        .context("Failed to peel HEAD to commit")?;

    let rev = head.id().to_string();
    let short_rev: String = rev.chars().take(7).collect();

    // Check for dirty status (tracked files only, matching Nix behavior)
    let is_dirty = is_repo_dirty(&repo)?;

    if is_dirty {
        // Dirty repo: only dirtyRev and dirtyShortRev
        info.dirty_rev = Some(format!("{}-dirty", rev));
        info.dirty_short_rev = Some(format!("{}-dirty", short_rev));
    } else {
        // Clean repo: rev, shortRev, and revCount
        info.rev = Some(rev.clone());
        info.short_rev = Some(short_rev);
        info.rev_count = Some(count_commits(&repo, &head)?);
    }

    // Get last modified time (always included)
    let commit_time = head.time().seconds();
    info.last_modified = Some(commit_time);

    // Format as YYYYMMDDHHMMSS like Nix does
    if let Some(dt) = chrono::DateTime::from_timestamp(commit_time, 0) {
        info.last_modified_date = Some(dt.format("%Y%m%d%H%M%S").to_string());
    }

    Ok(info)
}

/// Check if the repository has uncommitted changes to tracked files.
fn is_repo_dirty(repo: &Repository) -> Result<bool> {
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

/// Count the number of commits reachable from HEAD.
fn count_commits(repo: &Repository, head: &git2::Commit) -> Result<i64> {
    let mut revwalk = repo.revwalk().context("Failed to create revwalk")?;
    revwalk
        .push(head.id())
        .context("Failed to push HEAD to revwalk")?;

    let count = revwalk.count() as i64;
    Ok(count)
}
