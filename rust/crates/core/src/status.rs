use crate::{DetailedStatusRecord, FileChange, GitPath, Repository, Snapshot, args};
use anyhow::{Context, Result, bail};

impl Repository {
    /// Compare the index to HEAD (or Git's empty tree on an unborn branch).
    /// NUL records keep filenames unambiguous and byte-exact.
    pub(crate) async fn index_changes(&self) -> Result<serde_json::Value> {
        let output = self
            .run(&args(&[
                "diff",
                "--cached",
                "--name-status",
                "--no-renames",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "-z",
                "--",
            ]))
            .await?;
        if output.code != 0 {
            bail!(
                "Could not compare the index with HEAD: {}",
                output.text().stderr
            );
        }
        let mut fields = output.stdout.split(|byte| *byte == 0);
        let mut entries = Vec::new();
        loop {
            let status = fields.next().context("truncated index status")?;
            if status.is_empty() {
                anyhow::ensure!(fields.next().is_none(), "malformed index status terminator");
                break;
            }
            anyhow::ensure!(
                matches!(status, b"A" | b"D" | b"M" | b"T" | b"U" | b"X"),
                "unknown index status"
            );
            let path = fields.next().context("missing index path")?;
            anyhow::ensure!(!path.is_empty(), "empty index path");
            entries.push(serde_json::json!({"status":std::str::from_utf8(status)?,"path":GitPath::Bytes { bytes: path.to_vec() }}));
        }
        Ok(serde_json::json!(entries))
    }

    /// Preserve the renderer's Git-based conflict-marker and binary detection.
    pub(crate) async fn status_conflict_details(
        &self,
        kind: crate::protocol::StatusConflictKind,
    ) -> Result<serde_json::Value> {
        use crate::protocol::StatusConflictKind;
        let checked = self.run(&args(&["diff", "--check"])).await?;
        if !matches!(checked.code, 0 | 2) {
            bail!("{}", checked.text().stderr);
        }
        let reference = match kind {
            StatusConflictKind::Merge => "MERGE_HEAD",
            StatusConflictKind::Rebase => "REBASE_HEAD",
            StatusConflictKind::Working => "HEAD",
        };
        let binary = self
            .run(&args(&["diff", "--numstat", "-z", reference]))
            .await?;
        if binary.code != 0 {
            if matches!(kind, StatusConflictKind::Working) {
                return Ok(
                    serde_json::json!({"check":String::from_utf8_lossy(&checked.stdout), "numstat":"", "attributes":""}),
                );
            }
            bail!("{}", binary.text().stderr);
        }
        let records = self.detailed_status(true).await?;
        let mut input = Vec::new();
        for record in records {
            if let DetailedStatusRecord::Entry {
                entry_kind: 'u',
                path,
                ..
            } = record
            {
                if !input.is_empty() {
                    input.push(0);
                }
                match path {
                    GitPath::Bytes { bytes } => input.extend_from_slice(&bytes),
                    GitPath::Text(text) => input.extend_from_slice(text.as_bytes()),
                }
                anyhow::ensure!(
                    input.len() <= 2 * 1024 * 1024,
                    "conflict path list too large"
                );
            }
        }
        let attrs = crate::process::run_git(
            &self.path,
            &args(&["check-attr", "--stdin", "-z", "merge"]),
            Some(input),
            None,
        )
        .await?;
        if attrs.code != 0 {
            if matches!(kind, StatusConflictKind::Working) {
                return Ok(
                    serde_json::json!({"check":String::from_utf8_lossy(&checked.stdout), "numstat":"", "attributes":""}),
                );
            }
            bail!("{}", attrs.text().stderr);
        }
        Ok(
            serde_json::json!({"check":String::from_utf8_lossy(&checked.stdout), "numstat":String::from_utf8_lossy(&binary.stdout), "attributes":String::from_utf8_lossy(&attrs.stdout)}),
        )
    }
    /// Capture exactly the porcelain-v2 status data used by the desktop view.
    pub(crate) async fn detailed_status(
        &self,
        include_untracked: bool,
    ) -> Result<Vec<DetailedStatusRecord>> {
        let mut command = args(&["status", "--branch", "--porcelain=2", "-z"]);
        if include_untracked {
            command.push("--untracked-files=all".into());
        }
        let output = self.run(&command).await?;
        if output.code != 0 {
            bail!("{}", output.text().stderr);
        }
        parse_detailed_status(&output.stdout)
    }

    pub(crate) async fn snapshot(&self) -> Result<Snapshot> {
        let output = self
            .run(&args(&[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
            ]))
            .await?;
        if output.code != 0 {
            bail!("{}", output.text().stderr);
        }
        let mut files = parse_status(&output.stdout)?;
        for file in &mut files {
            let value = match &file.path {
                GitPath::Text(text) => std::ffi::OsString::from(text),
                GitPath::Bytes { bytes } => crate::os_from_bytes(bytes)?,
            };
            if let Ok(metadata) = std::fs::symlink_metadata(self.path.join(value)) {
                file.size = Some(metadata.len());
                file.modified_ns = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos());
            }
        }
        let branch = self
            .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
            .await?;
        let head = self.run(&args(&["rev-parse", "--verify", "HEAD"])).await?;
        let branch = (branch.code == 0).then(|| {
            let reference = branch.text().stdout;
            let reference = reference.trim_end_matches('\n');
            reference
                .strip_prefix("refs/heads/")
                .unwrap_or(reference)
                .to_owned()
        });
        let head = (head.code == 0).then(|| head.text().stdout.trim().to_owned());
        let (upstream, ahead, behind) = Box::pin(self.tracking(head.as_deref())).await?;
        let policy = Box::pin(self.branch_policy()).await?;
        let branch_policy = branch
            .as_deref()
            .and_then(|name| policy.branch(name))
            .cloned();
        Ok(Snapshot {
            branch_policy,
            default_branch: policy.default_branch,
            repository_context: policy.repository_context,
            repository_account: Box::pin(self.account_state()).await?.view,
            root_id: self.id(),
            root: self.path.display().to_string(),
            branch,
            head,
            files,
            upstream,
            ahead,
            behind,
            operation: self.operation_state().await?,
        })
    }
}

/// Parse the NUL-delimited form without decoding paths or splitting on newlines.
fn parse_detailed_status(bytes: &[u8]) -> Result<Vec<DetailedStatusRecord>> {
    if !bytes.is_empty() && !bytes.ends_with(b"\0") {
        bail!("truncated porcelain-v2 status");
    }
    let mut records = bytes.split(|byte| *byte == 0);
    let mut result = Vec::new();
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        let raw = |value: &[u8]| GitPath::Bytes {
            bytes: value.to_vec(),
        };
        if let Some(value) = record.strip_prefix(b"# ") {
            result.push(DetailedStatusRecord::Header { value: raw(value) });
            continue;
        }
        if let Some(path) = record.strip_prefix(b"? ") {
            anyhow::ensure!(!path.is_empty(), "empty status path");
            result.push(DetailedStatusRecord::Entry {
                entry_kind: '?',
                status_code: "??".into(),
                submodule_status_code: "????".into(),
                path: raw(path),
                old_path: None,
                rename_or_copy_score: None,
            });
            continue;
        }
        if record.starts_with(b"! ") {
            continue;
        }
        let (kind, fields) = match record.first() {
            Some(b'1') => ('1', 9),
            Some(b'2') => ('2', 10),
            Some(b'u') => ('u', 11),
            _ => bail!("unknown porcelain-v2 status record"),
        };
        let parts: Vec<&[u8]> = record.splitn(fields, |byte| *byte == b' ').collect();
        anyhow::ensure!(
            parts.len() == fields && parts[0] == [kind as u8] && !parts[fields - 1].is_empty(),
            "malformed porcelain-v2 status record"
        );
        anyhow::ensure!(
            parts[1].len() == 2
                && parts[2].len() == 4
                && parts[1].is_ascii()
                && parts[2].is_ascii(),
            "invalid porcelain-v2 status codes"
        );
        let score = if kind == '2' {
            let score = parts[8];
            anyhow::ensure!(
                matches!(score.first(), Some(b'R' | b'C')),
                "invalid rename or copy score"
            );
            let value: u16 = std::str::from_utf8(&score[1..])?
                .parse()
                .context("invalid rename or copy score")?;
            anyhow::ensure!(value <= 100, "invalid rename or copy score");
            Some(value)
        } else {
            None
        };
        let old_path = if kind == '2' {
            let path = records
                .next()
                .filter(|path| !path.is_empty())
                .context("missing rename or copy source")?;
            Some(raw(path))
        } else {
            None
        };
        result.push(DetailedStatusRecord::Entry {
            entry_kind: kind,
            status_code: std::str::from_utf8(parts[1])?.to_owned(),
            submodule_status_code: std::str::from_utf8(parts[2])?.to_owned(),
            path: raw(parts[fields - 1]),
            old_path,
            rename_or_copy_score: score,
        });
    }
    Ok(result)
}

fn parse_status(bytes: &[u8]) -> Result<Vec<FileChange>> {
    let mut records = bytes.split(|b| *b == 0).peekable();
    let mut files = Vec::new();
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        if record.len() < 4 || record[2] != b' ' {
            bail!("invalid porcelain status record");
        }
        let x = record[0] as char;
        let y = record[1] as char;
        let path = &record[3..];
        let original_path = if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            let original = records
                .next()
                .filter(|p| !p.is_empty())
                .ok_or_else(|| anyhow::anyhow!("missing rename source"))?;
            Some(GitPath::Bytes {
                bytes: original.to_vec(),
            })
        } else {
            None
        };
        files.push(FileChange {
            display_path: String::from_utf8_lossy(path).into_owned(),
            path: GitPath::Bytes {
                bytes: path.to_vec(),
            },
            original_path,
            index: x,
            worktree: y,
            size: None,
            modified_ns: None,
            conflicted: matches!(
                (x, y),
                ('D', 'D')
                    | ('A', 'U')
                    | ('U', 'D')
                    | ('U', 'A')
                    | ('D', 'U')
                    | ('A', 'A')
                    | ('U', 'U')
            ),
        });
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn handles_unusual_paths_and_renames() {
        let files = parse_status(b"R  new\nname\0old name\0?? invalid\xff\0UU conflict\0").unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].display_path, "new\nname");
        assert!(files[0].original_path.is_some());
        assert!(matches!(&files[1].path, GitPath::Bytes{bytes} if bytes == b"invalid\xff"));
        assert!(files[2].conflicted);
    }
    #[test]
    fn rejects_truncated_rename() {
        assert!(parse_status(b"R  new\0").is_err());
    }

    #[tokio::test]
    async fn index_changes_compare_staged_files_in_sha1_and_sha256_repositories() {
        use std::{fs, process::Command};
        for format in ["sha1", "sha256"] {
            let root = std::env::temp_dir()
                .join(format!("desktop-index-{format}-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            let git = |argv: &[&str]| {
                let output = Command::new("git")
                    .current_dir(&root)
                    .args(argv)
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_CONFIG_GLOBAL", root.join("none"))
                    .env_remove("GIT_DIR")
                    .env_remove("GIT_WORK_TREE")
                    .env_remove("GIT_INDEX_FILE")
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "git {argv:?}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            };
            git(&[
                "init",
                "-q",
                "-b",
                "main",
                &format!("--object-format={format}"),
            ]);
            let repo = Repository::open(&root).await.unwrap();
            assert_eq!(repo.index_changes().await.unwrap(), json!([]));
            fs::write(root.join("old\nname"), "base").unwrap();
            git(&["add", "--", "old\nname"]);
            assert_eq!(
                repo.index_changes().await.unwrap(),
                json!([{"status":"A","path":{"bytes":b"old\nname"}}])
            );
            git(&[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "initial",
            ]);
            git(&["mv", "--", "old\nname", "new\nname"]);
            let changes = repo.index_changes().await.unwrap();
            assert_eq!(
                changes,
                json!([
                    {"status":"A","path":{"bytes":b"new\nname"}},
                    {"status":"D","path":{"bytes":b"old\nname"}}
                ])
            );
            git(&["reset", "--hard", "-q", "HEAD"]);
            git(&["switch", "-q", "-c", "side"]);
            fs::write(root.join("old\nname"), "side\n").unwrap();
            git(&["add", "--", "old\nname"]);
            git(&[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "side",
            ]);
            git(&["switch", "-q", "main"]);
            fs::write(root.join("old\nname"), "main\n").unwrap();
            git(&["add", "--", "old\nname"]);
            git(&[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "main",
            ]);
            let merge = Command::new("git")
                .current_dir(&root)
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "merge",
                    "side",
                ])
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", root.join("none"))
                .output()
                .unwrap();
            assert!(!merge.status.success());
            assert_eq!(
                repo.index_changes().await.unwrap(),
                json!([{"status":"U","path":{"bytes":b"old\nname"}}])
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn porcelain_v2_preserves_headers_submodules_renames_conflicts_and_path_bytes() {
        let records = parse_detailed_status(
            concat!(
                "# branch.head main\0",
                "1 .M S.MU 100644 100644 100644 abc def sub module\0",
                "2 R. N... 100644 100644 100644 abc def R87 new\nname\0old name\0",
                "u UU N... 100644 100644 100644 100644 abc def 012 conflict\0",
            )
            .as_bytes(),
        )
        .unwrap();
        let values = serde_json::to_value(records).unwrap();
        assert_eq!(
            values[0],
            json!({"kind":"header","value":{"bytes":b"branch.head main"}})
        );
        assert_eq!(values[1]["submodule_status_code"], "S.MU");
        assert_eq!(values[1]["path"]["bytes"], json!(b"sub module"));
        assert_eq!(values[2]["rename_or_copy_score"], 87);
        assert_eq!(values[2]["path"]["bytes"], json!(b"new\nname"));
        assert_eq!(values[2]["old_path"]["bytes"], json!(b"old name"));
        assert_eq!(values[3]["status_code"], "UU");
        let invalid = parse_detailed_status(b"? name\xff\0").unwrap();
        assert_eq!(
            serde_json::to_value(invalid).unwrap()[0]["path"]["bytes"],
            json!(b"name\xff")
        );
        assert!(parse_detailed_status(b"? no-nul").is_err());
        assert!(
            parse_detailed_status(b"2 R. N... 100644 100644 100644 abc def R100 dest\0").is_err()
        );
        assert!(
            parse_detailed_status(b"2 R. N... 100644 100644 100644 abc def R101 dest\0src\0")
                .is_err()
        );
        assert!(parse_detailed_status(b"unknown\0").is_err());
    }

    #[tokio::test]
    async fn detailed_status_uses_real_git_and_honors_untracked_mode() {
        use std::{fs, process::Command};
        let root = std::env::temp_dir().join(format!("desktop-status-v2-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let git = |argv: &[&str]| {
            let output = Command::new("git")
                .current_dir(&root)
                .args(argv)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", root.join("empty-gitconfig"))
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {argv:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        fs::write(root.join("old name"), b"initial").unwrap();
        git(&["add", "old name"]);
        git(&[
            "-c",
            "user.name=Status Test",
            "-c",
            "user.email=status@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "initial",
        ]);
        git(&["mv", "old name", "new name"]);
        fs::create_dir(root.join("untracked")).unwrap();
        fs::write(root.join("untracked/file"), b"untracked").unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let full = serde_json::to_value(repo.detailed_status(true).await.unwrap()).unwrap();
        let normal = serde_json::to_value(repo.detailed_status(false).await.unwrap()).unwrap();
        let request: crate::Operation =
            serde_json::from_value(json!({"method":"status_detailed"})).unwrap();
        assert_eq!(repo.execute(request).await.unwrap(), full);
        assert!(
            full.as_array()
                .unwrap()
                .iter()
                .any(|item| item["entry_kind"] == "2"
                    && item["old_path"]["bytes"] == json!(b"old name")
                    && item["path"]["bytes"] == json!(b"new name"))
        );
        assert!(
            full.as_array()
                .unwrap()
                .iter()
                .any(|item| item["path"]["bytes"] == json!(b"untracked/file"))
        );
        assert!(
            normal
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["path"]["bytes"] == json!(b"untracked/"))
        );
        fs::remove_dir_all(root).unwrap();
    }
}
