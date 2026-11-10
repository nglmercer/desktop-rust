//! Port of `app/src/lib/git/log.ts` (`getCommits`, `getCommit`,
//! `getChangedFiles`, `parseRawLogWithNumstat`).

use std::path::Path;

use crate::error::Result;
use crate::models::commit::{ChangedFile, Commit, CommitIdentity};

use super::core::{GitEngine, GitOptions};

/// Unit and record separators are safe because git never emits them in
/// commit metadata; Desktop uses the same trick with its delimiter parser.
const FIELD: &str = "%x1f";
const RECORD: &str = "%x1e";

fn pretty_format() -> String {
    format!(
        "--pretty=format:%H{FIELD}%h{FIELD}%s{FIELD}%b{FIELD}%an{FIELD}%ae{FIELD}%aI{FIELD}%P{FIELD}%cn{FIELD}%ce{FIELD}%cI{FIELD}%D{RECORD}"
    )
}

pub fn get_commits(
    git: &GitEngine,
    repo: &Path,
    revision: Option<&str>,
    limit: u32,
    skip: u32,
) -> Result<Vec<Commit>> {
    let format = pretty_format();
    let limit_arg = format!("--max-count={limit}");
    let skip_arg = format!("--skip={skip}");
    let mut argv: Vec<&str> = vec!["log", &format, "--no-color", &limit_arg];
    if skip > 0 {
        argv.push(&skip_arg);
    }
    if let Some(rev) = revision {
        argv.push(rev);
    } else {
        argv.push("HEAD");
    }
    argv.push("--");

    let out = git.run(&argv, repo, GitOptions::new().success(128))?;
    if out.exit_code != 0 {
        return Ok(Vec::new());
    }
    Ok(parse_log(&out.stdout))
}

pub fn get_commit(git: &GitEngine, repo: &Path, sha: &str) -> Result<Option<Commit>> {
    Ok(get_commits(git, repo, Some(sha), 1, 0)?.into_iter().next())
}

fn parse_log(raw: &str) -> Vec<Commit> {
    raw.split('\x1e')
        .map(|record| record.trim_matches(|c: char| c == '\n' || c == '\r' || c == '\0'))
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let fields: Vec<&str> = record.split('\x1f').collect();
            if fields.len() < 12 {
                return None;
            }
            let field = |i: usize| fields.get(i).copied().unwrap_or("").trim().to_string();
            let sha = field(0);
            if sha.is_empty() {
                return None;
            }
            let parent_shas = field(7)
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>();
            let tags = field(11)
                .split(',')
                .map(str::trim)
                .filter(|s| s.starts_with("tag: "))
                .map(|s| s.trim_start_matches("tag: ").to_string())
                .collect::<Vec<_>>();

            Some(Commit {
                short_sha: if field(1).is_empty() {
                    Commit::short(&sha)
                } else {
                    field(1)
                },
                sha,
                summary: field(2),
                body: field(3).trim().to_string(),
                author: CommitIdentity {
                    name: field(4),
                    email: field(5),
                    date: field(6),
                    tz_offset: 0,
                },
                committer: Some(CommitIdentity {
                    name: field(8),
                    email: field(9),
                    date: field(10),
                    tz_offset: 0,
                }),
                parent_shas,
                trailers: Vec::new(),
                tags,
                co_authors: Vec::new(),
            })
        })
        .collect()
}

/// `getChangedFiles` via `log --numstat` for a single commit.
pub fn get_changed_files(git: &GitEngine, repo: &Path, sha: &str) -> Result<Vec<ChangedFile>> {
    let out = git.run(
        &["show", "--numstat", "--format=", "--no-renames", sha],
        repo,
        GitOptions::new().success(128),
    )?;
    Ok(out
        .stdout_lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let additions = parts.next()?.parse().unwrap_or(0);
            let deletions = parts.next()?.parse().unwrap_or(0);
            let path = parts.next()?.to_string();
            Some(ChangedFile {
                path,
                status: if additions > 0 && deletions == 0 {
                    "added"
                } else if deletions > 0 && additions == 0 {
                    "deleted"
                } else {
                    "modified"
                }
                .to_string(),
                old_path: None,
                additions,
                deletions,
            })
        })
        .collect())
}

/// `getAuthors` — distinct authors in a repository, used by the co-author UI.
pub fn get_authors(git: &GitEngine, repo: &Path) -> Result<Vec<String>> {
    let out = git.run(
        &["log", "--format=%an <%ae>", "--all"],
        repo,
        GitOptions::new(),
    )?;
    let mut authors: Vec<String> = out
        .stdout_lines()
        .map(str::to_string)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    authors.sort();
    Ok(authors)
}
