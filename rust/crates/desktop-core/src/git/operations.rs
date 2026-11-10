//! Port of the mutating git modules in `app/src/lib/git/*.ts`:
//! `checkout`, `commit`, `add`, `reset`, `apply`, `fetch`, `pull`, `push`,
//! `merge`, `rebase`, `cherry-pick`, `revert`, `clone`, `branch`, `tag`,
//! `remote`, `stash`, `worktree` and `format-patch`.
//!
//! Every helper returns the raw [`GitOutput`] so the dispatcher can decide
//! whether to classify stderr, refresh status, or emit a banner.

use std::path::Path;

use crate::error::Result;
use crate::models::branch::StashEntry;

use super::core::{GitEngine, GitOptions, GitOutput};

fn exec(git: &GitEngine, repo: &Path, args: Vec<&str>) -> Result<GitOutput> {
    let refs: Vec<&str> = args.clone();
    git.run(&refs, repo, GitOptions::new())
}

fn exec_owned(git: &GitEngine, repo: &Path, args: Vec<String>) -> Result<GitOutput> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    git.run(&refs, repo, GitOptions::new())
}

// -- checkout ----------------------------------------------------------------

pub fn checkout_branch(git: &GitEngine, repo: &Path, branch: &str) -> Result<GitOutput> {
    exec(git, repo, vec!["checkout", "--progress", branch, "--"])
}

pub fn create_and_checkout_branch(
    git: &GitEngine,
    repo: &Path,
    name: &str,
    start_point: Option<&str>,
) -> Result<GitOutput> {
    let mut args = vec!["checkout", "-b", name];
    if let Some(start) = start_point {
        args.push(start);
    }
    exec(git, repo, args)
}

pub fn checkout_commit(git: &GitEngine, repo: &Path, sha: &str) -> Result<GitOutput> {
    exec(git, repo, vec!["checkout", "--detach", sha])
}

pub fn checkout_paths(git: &GitEngine, repo: &Path, paths: &[String]) -> Result<GitOutput> {
    let mut owned = vec!["checkout".to_string(), "--".to_string()];
    owned.extend(paths.iter().cloned());
    exec_owned(git, repo, owned)
}

// -- staging / commit --------------------------------------------------------

pub fn stage_paths(git: &GitEngine, repo: &Path, paths: &[String]) -> Result<GitOutput> {
    let mut owned = vec!["add".to_string(), "--".to_string()];
    owned.extend(paths.iter().cloned());
    exec_owned(git, repo, owned)
}

pub fn unstage_all(git: &GitEngine, repo: &Path) -> Result<GitOutput> {
    exec(git, repo, vec!["reset", "--mixed", "HEAD", "--"])
}

pub fn create_commit(git: &GitEngine, repo: &Path, summary: &str, body: &str) -> Result<GitOutput> {
    let message = if body.trim().is_empty() {
        summary.to_string()
    } else {
        format!("{summary}\n\n{body}")
    };
    exec(git, repo, vec!["commit", "-m", &message])
}

pub fn amend_commit(git: &GitEngine, repo: &Path, summary: &str, body: &str) -> Result<GitOutput> {
    let message = format!("{summary}\n\n{body}");
    exec(git, repo, vec!["commit", "--amend", "-m", &message])
}

pub fn discard_changes(git: &GitEngine, repo: &Path, paths: &[String]) -> Result<GitOutput> {
    let mut owned = vec!["checkout".to_string(), "--".to_string()];
    owned.extend(paths.iter().cloned());
    exec_owned(git, repo, owned)
}

pub fn reset_to_commit(git: &GitEngine, repo: &Path, sha: &str, mode: &str) -> Result<GitOutput> {
    exec(git, repo, vec!["reset", mode, sha])
}

// -- branches / tags / remotes ----------------------------------------------

pub fn create_branch(
    git: &GitEngine,
    repo: &Path,
    name: &str,
    start_point: &str,
) -> Result<GitOutput> {
    exec(git, repo, vec!["branch", name, start_point])
}

pub fn rename_branch(git: &GitEngine, repo: &Path, old: &str, new: &str) -> Result<GitOutput> {
    exec(git, repo, vec!["branch", "-m", old, new])
}

pub fn delete_local_branch(git: &GitEngine, repo: &Path, name: &str) -> Result<GitOutput> {
    exec(git, repo, vec!["branch", "-D", name])
}

pub fn delete_remote_branch(
    git: &GitEngine,
    repo: &Path,
    remote: &str,
    name: &str,
) -> Result<GitOutput> {
    let refspec = format!(":{name}");
    exec(git, repo, vec!["push", remote, &refspec])
}

pub fn create_tag(git: &GitEngine, repo: &Path, name: &str, target: &str) -> Result<GitOutput> {
    exec(git, repo, vec!["tag", "-a", name, "-m", name, target])
}

pub fn delete_tag(git: &GitEngine, repo: &Path, name: &str) -> Result<GitOutput> {
    exec(git, repo, vec!["tag", "-d", name])
}

pub fn add_remote(git: &GitEngine, repo: &Path, name: &str, url: &str) -> Result<GitOutput> {
    exec(git, repo, vec!["remote", "add", name, url])
}

pub fn set_remote_url(git: &GitEngine, repo: &Path, name: &str, url: &str) -> Result<GitOutput> {
    exec(git, repo, vec!["remote", "set-url", name, url])
}

// -- network operations ------------------------------------------------------

pub fn fetch(git: &GitEngine, repo: &Path, remote: &str, prune: bool) -> Result<GitOutput> {
    let args = vec![
        "fetch".to_string(),
        "--progress".to_string(),
        format!("--prune={prune}"),
        remote.to_string(),
    ];
    exec_owned(git, repo, args)
}

pub fn fetch_refspec(
    git: &GitEngine,
    repo: &Path,
    remote: &str,
    refspec: &str,
) -> Result<GitOutput> {
    exec(git, repo, vec!["fetch", remote, refspec])
}

pub fn pull(
    git: &GitEngine,
    repo: &Path,
    remote: &str,
    branch: Option<&str>,
    rebase: bool,
) -> Result<GitOutput> {
    let mut owned = vec!["pull".to_string(), "--progress".to_string()];
    if rebase {
        owned.push("--rebase".to_string());
    }
    owned.push(remote.to_string());
    if let Some(b) = branch {
        owned.push(b.to_string());
    }
    exec_owned(git, repo, owned)
}

pub fn push(
    git: &GitEngine,
    repo: &Path,
    remote: &str,
    branch: &str,
    set_upstream: bool,
    force_with_lease: bool,
) -> Result<GitOutput> {
    let mut owned = vec!["push".to_string(), "--progress".to_string()];
    if set_upstream {
        owned.push("-u".to_string());
    }
    if force_with_lease {
        owned.push("--force-with-lease".to_string());
    }
    owned.push(remote.to_string());
    owned.push(branch.to_string());
    exec_owned(git, repo, owned)
}

pub fn clone(
    git: &GitEngine,
    parent: &Path,
    url: &str,
    target: &str,
    branch: Option<&str>,
) -> Result<GitOutput> {
    let mut owned = vec!["clone".to_string(), "--progress".to_string()];
    if let Some(b) = branch {
        owned.push("--branch".to_string());
        owned.push(b.to_string());
    }
    owned.push(url.to_string());
    owned.push(target.to_string());
    exec_owned(git, parent, owned)
}

// -- merge / rebase / cherry-pick / revert -----------------------------------

pub fn merge(git: &GitEngine, repo: &Path, branch: &str, squash: bool) -> Result<GitOutput> {
    let mut args = vec!["merge", "--no-edit"];
    if squash {
        args.push("--squash");
    }
    args.push(branch);
    exec(git, repo, args)
}

pub fn abort_merge(git: &GitEngine, repo: &Path) -> Result<GitOutput> {
    exec(git, repo, vec!["merge", "--abort"])
}

pub fn continue_rebase(git: &GitEngine, repo: &Path) -> Result<GitOutput> {
    exec(git, repo, vec!["rebase", "--continue"])
}

pub fn abort_rebase(git: &GitEngine, repo: &Path) -> Result<GitOutput> {
    exec(git, repo, vec!["rebase", "--abort"])
}

pub fn rebase(git: &GitEngine, repo: &Path, branch: &str, interactive: bool) -> Result<GitOutput> {
    let mut args = vec!["rebase"];
    if interactive {
        args.push("-i");
    }
    args.push(branch);
    exec(git, repo, args)
}

pub fn cherry_pick(git: &GitEngine, repo: &Path, sha: &str) -> Result<GitOutput> {
    exec(git, repo, vec!["cherry-pick", sha])
}

pub fn revert_commit(git: &GitEngine, repo: &Path, sha: &str) -> Result<GitOutput> {
    exec(git, repo, vec!["revert", "--no-edit", sha])
}

// -- stash -------------------------------------------------------------------

const DESKTOP_STASH_PREFIX: &str = "!!GitHub_Desktop<";

pub fn create_desktop_stash(git: &GitEngine, repo: &Path, branch: &str) -> Result<GitOutput> {
    let message = format!("{DESKTOP_STASH_PREFIX}{branch}>");
    exec(
        git,
        repo,
        vec!["stash", "push", "--include-untracked", "-m", &message],
    )
}

pub fn get_stashes(git: &GitEngine, repo: &Path) -> Result<Vec<StashEntry>> {
    let out = git.run(
        &["stash", "list", "--format=%gd\x1f%H\x1f%gs"],
        repo,
        GitOptions::new().success(128),
    )?;
    Ok(out
        .stdout_lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\x1f').collect();
            if fields.len() < 3 {
                return None;
            }
            let branch_name = fields[2]
                .split(DESKTOP_STASH_PREFIX)
                .nth(1)
                .and_then(|s| s.split('>').next())
                .unwrap_or_default()
                .to_string();
            Some(StashEntry {
                name: fields[0].to_string(),
                sha: fields[1].to_string(),
                branch_name,
                files: Vec::new(),
            })
        })
        .collect())
}

pub fn pop_stash(git: &GitEngine, repo: &Path, name: &str) -> Result<GitOutput> {
    exec(git, repo, vec!["stash", "pop", name])
}

pub fn drop_stash(git: &GitEngine, repo: &Path, name: &str) -> Result<GitOutput> {
    exec(git, repo, vec!["stash", "drop", name])
}

// -- worktrees / patches -----------------------------------------------------

pub fn add_worktree(git: &GitEngine, repo: &Path, path: &str, branch: &str) -> Result<GitOutput> {
    exec(git, repo, vec!["worktree", "add", path, branch])
}

pub fn remove_worktree(git: &GitEngine, repo: &Path, path: &str) -> Result<GitOutput> {
    exec(git, repo, vec!["worktree", "remove", path])
}

pub fn format_patch(
    git: &GitEngine,
    repo: &Path,
    range: &str,
    output_dir: &str,
) -> Result<GitOutput> {
    exec(git, repo, vec!["format-patch", range, "-o", output_dir])
}

pub fn apply_patch(git: &GitEngine, repo: &Path, patch: &str) -> Result<GitOutput> {
    let opts = GitOptions {
        stdin: Some(patch.to_string()),
        ..GitOptions::new()
    };
    git.run(&["apply", "--whitespace=nowarn", "-"], repo, opts)
}
