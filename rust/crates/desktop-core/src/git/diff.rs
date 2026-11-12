//! Port of `app/src/lib/diff-parser.ts` + the diff entry points in
//! `app/src/lib/git/diff.ts`.

use std::path::Path;

use crate::error::Result;
use crate::models::diff::{Diff, DiffHunk, DiffLine, DiffLineType};

use super::core::{GitEngine, GitOptions};

fn exec(git: &GitEngine, repo: &Path, args: Vec<&str>) -> Result<String> {
    Ok(git.run(&args, repo, GitOptions::new().success(1))?.stdout)
}

pub fn get_working_directory_diff(git: &GitEngine, repo: &Path, path: &str) -> Result<Diff> {
    let mut args = vec![
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--unified=3",
        "HEAD",
        "--",
    ];
    if !path.is_empty() {
        args.push(path);
    }
    let raw = exec(git, repo, args)?;
    Ok(Diff::text(path, parse_unified_diff(&raw)))
}

pub fn get_commit_diff(git: &GitEngine, repo: &Path, sha: &str, path: &str) -> Result<Diff> {
    let parent = format!("{sha}^");
    let mut args = vec![
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--unified=3",
        "--find-renames",
    ];
    args.push(&parent);
    args.push(sha);
    args.push("--");
    if !path.is_empty() {
        args.push(path);
    }
    let raw = exec(git, repo, args)?;
    Ok(Diff::text(path, parse_unified_diff(&raw)))
}

pub fn get_commit_range_diff(git: &GitEngine, repo: &Path, from: &str, to: &str) -> Result<String> {
    exec(
        git,
        repo,
        vec!["diff", "--no-color", "--no-ext-diff", from, to],
    )
}

pub fn get_blob_contents(git: &GitEngine, repo: &Path, sha: &str, path: &str) -> Result<String> {
    let spec = format!("{sha}:{path}");
    exec(git, repo, vec!["show", &spec])
}

pub fn get_binary_paths(git: &GitEngine, repo: &Path, from: &str, to: &str) -> Result<Vec<String>> {
    let out = exec(
        git,
        repo,
        vec!["diff", "--numstat", "-z", "--no-color", from, to],
    )?;
    Ok(out
        .split('\0')
        .filter(|s| s.starts_with("-\t-"))
        .filter_map(|s| s.rsplit('\t').next().map(str::to_string))
        .collect())
}

/// Minimal unified-diff parser. Produces the same `DiffHunk` / `DiffLine`
/// shapes as `diff-parser.ts` so the existing diff renderer can be reused.
pub fn parse_unified_diff(raw: &str) -> Vec<DiffHunk> {
    let mut hunks = Vec::new();
    let mut current: Option<DiffHunk> = None;
    let mut old_line = 0u32;
    let mut new_line = 0u32;

    for line in raw.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            if let Some(hunk) = current.take() {
                hunks.push(hunk);
            }
            let (ranges, _context) = header.split_once(" @@").unwrap_or((header, ""));
            let mut old_start = 0;
            let mut old_count = 0;
            let mut new_start = 0;
            let mut new_count = 0;
            for part in ranges.split_whitespace() {
                let clean = part.trim_start_matches(['-', '+']);
                let mut nums = clean.split(',');
                let start = nums.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
                let count = nums.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(1);
                if part.starts_with('-') {
                    old_start = start;
                    old_count = count;
                } else if part.starts_with('+') {
                    new_start = start;
                    new_count = count;
                }
            }
            old_line = old_start;
            new_line = new_start;
            current = Some(DiffHunk {
                header: format!("@@ {ranges} @@{_context}"),
                lines: Vec::new(),
                old_start_line: old_start,
                old_line_count: old_count,
                new_start_line: new_start,
                new_line_count: new_count,
                expansion_type: None,
            });
            continue;
        }
        let Some(hunk) = current.as_mut() else {
            continue;
        };
        if line.starts_with("diff --git")
            || line.starts_with("index ")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
        {
            continue;
        }
        let (kind, text, old_no, new_no) = if let Some(t) = line.strip_prefix('+') {
            let n = Some(new_line);
            new_line += 1;
            (DiffLineType::Add, t, None, n)
        } else if let Some(t) = line.strip_prefix('-') {
            let n = Some(old_line);
            old_line += 1;
            (DiffLineType::Delete, t, n, None)
        } else if let Some(text) = line.strip_prefix(' ') {
            let (o, n) = (Some(old_line), Some(new_line));
            old_line += 1;
            new_line += 1;
            (DiffLineType::Context, text, o, n)
        } else {
            continue;
        };
        hunk.lines.push(DiffLine {
            text: text.to_string(),
            kind,
            old_line_number: old_no,
            new_line_number: new_no,
            no_trailing_new_line: false,
        });
    }
    if let Some(hunk) = current {
        hunks.push(hunk);
    }
    hunks
}
