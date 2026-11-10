//! Port of `app/src/lib/status-parser.ts` + `app/src/lib/git/status.ts`.
//!
//! Uses the exact same porcelain format as Desktop
//! (`status --porcelain=v1 -z --branch --untracked-files=all`) so conflict,
//! rename and submodule states map onto the same `StatusResult`.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::models::repository::AheadBehind;
use crate::models::status::{
    AppFileStatusKind, StatusResult, WorkingDirectoryFileChange, git_status_entry, status_kind,
};

use super::core::{GitEngine, GitOptions};

/// `getStatus` — the primary repository refresh call.
pub fn get_status(git: &GitEngine, repo: &Path) -> Result<StatusResult> {
    let mut result = StatusResult {
        exists: true,
        ..Default::default()
    };

    let raw = git
        .run(
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--branch",
                "--untracked-files=all",
            ],
            repo,
            GitOptions::new().success(128),
        )
        .unwrap_or_default_losing_errors();

    if raw.exit_code != 0 && raw.stdout.is_empty() {
        result.exists = false;
        return Ok(result);
    }

    let records: Vec<&str> = raw.stdout.split('\0').filter(|s| !s.is_empty()).collect();
    let mut index = 0usize;

    if let Some(first) = records.first() {
        if let Some(header) = first.strip_prefix("## ") {
            index = 1;
            parse_branch_header(header, &mut result);
        }
    }

    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.len() < 4
            || !record
                .starts_with(|c: char| c == ' ' || c.is_ascii_alphabetic() || c == '?' || c == '!')
        {
            continue;
        }
        let bytes = record.as_bytes();
        let x = bytes[0] as char;
        let y = bytes[1] as char;
        // A path starts at byte 3 ("XY " prefix).
        let path = record.get(3..).unwrap_or_default().to_string();
        if path.is_empty() {
            continue;
        }

        let is_rename = x == 'R' || y == 'R' || x == 'C' || y == 'C';
        let old_path = if is_rename && index < records.len() {
            let candidate = records[index];
            // The following NUL record is the source path. Guard against the
            // next status entry being mistaken for it.
            if candidate.len() >= 4 && candidate.as_bytes()[2] == b' ' {
                None
            } else {
                index += 1;
                Some(candidate.to_string())
            }
        } else {
            None
        };

        let kind = status_kind(x, y);
        if kind == AppFileStatusKind::Conflicted {
            result.do_conflicted_files_exist = true;
        }

        result
            .working_directory_files
            .push(WorkingDirectoryFileChange {
                change: crate::models::status::FileChange {
                    path,
                    status: kind,
                    old_path,
                },
                index_status: git_status_entry(x),
                working_tree_status: git_status_entry(y),
                selected: false,
            });
    }

    result.current_tip = git
        .run(&["rev-parse", "HEAD"], repo, GitOptions::new().success(128))
        .ok()
        .filter(|o| o.exit_code == 0)
        .map(|o| o.stdout.trim().to_string());

    result.merge_head_found = git_dir(repo).join("MERGE_HEAD").exists();
    result.squash_msg_found = git_dir(repo).join("SQUASH_MSG").exists();
    result.is_detached_head = result.current_branch.is_none()
        || result.current_tip.is_some() && {
            git.run(
                &["symbolic-ref", "-q", "HEAD"],
                repo,
                GitOptions::new().success(1),
            )
            .map(|o| o.exit_code != 0)
            .unwrap_or(true)
        };

    if let (Some(branch), Some(upstream)) = (
        result.current_branch.clone(),
        result.current_upstream_branch.clone(),
    ) {
        result.branch_ahead_behind = Some(get_ahead_behind(
            git,
            repo,
            &format!("{branch}...{upstream}"),
        ));
    }

    Ok(result)
}

fn parse_branch_header(header: &str, result: &mut StatusResult) {
    // Examples: "main...origin/main [ahead 1, behind 2]", "HEAD (no branch)",
    // "No commits yet on main", "main...origin/main [gone]".
    if header.starts_with("HEAD (no branch)") || header.starts_with("No commits yet on") {
        return;
    }
    let (names, rest) = match header.split_once(' ') {
        Some((names, rest)) => (names, Some(rest)),
        None => (header, None),
    };
    if let Some((branch, upstream)) = names.split_once("...") {
        result.current_branch = Some(branch.to_string());
        result.current_upstream_branch = Some(upstream.to_string());
    } else {
        result.current_branch = Some(names.to_string());
    }
    if let Some(rest) = rest {
        let (ahead, behind) = parse_ahead_behind_text(rest);
        result.branch_ahead_behind = Some(AheadBehind { ahead, behind });
    }
}

fn parse_ahead_behind_text(text: &str) -> (u32, u32) {
    let mut ahead = 0;
    let mut behind = 0;
    for part in text.trim_matches(|c| c == '[' || c == ']').split(',') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("ahead ") {
            ahead = v.parse().unwrap_or(0);
        } else if let Some(v) = part.strip_prefix("behind ") {
            behind = v.parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}

/// `getAheadBehind` via `rev-list --left-right --count`.
pub fn get_ahead_behind(git: &GitEngine, repo: &Path, range: &str) -> AheadBehind {
    let out = git.run(
        &["rev-list", "--left-right", "--count", range, "--"],
        repo,
        GitOptions::new().success(128),
    );
    match out {
        Ok(o) if o.exit_code == 0 => {
            let mut parts = o.stdout.split_whitespace();
            AheadBehind {
                ahead: parts.next().and_then(|s| s.parse().ok()).unwrap_or(0),
                behind: parts.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            }
        }
        _ => AheadBehind::default(),
    }
}

pub fn git_dir(repo: &Path) -> PathBuf {
    // `git rev-parse --git-dir` is authoritative for worktrees/submodules.
    repo.join(".git")
}

/// Small helper keeping `get_status` readable when git is unavailable.
trait UnwrapOrEmpty {
    fn unwrap_or_default_losing_errors(self) -> super::core::GitOutput;
}
impl UnwrapOrEmpty for Result<super::core::GitOutput> {
    fn unwrap_or_default_losing_errors(self) -> super::core::GitOutput {
        self.unwrap_or(super::core::GitOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: -1,
        })
    }
}
