//! Review staged Git blob sizes before invoking Git's normal commit workflow.
use crate::{GitPath, Repository, args, cancellation, diff::read_index, process, validate_path};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

const LARGE_BLOB_LIMIT: u64 = 100 * 1024 * 1024;

#[cfg(test)]
mod trailer_tests {
    use super::*;
    #[tokio::test]
    async fn reviewed_tree_verification_observes_index_and_head_without_changing_index() {
        let root =
            std::env::temp_dir().join(format!("desktop-review-tree-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let git = |arguments: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(arguments)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        std::fs::write(root.join("file"), b"before\n").unwrap();
        git(&["add", "file"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "first",
        ]);
        std::fs::write(root.join("file"), b"after\n").unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let review = repo
            .selected_commit_preview_with_empty(
                vec![GitPath::Text("file".into())],
                vec![],
                vec![],
                false,
                None,
                None,
            )
            .await
            .unwrap();
        let tree = review["tree"].as_str().unwrap();
        let head = review["head"].as_str();
        let branch = review["branch"].as_str();
        let index = std::fs::read(root.join(".git/index")).unwrap();
        assert_eq!(
            repo.verify_selected_index(tree, head, branch)
                .await
                .unwrap()["matches"],
            false
        );
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        git(&["add", "file"]);
        let index = std::fs::read(root.join(".git/index")).unwrap();
        assert_eq!(
            repo.verify_selected_index(tree, head, branch)
                .await
                .unwrap()["matches"],
            true
        );
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        git(&["checkout", "-qb", "other"]);
        assert_eq!(
            repo.verify_selected_index(tree, head, branch)
                .await
                .unwrap()["matches"],
            false
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn initial_head_deletion_requires_the_selected_root_commit() {
        let root =
            std::env::temp_dir().join(format!("desktop-initial-delete-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let git = |arguments: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(arguments)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q"]);
        std::fs::write(root.join("file"), "content").unwrap();
        git(&["add", "file"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "first",
        ]);
        let first = git(&["rev-parse", "HEAD"]);
        let branch = git(&["symbolic-ref", "HEAD"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "second",
        ]);
        let second = git(&["rev-parse", "HEAD"]);
        let repo = Repository::open(&root).await.unwrap();
        assert!(repo.renderer_delete_initial_head(&second).await.is_err());
        assert_ne!(
            repo.renderer_delete_initial_head(&first).await.unwrap()["code"],
            0
        );
        assert_eq!(git(&["rev-parse", "HEAD"]), second);
        git(&["reset", "--hard", &first]);
        assert_eq!(
            repo.renderer_delete_initial_head(&first).await.unwrap()["code"],
            0
        );
        assert_eq!(git(&["symbolic-ref", "HEAD"]), branch);
        assert_eq!(std::fs::read(root.join("file")).unwrap(), b"content");
        assert!(
            !std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["rev-parse", "--verify", "HEAD"])
                .output()
                .unwrap()
                .status
                .success()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn annotated_tag_preserves_empty_message_and_validates_target() {
        let root = std::env::temp_dir().join(format!("desktop-tag-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let git = |arguments: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(arguments)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.invalid"]);
        git(&["config", "tag.gpgSign", "false"]);
        git(&["commit", "--allow-empty", "-qm", "initial"]);
        let head = git(&["rev-parse", "HEAD"]);
        let repo = Repository::open(&root).await.unwrap();
        let created = repo
            .execute(crate::Operation::TagCreateAnnotated {
                name: "release/topic".into(),
                target: head.clone(),
            })
            .await
            .unwrap();
        assert_eq!(created["code"], 0);
        assert_eq!(git(&["cat-file", "-t", "refs/tags/release/topic"]), "tag");
        assert_eq!(git(&["rev-parse", "refs/tags/release/topic^{}"]), head);
        assert!(
            repo.execute(crate::Operation::TagCreateAnnotated {
                name: "-bad".into(),
                target: head.clone()
            })
            .await
            .is_err()
        );
        assert!(
            repo.execute(crate::Operation::TagCreateAnnotated {
                name: "other".into(),
                target: "--bad".into()
            })
            .await
            .is_err()
        );
        assert_eq!(
            repo.execute(crate::Operation::TagCreateAnnotated {
                name: "symbolic".into(),
                target: "HEAD".into()
            })
            .await
            .unwrap()["code"],
            0
        );
        assert_eq!(
            repo.execute(crate::Operation::TagDelete {
                name: "symbolic".into()
            })
            .await
            .unwrap()["code"],
            0
        );
        let deleted = repo
            .execute(crate::Operation::TagDelete {
                name: "release/topic".into(),
            })
            .await
            .unwrap();
        assert_eq!(deleted["code"], 0);
        assert_eq!(git(&["tag", "--list"]), "");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn merge_preview_reports_clean_conflicts_and_unrelated_histories() {
        let root =
            std::env::temp_dir().join(format!("desktop-merge-preview-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let git = |command: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(command)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.invalid"]);
        std::fs::write(root.join("file"), "base\n").unwrap();
        git(&["add", "file"]);
        git(&["commit", "-qm", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        std::fs::write(root.join("file"), "ours\n").unwrap();
        git(&["commit", "-qam", "ours"]);
        let ours = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "--detach", &base]);
        std::fs::write(root.join("file"), "theirs\n").unwrap();
        git(&["commit", "-qam", "theirs"]);
        let theirs = git(&["rev-parse", "HEAD"]);
        let tree = git(&["rev-parse", "HEAD^{tree}"]);
        let unrelated = git(&["commit-tree", &tree, "-m", "unrelated"]);
        let index = std::fs::read(root.join(".git/index")).unwrap();
        let repo = Repository::open(&root).await.unwrap();
        assert_eq!(repo.merge_base(&ours, &theirs).await.unwrap(), json!(base));
        assert_eq!(repo.merge_base("HEAD", &ours).await.unwrap(), json!(base));
        assert_eq!(
            repo.merge_base(&ours, &unrelated).await.unwrap(),
            Value::Null
        );
        assert_eq!(
            repo.merge_base("missing", &ours).await.unwrap(),
            Value::Null
        );
        assert_eq!(repo.merge_base("--all", &ours).await.unwrap(), Value::Null);
        assert!(repo.merge_base("", &ours).await.is_err());
        assert_eq!(
            repo.merge_tree_preview(&base, &ours).await.unwrap(),
            json!({"kind":"clean"})
        );
        assert_eq!(
            repo.merge_tree_preview(&ours, &theirs).await.unwrap(),
            json!({"kind":"conflicts","conflicted_files":1})
        );
        assert_eq!(
            repo.merge_tree_preview(&ours, &unrelated).await.unwrap(),
            json!({"kind":"invalid"})
        );
        assert!(repo.merge_tree_preview("--bad", &theirs).await.is_err());
        assert!(
            repo.merge_tree_preview(&"f".repeat(40), &theirs)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn preserves_git_trailer_configuration_and_parse_semantics() {
        let root = std::env::temp_dir().join(format!("desktop-trailers-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let git = |command: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(command)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "trailer.separators", "=:"]);
        git(&["config", "trailer.ifexists", "replace"]);
        let repo = Repository::open(&root).await.unwrap();
        let message = "Summary\n\nReviewed-by= Old\n continuation\n";
        let trailer = || crate::CommitTrailer {
            token: "Reviewed-by".into(),
            value: "New <n@example.test>".into(),
        };
        let expected = process::run_git(
            &root,
            &args(&[
                "interpret-trailers",
                "--no-divider",
                "--unfold",
                "--trailer",
                "Reviewed-by=New <n@example.test>",
            ]),
            Some(message.as_bytes().to_vec()),
            None,
        )
        .await
        .unwrap();
        let actual = repo
            .interpret_trailers(message.into(), false, true, vec![trailer()])
            .await
            .unwrap();
        assert_eq!(actual.as_str().unwrap().as_bytes(), expected.stdout);
        let expected = process::run_git(
            &root,
            &args(&["interpret-trailers", "--parse"]),
            Some(message.as_bytes().to_vec()),
            None,
        )
        .await
        .unwrap();
        let actual = repo
            .interpret_trailers(message.into(), true, false, vec![])
            .await
            .unwrap();
        assert_eq!(actual.as_str().unwrap().as_bytes(), expected.stdout);
        assert!(
            repo.interpret_trailers(message.into(), true, false, vec![trailer()])
                .await
                .is_err()
        );
        assert!(
            repo.interpret_trailers("bad\0message".into(), false, false, vec![])
                .await
                .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

impl Repository {
    /// Observe whether the live index and HEAD still match the reviewed tree.
    pub(crate) async fn verify_selected_index(
        &self,
        tree: &str,
        head: Option<&str>,
        branch: Option<&str>,
    ) -> Result<Value> {
        let oid = |s: &str| matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit());
        anyhow::ensure!(
            oid(tree) && head.is_none_or(oid),
            "Invalid reviewed object ID"
        );
        anyhow::ensure!(
            branch.is_none_or(|value| value.starts_with("refs/heads/")
                && !value.contains(['\0', '\n', '\r'])
                && value.len() <= 4096),
            "Invalid reviewed branch"
        );
        let current = || async {
            let head = self
                .run(&args(&["rev-parse", "--verify", "--quiet", "HEAD"]))
                .await?;
            let branch = self
                .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
                .await?;
            anyhow::ensure!(
                matches!(head.code, 0 | 1) && matches!(branch.code, 0 | 1),
                "Cannot inspect HEAD"
            );
            Ok::<_, anyhow::Error>((
                if head.code == 0 {
                    Some(String::from_utf8(head.stdout)?.trim().to_owned())
                } else {
                    None
                },
                if branch.code == 0 {
                    Some(String::from_utf8(branch.stdout)?.trim().to_owned())
                } else {
                    None
                },
            ))
        };
        let before = current().await?;
        if before.0.as_deref() != head || before.1.as_deref() != branch {
            return Ok(json!({"matches":false}));
        }
        let actual = self.live_index_tree().await?;
        let after = current().await?;
        Ok(json!({"matches": before == after && actual == tree}))
    }

    /// Delete the selected initial commit only if it is still HEAD.
    pub(crate) async fn renderer_delete_initial_head(&self, expected_oid: &str) -> Result<Value> {
        anyhow::ensure!(
            matches!(expected_oid.len(), 40 | 64)
                && expected_oid.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid expected commit ID"
        );
        let parents = self
            .run(&args(&[
                "rev-list",
                "--parents",
                "-n",
                "1",
                "--end-of-options",
                expected_oid,
                "--",
            ]))
            .await?;
        anyhow::ensure!(parents.code == 0, "Cannot read selected commit");
        let fields: Vec<_> = parents
            .stdout
            .split(|b| b.is_ascii_whitespace())
            .filter(|v| !v.is_empty())
            .collect();
        anyhow::ensure!(
            fields.len() == 1 && fields[0].eq_ignore_ascii_case(expected_oid.as_bytes()),
            "Selected commit is not an initial commit"
        );
        let output = self
            .run(&args(&[
                "update-ref",
                "-d",
                "-m",
                "Reverting first commit",
                "HEAD",
                expected_oid,
            ]))
            .await?;
        Ok(serde_json::to_value(output.text())?)
    }

    /// Match the renderer's merge-base behavior without accepting Git options.
    pub(crate) async fn merge_base(&self, first: &str, second: &str) -> Result<Value> {
        for revision in [first, second] {
            anyhow::ensure!(
                !revision.is_empty() && revision.len() <= 4096 && !revision.contains('\0'),
                "invalid merge-base revision"
            );
        }
        let output = self
            .run(&args(&["merge-base", "--", first, second]))
            .await?;
        if matches!(output.code, 1 | 128) {
            return Ok(Value::Null);
        }
        let output = checked(output)?;
        let oid = std::str::from_utf8(&output.stdout)?.trim();
        anyhow::ensure!(
            matches!(oid.len(), 40 | 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid merge-base result"
        );
        Ok(json!(oid))
    }

    /// Compute mergeability for immutable commit IDs without touching the index.
    pub(crate) async fn merge_tree_preview(&self, ours: &str, theirs: &str) -> Result<Value> {
        let valid = |oid: &str| {
            matches!(oid.len(), 40 | 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit())
        };
        anyhow::ensure!(
            valid(ours) && valid(theirs) && ours.len() == theirs.len(),
            "invalid merge commit IDs"
        );
        let base = self.run(&args(&["merge-base", "--", ours, theirs])).await?;
        if base.code == 1 && base.stdout.is_empty() && base.stderr.is_empty() {
            return Ok(json!({"kind": "invalid"}));
        }
        checked(base)?;
        let output = self
            .run(&args(&[
                "merge-tree",
                "--write-tree",
                "--name-only",
                "--no-messages",
                "-z",
                ours,
                theirs,
            ]))
            .await?;
        anyhow::ensure!(
            matches!(output.code, 0 | 1),
            "merge-tree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        anyhow::ensure!(
            output.stdout.last() == Some(&0),
            "invalid merge-tree result"
        );
        let count = output
            .stdout
            .iter()
            .filter(|byte| **byte == 0)
            .count()
            .saturating_sub(1);
        if count == 0 {
            anyhow::ensure!(
                output.code == 0,
                "merge-tree reported a conflict without paths"
            );
            Ok(json!({"kind": "clean"}))
        } else {
            anyhow::ensure!(
                output.code == 1,
                "merge-tree returned unexpected conflict paths"
            );
            Ok(json!({"kind": "conflicts", "conflicted_files": count}))
        }
    }

    /// Preserve Git's configured trailer placement, duplication and commands.
    pub(crate) async fn interpret_trailers(
        &self,
        message: String,
        parse: bool,
        unfold: bool,
        trailers: Vec<crate::CommitTrailer>,
    ) -> Result<Value> {
        anyhow::ensure!(
            !message.contains('\0') && message.len() <= 1024 * 1024,
            "invalid commit message"
        );
        anyhow::ensure!(
            trailers.len() <= 1024 && (!parse || (!unfold && trailers.is_empty())),
            "invalid trailer request"
        );
        let mut command = args(&["interpret-trailers"]);
        if parse {
            command.push("--parse".into());
        } else {
            command.push("--no-divider".into());
            if unfold {
                command.push("--unfold".into());
            }
            let mut size = 0usize;
            for trailer in trailers {
                anyhow::ensure!(
                    !trailer.token.contains('\0') && !trailer.value.contains('\0'),
                    "trailer contains NUL"
                );
                size = size
                    .saturating_add(trailer.token.len())
                    .saturating_add(trailer.value.len());
                anyhow::ensure!(size <= 1024 * 1024, "trailers exceed the request limit");
                command.extend(args(&[
                    "--trailer",
                    &format!("{}={}", trailer.token, trailer.value),
                ]));
            }
        }
        let output = checked(
            process::run_git(&self.path, &command, Some(message.into_bytes()), None).await?,
        )?;
        Ok(json!(String::from_utf8_lossy(&output.stdout)))
    }
    /// Inspect selected worktree entries without following a file symlink out
    /// of the repository. Git stores the link itself, not its target.
    pub(crate) async fn renderer_large_files(&self, paths: Vec<GitPath>) -> Result<Value> {
        anyhow::ensure!(paths.len() <= 1000, "too many paths for file-size review");
        let paths = paths
            .into_iter()
            .map(validate_path)
            .collect::<Result<Vec<_>>>()?;
        anyhow::ensure!(
            paths
                .iter()
                .map(|path| path.as_encoded_bytes().len())
                .sum::<usize>()
                <= 60_000,
            "file-size review paths exceed the request limit"
        );
        let root = self.path.clone();
        let large = cancellation::blocking(move || {
            let directory = cap_std::fs::Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
            let mut large = Vec::with_capacity(paths.len());
            for path in paths {
                cancellation::Cancellation::current().check()?;
                match directory.symlink_metadata(path) {
                    Ok(metadata) => {
                        large.push(metadata.is_file() && metadata.len() > LARGE_BLOB_LIMIT)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => large.push(false),
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(large)
        })
        .await?;
        Ok(json!(large))
    }
}
#[derive(Serialize)]
pub(crate) struct LargeFile {
    path: GitPath,
    display_path: String,
    oid: String,
    size: u64,
    tracked_by_lfs: bool,
}
#[derive(Serialize)]
pub(crate) struct StagedReview {
    pub threshold_bytes: u64,
    pub staged_count: usize,
    pub files: Vec<GitPath>,
    pub large_files: Vec<LargeFile>,
}
#[derive(Serialize)]
struct CommitPreview {
    basis: String,
    threshold_bytes: u64,
    staged_count: usize,
    large_files: Vec<LargeFile>,
}
struct Candidate {
    path: Vec<u8>,
    oid: String,
}

impl Repository {
    /// Create the first commit from the paths shown by the creation dialog.
    /// Refuse to reset an existing index or commit on an already born branch.
    pub(crate) async fn initial_commit(&self, paths: Vec<GitPath>) -> Result<Value> {
        anyhow::ensure!(
            !paths.is_empty() && paths.len() <= 32_768,
            "Select between 1 and 32768 files"
        );
        let mut input = Vec::new();
        let mut unique = BTreeSet::new();
        for path in paths {
            let path = validate_path(path)?;
            let bytes = path.as_encoded_bytes();
            anyhow::ensure!(
                unique.insert(bytes.to_vec()),
                "Duplicate initial commit path"
            );
            input.extend_from_slice(bytes);
            input.push(0);
            anyhow::ensure!(
                input.len() <= 1024 * 1024,
                "Initial commit paths exceed 1 MiB"
            );
        }
        anyhow::ensure!(
            self.commit_head_identity().await?.0.is_none(),
            "Repository already has a commit"
        );
        let index = checked(self.run(&args(&["ls-files", "--stage", "-z"])).await?)?;
        anyhow::ensure!(
            index.stdout.is_empty(),
            "Repository index is not empty; inspect staged files before committing"
        );
        checked(
            process::update_index(
                &self.path,
                &args(&[
                    "update-index",
                    "--add",
                    "--remove",
                    "--replace",
                    "-z",
                    "--stdin",
                ]),
                input,
            )
            .await?,
        )
        .context("Staging failed; inspect the index before retrying")?;
        anyhow::ensure!(
            self.commit_head_identity().await?.0.is_none(),
            "Repository HEAD changed during staging; inspect the index before retrying"
        );
        self.reviewed_commit("Initial commit".into(), String::new(), false, None)
            .await
            .context("Initial commit failed; inspect the index and repository before retrying")
    }

    pub(crate) async fn commit_preview(&self) -> Result<Value> {
        Ok(serde_json::to_value(self.capture_commit_preview().await?)?)
    }

    pub(crate) async fn reviewed_commit(
        &self,
        summary: String,
        description: String,
        allow_large_files: bool,
        basis: Option<String>,
    ) -> Result<Value> {
        anyhow::ensure!(
            !summary.trim().is_empty() && !summary.contains(['\n', '\r', '\0']),
            "commit summary must be a nonempty single line"
        );
        anyhow::ensure!(
            !description.contains('\0'),
            "commit description contains NUL"
        );
        anyhow::ensure!(
            !allow_large_files || basis.is_some(),
            "review commit-preview and supply its basis before allowing large files"
        );
        let preview = self.capture_commit_preview().await?;
        if let Some(basis) = basis {
            anyhow::ensure!(
                basis == preview.basis,
                "Staged files, HEAD, branch, or LFS attributes changed; review the commit again"
            );
        }
        if !preview.large_files.is_empty() && !allow_large_files {
            return Ok(
                json!({"code":2,"stdout":"","stderr":"Staged Git blobs exceed 100 MiB. Use Git LFS or unstage them, or review commit-preview and explicitly allow large files with its basis.","requires_confirmation":true,"preview":preview}),
            );
        }
        // The certificate is checked immediately before invocation. Git owns
        // index/ref locking and hook/signing semantics; hooks can change the
        // index just as they can in an ordinary terminal commit.
        let mut command = args(&["commit", "-m", &summary]);
        if !description.is_empty() {
            command.extend(args(&["-m", &description]));
        }
        Ok(serde_json::to_value(
            process::run_commit(&self.path, &command).await?.text(),
        )?)
    }

    async fn commit_head_identity(&self) -> Result<(Option<String>, Option<String>)> {
        let head = self.run(&args(&["rev-parse", "--verify", "HEAD"])).await?;
        let head = if head.code == 0 {
            Some(String::from_utf8(head.stdout)?.trim().to_owned())
        } else {
            // A quiet lookup distinguishes a genuinely unborn HEAD from a broken
            // repository while retaining Git's native diagnostic on failure.
            let missing = self
                .run(&args(&["rev-parse", "--verify", "--quiet", "HEAD"]))
                .await?;
            anyhow::ensure!(missing.code == 1, "cannot resolve HEAD for commit review");
            None
        };
        let symbolic = self
            .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
            .await?;
        anyhow::ensure!(
            matches!(symbolic.code, 0 | 1),
            "cannot inspect the current branch"
        );
        Ok((
            head,
            if symbolic.code == 0 {
                Some(String::from_utf8(symbolic.stdout)?.trim().to_owned())
            } else {
                None
            },
        ))
    }

    async fn capture_commit_preview(&self) -> Result<CommitPreview> {
        let index_path = self.index_path().await?;
        let index = index_identity(index_path.clone()).await?;
        let head = self.commit_head_identity().await?;
        let git_directory = self.git_directory().await?;
        let operation = cancellation::blocking({
            let path = git_directory.clone();
            move || crate::discard::operation_certificate(&path)
        })
        .await?;
        let review = self.review_staged_index(None, None).await?;
        let StagedReview {
            staged_count,
            large_files,
            ..
        } = review;
        anyhow::ensure!(
            index_identity(index_path).await? == index
                && self.commit_head_identity().await? == head,
            "Index or HEAD changed while loading commit review; retry"
        );
        anyhow::ensure!(
            cancellation::blocking(move || crate::discard::operation_certificate(&git_directory))
                .await?
                == operation,
            "Repository operation changed during commit review; retry"
        );
        let basis = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(
                self.id(),
                index,
                head,
                LARGE_BLOB_LIMIT,
                staged_count,
                &operation,
                &large_files
            ))?)
        );
        Ok(CommitPreview {
            basis,
            threshold_bytes: LARGE_BLOB_LIMIT,
            staged_count,
            large_files,
        })
    }
    pub(crate) async fn review_staged_index(
        &self,
        index: Option<&Path>,
        base: Option<&str>,
    ) -> Result<StagedReview> {
        // Replacement refs must not hide the size of the real blobs that will
        // be recorded in a tree and sent to a remote. Disable rename detection
        // so every new/changed destination has one unambiguous raw record.
        let mut command = args(&[
            "--no-replace-objects",
            "diff",
            "--cached",
            "--raw",
            "--no-abbrev",
            "--no-renames",
            "--ita-invisible-in-index",
            "--no-ext-diff",
            "--no-textconv",
            "-z",
        ]);
        if let Some(base) = base {
            command.push(base.into());
        }
        command.push("--".into());
        let raw = checked(process::run_git(&self.path, &command, None, index).await?)?;
        let (staged_count, candidates, files) = candidates(&raw.stdout)?;
        let unique: BTreeSet<_> = candidates.iter().map(|entry| entry.oid.as_str()).collect();
        let mut sizes = BTreeMap::new();
        if !unique.is_empty() {
            let input = unique
                .iter()
                .map(|oid| format!("{oid}\n"))
                .collect::<String>()
                .into_bytes();
            let output = checked(
                process::run_git(
                    &self.path,
                    &args(&[
                        "--no-replace-objects",
                        "cat-file",
                        "--batch-check=%(objectname) %(objecttype) %(objectsize)",
                    ]),
                    Some(input),
                    None,
                )
                .await?,
            )?;
            for line in std::str::from_utf8(&output.stdout)?.lines() {
                let fields: Vec<_> = line.split_ascii_whitespace().collect();
                anyhow::ensure!(
                    fields.len() == 3 && fields[1] == "blob" && unique.contains(fields[0]),
                    "missing or invalid staged Git blob"
                );
                let size = fields[2].parse::<u64>()?;
                anyhow::ensure!(
                    sizes.insert(fields[0].to_owned(), size).is_none(),
                    "duplicate staged blob metadata"
                );
            }
            anyhow::ensure!(
                sizes.len() == unique.len(),
                "incomplete staged blob metadata"
            );
        }
        let mut large_files = Vec::new();
        for entry in candidates {
            let size = *sizes.get(&entry.oid).context("missing staged blob size")?;
            if size > LARGE_BLOB_LIMIT {
                anyhow::ensure!(
                    large_files.len() < 1000,
                    "more than 1000 oversized paths are staged; split the commit before review"
                );
                large_files.push(LargeFile {
                    display_path: String::from_utf8_lossy(&entry.path).into_owned(),
                    path: GitPath::Bytes { bytes: entry.path },
                    oid: entry.oid,
                    size,
                    tracked_by_lfs: false,
                });
            }
        }
        if !large_files.is_empty() {
            let mut command = args(&["check-attr", "-z", "filter", "--"]);
            for file in &large_files {
                command.push(validate_path(file.path.clone())?);
            }
            let attributes = checked(self.run(&command).await?)?;
            let fields: Vec<_> = attributes.stdout.split(|byte| *byte == 0).collect();
            anyhow::ensure!(
                fields.len() == large_files.len() * 3 + 1 && fields.last() == Some(&b"".as_slice()),
                "invalid LFS attribute response during commit review"
            );
            for (file, fields) in large_files.iter_mut().zip(fields.as_chunks::<3>().0) {
                let GitPath::Bytes { bytes } = &file.path else {
                    unreachable!("candidate paths use bytes")
                };
                anyhow::ensure!(
                    fields[0] == bytes && fields[1] == b"filter",
                    "unexpected commit attribute"
                );
                file.tracked_by_lfs = fields[2] == b"lfs";
            }
        }
        Ok(StagedReview {
            threshold_bytes: LARGE_BLOB_LIMIT,
            staged_count,
            files,
            large_files,
        })
    }
}

async fn index_identity(path: PathBuf) -> Result<Option<String>> {
    cancellation::blocking(move || {
        let Some(bytes) = read_index(&path)? else {
            return Ok(None);
        };
        let mut digest = Sha256::new();
        for chunk in bytes.chunks(64 * 1024) {
            cancellation::Cancellation::current().check()?;
            digest.update(chunk);
        }
        Ok(Some(format!("{:x}", digest.finalize())))
    })
    .await
}
fn checked(output: process::RawOutput) -> Result<process::RawOutput> {
    if output.code != 0 {
        bail!("{}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(output)
}
fn candidates(bytes: &[u8]) -> Result<(usize, Vec<Candidate>, Vec<GitPath>)> {
    let mut fields = bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    let mut count = 0;
    let mut files = Vec::new();
    let mut paths = Vec::new();
    while let Some(header) = fields.next() {
        let header = std::str::from_utf8(header)?
            .strip_prefix(':')
            .context("invalid staged raw record")?;
        let metadata: Vec<_> = header.split_ascii_whitespace().collect();
        anyhow::ensure!(metadata.len() == 5, "invalid staged raw fields");
        let first = fields.next().context("missing staged path")?;
        let path = if metadata[4].starts_with(['R', 'C']) {
            fields.next().context("missing staged rename path")?
        } else {
            first
        };
        count += 1;
        let git_path = GitPath::Bytes {
            bytes: path.to_vec(),
        };
        validate_path(git_path.clone())?;
        paths.push(git_path);
        if matches!(metadata[1], "100644" | "100755")
            && !metadata[3].bytes().all(|byte| byte == b'0')
        {
            anyhow::ensure!(
                matches!(metadata[3].len(), 40 | 64)
                    && metadata[3].bytes().all(|byte| byte.is_ascii_hexdigit()),
                "invalid staged object ID"
            );
            files.push(Candidate {
                path: path.to_vec(),
                oid: metadata[3].to_owned(),
            });
        }
    }
    Ok((count, files, paths))
}
