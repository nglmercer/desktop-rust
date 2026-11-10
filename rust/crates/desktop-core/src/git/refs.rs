//! Port of `app/src/lib/git/for-each-ref.ts`, `branch.ts`, `remote.ts`,
//! `tag.ts`, `rev-list.ts` and `config.ts`.

use std::path::Path;

use crate::error::Result;
use crate::models::branch::{Branch, BranchTip, BranchType};
use crate::models::repository::{Remote, WorktreeEntry};

use super::core::{GitEngine, GitOptions};

/// `getBranches` — local + remote refs in one `for-each-ref` call, matching
/// the delimiter format Desktop uses.
pub fn get_branches(git: &GitEngine, repo: &Path) -> Result<Vec<Branch>> {
    let format =
        "--format=%(refname)%1f%(objectname)%1f%(upstream:short)%1f%(HEAD)%1f%(refname:short)";
    let out = git.run(
        &["for-each-ref", format, "refs/heads", "refs/remotes"],
        repo,
        GitOptions::new().success(128),
    )?;
    let current = get_current_branch(git, repo).unwrap_or_default();

    Ok(out
        .stdout_lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\x1f').collect();
            if fields.len() < 3 {
                return None;
            }
            let name = fields[4].trim().to_string();
            let kind = if fields[0].starts_with("refs/remotes/") {
                BranchType::Remote
            } else {
                BranchType::Local
            };
            let upstream = fields[2].trim();
            Some(Branch {
                is_head: fields[3].trim() == "*" || name == current,
                name,
                upstream: if upstream.is_empty() {
                    None
                } else {
                    Some(upstream.to_string())
                },
                tip: BranchTip {
                    sha: fields[1].trim().to_string(),
                },
                kind,
                ref_name: fields[0].trim().to_string(),
            })
        })
        .collect())
}

pub fn get_current_branch(git: &GitEngine, repo: &Path) -> Option<String> {
    let out = git
        .run(
            &["symbolic-ref", "--short", "HEAD"],
            repo,
            GitOptions::new().success(1).success(128),
        )
        .ok()?;
    if out.exit_code != 0 {
        return None;
    }
    Some(out.stdout.trim().to_string())
}

pub fn get_branch_names(git: &GitEngine, repo: &Path, remote: bool) -> Result<Vec<String>> {
    let reference = if remote { "refs/remotes" } else { "refs/heads" };
    let format = "--format=%(refname:short)";
    let out = git.run(
        &["for-each-ref", format, reference],
        repo,
        GitOptions::new().success(128),
    )?;
    Ok(out.stdout_lines().map(str::to_string).collect())
}

pub fn get_remotes(git: &GitEngine, repo: &Path) -> Result<Vec<Remote>> {
    let out = git.run(&["remote", "-v"], repo, GitOptions::new().success(128))?;
    let mut seen = std::collections::BTreeMap::new();
    for line in out.stdout_lines() {
        let mut parts = line.split_whitespace();
        if let (Some(name), Some(url)) = (parts.next(), parts.next()) {
            seen.entry(name.to_string())
                .or_insert_with(|| url.to_string());
        }
    }
    Ok(seen
        .into_iter()
        .map(|(name, url)| Remote { name, url })
        .collect())
}

pub fn get_tags(git: &GitEngine, repo: &Path) -> Result<Vec<String>> {
    let out = git.run(&["tag", "--list"], repo, GitOptions::new().success(128))?;
    Ok(out.stdout_lines().map(str::to_string).collect())
}

pub fn get_merge_base(git: &GitEngine, repo: &Path, a: &str, b: &str) -> Result<Option<String>> {
    let out = git.run(
        &["merge-base", a, b],
        repo,
        GitOptions::new().success(1).success(128),
    )?;
    if out.exit_code != 0 {
        return Ok(None);
    }
    Ok(Some(out.stdout.trim().to_string()))
}

pub fn get_worktrees(git: &GitEngine, repo: &Path) -> Result<Vec<WorktreeEntry>> {
    let out = git.run(
        &["worktree", "list", "--porcelain", "-z"],
        repo,
        GitOptions::new().success(128),
    )?;
    let mut result = Vec::new();
    let mut current = WorktreeEntry {
        path: String::new(),
        head: String::new(),
        branch: None,
        is_bare: false,
        is_locked: false,
        is_prunable: false,
    };
    for record in out.stdout.split('\0') {
        let record = record.trim();
        if record.is_empty() {
            continue;
        }
        if let Some(path) = record.strip_prefix("worktree ") {
            if !current.path.is_empty() {
                result.push(std::mem::take(&mut current));
            }
            current.path = path.to_string();
        } else if let Some(head) = record.strip_prefix("HEAD ") {
            current.head = head.to_string();
        } else if let Some(branch) = record.strip_prefix("branch ") {
            current.branch = Some(branch.trim_start_matches("refs/heads/").to_string());
        } else if record == "bare" {
            current.is_bare = true;
        } else if record == "locked" {
            current.is_locked = true;
        } else if record == "prunable" {
            current.is_prunable = true;
        }
    }
    if !current.path.is_empty() {
        result.push(current);
    }
    Ok(result)
}
