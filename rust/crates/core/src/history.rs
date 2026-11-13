//! Stable, paginated history and byte-preserving committed file lists.
use crate::{GitPath, Repository, args, process};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Serialize;
use serde_json::{Value, json};

const RENDERER_LOG_FORMAT: &str = "--format=%H%x00%h%x00%s%x00%b%x00%an <%ae> %ad%x00%cn <%ce> %cd%x00%P%x00%(trailers:unfold,only)%x00%D";

#[derive(Serialize)]
pub(crate) struct Identity {
    pub name: String,
    pub email: String,
    pub timestamp: i64,
    pub timezone: String,
}
#[derive(Serialize)]
pub(crate) struct Commit {
    pub oid: String,
    pub tree: String,
    pub parents: Vec<String>,
    pub author: Identity,
    pub committer: Identity,
    pub summary: String,
    pub message: String,
    pub encoding: Option<String>,
    pub signed: bool,
    pub trailers: Vec<Value>,
}

impl Repository {
    /// Render the same raw patch format used by the comparison and history UI.
    /// Only fixed Git options are accepted, and paths are always literal.
    pub(crate) async fn renderer_compare_diff(
        &self,
        before: &str,
        after: &str,
        merge_base: bool,
        path: GitPath,
        old_path: Option<GitPath>,
        hide_whitespace: bool,
    ) -> Result<Value> {
        for revision in [before, after] {
            anyhow::ensure!(
                !revision.is_empty() && revision.len() <= 4096 && !revision.contains('\0'),
                "Invalid comparison revision"
            );
        }
        let mut command = args(&["--literal-pathspecs", "diff"]);
        if merge_base {
            command.push("--merge-base".into());
        }
        if hide_whitespace {
            command.push("-w".into());
        }
        command.extend(args(&["--patch-with-raw", "-z", "--no-color"]));
        if !merge_base {
            command.push("--format=".into());
        }
        command.extend(args(&["--end-of-options", before, after, "--"]));
        command.push(crate::validate_path(path)?);
        if let Some(old_path) = old_path {
            command.push(crate::validate_path(old_path)?);
        }
        let output = self.run(&command).await?;
        if !merge_base && output.bad_revision() {
            return Ok(json!({"kind":"bad_revision"}));
        }
        ensure(&output)?;
        Ok(json!({"kind":"diff", "data":STANDARD.encode(output.stdout)}))
    }

    pub(crate) async fn renderer_commit_diff(
        &self,
        revision: &str,
        path: GitPath,
        old_path: Option<GitPath>,
        hide_whitespace: bool,
    ) -> Result<Value> {
        let commit = self.resolve_commit(revision).await?;
        let path = crate::validate_path(path)?;
        let mut command = args(&["log"]);
        if hide_whitespace {
            command.push("-w".into());
        }
        command.extend(args(&[
            "-m",
            "-1",
            "--first-parent",
            "--patch-with-raw",
            "--format=",
            "-z",
            "--no-color",
            "--end-of-options",
            &commit,
            "--",
        ]));
        command.push(path);
        if let Some(old_path) = old_path {
            command.push(crate::validate_path(old_path)?);
        }
        let output = self.run(&command).await?;
        ensure(&output)?;
        Ok(json!(STANDARD.encode(output.stdout)))
    }

    pub(crate) async fn renderer_tag_refs(&self) -> Result<Value> {
        let output = self.run(&args(&["show-ref", "--tags", "-d"])).await?;
        if !matches!(output.code, 0 | 1) {
            ensure(&output)?;
        }
        Ok(json!(String::from_utf8_lossy(&output.stdout)))
    }

    pub(crate) async fn renderer_commit_changes(&self, revision: &str) -> Result<Value> {
        let commit = self.resolve_commit(revision).await?;
        let output = self
            .run(&args(&[
                "log",
                "-C",
                "-M",
                "-m",
                "-1",
                "--no-show-signature",
                "--first-parent",
                "--raw",
                "--format=format:",
                "--numstat",
                "-z",
                "--end-of-options",
                &commit,
                "--",
            ]))
            .await?;
        ensure(&output)?;
        Ok(json!(String::from_utf8_lossy(&output.stdout)))
    }

    /// Preserve the renderer's log format and supported traversal modes without
    /// allowing the client to supply arbitrary Git options.
    pub(crate) async fn renderer_history(
        &self,
        revision: Option<&str>,
        limit: Option<u64>,
        skip: Option<u64>,
        first_parent: bool,
        exclude_remotes: bool,
    ) -> Result<Value> {
        anyhow::ensure!(
            revision.is_none_or(|value| value.len() <= 4096 && !value.contains('\0')),
            "invalid history revision"
        );
        const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
        anyhow::ensure!(
            limit.is_none_or(|value| value <= MAX_SAFE_INTEGER)
                && skip.is_none_or(|value| value <= MAX_SAFE_INTEGER),
            "invalid history pagination"
        );
        let mut command = args(&["log", "--date=raw"]);
        if let Some(limit) = limit {
            command.push(format!("--max-count={limit}").into());
        }
        if let Some(skip) = skip {
            command.push(format!("--skip={skip}").into());
        }
        command.extend(args(&[
            "-z",
            RENDERER_LOG_FORMAT,
            "--no-show-signature",
            "--no-color",
        ]));
        if first_parent {
            command.push("--first-parent".into());
        }
        if exclude_remotes {
            command.extend(args(&["--not", "--remotes"]));
        }
        if let Some(revision) = revision {
            if exclude_remotes {
                command.push("--not".into());
            }
            command.extend(args(&["--end-of-options", revision]));
        }
        command.push("--".into());
        let output = self.run(&command).await?;
        if output.code == 128 {
            return Ok(Value::Null);
        }
        ensure(&output)?;
        Ok(json!(STANDARD.encode(output.stdout)))
    }

    /// Fixed-format reflog reads used by the renderer's branch and stash lists.
    pub(crate) async fn renderer_reflog(&self, stash: bool) -> Result<Value> {
        let command = if stash {
            args(&[
                "log",
                "-g",
                "-z",
                "--format=%gD%x00%H%x00%gs%x00%T%x00%P",
                "refs/stash",
                "--",
            ])
        } else {
            args(&[
                "log",
                "-g",
                "--no-abbrev-commit",
                "--pretty=oneline",
                "HEAD",
                "-n",
                "2500",
                "--",
            ])
        };
        let output = self.run(&command).await?;
        if output.code == 128 {
            return Ok(Value::Null);
        }
        ensure(&output)?;
        Ok(json!(String::from_utf8_lossy(&output.stdout)))
    }

    /// Checkout history used to protect recently used branches from pruning.
    pub(crate) async fn renderer_branch_checkouts(&self, after_date: &str) -> Result<Value> {
        anyhow::ensure!(
            valid_checkout_date(after_date),
            "invalid checkout history date"
        );
        let date = format!("--after=\"{after_date}\"");
        let output = self
            .run(&args(&[
                "reflog",
                "--date=iso",
                &date,
                "--pretty=%H %gd %gs",
                "--grep-reflog=checkout: moving from .* to .*$",
                "--",
            ]))
            .await?;
        if output.code == 128 {
            let error = String::from_utf8_lossy(&output.stderr);
            if error.contains("fatal: your current branch '")
                && error.contains("' does not have any commits yet")
            {
                return Ok(Value::Null);
            }
        }
        ensure(&output)?;
        Ok(json!(String::from_utf8_lossy(&output.stdout)))
    }

    /// Read a bounded prefix of a blob, keeping invalid revisions distinct
    /// from a path that is absent in a valid tree or index stage.
    pub(crate) async fn renderer_blob_prefix(
        &self,
        revision: &str,
        path: GitPath,
        length: u32,
    ) -> Result<Value> {
        let result = self
            .read_renderer_blob(revision, path, 0, length, None)
            .await?;
        Ok(result.map_or(Value::Null, |(_, _, data)| json!(data)))
    }

    pub(crate) async fn renderer_blob_chunk(
        &self,
        revision: &str,
        path: GitPath,
        offset: u64,
        length: u32,
        expected_oid: Option<&str>,
    ) -> Result<Value> {
        let result = self
            .read_renderer_blob(revision, path, offset, length, expected_oid)
            .await?;
        Ok(result.map_or(
            Value::Null,
            |(oid, size, data)| json!({"oid": oid, "size": size, "data": data}),
        ))
    }

    async fn read_renderer_blob(
        &self,
        revision: &str,
        path: GitPath,
        offset: u64,
        length: u32,
        expected_oid: Option<&str>,
    ) -> Result<Option<(String, u64, String)>> {
        anyhow::ensure!(
            length <= 1024 * 1024,
            "blob prefix exceeds the preview limit"
        );
        anyhow::ensure!(
            revision.len() <= 4096 && !revision.contains('\0'),
            "invalid blob revision"
        );
        let path = crate::validate_path(path)?;
        let stage = match revision {
            "" | ":0" => Some("0"),
            ":1" => Some("1"),
            ":2" => Some("2"),
            ":3" => Some("3"),
            _ => None,
        };
        let mut command = if stage.is_some() {
            args(&["ls-files", "--stage", "-z", "--"])
        } else {
            let tree = self
                .run(&args(&[
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    &format!("{revision}^{{tree}}"),
                ]))
                .await?;
            ensure(&tree)?;
            let tree = String::from_utf8(tree.stdout)?.trim().to_owned();
            args(&["ls-tree", "-z", &tree, "--"])
        };
        command.push(path.clone());
        let resolved = self.run(&command).await?;
        ensure(&resolved)?;
        let mut found = None;
        for record in resolved
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
        {
            let tab = record
                .iter()
                .position(|byte| *byte == b'\t')
                .context("invalid blob entry")?;
            let (metadata, name) = (&record[..tab], &record[tab + 1..]);
            if name != path.as_encoded_bytes() {
                continue;
            }
            let fields: Vec<_> = std::str::from_utf8(metadata)?.split_whitespace().collect();
            anyhow::ensure!(fields.len() == 3, "invalid blob metadata");
            let oid = if let Some(stage) = stage {
                if fields[2] != stage {
                    continue;
                }
                fields[1]
            } else {
                anyhow::ensure!(fields[1] == "blob", "path is not a blob");
                fields[2]
            };
            anyhow::ensure!(found.is_none(), "duplicate blob entry");
            found = Some(oid.to_owned());
        }
        let Some(oid) = found else {
            anyhow::ensure!(expected_oid.is_none(), "blob changed during read");
            return Ok(None);
        };
        anyhow::ensure!(
            (oid.len() == 40 || oid.len() == 64)
                && oid.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid blob object id"
        );
        anyhow::ensure!(
            expected_oid.is_none_or(|expected| expected == oid),
            "blob changed during read"
        );
        let kind = self.run(&args(&["cat-file", "-t", &oid])).await?;
        ensure(&kind)?;
        anyhow::ensure!(kind.stdout == b"blob\n", "path is not a blob");
        let size = self.run(&args(&["cat-file", "-s", &oid])).await?;
        ensure(&size)?;
        let size: u64 = std::str::from_utf8(&size.stdout)?.trim().parse()?;
        anyhow::ensure!(offset <= size, "blob offset exceeds size");
        let output = process::blob_range(&self.path, &oid, offset, u64::from(length)).await?;
        let expected = (size - offset).min(u64::from(length)) as usize;
        anyhow::ensure!(
            output.stdout.len() == expected,
            "could not read blob prefix"
        );
        if size - offset <= u64::from(length) {
            ensure(&output)?;
        }
        Ok(Some((oid, size, STANDARD.encode(output.stdout))))
    }

    /// Return the renderer's exact one-commit Git log format without exposing
    /// a generic command interface. The existing parser keeps Git's encoding,
    /// trailer, short-hash and decoration semantics.
    pub(crate) async fn renderer_commit(&self, revision: &str) -> Result<Value> {
        anyhow::ensure!(
            !revision.is_empty() && revision.len() <= 4096 && !revision.contains('\0'),
            "invalid commit revision"
        );
        let output = self
            .run(&args(&[
                "log",
                "--date=raw",
                "--max-count=1",
                "-z",
                RENDERER_LOG_FORMAT,
                "--no-show-signature",
                "--no-color",
                "--end-of-options",
                revision,
                "--",
            ]))
            .await?;
        if output.code == 128 {
            return Ok(Value::Null);
        }
        ensure(&output)?;
        Ok(json!(STANDARD.encode(output.stdout)))
    }

    /// Match the renderer's rebase-order `rev-list` output exactly; the
    /// renderer's parser owns its existing commit-ID and summary semantics.
    pub(crate) async fn revision_list(&self, range: &str) -> Result<Value> {
        let output = self
            .run(&args(&[
                "rev-list",
                "--reverse",
                "--oneline",
                "--no-abbrev-commit",
                "--end-of-options",
                range,
                "--",
            ]))
            .await?;
        if output.bad_revision() {
            return Ok(Value::Null);
        }
        ensure(&output)?;
        Ok(json!(String::from_utf8_lossy(&output.stdout)))
    }

    pub(crate) async fn has_merge_commits(&self, range: &str) -> Result<Value> {
        let output = self
            .run(&args(&[
                "rev-list",
                "-1",
                "--merges",
                "--end-of-options",
                range,
                "--",
            ]))
            .await?;
        match output.code {
            0 | 128 => Ok(json!(!output.stdout.is_empty())),
            _ => {
                ensure(&output)?;
                unreachable!("nonzero output was rejected")
            }
        }
    }

    pub(crate) async fn history(
        &self,
        revision: Option<String>,
        offset: usize,
        limit: u16,
    ) -> Result<Value> {
        if !(1..=100).contains(&limit) || offset > 1_000_000 {
            bail!("history limit must be 1..100 and offset at most 1000000");
        }
        let revision = match revision {
            Some(revision) => self.resolve_commit(&revision).await?,
            None => {
                let output = self.run(&args(&["rev-parse", "--verify", "HEAD"])).await?;
                if output.code != 0 {
                    return Ok(json!({"revision":null,"commits":[],"next_offset":null}));
                }
                String::from_utf8(output.stdout)?.trim().to_owned()
            }
        };
        let listed = self
            .run(&args(&[
                "rev-list",
                "--topo-order",
                &format!("--max-count={}", limit + 1),
                &format!("--skip={offset}"),
                &revision,
                "--",
            ]))
            .await?;
        ensure(&listed)?;
        let ids: Vec<_> = std::str::from_utf8(&listed.stdout)?.lines().collect();
        let more = ids.len() > limit as usize;
        let chosen = &ids[..ids.len().min(limit as usize)];
        let mut commits = if chosen.is_empty() {
            Vec::new()
        } else {
            let output = process::run_git(
                &self.path,
                &args(&["cat-file", "--batch"]),
                Some(format!("{}\n", chosen.join("\n")).into_bytes()),
                None,
            )
            .await?;
            ensure(&output)?;
            parse_batch(&output.stdout)?
        };
        let separators = self
            .run(&args(&["config", "--get", "trailer.separators"]))
            .await?;
        let separators = if separators.code == 0 {
            String::from_utf8(separators.stdout)?.trim().to_owned()
        } else {
            ":".to_owned()
        };
        for commit in &mut commits {
            let output = process::run_git(
                &self.path,
                &args(&["interpret-trailers", "--parse"]),
                Some(commit.message.as_bytes().to_vec()),
                None,
            )
            .await?;
            ensure(&output)?;
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if let Some((token, value)) = line.split_once(|c: char| separators.contains(c)) {
                    commit
                        .trailers
                        .push(json!({"token":token.trim(),"value":value.trim()}));
                }
            }
        }
        Ok(
            json!({"revision":revision,"commits":commits,"next_offset":more.then_some(offset+commits.len())}),
        )
    }

    pub(crate) async fn commit_files(&self, revision: &str, parent: usize) -> Result<Value> {
        let commit = self.resolve_commit(revision).await?;
        let base = self.commit_parent(&commit, parent).await?;
        let mut command = args(&[
            "diff-tree",
            "-r",
            "--root",
            "--no-commit-id",
            "--name-status",
            "-z",
            "-M",
        ]);
        if let Some(base) = &base {
            command.push(base.into());
        }
        command.extend(args(&[&commit, "--"]));
        let output = self.run(&command).await?;
        ensure(&output)?;
        Ok(json!({"commit":commit,"parent":base,"files":parse_files(&output.stdout)?}))
    }

    pub(crate) async fn commit_parent(
        &self,
        commit: &str,
        parent: usize,
    ) -> Result<Option<String>> {
        let output = self
            .run(&args(&["rev-list", "--parents", "-n", "1", commit, "--"]))
            .await?;
        ensure(&output)?;
        let fields: Vec<_> = std::str::from_utf8(&output.stdout)?
            .split_whitespace()
            .collect();
        if fields.len() == 1 && parent == 0 {
            return Ok(None);
        }
        Ok(Some(
            fields
                .get(
                    parent
                        .checked_add(1)
                        .context("parent index exceeds usize")?,
                )
                .context("commit has no such parent")?
                .to_string(),
        ))
    }
}

fn valid_checkout_date(value: &str) -> bool {
    let b = value.as_bytes();
    if b.len() != 24
        || [
            (4, b'-'),
            (7, b'-'),
            (10, b'T'),
            (13, b':'),
            (16, b':'),
            (19, b'.'),
            (23, b'Z'),
        ]
        .iter()
        .any(|(i, expected)| b[*i] != *expected)
        || b.iter()
            .enumerate()
            .any(|(i, byte)| ![4, 7, 10, 13, 16, 19, 23].contains(&i) && !byte.is_ascii_digit())
    {
        return false;
    }
    let number = |start: usize, end: usize| -> u32 {
        b[start..end]
            .iter()
            .fold(0, |n, digit| n * 10 + u32::from(digit - b'0'))
    };
    let (year, month, day) = (number(0, 4), number(5, 7), number(8, 10));
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    // Git's approximate-date parser can silently reinterpret years beyond
    // the signed 32-bit timestamp range as a different, recent date. Fail
    // closed instead of returning incomplete history to branch pruning.
    (1970..=2037).contains(&year)
        && day > 0
        && day <= days
        && number(11, 13) < 24
        && number(14, 16) < 60
        && number(17, 19) < 60
}

#[derive(Serialize)]
struct ChangedFile {
    status: String,
    path: GitPath,
    display_path: String,
    original_path: Option<GitPath>,
}
fn parse_files(bytes: &[u8]) -> Result<Vec<ChangedFile>> {
    let mut fields = bytes.split(|b| *b == 0).filter(|b| !b.is_empty());
    let mut files = Vec::new();
    while let Some(status) = fields.next() {
        let first = fields.next().context("missing committed file path")?;
        let renamed = matches!(status.first(), Some(b'R' | b'C'));
        let path = if renamed {
            fields.next().context("missing rename destination")?
        } else {
            first
        };
        files.push(ChangedFile {
            status: std::str::from_utf8(status)?.to_owned(),
            path: GitPath::Bytes {
                bytes: path.to_vec(),
            },
            display_path: String::from_utf8_lossy(path).into_owned(),
            original_path: renamed.then(|| GitPath::Bytes {
                bytes: first.to_vec(),
            }),
        });
    }
    Ok(files)
}
/// Reuse the byte-preserving name-status parser for index-only diff lookup.
pub(crate) fn staged_rename_source(
    bytes: &[u8],
    path: &std::ffi::OsStr,
) -> Result<Option<GitPath>> {
    Ok(parse_files(bytes)?
        .into_iter()
        .find(|file| {
            file.status.starts_with('R')
                && crate::validate_path(file.path.clone()).is_ok_and(|candidate| candidate == path)
        })
        .and_then(|file| file.original_path))
}
fn ensure(output: &process::RawOutput) -> Result<()> {
    if output.code != 0 {
        bail!("{}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(())
}
fn parse_batch(mut bytes: &[u8]) -> Result<Vec<Commit>> {
    let mut commits = Vec::new();
    while !bytes.is_empty() {
        let split = bytes
            .iter()
            .position(|b| *b == b'\n')
            .context("missing batch header")?;
        let header = std::str::from_utf8(&bytes[..split])?;
        let fields: Vec<_> = header.split_whitespace().collect();
        if fields.len() != 3 || fields[1] != "commit" {
            bail!("expected a commit in batch response");
        }
        let size: usize = fields[2].parse()?;
        bytes = &bytes[split + 1..];
        if size >= bytes.len() || bytes[size] != b'\n' {
            bail!("truncated commit batch response");
        }
        commits.push(parse_commit(fields[0], &bytes[..size])?);
        bytes = &bytes[size + 1..];
    }
    Ok(commits)
}
fn parse_commit(oid: &str, raw: &[u8]) -> Result<Commit> {
    let boundary = raw
        .windows(2)
        .position(|b| b == b"\n\n")
        .context("commit lacks a message boundary")?;
    let mut tree = None;
    let mut author = None;
    let mut committer = None;
    let mut parents = Vec::new();
    let mut encoding = None;
    let mut signed = false;
    for line in String::from_utf8_lossy(&raw[..boundary]).lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        match key {
            "tree" => tree = Some(value.to_owned()),
            "parent" => parents.push(value.to_owned()),
            "author" => author = Some(parse_identity(value)?),
            "committer" => committer = Some(parse_identity(value)?),
            "encoding" => encoding = Some(value.to_owned()),
            "gpgsig" | "gpgsig-sha256" => signed = true,
            _ => {}
        }
    }
    let message = String::from_utf8_lossy(&raw[boundary + 2..]).into_owned();
    Ok(Commit {
        oid: oid.to_owned(),
        tree: tree.context("missing commit tree")?,
        parents,
        author: author.context("missing commit author")?,
        committer: committer.context("missing commit committer")?,
        summary: message.lines().next().unwrap_or_default().to_owned(),
        message,
        encoding,
        signed,
        trailers: Vec::new(),
    })
}
fn parse_identity(value: &str) -> Result<Identity> {
    let (who, when) = value.rsplit_once("> ").context("invalid commit identity")?;
    let (name, email) = who.rsplit_once(" <").context("invalid identity email")?;
    let (timestamp, timezone) = when.split_once(' ').context("invalid identity date")?;
    Ok(Identity {
        name: name.to_owned(),
        email: email.to_owned(),
        timestamp: timestamp.parse()?,
        timezone: timezone.to_owned(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn renderer_comparison_diffs_match_git_and_keep_paths_literal() {
        use std::{fs, path::Path, process::Command};
        let root =
            std::env::temp_dir().join(format!("desktop-compare-diff-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let git = |arguments: &[&str]| {
            let output = Command::new("git")
                .current_dir(&root)
                .args(arguments)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.invalid"]);
        git(&["config", "commit.gpgsign", "false"]);
        fs::write(root.join("[a].txt"), "first\n").unwrap();
        fs::write(root.join("a.txt"), "other\n").unwrap();
        git(&["add", "--", "."]);
        git(&["commit", "-qm", "first"]);
        let first = String::from_utf8(git(&["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        fs::write(root.join("[a].txt"), "second\n").unwrap();
        fs::write(root.join("a.txt"), "changed\n").unwrap();
        git(&["add", "--", "."]);
        git(&["commit", "-qm", "second"]);
        let head = String::from_utf8(git(&["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        let repo = Repository::open(Path::new(&root)).await.unwrap();
        for merge_base in [true, false] {
            let actual = repo
                .renderer_compare_diff(
                    &first,
                    &head,
                    merge_base,
                    GitPath::Text("[a].txt".into()),
                    None,
                    false,
                )
                .await
                .unwrap();
            let mut args = vec!["--literal-pathspecs", "diff"];
            if merge_base {
                args.push("--merge-base");
            }
            args.extend(["--patch-with-raw", "-z", "--no-color"]);
            if !merge_base {
                args.push("--format=");
            }
            args.extend(["--end-of-options", &first, &head, "--", "[a].txt"]);
            assert_eq!(
                STANDARD.decode(actual["data"].as_str().unwrap()).unwrap(),
                git(&args)
            );
        }
        let missing_parent = format!("{first}^");
        assert_eq!(
            repo.renderer_compare_diff(
                &missing_parent,
                &head,
                false,
                GitPath::Text("[a].txt".into()),
                None,
                false
            )
            .await
            .unwrap()["kind"],
            "bad_revision"
        );
        let empty_tree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
        let initial = repo
            .renderer_compare_diff(
                empty_tree,
                &first,
                false,
                GitPath::Text("[a].txt".into()),
                None,
                false,
            )
            .await
            .unwrap();
        assert_eq!(
            STANDARD.decode(initial["data"].as_str().unwrap()).unwrap(),
            git(&[
                "--literal-pathspecs",
                "diff",
                "--patch-with-raw",
                "-z",
                "--no-color",
                "--format=",
                "--end-of-options",
                empty_tree,
                &first,
                "--",
                "[a].txt"
            ])
        );
        git(&["mv", "--", "[a].txt", "[b].txt"]);
        git(&["commit", "-qm", "rename"]);
        let renamed = String::from_utf8(git(&["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        let actual = repo
            .renderer_compare_diff(
                &head,
                &renamed,
                false,
                GitPath::Text("[b].txt".into()),
                Some(GitPath::Text("[a].txt".into())),
                false,
            )
            .await
            .unwrap();
        assert_eq!(
            STANDARD.decode(actual["data"].as_str().unwrap()).unwrap(),
            git(&[
                "--literal-pathspecs",
                "diff",
                "--patch-with-raw",
                "-z",
                "--no-color",
                "--format=",
                "--end-of-options",
                &head,
                &renamed,
                "--",
                "[b].txt",
                "[a].txt"
            ])
        );
        fs::write(root.join("[b].txt"), b"binary\0payload\n").unwrap();
        git(&["add", "--", "."]);
        git(&["commit", "-qm", "binary"]);
        let binary_head = String::from_utf8(git(&["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        let binary = repo
            .renderer_compare_diff(
                &renamed,
                &binary_head,
                false,
                GitPath::Text("[b].txt".into()),
                None,
                false,
            )
            .await
            .unwrap();
        assert_eq!(
            STANDARD.decode(binary["data"].as_str().unwrap()).unwrap(),
            git(&[
                "--literal-pathspecs",
                "diff",
                "--patch-with-raw",
                "-z",
                "--no-color",
                "--format=",
                "--end-of-options",
                &renamed,
                &binary_head,
                "--",
                "[b].txt"
            ])
        );
        assert!(
            repo.renderer_compare_diff(
                "missing-branch",
                &head,
                true,
                GitPath::Text("[a].txt".into()),
                None,
                false
            )
            .await
            .is_err()
        );
        assert!(
            repo.renderer_compare_diff(
                &first,
                &head,
                false,
                GitPath::Text("../outside".into()),
                None,
                false
            )
            .await
            .is_err()
        );
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let raw_path = b"byte-\xff.txt".to_vec();
            fs::write(
                root.join(std::ffi::OsString::from_vec(raw_path.clone())),
                b"raw\n",
            )
            .unwrap();
            git(&["add", "--", "."]);
            git(&["commit", "-qm", "raw path"]);
            let raw_head = String::from_utf8(git(&["rev-parse", "HEAD"]))
                .unwrap()
                .trim()
                .to_owned();
            let actual = repo
                .renderer_compare_diff(
                    &binary_head,
                    &raw_head,
                    false,
                    GitPath::Bytes {
                        bytes: raw_path.clone(),
                    },
                    None,
                    false,
                )
                .await
                .unwrap();
            let output = Command::new("git")
                .current_dir(&root)
                .args([
                    "--literal-pathspecs",
                    "diff",
                    "--patch-with-raw",
                    "-z",
                    "--no-color",
                    "--format=",
                    "--end-of-options",
                    &binary_head,
                    &raw_head,
                    "--",
                ])
                .arg(std::ffi::OsString::from_vec(raw_path))
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                STANDARD.decode(actual["data"].as_str().unwrap()).unwrap(),
                output.stdout
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn checkout_date_must_be_a_real_iso_timestamp() {
        assert!(valid_checkout_date("2024-02-29T23:59:59.999Z"));
        for invalid in [
            "2023-02-29T23:59:59.999Z",
            "2024-13-01T00:00:00.000Z",
            "2024-01-01T24:00:00.000Z",
            "2999-01-01T00:00:00.000Z",
            "----00-00T00:00:00.000Z",
            "--all",
        ] {
            assert!(!valid_checkout_date(invalid), "{invalid}");
        }
    }
    #[test]
    fn length_framing_preserves_delimiters_in_messages() {
        let raw=b"tree abc\nauthor A <a@b> -1 +0200\ncommitter C <c@d> 2 -0500\ngpgsig signature\n continuation\n\nsubject\n\nbody\0with delimiter\n";
        let mut batch = format!("abc commit {}\n", raw.len()).into_bytes();
        batch.extend(raw);
        batch.push(b'\n');
        let parsed = parse_batch(&batch).unwrap();
        assert!(parsed[0].message.contains('\0'));
        assert!(parsed[0].signed);
        assert_eq!(parsed[0].author.timestamp, -1);
        batch.pop();
        assert!(parse_batch(&batch).is_err());
    }
}
