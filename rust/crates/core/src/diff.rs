//! Captured file diffs and content-based partial index transactions.
use crate::{GitPath, Repository, args, os_from_bytes, process, validate_path};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

impl Repository {
    /// Capture the currently staged patch for commit-message generation.
    pub(crate) async fn renderer_staged_diff(&self, amend_head: Option<&str>) -> Result<Value> {
        let mut command = args(&[
            "diff",
            "--no-ext-diff",
            "--patch-with-raw",
            "--no-color",
            "--staged",
        ]);
        if let Some(head) = amend_head {
            anyhow::ensure!(
                matches!(head.len(), 40 | 64) && head.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "Invalid amend commit"
            );
            command.push(format!("{head}^").into());
        }
        let permit = crate::comparison_snapshots::reserve().await?;
        let output = process::run_staged_patch(&self.path, &command).await?;
        ensure_success(&output)?;
        anyhow::ensure!(
            output.stdout.len() <= 10 * 1024 * 1024,
            "Diff is too large to render"
        );
        crate::comparison_snapshots::insert(&self.id(), output.stdout, permit)
    }

    /// Compute a tree from a private copy without refreshing the live index.
    pub(crate) async fn live_index_tree(&self) -> Result<String> {
        let index_path = self.index_path().await?;
        let original = read_index(&index_path)?;
        let scratch = Scratch::new(&index_path)?;
        if let Some(index) = &original {
            scratch.write_index(index)?;
        } else {
            let empty = process::run_git(
                &self.path,
                &args(&["read-tree", "--empty"]),
                None,
                Some(&scratch.alternate_index),
            )
            .await?;
            ensure_success(&empty)?;
        }
        let output = process::run_git(
            &self.path,
            &args(&["write-tree"]),
            None,
            Some(&scratch.alternate_index),
        )
        .await?;
        ensure_success(&output)?;
        anyhow::ensure!(
            read_index(&index_path)? == original,
            "Index changed while checking the reviewed tree"
        );
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    }

    /// Produce the renderer's raw working diff without exposing arbitrary Git commands.
    pub(crate) async fn renderer_working_diff(
        &self,
        path: GitPath,
        base: crate::RendererDiffBase,
        hide_whitespace: bool,
        no_textconv: bool,
    ) -> Result<Value> {
        let path = validate_path(path)?;
        let mut command = args(&["diff"]);
        if no_textconv {
            command.extend(args(&["--no-textconv"]));
        }
        if hide_whitespace {
            command.extend(args(&["-w"]));
        }
        command.extend(args(&[
            "--no-ext-diff",
            "--patch-with-raw",
            "-z",
            "--no-color",
        ]));
        let output = match base {
            crate::RendererDiffBase::Empty => {
                // Snapshot the file before invoking Git; `--no-index` must not
                // follow a renderer-supplied path outside the repository.
                let side = self.worktree_side(&path).await?;
                if side.entry.is_none() {
                    bail!("working file no longer exists");
                }
                if side.oversized {
                    bail!("file exceeds the working diff limit");
                }
                command.extend(args(&["--no-index", "--", "/dev/null", "-"]));
                process::run_git(&self.path, &command, Some(side.bytes), None).await?
            }
            _ => {
                if matches!(base, crate::RendererDiffBase::Head) {
                    command.push("HEAD".into());
                }
                command.push("--".into());
                command.push(path);
                self.run(&command).await?
            }
        };
        if output.code != 0 && !(matches!(base, crate::RendererDiffBase::Empty) && output.code == 1)
        {
            ensure_success(&output)?;
        }
        Ok(
            json!({"stdout":STANDARD.encode(output.stdout), "stderr":STANDARD.encode(output.stderr)}),
        )
    }
}

const CONTENT_LIMIT: usize = 1024 * 1024;
const INDEX_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Clone)]
struct Entry {
    mode: String,
    oid: String,
}

#[derive(Clone, Serialize)]
struct LfsPreview {
    #[serde(flatten)]
    object: crate::lfs::LfsObject,
    state: &'static str,
}
struct Side {
    lfs: Option<LfsPreview>,
    entry: Option<Entry>,
    bytes: Vec<u8>,
    oversized: bool,
    size: u64,
}

pub(crate) struct SelectedWorktreeSnapshot {
    patch: Vec<u8>,
    pub(crate) index: Vec<u8>,
    pub(crate) live_index: Option<Vec<u8>>,
    pub(crate) head: Vec<u8>,
    pub(crate) branch: Vec<u8>,
    pub(crate) operation: String,
    pub(crate) tree: Option<String>,
    review: Option<crate::commit::StagedReview>,
}

#[derive(Serialize)]
struct Preview {
    #[serde(skip_serializing_if = "Option::is_none")]
    lfs: Option<LfsPreview>,
    mode: Option<String>,
    oid: Option<String>,
    size: u64,
    image: Option<Image>,
    text: Option<String>,
}
#[derive(Serialize)]
struct Image {
    mime: &'static str,
    data: String,
}
#[derive(Serialize)]
struct View {
    #[serde(skip)]
    selection_lfs: bool,
    path: GitPath,
    staged: bool,
    kind: &'static str,
    basis: String,
    can_select: bool,
    before: Preview,
    after: Preview,
    hunks: Vec<Hunk>,
}
#[derive(Serialize)]
struct Hunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    lines: Vec<Line>,
}
#[derive(Serialize)]
struct Line {
    id: usize,
    kind: &'static str,
    old_line: Option<usize>,
    new_line: Option<usize>,
    text: String,
    no_newline: bool,
    #[serde(skip)]
    bytes: Vec<u8>,
}

impl Side {
    fn preview(&self) -> Preview {
        Preview {
            lfs: self.lfs.clone(),
            mode: self.entry.as_ref().map(|entry| entry.mode.clone()),
            oid: self.entry.as_ref().map(|entry| entry.oid.clone()),
            size: self.size,
            text: self
                .entry
                .as_ref()
                .filter(|entry| entry.mode == "120000")
                .map(|_| String::from_utf8_lossy(&self.bytes).into_owned()),
            image: image_type(&self.bytes).map(|mime| Image {
                mime,
                data: STANDARD.encode(&self.bytes),
            }),
        }
    }
    fn lfs_unavailable(&self) -> bool {
        self.lfs
            .as_ref()
            .is_some_and(|preview| !matches!(preview.state, "available" | "too_large"))
    }
    fn exists(&self) -> bool {
        self.entry.is_some()
    }
    fn regular(&self) -> bool {
        self.entry
            .as_ref()
            .is_none_or(|entry| matches!(entry.mode.as_str(), "100644" | "100755"))
    }
}

fn normalized_selection_path(path: GitPath) -> Result<std::ffi::OsString> {
    let path = validate_path(path)?;
    Ok(Path::new(&path)
        .components()
        .collect::<PathBuf>()
        .into_os_string())
}

impl Repository {
    async fn snapshot_head_index(
        &self,
        scratch: &Scratch,
    ) -> Result<(process::RawOutput, process::RawOutput)> {
        let head = self
            .run(&args(&["rev-parse", "--verify", "--quiet", "HEAD"]))
            .await?;
        anyhow::ensure!(
            matches!(head.code, 0 | 1),
            "cannot resolve HEAD for selected changes"
        );
        let branch = self
            .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
            .await?;
        anyhow::ensure!(
            matches!(branch.code, 0 | 1),
            "cannot resolve branch for selected changes"
        );
        let base = if head.code == 0 {
            args(&["read-tree", std::str::from_utf8(&head.stdout)?.trim()])
        } else {
            args(&["read-tree", "--empty"])
        };
        let initialized =
            process::run_git(&self.path, &base, None, Some(&scratch.alternate_index)).await?;
        anyhow::ensure!(
            initialized.code == 0,
            "cannot initialize selected-change snapshot"
        );
        Ok((head, branch))
    }

    async fn ensure_snapshot_head(
        &self,
        head: &process::RawOutput,
        branch: &process::RawOutput,
    ) -> Result<()> {
        let now = self
            .run(&args(&["rev-parse", "--verify", "--quiet", "HEAD"]))
            .await?;
        let current = self
            .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
            .await?;
        anyhow::ensure!(
            head.code == now.code
                && head.stdout == now.stdout
                && branch.code == current.code
                && branch.stdout == current.stdout,
            "HEAD or branch changed while capturing selected changes; retry"
        );
        Ok(())
    }

    /// Preview selectable changes against HEAD regardless of the live index.
    pub(crate) async fn copilot_commit_diff_detail(
        &self,
        path: GitPath,
        original_path: Option<GitPath>,
    ) -> Result<Value> {
        let path = normalized_selection_path(path)?;
        let original_path = original_path.map(normalized_selection_path).transpose()?;
        let scratch = Scratch::new(&self.index_path().await?)?;
        let (head, branch) = self.snapshot_head_index(&scratch).await?;
        let original = read_index(&scratch.alternate_index)?;
        let (view, _, _) = self
            .capture_diff(
                &path,
                false,
                &original,
                &scratch,
                true,
                original_path.as_deref(),
            )
            .await?;
        self.ensure_snapshot_head(&head, &branch).await?;
        Ok(serde_json::to_value(view)?)
    }

    /// Snapshot reviewed working-tree selections in a private index based on
    /// HEAD, without changing the caller's real index or working tree.
    pub(crate) async fn selected_worktree_patch(
        &self,
        paths: Vec<GitPath>,
        remove_paths: Vec<GitPath>,
        partial: Vec<crate::CopilotLineSelection>,
        amend_head: Option<&str>,
        expected_head: Option<&crate::protocol::CopilotExpectedHead>,
    ) -> Result<Vec<u8>> {
        Ok(self
            .capture_selected_worktree(
                paths,
                remove_paths,
                partial,
                amend_head,
                expected_head,
                false,
                false,
            )
            .await?
            .patch)
    }

    /// Preview the exact private index that a later selected-file commit must
    /// recapture and compare before publishing. This operation does not write
    /// the live index or create a commit.
    #[cfg(test)]
    async fn selected_commit_preview(
        &self,
        paths: Vec<GitPath>,
        remove_paths: Vec<GitPath>,
        partial: Vec<crate::CopilotLineSelection>,
        amend_head: Option<&str>,
        expected_head: Option<&crate::protocol::CopilotExpectedHead>,
    ) -> Result<Value> {
        self.selected_commit_preview_with_empty(
            paths,
            remove_paths,
            partial,
            false,
            amend_head,
            expected_head,
        )
        .await
    }

    pub(crate) async fn selected_commit_preview_with_empty(
        &self,
        paths: Vec<GitPath>,
        remove_paths: Vec<GitPath>,
        partial: Vec<crate::CopilotLineSelection>,
        allow_empty: bool,
        amend_head: Option<&str>,
        expected_head: Option<&crate::protocol::CopilotExpectedHead>,
    ) -> Result<Value> {
        Ok(self
            .capture_selected_commit_review(
                paths,
                remove_paths,
                partial,
                allow_empty,
                amend_head,
                expected_head,
            )
            .await?
            .1)
    }

    pub(crate) async fn capture_selected_commit_review(
        &self,
        paths: Vec<GitPath>,
        remove_paths: Vec<GitPath>,
        partial: Vec<crate::CopilotLineSelection>,
        allow_empty: bool,
        amend_head: Option<&str>,
        expected_head: Option<&crate::protocol::CopilotExpectedHead>,
    ) -> Result<(SelectedWorktreeSnapshot, Value)> {
        let requested = serde_json::to_vec(&(
            &paths,
            &remove_paths,
            &partial,
            allow_empty,
            amend_head,
            expected_head,
        ))?;
        anyhow::ensure!(
            requested.len() <= 1024 * 1024,
            "selected commit review request is too large"
        );
        let snapshot = self
            .capture_selected_worktree(
                paths,
                remove_paths,
                partial,
                amend_head,
                expected_head,
                true,
                allow_empty,
            )
            .await?;
        let review = snapshot
            .review
            .as_ref()
            .context("missing selected-index review")?;
        let review_bytes = serde_json::to_vec(review)?;
        anyhow::ensure!(
            review_bytes.len() <= 1024 * 1024,
            "selected commit review is too large; split the selection"
        );
        let mut digest = Sha256::new();
        digest.update(b"desktop-selected-commit-preview-v3\0");
        let repository_id = self.id();
        for value in [
            repository_id.as_bytes(),
            &requested[..],
            &snapshot.head,
            &snapshot.branch,
            snapshot.operation.as_bytes(),
            &snapshot.index,
            &review_bytes,
        ] {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value);
        }
        match &snapshot.live_index {
            Some(index) => {
                digest.update([1]);
                digest.update((index.len() as u64).to_be_bytes());
                digest.update(index);
            }
            None => digest.update([0]),
        }
        let result = json!({"basis":format!("{:x}", digest.finalize()), "tree":snapshot.tree, "head": if snapshot.head.is_empty() { None } else { Some(std::str::from_utf8(&snapshot.head)?.trim().to_owned()) }, "branch": if snapshot.branch.is_empty() { None } else { Some(std::str::from_utf8(&snapshot.branch)?.trim().to_owned()) }, "review":review});
        Ok((snapshot, result))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Selection and review options are independently verified"
    )]
    async fn capture_selected_worktree(
        &self,
        paths: Vec<GitPath>,
        remove_paths: Vec<GitPath>,
        partial: Vec<crate::CopilotLineSelection>,
        amend_head: Option<&str>,
        expected_head: Option<&crate::protocol::CopilotExpectedHead>,
        certify_live_index: bool,
        allow_empty: bool,
    ) -> Result<SelectedWorktreeSnapshot> {
        anyhow::ensure!(
            !paths.is_empty()
                || !remove_paths.is_empty()
                || !partial.is_empty()
                || amend_head.is_some()
                || allow_empty,
            "select at least one working-tree file"
        );
        let mut path_bytes = Vec::new();
        let mut remove_bytes = Vec::new();
        let mut removed_names = HashSet::new();
        for path in remove_paths {
            let path = normalized_selection_path(path)?;
            anyhow::ensure!(
                removed_names.insert(path.as_encoded_bytes().to_vec()),
                "removed paths must not repeat"
            );
            remove_bytes.extend_from_slice(path.as_encoded_bytes());
            remove_bytes.push(0);
            anyhow::ensure!(
                remove_bytes.len() <= 1024 * 1024,
                "removed paths are too large"
            );
        }
        let mut names = HashSet::new();
        for path in paths {
            let path = normalized_selection_path(path)?;
            let absolute = self.path.join(&path);
            if let Ok(metadata) = fs::symlink_metadata(&absolute) {
                anyhow::ensure!(
                    !metadata.is_dir(),
                    "select individual files, not directories"
                );
            }
            anyhow::ensure!(
                names.insert(path.as_encoded_bytes().to_vec()),
                "selected paths must not overlap"
            );
            path_bytes.extend_from_slice(path.as_encoded_bytes());
            path_bytes.push(0);
            anyhow::ensure!(
                path_bytes.len() <= 1024 * 1024,
                "selected paths are too large"
            );
        }
        let scratch = Scratch::new(&self.index_path().await?)?;
        let live_index = if certify_live_index {
            read_index(&scratch.index_path)?
        } else {
            None
        };
        let git_directory = if certify_live_index {
            Some(self.git_directory().await?)
        } else {
            None
        };
        let operation = if let Some(directory) = &git_directory {
            let directory = directory.clone();
            crate::cancellation::blocking(move || crate::discard::operation_certificate(&directory))
                .await?
        } else {
            String::new()
        };
        let (head, branch) = self.snapshot_head_index(&scratch).await?;
        if let Some(expected) = expected_head {
            let oid = (head.code == 0)
                .then(|| std::str::from_utf8(&head.stdout).map(str::trim))
                .transpose()?;
            let reference = (branch.code == 0)
                .then(|| std::str::from_utf8(&branch.stdout).map(str::trim))
                .transpose()?;
            anyhow::ensure!(
                expected.sha.as_deref() == oid && expected.reference.as_deref() == reference,
                "HEAD or branch changed since selection was restored; review selected changes"
            );
        }
        let head_oid = std::str::from_utf8(&head.stdout)?.trim();
        let parent = if let Some(expected) = amend_head {
            anyhow::ensure!(
                head.code == 0 && expected == head_oid,
                "HEAD changed since the amend request was prepared"
            );
            let result = self
                .run(&args(&[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{head_oid}^"),
                ]))
                .await?;
            if result.code == 0 {
                Some(String::from_utf8(result.stdout)?.trim().to_owned())
            } else {
                anyhow::ensure!(result.code == 1, "cannot resolve the amend parent");
                // A failed parent lookup may also mean a shallow or damaged
                // history. Only use the empty tree after inspecting the raw
                // commit and proving it really has no parent.
                let commit = self
                    .run(&args(&[
                        "--no-replace-objects",
                        "cat-file",
                        "commit",
                        head_oid,
                    ]))
                    .await?;
                anyhow::ensure!(commit.code == 0, "cannot inspect the amend commit");
                let header = commit
                    .stdout
                    .split(|b| *b == b'\n')
                    .take_while(|line| !line.is_empty());
                anyhow::ensure!(
                    !header.into_iter().any(|line| line.starts_with(b"parent ")),
                    "cannot amend a commit whose parent is unavailable"
                );
                let empty = process::run_git(
                    &self.path,
                    &args(&["hash-object", "-w", "-t", "tree", "--stdin"]),
                    Some(Vec::new()),
                    None,
                )
                .await?;
                anyhow::ensure!(
                    empty.code == 0,
                    "cannot create the empty tree for amend review"
                );
                let oid = String::from_utf8(empty.stdout)?.trim().to_owned();
                anyhow::ensure!(
                    oid.len() == head_oid.len() && oid.bytes().all(|b| b.is_ascii_hexdigit()),
                    "invalid empty tree object ID"
                );
                Some(oid)
            }
        } else {
            None
        };
        let original = read_index(&scratch.alternate_index)?;
        let mut instructions = Vec::new();
        let partial = partial
            .into_iter()
            .map(|selection| {
                let path = normalized_selection_path(selection.path)?;
                anyhow::ensure!(
                    names.insert(path.as_encoded_bytes().to_vec()),
                    "selected paths must not overlap"
                );
                Ok((
                    path,
                    selection
                        .original_path
                        .map(normalized_selection_path)
                        .transpose()?,
                    selection.basis,
                    selection.lines,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let partial_names: HashSet<_> = partial
            .iter()
            .map(|(path, _, _, _)| path.as_encoded_bytes().to_vec())
            .collect();
        let mut rename_sources = HashSet::new();
        for (path, original_path, basis, lines) in partial {
            if let Some(source) = original_path.as_deref() {
                anyhow::ensure!(
                    !partial_names.contains(source.as_encoded_bytes())
                        && rename_sources.insert(source.as_encoded_bytes().to_vec()),
                    "rename source overlaps another selection"
                );
                // A recreated source is excluded unless it was selected as a
                // separate file. Never leak it into the generated prompt.
                if !names.contains(source.as_encoded_bytes())
                    && removed_names.insert(source.as_encoded_bytes().to_vec())
                {
                    remove_bytes.extend_from_slice(source.as_encoded_bytes());
                    remove_bytes.push(0);
                    anyhow::ensure!(
                        remove_bytes.len() <= 1024 * 1024,
                        "removed paths are too large"
                    );
                }
            }
            anyhow::ensure!(!lines.is_empty(), "select at least one changed line");
            let (view, before, after) = self
                .capture_diff(
                    &path,
                    false,
                    &original,
                    &scratch,
                    true,
                    original_path.as_deref(),
                )
                .await?;
            anyhow::ensure!(
                view.can_select,
                "This diff does not support line selection; use whole-file staging"
            );
            anyhow::ensure!(
                view.basis == basis,
                "File or HEAD changed; reload the diff and select lines again"
            );
            let selected: HashSet<_> = lines.into_iter().collect();
            let valid: HashSet<_> = view
                .hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .filter(|line| matches!(line.kind, "added" | "deleted"))
                .map(|line| line.id)
                .collect();
            anyhow::ensure!(
                selected.is_subset(&valid),
                "selection contains a context or unknown line"
            );
            let desired = select_content(&before.bytes, &view.hunks, &selected, false)?;
            let destination_exists = after.exists() || !desired.is_empty();
            let (mode, oid) = if destination_exists {
                let mode = before
                    .entry
                    .as_ref()
                    .or(after.entry.as_ref())
                    .context("missing file mode")?
                    .mode
                    .clone();
                (
                    mode,
                    self.hash_selection(&path, desired, view.selection_lfs)
                        .await?,
                )
            } else {
                let length = before
                    .entry
                    .as_ref()
                    .context("file is no longer tracked")?
                    .oid
                    .len();
                ("0".to_owned(), "0".repeat(length))
            };
            instructions.extend_from_slice(format!("{mode} {oid}\t").as_bytes());
            instructions.extend_from_slice(path.as_encoded_bytes());
            instructions.push(0);
            anyhow::ensure!(
                instructions.len() <= 1024 * 1024,
                "selected paths are too large"
            );
        }
        if !remove_bytes.is_empty() {
            let removed = process::run_git(
                &self.path,
                &args(&["update-index", "--force-remove", "-z", "--stdin"]),
                Some(remove_bytes),
                Some(&scratch.alternate_index),
            )
            .await?;
            anyhow::ensure!(
                removed.code == 0,
                "cannot snapshot removed working-tree files"
            );
        }
        if !path_bytes.is_empty() {
            let updated = process::run_git(
                &self.path,
                &args(&[
                    "update-index",
                    "--add",
                    "--remove",
                    "--replace",
                    "-z",
                    "--stdin",
                ]),
                Some(path_bytes),
                Some(&scratch.alternate_index),
            )
            .await?;
            anyhow::ensure!(
                updated.code == 0,
                "cannot snapshot selected working-tree files"
            );
        }
        if !instructions.is_empty() {
            let updated = process::run_git(
                &self.path,
                &args(&["update-index", "-z", "--index-info"]),
                Some(instructions),
                Some(&scratch.alternate_index),
            )
            .await?;
            anyhow::ensure!(updated.code == 0, "cannot snapshot selected lines");
        }
        let tree = if certify_live_index {
            let output = process::run_git(
                &self.path,
                &args(&["write-tree"]),
                None,
                Some(&scratch.alternate_index),
            )
            .await?;
            ensure_success(&output)?;
            let oid = String::from_utf8(output.stdout)?.trim().to_owned();
            anyhow::ensure!(
                matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid selected tree ID"
            );
            Some(oid)
        } else {
            None
        };
        let selected_index =
            read_index(&scratch.alternate_index)?.context("selected index is missing")?;
        let review = if certify_live_index {
            Some(
                self.review_staged_index(Some(&scratch.alternate_index), parent.as_deref())
                    .await?,
            )
        } else {
            None
        };
        let mut command = args(&[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
        ]);
        if let Some(parent) = parent {
            command.push(parent.into());
        }
        command.push("--".into());
        let patch = if certify_live_index {
            Vec::new()
        } else {
            let patch =
                process::run_git(&self.path, &command, None, Some(&scratch.alternate_index))
                    .await?;
            anyhow::ensure!(
                patch.code == 0,
                "cannot compare selected working-tree files"
            );
            patch.stdout
        };
        self.ensure_snapshot_head(&head, &branch).await?;
        if certify_live_index {
            anyhow::ensure!(
                read_index(&scratch.alternate_index)?.as_ref() == Some(&selected_index),
                "Selected index changed during review; retry"
            );
            anyhow::ensure!(
                read_index(&scratch.index_path)? == live_index,
                "Index changed while reviewing selected changes; retry"
            );
            let directory = git_directory.context("missing Git directory")?;
            let now = crate::cancellation::blocking(move || {
                crate::discard::operation_certificate(&directory)
            })
            .await?;
            anyhow::ensure!(
                now == operation,
                "Repository operation changed while reviewing selected changes; retry"
            );
        }
        Ok(SelectedWorktreeSnapshot {
            patch,
            index: selected_index,
            live_index,
            head: if head.code == 0 {
                head.stdout
            } else {
                Vec::new()
            },
            branch: if branch.code == 0 {
                branch.stdout
            } else {
                Vec::new()
            },
            operation,
            tree,
            review,
        })
    }

    pub(crate) async fn diff_detail(&self, path: GitPath, staged: bool) -> Result<Value> {
        let path = validate_path(path)?;
        let scratch = Scratch::new(&self.index_path().await?)?;
        let index_before = read_index(&scratch.index_path)?;
        let (view, _, _) = self
            .capture_diff(&path, staged, &index_before, &scratch, false, None)
            .await?;
        if read_index(&scratch.index_path)? != index_before {
            bail!("Index changed while loading diff; reload the file");
        }
        Ok(serde_json::to_value(view)?)
    }

    pub(crate) async fn select_diff(
        &self,
        path: GitPath,
        staged: bool,
        basis: &str,
        selected: Vec<usize>,
    ) -> Result<Value> {
        if selected.is_empty() {
            bail!("select at least one changed line");
        }
        let path = validate_path(path)?;
        let index_path = self.index_path().await?;
        let mut transaction = IndexTransaction::begin(&index_path)?;
        let original = read_index(&index_path)?;
        let scratch = Scratch::new(&index_path)?;
        let (view, before, after) = self
            .capture_diff(&path, staged, &original, &scratch, false, None)
            .await?;
        if !view.can_select {
            bail!("This diff does not support line selection; use whole-file staging");
        }
        if view.basis != basis {
            bail!("File or index changed; reload the diff and select lines again");
        }
        let selected: HashSet<_> = selected.into_iter().collect();
        let valid: HashSet<_> = view
            .hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter(|line| matches!(line.kind, "added" | "deleted"))
            .map(|line| line.id)
            .collect();
        if !selected.is_subset(&valid) {
            bail!("selection contains a context or unknown line");
        }
        let desired = select_content(&before.bytes, &view.hunks, &selected, staged)?;
        let destination_exists = if staged {
            before.exists() || !desired.is_empty()
        } else {
            after.exists() || !desired.is_empty()
        };
        let index_entry = self.index_entry(&path).await?;
        let (mode, oid) = if destination_exists {
            let mode = if staged {
                after.entry.as_ref().or(before.entry.as_ref())
            } else {
                before.entry.as_ref().or(after.entry.as_ref())
            }
            .context("missing file mode")?
            .mode
            .clone();
            (
                mode,
                self.hash_selection(&path, desired, view.selection_lfs)
                    .await?,
            )
        } else {
            (
                "0".to_owned(),
                "0".repeat(
                    index_entry
                        .as_ref()
                        .context("file is no longer indexed")?
                        .oid
                        .len(),
                ),
            )
        };
        // An alternate index preserves every unrelated entry, extension, and file mode.
        if let Some(original) = &original {
            scratch.write_index(original)?;
        }
        let mut instruction = format!("{mode} {oid}\t").into_bytes();
        instruction.extend(path.as_encoded_bytes());
        instruction.push(0);
        let updated = process::run_git(
            &self.path,
            &args(&["update-index", "-z", "--index-info"]),
            Some(instruction),
            Some(&scratch.alternate_index),
        )
        .await?;
        ensure_success(&updated)?;
        let replacement =
            read_index(&scratch.alternate_index)?.context("Git did not produce an index")?;
        transaction.commit(&replacement)?;
        Ok(json!({"code":0,"stdout":"","stderr":""}))
    }

    /// Apply a reviewed rename selection to the live index without losing
    /// unrelated entries or including a recreated, unselected source file.
    pub(crate) async fn select_renamed_diff(
        &self,
        path: GitPath,
        original_path: GitPath,
        basis: &str,
        selected: Vec<usize>,
        preserve_source: bool,
    ) -> Result<Value> {
        anyhow::ensure!(!selected.is_empty(), "select at least one changed line");
        let path = normalized_selection_path(path)?;
        let source = normalized_selection_path(original_path)?;
        anyhow::ensure!(path != source, "invalid rename source");
        let index_path = self.index_path().await?;
        let mut transaction = IndexTransaction::begin(&index_path)?;
        let original = read_index(&index_path)?;
        let head_scratch = Scratch::new(&index_path)?;
        let (head, branch) = self.snapshot_head_index(&head_scratch).await?;
        let head_index = read_index(&head_scratch.alternate_index)?;
        let (view, before, after) = self
            .capture_diff(
                &path,
                false,
                &head_index,
                &head_scratch,
                true,
                Some(&source),
            )
            .await?;
        anyhow::ensure!(
            view.can_select && view.basis == basis,
            "File or HEAD changed; reload the diff and select lines again"
        );
        let selected: HashSet<_> = selected.into_iter().collect();
        let valid: HashSet<_> = view
            .hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter(|line| matches!(line.kind, "added" | "deleted"))
            .map(|line| line.id)
            .collect();
        anyhow::ensure!(
            selected.is_subset(&valid),
            "selection contains a context or unknown line"
        );
        let desired = select_content(&before.bytes, &view.hunks, &selected, false)?;
        let exists = after.exists() || !desired.is_empty();
        let (mode, oid) = if exists {
            let mode = before
                .entry
                .as_ref()
                .or(after.entry.as_ref())
                .context("missing file mode")?
                .mode
                .clone();
            (
                mode,
                self.hash_selection(&path, desired, view.selection_lfs)
                    .await?,
            )
        } else {
            (
                "0".to_owned(),
                "0".repeat(
                    before
                        .entry
                        .as_ref()
                        .context("missing source entry")?
                        .oid
                        .len(),
                ),
            )
        };
        let scratch = Scratch::new(&index_path)?;
        if let Some(original) = &original {
            scratch.write_index(original)?;
        } else {
            let empty = process::run_git(
                &self.path,
                &args(&["read-tree", "--empty"]),
                None,
                Some(&scratch.alternate_index),
            )
            .await?;
            ensure_success(&empty)?;
        }
        if !preserve_source {
            let mut input = source.as_encoded_bytes().to_vec();
            input.push(0);
            let updated = process::run_git(
                &self.path,
                &args(&["update-index", "--force-remove", "-z", "--stdin"]),
                Some(input),
                Some(&scratch.alternate_index),
            )
            .await?;
            ensure_success(&updated)?;
        }
        let mut instruction = format!("{mode} {oid}\t").into_bytes();
        instruction.extend(path.as_encoded_bytes());
        instruction.push(0);
        let updated = process::run_git(
            &self.path,
            &args(&["update-index", "-z", "--index-info"]),
            Some(instruction),
            Some(&scratch.alternate_index),
        )
        .await?;
        ensure_success(&updated)?;
        let replacement =
            read_index(&scratch.alternate_index)?.context("Git did not produce an index")?;
        let (current, _, _) = self
            .capture_diff(
                &path,
                false,
                &head_index,
                &head_scratch,
                true,
                Some(&source),
            )
            .await?;
        anyhow::ensure!(
            current.basis == basis && read_index(&index_path)? == original,
            "File or index changed; reload the diff and select lines again"
        );
        self.ensure_snapshot_head(&head, &branch).await?;
        crate::cancellation::Cancellation::current().check()?;
        transaction.commit(&replacement)?;
        Ok(json!({"code":0,"stdout":"","stderr":""}))
    }

    /// Build the canonical blob after reversing a certified unstaged selection.
    pub(crate) async fn discarded_selection(
        &self,
        path: &std::ffi::OsStr,
        basis: &str,
        selected: &[usize],
    ) -> Result<Option<(String, String)>> {
        if selected.is_empty() {
            bail!("select at least one changed line");
        }
        let index_path = self.index_path().await?;
        let original = read_index(&index_path)?;
        let scratch = Scratch::new(&index_path)?;
        let (view, before, after) = self
            .capture_diff(path, false, &original, &scratch, false, None)
            .await?;
        if view.basis != basis {
            bail!("File, attributes, or index changed; reload the diff");
        }
        if !view.can_select {
            bail!("This diff does not support selected-line discard");
        }
        let selected: HashSet<_> = selected.iter().copied().collect();
        let valid: HashSet<_> = view
            .hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter(|line| matches!(line.kind, "added" | "deleted"))
            .map(|line| line.id)
            .collect();
        if !selected.is_subset(&valid) {
            bail!("selection contains a context or unknown line");
        }
        let desired = select_content(&before.bytes, &view.hunks, &selected, true)?;
        if !before.exists() && desired.is_empty() {
            return Ok(None);
        }
        let mode = after
            .entry
            .as_ref()
            .or(before.entry.as_ref())
            .context("missing file mode")?
            .mode
            .clone();
        let oid = self
            .hash_selection(path, desired, view.selection_lfs)
            .await?;
        Ok(Some((mode, oid)))
    }

    async fn hash_selection(&self, path: &OsStr, bytes: Vec<u8>, lfs: bool) -> Result<String> {
        let expected = crate::lfs::LfsObject {
            oid: format!("{:x}", Sha256::digest(&bytes)),
            size: bytes.len() as u64,
        };
        let mut command = args(&["hash-object", "-w", "--stdin"]);
        if lfs {
            command.push("--path".into());
            command.push(path.to_owned());
        }
        let output = process::run_git(&self.path, &command, Some(bytes), None).await?;
        ensure_success(&output)?;
        let oid = std::str::from_utf8(&output.stdout)?.trim().to_owned();
        if lfs {
            let blob = self.run(&args(&["cat-file", "blob", &oid])).await?;
            ensure_success(&blob)?;
            anyhow::ensure!(
                if expected.size == 0 {
                    blob.stdout.is_empty()
                } else {
                    crate::lfs::pointer(&blob.stdout).as_ref() == Some(&expected)
                        && !crate::lfs::has_extensions(&blob.stdout)
                },
                "Git LFS clean did not produce the selected content's pointer; verify LFS filters before retrying"
            );
            if expected.size != 0 {
                let payload = crate::lfs::preview_payload(
                    self.lfs_media_directory().await?,
                    expected.clone(),
                    expected.size,
                )
                .await?;
                anyhow::ensure!(
                    payload.state == "available",
                    "Git LFS did not save a verified payload; index was not changed"
                );
            }
        }
        Ok(oid)
    }

    async fn hydrate_sides(&self, sides: &mut [&mut Side], enabled: bool) -> Result<()> {
        if !enabled {
            return Ok(());
        }
        let mut media = None;
        for side in sides {
            if !side.regular() {
                continue;
            }
            let Some(object) = crate::lfs::pointer(&side.bytes) else {
                continue;
            };
            let payload = if crate::lfs::has_extensions(&side.bytes) {
                crate::lfs::PreviewPayload::unavailable("extension")
            } else if object.size == 0 {
                crate::lfs::PreviewPayload::unavailable(
                    if object.oid == format!("{:x}", Sha256::digest([])) {
                        "available"
                    } else {
                        "corrupt"
                    },
                )
            } else {
                if media.is_none() {
                    media = Some(match self.lfs_media_directory().await {
                        Ok(path) => Some(path),
                        Err(_) => {
                            crate::cancellation::Cancellation::current().check()?;
                            None
                        }
                    });
                }
                match media.as_ref().and_then(|path| path.clone()) {
                    Some(path) => {
                        crate::lfs::preview_payload(path, object.clone(), CONTENT_LIMIT as u64)
                            .await?
                    }
                    None => crate::lfs::PreviewPayload::unavailable("unavailable"),
                }
            };
            side.size = object.size;
            side.bytes = payload.bytes;
            side.oversized = payload.state == "too_large";
            side.lfs = Some(LfsPreview {
                object,
                state: payload.state,
            });
        }
        Ok(())
    }

    async fn capture_diff(
        &self,
        path: &OsStr,
        staged: bool,
        original_index: &Option<Vec<u8>>,
        scratch: &Scratch,
        head_index: bool,
        original_path: Option<&OsStr>,
    ) -> Result<(View, Side, Side)> {
        // ls-files excludes ignored metadata and traversals through symlink directories.
        let mut allowed = args(&[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
        ]);
        allowed.push(path.to_owned());
        let allowed = self.run(&allowed).await?;
        ensure_success(&allowed)?;
        if !allowed
            .stdout
            .split(|b| *b == 0)
            .any(|item| item == path.as_encoded_bytes())
            && !self.head_side(path).await?.exists()
        {
            bail!("path is not a tracked or untracked repository file");
        }
        let index_entry = self
            .index_entry_at(
                path,
                head_index.then_some(scratch.alternate_index.as_path()),
            )
            .await?;
        let mut before = if let Some(source) = original_path {
            anyhow::ensure!(
                head_index && !staged && source != path,
                "invalid rename source"
            );
            anyhow::ensure!(
                !self.head_side(path).await?.exists(),
                "rename destination already exists in HEAD"
            );
            let side = self.head_side(source).await?;
            anyhow::ensure!(side.exists(), "rename source is not tracked in HEAD");
            side
        } else if staged {
            let renamed = self
                .run(&args(&[
                    "diff",
                    "--cached",
                    "--name-status",
                    "--find-renames",
                    "-z",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--",
                ]))
                .await?;
            ensure_success(&renamed)?;
            let source = crate::history::staged_rename_source(&renamed.stdout, path)?;
            self.head_side(
                source
                    .map(validate_path)
                    .transpose()?
                    .as_deref()
                    .unwrap_or(path),
            )
            .await?
        } else {
            self.blob_side(
                if !head_index && self.intent_to_add(path, index_entry.is_some()).await? {
                    None
                } else {
                    index_entry.clone()
                },
            )
            .await?
        };
        let mut after = if staged {
            self.blob_side(index_entry).await?
        } else {
            self.worktree_side(path).await?
        };
        let raw_after = after.bytes.clone();
        let mut attributes = args(&[
            "check-attr",
            "-z",
            "filter",
            "working-tree-encoding",
            "eol",
            "text",
            "ident",
            "--",
        ]);
        attributes.push(path.to_owned());
        let attributes = self.run(&attributes).await?;
        ensure_success(&attributes)?;
        let filter = attributes
            .stdout
            .split(|b| *b == 0)
            .nth(2)
            .unwrap_or_default();
        let custom_filter = !matches!(filter, b"unspecified" | b"unset" | b"");
        let lfs_filter = filter == b"lfs";
        // Git blobs identify their content by pointer, even after attributes
        // change. Only interpret working-file pointers when LFS is selected.
        self.hydrate_sides(&mut [&mut before], true).await?;
        self.hydrate_sides(&mut [&mut after], staged || lfs_filter)
            .await?;
        let lfs_configuration = self
            .run(&args(&[
                "config",
                "--null",
                "--get-regexp",
                "^(filter\\.lfs\\.|lfs\\.extension\\.)",
            ]))
            .await?;
        if !matches!(lfs_configuration.code, 0 | 1) {
            ensure_success(&lfs_configuration)?;
        }
        let extensions_configured = lfs_configuration
            .stdout
            .split(|byte| *byte == 0)
            .any(|record| record.starts_with(b"lfs.extension."));

        if !staged && after.exists() && after.regular() && !after.oversized && !custom_filter {
            let mode = after
                .entry
                .as_ref()
                .context("missing worktree mode")?
                .mode
                .clone();
            let mut command = args(&["hash-object", "-w", "--stdin", "--path"]);
            command.push(path.to_owned());
            let hash =
                process::run_git(&self.path, &command, Some(after.bytes.clone()), None).await?;
            ensure_success(&hash)?;
            after = self
                .blob_side(Some(Entry {
                    mode,
                    oid: String::from_utf8(hash.stdout)?.trim().to_owned(),
                }))
                .await?;
        }
        let conversion = self
            .run(&args(&[
                "config",
                "--null",
                "--get-regexp",
                "^core\\.(autocrlf|eol|safecrlf|filemode|symlinks)$",
            ]))
            .await?;
        if !matches!(conversion.code, 0 | 1) {
            ensure_success(&conversion)?;
        }
        let identity = serde_json::to_vec(&(
            before.entry.as_ref().map(|entry| entry.oid.as_str()),
            after.entry.as_ref().map(|entry| entry.oid.as_str()),
            &before.lfs,
            &after.lfs,
        ))?;
        let mut digest = Sha256::new();
        for bytes in [
            identity.as_slice(),
            lfs_configuration.stdout.as_slice(),
            raw_after.as_slice(),
            attributes.stdout.as_slice(),
            conversion.stdout.as_slice(),
            path.as_encoded_bytes(),
            original_path.map_or(b"".as_slice(), OsStr::as_encoded_bytes),
            &[u8::from(staged)],
            original_index.as_deref().unwrap_or_default(),
            before.bytes.as_slice(),
            after.bytes.as_slice(),
            before
                .entry
                .as_ref()
                .map_or(b"".as_slice(), |entry| entry.mode.as_bytes()),
            after
                .entry
                .as_ref()
                .map_or(b"".as_slice(), |entry| entry.mode.as_bytes()),
        ] {
            digest.update((bytes.len() as u64).to_le_bytes());
            digest.update(bytes);
        }
        let mut view = View {
            selection_lfs: lfs_filter,
            path: GitPath::Bytes {
                bytes: path.as_encoded_bytes().to_vec(),
            },
            staged,
            kind: "text",
            basis: format!("{:x}", digest.finalize()),
            can_select: false,
            before: before.preview(),
            after: after.preview(),
            hunks: Vec::new(),
        };
        if before.lfs_unavailable() || after.lfs_unavailable() {
            view.kind = "lfs_unavailable";
        } else if before.oversized || after.oversized {
            view.kind = "too_large";
        } else if !before.regular() || !after.regular() {
            view.kind = if before
                .entry
                .as_ref()
                .is_some_and(|entry| entry.mode == "160000")
                || after
                    .entry
                    .as_ref()
                    .is_some_and(|entry| entry.mode == "160000")
            {
                "submodule"
            } else {
                "symlink"
            };
        } else if view.before.image.is_some() || view.after.image.is_some() {
            view.kind = "image";
        } else {
            let mut attr = args(&["check-attr", "-z", "diff", "--"]);
            attr.push(path.to_owned());
            let attr = self.run(&attr).await?;
            ensure_success(&attr)?;
            let binary_attribute = attr.stdout.split(|b| *b == 0).nth(2) == Some(b"unset");
            if binary_attribute || before.bytes.contains(&0) || after.bytes.contains(&0) {
                view.kind = "binary";
            } else {
                fs::write(&scratch.before, &before.bytes)?;
                fs::write(&scratch.after, &after.bytes)?;
                let diff = process::diff_files(&self.path, &scratch.before, &scratch.after).await?;
                if !matches!(diff.code, 0 | 1) {
                    ensure_success(&diff)?;
                }
                view.hunks = parse_hunks(&diff.stdout)?;
                view.can_select = !view.hunks.is_empty()
                    && (!custom_filter || lfs_filter && !extensions_configured)
                    && (lfs_filter || before.lfs.is_none() && after.lfs.is_none());
            }
        }
        Ok((view, before, after))
    }

    pub(crate) async fn commit_file_diff(
        &self,
        path: GitPath,
        revision: &str,
        parent: usize,
    ) -> Result<Value> {
        let path = validate_path(path)?;
        let commit = self.resolve_commit(revision).await?;
        let base = self.commit_parent(&commit, parent).await?;
        let files = self.commit_files(&commit, parent).await?;
        let original = files["files"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|file| {
                serde_json::from_value::<GitPath>(file["path"].clone())
                    .ok()
                    .and_then(|path| validate_path(path).ok())
                    .is_some_and(|candidate| candidate == path)
            })
            .and_then(|file| serde_json::from_value::<GitPath>(file["original_path"].clone()).ok())
            .map(validate_path)
            .transpose()?;
        let mut before = if let Some(base) = base {
            self.tree_side(&base, original.as_deref().unwrap_or(&path))
                .await?
        } else {
            self.blob_side(None).await?
        };
        let mut after = self.tree_side(&commit, &path).await?;
        self.hydrate_sides(&mut [&mut before, &mut after], true)
            .await?;
        if !before.exists() && !after.exists() {
            bail!("file is absent from both commit trees");
        }
        let scratch = Scratch::new(&self.index_path().await?)?;
        let mut view = View {
            selection_lfs: false,
            path: GitPath::Bytes {
                bytes: path.as_encoded_bytes().to_vec(),
            },
            staged: false,
            kind: "text",
            basis: commit,
            can_select: false,
            before: before.preview(),
            after: after.preview(),
            hunks: Vec::new(),
        };
        view.kind = if before.lfs_unavailable() || after.lfs_unavailable() {
            "lfs_unavailable"
        } else if before.oversized || after.oversized {
            "too_large"
        } else if before
            .entry
            .as_ref()
            .is_some_and(|entry| entry.mode == "160000")
            || after
                .entry
                .as_ref()
                .is_some_and(|entry| entry.mode == "160000")
        {
            "submodule"
        } else if !before.regular() || !after.regular() {
            "symlink"
        } else if view.before.image.is_some() || view.after.image.is_some() {
            "image"
        } else if before.bytes.contains(&0) || after.bytes.contains(&0) {
            "binary"
        } else {
            "text"
        };
        if view.kind == "text" {
            fs::write(&scratch.before, &before.bytes)?;
            fs::write(&scratch.after, &after.bytes)?;
            let diff = process::diff_files(&self.path, &scratch.before, &scratch.after).await?;
            if !matches!(diff.code, 0 | 1) {
                ensure_success(&diff)?;
            }
            view.hunks = parse_hunks(&diff.stdout)?;
        }
        Ok(serde_json::to_value(view)?)
    }

    pub(crate) async fn conflict_detail(&self, path: GitPath) -> Result<Value> {
        let path = crate::discard_fs::checked_path(path)?;
        let mut command = args(&["ls-files", "--unmerged", "-z", "--"]);
        command.push(path.clone());
        let output = self.run(&command).await?;
        ensure_success(&output)?;
        let mut digest = Sha256::new();
        digest.update(self.id());
        digest.update(path.as_encoded_bytes());
        digest.update(&output.stdout);
        let gitlink = output
            .stdout
            .split(|byte| *byte == 0)
            .any(|record| record.starts_with(b"160000 "));
        let working = if gitlink {
            None
        } else {
            Some(self.conflict_working_state(&path).await?)
        };
        let context = self.conflict_context(&path).await?;
        digest.update(serde_json::to_vec(&working)?);
        digest.update(&context);
        let mut stages = Vec::new();
        for record in output
            .stdout
            .split(|b| *b == 0)
            .filter(|record| !record.is_empty())
        {
            let (metadata, name) = split_record(record)?;
            if name != path.as_encoded_bytes() {
                continue;
            }
            let fields: Vec<_> = metadata.split_whitespace().collect();
            if fields.len() != 3 {
                bail!("invalid unmerged index entry");
            }
            let mut side = self
                .blob_side(Some(Entry {
                    mode: fields[0].into(),
                    oid: fields[1].into(),
                }))
                .await?;
            self.hydrate_sides(&mut [&mut side], true).await?;
            stages.push(json!({"stage":fields[2].parse::<u8>()?,"preview":side.preview(),"text":(!side.lfs_unavailable()&&!side.oversized&&!side.bytes.contains(&0)&&side.regular()).then(||String::from_utf8_lossy(&side.bytes).into_owned())}));
        }
        if stages.is_empty() {
            bail!("file is no longer conflicted");
        }
        Ok(
            json!({"path":GitPath::Bytes{bytes:path.as_encoded_bytes().to_vec()},"basis":format!("{:x}",digest.finalize()),"stages":stages,"working":working}),
        )
    }

    async fn conflict_working_state(&self, path: &OsStr) -> Result<crate::discard_fs::FileState> {
        let root = self.path.clone();
        let path = path.to_owned();
        crate::cancellation::blocking(move || {
            let root = cap_std::fs::Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
            crate::discard_fs::capture(&root, Path::new(&path), None)
        })
        .await
    }

    async fn conflict_context(&self, path: &OsStr) -> Result<Vec<u8>> {
        let head = self.run(&args(&["rev-parse", "--verify", "HEAD"])).await?;
        ensure_success(&head)?;
        let symbolic = self
            .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
            .await?;
        let directory = self.git_directory().await?;
        let operation = crate::cancellation::blocking(move || {
            crate::discard::operation_certificate(&directory)
        })
        .await?;
        let mut attributes = args(&["check-attr", "-z", "--all", "--"]);
        attributes.push(path.to_owned());
        let attributes = self.run(&attributes).await?;
        ensure_success(&attributes)?;
        let configuration = self.run(&args(&["config", "--null", "--get-regexp", "^(filter\\.|lfs\\.extension\\.|core\\.(autocrlf|eol|safecrlf|filemode|symlinks|checkroundtripencoding))"])).await?;
        if !matches!(configuration.code, 0 | 1) {
            ensure_success(&configuration)?;
        }
        Ok(serde_json::to_vec(&(
            head.stdout,
            symbolic.stdout,
            operation,
            attributes.stdout,
            configuration.stdout,
        ))?)
    }

    /// Stage the externally edited version through a private index, preserving
    /// unrelated staged entries and refusing stale file/operation certificates.
    pub(crate) async fn stage_conflict(
        &self,
        path: GitPath,
        basis: &str,
        allow_markers: bool,
    ) -> Result<Value> {
        let checked = crate::discard_fs::checked_path(path.clone())?;
        let index_path = self.index_path().await?;
        let transaction = IndexTransaction::begin(&index_path)?;
        let original = read_index(&index_path)?.context("missing conflicted index")?;
        let detail = self.conflict_detail(path.clone()).await?;
        anyhow::ensure!(
            detail["basis"].as_str() == Some(basis),
            "Conflict or working file changed; refresh and review the edited resolution"
        );
        anyhow::ensure!(
            detail["stages"].as_array().is_some_and(|stages| stages
                .iter()
                .all(|entry| entry["preview"]["mode"] != "160000")),
            "Use the submodule conflict workflow for gitlinks"
        );
        let scratch = Scratch::new(&index_path)?;
        scratch.write_index(&original)?;
        let mut add = args(&["add", "--"]);
        add.push(checked.clone());
        let output =
            process::run_git(&self.path, &add, None, Some(&scratch.alternate_index)).await?;
        ensure_success(&output)?;
        let mut check = args(&[
            "diff",
            "--cached",
            "--check",
            "--no-ext-diff",
            "--no-textconv",
            "--",
        ]);
        check.push(checked);
        let diagnostic =
            process::conflict_markers(&self.path, &check, Some(&scratch.alternate_index)).await?;
        anyhow::ensure!(
            matches!(diagnostic.code, 0 | 2),
            "cannot check edited resolution for conflict markers"
        );
        let markers = diagnostic
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| line.ends_with(b": leftover conflict marker"))
            .count();
        if markers > 0 && !allow_markers {
            return Ok(
                json!({"code":2,"stdout":"","stderr":"Conflict markers remain. Edit the file and refresh, or explicitly stage the reviewed contents with markers.","requires_confirmation":true,"markers":markers}),
            );
        }
        let replacement = read_index(&scratch.alternate_index)?
            .context("Git did not produce a resolved index")?;
        anyhow::ensure!(
            self.conflict_detail(path.clone()).await?["basis"].as_str() == Some(basis)
                && read_index(&index_path)?.as_ref() == Some(&original),
            "Conflict, working file or filters changed while staging; refresh and review again"
        );
        crate::cancellation::Cancellation::current().check()?;
        self.publish_edited_conflict_with_undo(path, basis, original, replacement, transaction)
            .await
    }

    pub(crate) async fn resolve_conflict(
        &self,
        path: GitPath,
        choice: crate::ConflictChoice,
        basis: &str,
    ) -> Result<Value> {
        self.resolve_conflict_with_undo(path, choice, basis).await
    }

    pub(crate) async fn index_path(&self) -> Result<PathBuf> {
        let output = self
            .run(&args(&[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "index",
            ]))
            .await?;
        ensure_success(&output)?;
        let bytes = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
        Ok(PathBuf::from(os_from_bytes(bytes)?))
    }

    async fn intent_to_add(&self, path: &OsStr, indexed: bool) -> Result<bool> {
        if !indexed {
            return Ok(false);
        }
        let mut command = args(&[
            "diff",
            "--cached",
            "--raw",
            "--no-abbrev",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--ita-invisible-in-index",
            "-z",
            "--",
        ]);
        command.push(path.to_owned());
        let output = self.run(&command).await?;
        ensure_success(&output)?;
        let absent = output
            .stdout
            .split(|byte| *byte == 0)
            .next()
            .and_then(|header| header.split(|byte| *byte == b' ').nth(1))
            == Some(b"000000".as_slice());
        Ok(absent || output.stdout.is_empty() && !self.head_side(path).await?.exists())
    }

    async fn index_entry(&self, path: &OsStr) -> Result<Option<Entry>> {
        self.index_entry_at(path, None).await
    }

    async fn index_entry_at(&self, path: &OsStr, index: Option<&Path>) -> Result<Option<Entry>> {
        let mut command = args(&["ls-files", "--stage", "-z", "--"]);
        command.push(path.to_owned());
        let output = process::run_git(&self.path, &command, None, index).await?;
        ensure_success(&output)?;
        let mut found = None;
        for record in output
            .stdout
            .split(|b| *b == 0)
            .filter(|record| !record.is_empty())
        {
            let (metadata, name) = split_record(record)?;
            if name != path.as_encoded_bytes() {
                continue;
            }
            let fields: Vec<_> = metadata.split_whitespace().collect();
            if fields.len() != 3 || fields[2] != "0" {
                bail!("Resolve conflicts before loading a selectable diff");
            }
            found = Some(Entry {
                mode: fields[0].to_owned(),
                oid: fields[1].to_owned(),
            });
        }
        Ok(found)
    }

    async fn head_side(&self, path: &OsStr) -> Result<Side> {
        let head = self.run(&args(&["rev-parse", "--verify", "HEAD"])).await?;
        if head.code != 0 {
            return self.blob_side(None).await;
        }
        self.tree_side("HEAD", path).await
    }

    async fn tree_side(&self, tree: &str, path: &OsStr) -> Result<Side> {
        let mut command = args(&["ls-tree", "-z", tree, "--"]);
        command.push(path.to_owned());
        let output = self.run(&command).await?;
        ensure_success(&output)?;
        let mut entry = None;
        for record in output
            .stdout
            .split(|b| *b == 0)
            .filter(|record| !record.is_empty())
        {
            let (metadata, name) = split_record(record)?;
            if name != path.as_encoded_bytes() {
                continue;
            }
            let fields: Vec<_> = metadata.split_whitespace().collect();
            if fields.len() != 3 {
                bail!("invalid tree entry");
            }
            entry = Some(Entry {
                mode: fields[0].to_owned(),
                oid: fields[2].to_owned(),
            });
        }
        self.blob_side(entry).await
    }

    async fn blob_side(&self, entry: Option<Entry>) -> Result<Side> {
        let mut side = Side {
            lfs: None,
            entry,
            bytes: Vec::new(),
            oversized: false,
            size: 0,
        };
        if let Some(entry) = &side.entry {
            if entry.mode == "160000" {
                return Ok(side);
            }
            let size = self.run(&args(&["cat-file", "-s", &entry.oid])).await?;
            ensure_success(&size)?;
            side.size = std::str::from_utf8(&size.stdout)?.trim().parse::<u64>()?;
            if side.size > CONTENT_LIMIT as u64 {
                side.oversized = true;
                return Ok(side);
            }
            let output = self.run(&args(&["cat-file", "blob", &entry.oid])).await?;
            ensure_success(&output)?;
            side.bytes = output.stdout;
        }
        Ok(side)
    }

    async fn worktree_side(&self, path: &OsStr) -> Result<Side> {
        let mut file_path = self.path.clone();
        let components: Vec<_> = Path::new(path).components().collect();
        for (i, component) in components.iter().enumerate() {
            file_path.push(component.as_os_str());
            if i + 1 < components.len()
                && fs::symlink_metadata(&file_path)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                bail!("path traverses a symlink directory");
            }
        }
        let metadata = match fs::symlink_metadata(&file_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Side {
                    lfs: None,
                    entry: None,
                    bytes: Vec::new(),
                    oversized: false,
                    size: 0,
                });
            }
            Err(error) => return Err(error.into()),
        };
        let mut mode = if metadata.file_type().is_symlink() {
            "120000"
        } else if metadata.is_dir() {
            let entry = self.index_entry(path).await?;
            if entry.as_ref().is_some_and(|entry| entry.mode == "160000") {
                let mut entry = entry;
                if file_path.join(".git").exists() {
                    let head =
                        process::run(&file_path, &args(&["rev-parse", "--verify", "HEAD"])).await?;
                    if head.code == 0 {
                        entry.as_mut().context("missing gitlink")?.oid =
                            String::from_utf8(head.stdout)?.trim().to_owned();
                    }
                }
                return Ok(Side {
                    lfs: None,
                    entry,
                    bytes: Vec::new(),
                    oversized: false,
                    size: 0,
                });
            }
            bail!("path is a directory");
        } else {
            file_mode(&metadata)
        };
        if matches!(mode, "100644" | "100755") {
            let policy = self
                .run(&args(&["config", "--type=bool", "core.filemode"]))
                .await?;
            if policy.stdout.starts_with(b"false") {
                mode = if self
                    .index_entry(path)
                    .await?
                    .is_some_and(|entry| entry.mode == "100755")
                {
                    "100755"
                } else {
                    "100644"
                };
            }
        }
        let oversized = metadata.len() > CONTENT_LIMIT as u64;
        let bytes = if oversized {
            Vec::new()
        } else if mode == "120000" {
            fs::read_link(&file_path)?
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        } else {
            read_bounded_file(&file_path, CONTENT_LIMIT as u64)?
        };
        let after = fs::symlink_metadata(&file_path)?;
        if metadata.len() != after.len() || metadata.modified().ok() != after.modified().ok() {
            bail!("File changed while reading; reload the diff");
        }
        Ok(Side {
            lfs: None,
            entry: Some(Entry {
                mode: mode.to_owned(),
                oid: String::new(),
            }),
            bytes,
            oversized,
            size: metadata.len(),
        })
    }
}

fn ensure_success(output: &process::RawOutput) -> Result<()> {
    if output.code != 0 {
        bail!("{}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(())
}
fn split_record(record: &[u8]) -> Result<(&str, &[u8])> {
    let split = record
        .iter()
        .position(|b| *b == b'\t')
        .context("invalid Git entry")?;
    Ok((std::str::from_utf8(&record[..split])?, &record[split + 1..]))
}
fn file_mode(metadata: &fs::Metadata) -> &'static str {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 != 0 {
            "100755"
        } else {
            "100644"
        }
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        "100644"
    }
}
fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else {
        None
    }
}
fn read_bounded_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("file exceeds size limit");
    }
    Ok(bytes)
}
pub(crate) fn read_index(path: &Path) -> Result<Option<Vec<u8>>> {
    match read_bounded_file(path, INDEX_LIMIT) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

struct Scratch {
    index_path: PathBuf,
    directory: PathBuf,
    alternate_index: PathBuf,
    before: PathBuf,
    after: PathBuf,
}
impl Scratch {
    fn new(index_path: &Path) -> Result<Self> {
        let parent = index_path.parent().context("index has no parent")?;
        let directory = parent.join(format!("desktop-diff-{}", uuid::Uuid::new_v4()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
        Ok(Self {
            index_path: index_path.to_owned(),
            alternate_index: directory.join("index"),
            before: directory.join("before"),
            after: directory.join("after"),
            directory,
        })
    }
    fn write_index(&self, bytes: &[u8]) -> Result<()> {
        fs::write(&self.alternate_index, bytes)?;
        Ok(())
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
pub(crate) struct IndexTransaction {
    path: PathBuf,
    lock_path: PathBuf,
    lock: File,
    committed: bool,
}
impl IndexTransaction {
    pub(crate) fn begin(path: &Path) -> Result<Self> {
        let mut lock_path = path.as_os_str().to_owned();
        lock_path.push(".lock");
        let lock_path = PathBuf::from(lock_path);
        let lock = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&lock_path)
            .context("Git index is locked; wait for the other Git operation to finish")?;
        let transaction = Self {
            path: path.to_owned(),
            lock_path,
            lock,
            committed: false,
        };
        if let Ok(metadata) = fs::metadata(path) {
            transaction.lock.set_permissions(metadata.permissions())?;
        }
        Ok(transaction)
    }
    pub(crate) fn commit(&mut self, bytes: &[u8]) -> Result<()> {
        self.lock.write_all(bytes)?;
        self.lock.sync_all()?;
        fs::rename(&self.lock_path, &self.path)?;
        self.committed = true;
        #[cfg(unix)]
        File::open(self.path.parent().context("index has no parent")?)?.sync_all()?;
        Ok(())
    }
}
impl Drop for IndexTransaction {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.lock_path);
        }
    }
}

fn parse_hunks(patch: &[u8]) -> Result<Vec<Hunk>> {
    let mut hunks = Vec::<Hunk>::new();
    let mut id = 0;
    let mut old_cursor = 0;
    let mut new_cursor = 0;
    for raw in patch.split_inclusive(|b| *b == b'\n') {
        if raw.starts_with(b"@@ ") {
            let header = std::str::from_utf8(raw)?;
            let mut parts = header.split_whitespace();
            parts.next();
            let (old_start, old_count) = parse_range(parts.next().context("missing old range")?)?;
            let (new_start, new_count) = parse_range(parts.next().context("missing new range")?)?;
            old_cursor = old_start;
            new_cursor = new_start;
            hunks.push(Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = hunks.last_mut() else {
            continue;
        };
        if raw.starts_with(b"\\ No newline at end of file") {
            let previous = hunk
                .lines
                .last_mut()
                .context("newline marker without a line")?;
            previous.no_newline = true;
            if previous.bytes.ends_with(b"\n") {
                previous.bytes.pop();
            }
            continue;
        }
        let kind = match raw.first() {
            Some(b' ') => "context",
            Some(b'+') => "added",
            Some(b'-') => "deleted",
            _ => bail!("invalid diff line"),
        };
        let old_line = (kind != "added").then_some(old_cursor);
        let new_line = (kind != "deleted").then_some(new_cursor);
        if kind != "added" {
            old_cursor += 1;
        }
        if kind != "deleted" {
            new_cursor += 1;
        }
        let bytes = raw[1..].to_vec();
        let text =
            String::from_utf8_lossy(bytes.strip_suffix(b"\n").unwrap_or(&bytes)).into_owned();
        hunk.lines.push(Line {
            id,
            kind,
            old_line,
            new_line,
            text,
            no_newline: false,
            bytes,
        });
        id += 1;
    }
    Ok(hunks)
}
fn parse_range(value: &str) -> Result<(usize, usize)> {
    let value = &value[1..];
    if let Some((start, count)) = value.split_once(',') {
        Ok((start.parse()?, count.parse()?))
    } else {
        Ok((value.parse()?, 1))
    }
}
fn select_content(
    old: &[u8],
    hunks: &[Hunk],
    selected: &HashSet<usize>,
    reverse: bool,
) -> Result<Vec<u8>> {
    let old_lines: Vec<_> = old.split_inclusive(|b| *b == b'\n').collect();
    let mut cursor = 0;
    let mut desired = Vec::new();
    for hunk in hunks {
        let start = if hunk.old_count == 0 {
            hunk.old_start
        } else {
            hunk.old_start.saturating_sub(1)
        };
        if start < cursor || start > old_lines.len() {
            bail!("invalid diff old range");
        }
        for line in &old_lines[cursor..start] {
            desired.extend(*line);
        }
        cursor = start;
        for line in &hunk.lines {
            let include = match line.kind {
                "context" => true,
                "deleted" => selected.contains(&line.id) == reverse,
                "added" => selected.contains(&line.id) != reverse,
                _ => false,
            };
            if line.kind != "added" {
                if old_lines.get(cursor) != Some(&line.bytes.as_slice()) {
                    bail!("diff no longer matches its captured base");
                }
                cursor += 1;
            }
            if include {
                desired.extend(&line.bytes);
            }
        }
    }
    for line in &old_lines[cursor..] {
        desired.extend(*line);
    }
    Ok(desired)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn staged_patch_snapshot_matches_git_and_preserves_index() {
        let root =
            std::env::temp_dir().join(format!("desktop-staged-diff-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let git = |arguments: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(arguments)
                .output()
                .unwrap()
        };
        assert!(git(&["init", "-q"]).status.success());
        fs::write(root.join("file"), "first\n").unwrap();
        assert!(git(&["add", "file"]).status.success());
        assert!(
            git(&[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "first"
            ])
            .status
            .success()
        );
        let first = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_owned();
        fs::write(root.join("file"), "second\n").unwrap();
        assert!(git(&["add", "file"]).status.success());
        assert!(
            git(&[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "second"
            ])
            .status
            .success()
        );
        let head = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_owned();
        fs::write(root.join("file"), "third\n").unwrap();
        assert!(git(&["add", "file"]).status.success());
        let index = fs::read(root.join(".git/index")).unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let owner = crate::comparison_snapshots::Owner::new();
        crate::comparison_snapshots::scope(owner.clone(), async {
            for amend in [None, Some(head.as_str())] {
                let result = repo.renderer_staged_diff(amend).await.unwrap();
                let id = uuid::Uuid::parse_str(result["id"].as_str().unwrap()).unwrap();
                let size = result["size"].as_u64().unwrap();
                let chunk =
                    crate::comparison_snapshots::chunk(&repo.id(), id, 0, size as u32).unwrap();
                let actual = STANDARD.decode(chunk["data"].as_str().unwrap()).unwrap();
                let mut args = vec![
                    "diff",
                    "--no-ext-diff",
                    "--patch-with-raw",
                    "--no-color",
                    "--staged",
                ];
                let base = format!("{head}^");
                if amend.is_some() {
                    args.push(&base);
                }
                assert_eq!(actual, git(&args).stdout);
                crate::comparison_snapshots::release(&repo.id(), id).unwrap();
            }
            assert!(repo.renderer_staged_diff(Some(&first)).await.is_err());
            assert!(repo.renderer_staged_diff(Some("--all")).await.is_err());
            assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
            fs::write(root.join("large"), "x\n".repeat(5 * 1024 * 1024 + 1)).unwrap();
            assert!(git(&["add", "large"]).status.success());
            assert!(
                repo.renderer_staged_diff(None)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("too large")
            );
        })
        .await;
        owner.close();
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn selected_line_staging_preserves_other_entries_and_rejects_stale_content() {
        let root =
            std::env::temp_dir().join(format!("desktop-stage-lines-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        git(&["init", "-q"]);
        fs::write(root.join("selected"), b"first\nsecond\n").unwrap();
        fs::write(root.join("other"), b"staged\n").unwrap();
        git(&["add", "other"]);
        let original_index = fs::read(root.join(".git/index")).unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let path = || GitPath::Text("selected".into());
        let preview = repo.diff_detail(path(), false).await.unwrap();
        let first = preview["hunks"][0]["lines"][0]["id"].as_u64().unwrap() as usize;
        fs::write(root.join("selected"), b"changed\n").unwrap();
        assert!(
            repo.select_diff(
                path(),
                false,
                preview["basis"].as_str().unwrap(),
                vec![first]
            )
            .await
            .is_err()
        );
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), original_index);
        fs::write(root.join("selected"), b"first\nsecond\n").unwrap();
        let fresh = repo.diff_detail(path(), false).await.unwrap();
        let first = fresh["hunks"][0]["lines"][0]["id"].as_u64().unwrap() as usize;
        assert_eq!(
            repo.select_diff(path(), false, fresh["basis"].as_str().unwrap(), vec![first])
                .await
                .unwrap()["code"],
            0
        );
        assert_eq!(git(&["show", ":selected"]), b"first\n");
        assert_eq!(git(&["show", ":other"]), b"staged\n");
        assert_eq!(fs::read(root.join("selected")).unwrap(), b"first\nsecond\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn selected_empty_commit_review_requires_explicit_permission() {
        let root =
            std::env::temp_dir().join(format!("desktop-empty-commit-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let run = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init", "-q"]);
        fs::write(root.join("staged"), b"preserve staged data\n").unwrap();
        run(&["add", "staged"]);
        let index = fs::read(root.join(".git/index")).unwrap();
        let repo = Repository::open(&root).await.unwrap();
        assert!(
            repo.selected_commit_preview_with_empty(vec![], vec![], vec![], false, None, None)
                .await
                .is_err()
        );
        let empty = repo
            .selected_commit_preview_with_empty(vec![], vec![], vec![], true, None, None)
            .await
            .unwrap();
        assert_eq!(empty["review"]["staged_count"], 0);
        assert_eq!(empty["review"]["files"], json!([]));
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        run(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "initial",
        ]);
        fs::write(root.join("staged"), b"new staged data\n").unwrap();
        run(&["add", "staged"]);
        let index = fs::read(root.join(".git/index")).unwrap();
        let selected = || vec![GitPath::Text("staged".into())];
        let with_empty = repo
            .selected_commit_preview_with_empty(selected(), vec![], vec![], true, None, None)
            .await
            .unwrap();
        let normal = repo
            .selected_commit_preview_with_empty(selected(), vec![], vec![], false, None, None)
            .await
            .unwrap();
        assert_ne!(normal["basis"], with_empty["basis"]);
        let empty = repo
            .selected_commit_preview_with_empty(vec![], vec![], vec![], true, None, None)
            .await
            .unwrap();
        assert_eq!(empty["review"]["staged_count"], 0);
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn selected_worktree_patch_handles_unborn_head_without_changing_index() {
        let root =
            std::env::temp_dir().join(format!("desktop-selected-diff-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        fs::write(
            root.join("selected"),
            b"selected content\nexcluded content\n",
        )
        .unwrap();
        fs::write(root.join("other"), b"unrelated staged content\n").unwrap();
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["add", "--", "other"])
                .status()
                .unwrap()
                .success()
        );
        let index = fs::read(root.join(".git/index")).unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let review = || {
            repo.selected_commit_preview(
                vec![GitPath::Text("selected".into())],
                vec![],
                vec![],
                None,
                None,
            )
        };
        let first_review = review().await.unwrap();
        let same_review = review().await.unwrap();
        assert_eq!(first_review["basis"], same_review["basis"]);
        assert_eq!(first_review["head"], Value::Null);
        assert_eq!(first_review["review"]["staged_count"], 1);
        assert_eq!(
            first_review["review"]["files"],
            json!([{ "bytes": b"selected".to_vec() }])
        );
        assert_eq!(first_review["review"]["large_files"], json!([]));
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        let patch = repo
            .selected_worktree_patch(
                vec![GitPath::Text("selected".into())],
                vec![],
                vec![],
                None,
                None,
            )
            .await
            .unwrap();
        let patch = String::from_utf8(patch).unwrap();
        assert!(patch.contains("+selected content"));
        assert!(!patch.contains("unrelated staged content"));
        let preview = repo
            .copilot_commit_diff_detail(GitPath::Text("selected".into()), None)
            .await
            .unwrap();
        let first_line = preview["hunks"][0]["lines"][0]["id"].as_u64().unwrap() as usize;
        let partial_selection = crate::CopilotLineSelection {
            path: GitPath::Text("selected".into()),
            original_path: None,
            basis: preview["basis"].as_str().unwrap().to_owned(),
            lines: vec![first_line],
        };
        let partial_review = repo
            .selected_commit_preview(vec![], vec![], vec![partial_selection.clone()], None, None)
            .await
            .unwrap();
        assert_ne!(first_review["basis"], partial_review["basis"]);
        assert_eq!(
            partial_review,
            repo.selected_commit_preview(
                vec![],
                vec![],
                vec![partial_selection.clone()],
                None,
                None
            )
            .await
            .unwrap()
        );
        assert!(
            repo.selected_commit_preview(
                vec![],
                vec![],
                vec![partial_selection.clone()],
                None,
                Some(&crate::CopilotExpectedHead {
                    reference: None,
                    sha: Some("a".repeat(40))
                })
            )
            .await
            .is_err()
        );
        let partial = repo
            .selected_worktree_patch(
                vec![],
                vec![],
                vec![crate::CopilotLineSelection {
                    path: GitPath::Text("selected".into()),
                    original_path: None,
                    basis: preview["basis"].as_str().unwrap().to_owned(),
                    lines: vec![first_line],
                }],
                None,
                None,
            )
            .await
            .unwrap();
        let partial = String::from_utf8(partial).unwrap();
        assert!(partial.contains("+selected content"));
        assert!(!partial.contains("excluded content"));
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        fs::write(root.join("selected"), b"changed after review\n").unwrap();
        assert!(
            repo.selected_commit_preview(vec![], vec![], vec![partial_selection], None, None)
                .await
                .is_err()
        );
        assert_ne!(first_review["basis"], review().await.unwrap()["basis"]);
        fs::write(
            root.join("selected"),
            b"selected content\nexcluded content\n",
        )
        .unwrap();
        fs::write(root.join("other"), b"new staged state\n").unwrap();
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["add", "--", "other"])
                .status()
                .unwrap()
                .success()
        );
        assert_ne!(first_review["basis"], review().await.unwrap()["basis"]);
        let after_index_change = review().await.unwrap();
        fs::write(root.join(".git/MERGE_MSG"), b"merge state changed\n").unwrap();
        assert_ne!(
            after_index_change["basis"],
            review().await.unwrap()["basis"]
        );
        assert!(
            repo.selected_commit_preview(vec![], vec![], vec![], None, None)
                .await
                .is_err()
        );
        assert!(
            repo.selected_commit_preview(
                vec![GitPath::Text("../outside".into())],
                vec![],
                vec![],
                None,
                None
            )
            .await
            .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn selected_rename_removes_old_path_even_if_it_was_recreated() {
        let root =
            std::env::temp_dir().join(format!("desktop-selected-rename-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        fs::write(root.join("old"), b"original\n").unwrap();
        git(&["add", "--", "old"]);
        git(&["commit", "-qm", "initial"]);
        git(&["mv", "--", "old", "new"]);
        fs::write(root.join("old"), b"replacement\n").unwrap();
        let index = fs::read(root.join(".git/index")).unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let output = |arguments: &[&str]| {
            let result = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(arguments)
                .output()
                .unwrap();
            assert!(result.status.success());
            String::from_utf8(result.stdout).unwrap().trim().to_owned()
        };
        let expected = crate::protocol::CopilotExpectedHead {
            reference: Some(output(&["symbolic-ref", "HEAD"])),
            sha: Some(output(&["rev-parse", "HEAD"])),
        };
        let other_branch = crate::protocol::CopilotExpectedHead {
            reference: Some("refs/heads/other".into()),
            sha: expected.sha.clone(),
        };
        assert!(
            repo.selected_worktree_patch(
                vec![GitPath::Text("new".into())],
                vec![GitPath::Text("old".into())],
                vec![],
                None,
                Some(&other_branch)
            )
            .await
            .is_err()
        );
        let patch = repo
            .selected_worktree_patch(
                vec![GitPath::Text("new".into())],
                vec![GitPath::Text("old".into())],
                vec![],
                None,
                Some(&expected),
            )
            .await
            .unwrap();
        let patch = String::from_utf8(patch).unwrap();
        assert!(patch.contains("rename from old"), "{patch}");
        assert!(patch.contains("rename to new"), "{patch}");
        assert!(!patch.contains("replacement"), "{patch}");
        let review = repo
            .selected_commit_preview(
                vec![GitPath::Text("new".into())],
                vec![GitPath::Text("old".into())],
                vec![],
                None,
                Some(&expected),
            )
            .await
            .unwrap();
        assert_eq!(review["review"]["staged_count"], 2);
        assert_eq!(
            review["review"]["files"],
            json!([{ "bytes": b"new".to_vec() }, { "bytes": b"old".to_vec() }])
        );
        let both = repo
            .selected_worktree_patch(
                vec![GitPath::Text("new".into()), GitPath::Text("old".into())],
                vec![GitPath::Text("old".into())],
                vec![],
                None,
                None,
            )
            .await
            .unwrap();
        assert!(String::from_utf8(both).unwrap().contains("+replacement"));
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn selected_amend_review_includes_original_commit_files() {
        let root =
            std::env::temp_dir().join(format!("desktop-selected-amend-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let git = |arguments: &[&str]| {
            let result = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(arguments)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            String::from_utf8(result.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.invalid"]);
        fs::write(root.join("existing"), b"base\n").unwrap();
        git(&["add", "--", "existing"]);
        git(&["commit", "-qm", "base"]);
        fs::write(root.join("existing"), b"original commit\n").unwrap();
        git(&["add", "--", "existing"]);
        git(&["commit", "-qm", "original"]);
        let head = git(&["rev-parse", "HEAD"]);
        fs::write(root.join("new"), b"selected\n").unwrap();
        let index = fs::read(root.join(".git/index")).unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let selected = || vec![GitPath::Text("new".into())];
        let ordinary = repo
            .selected_commit_preview(selected(), vec![], vec![], None, None)
            .await
            .unwrap();
        let amended = repo
            .selected_commit_preview(selected(), vec![], vec![], Some(&head), None)
            .await
            .unwrap();
        assert_eq!(
            ordinary["review"]["files"],
            json!([{ "bytes": b"new".to_vec() }])
        );
        assert_eq!(
            amended["review"]["files"],
            json!([{ "bytes": b"existing".to_vec() }, { "bytes": b"new".to_vec() }])
        );
        assert_ne!(ordinary["basis"], amended["basis"]);
        // A shallow boundary is not a root commit even though Git cannot
        // resolve its parent through the revision walker.
        fs::write(root.join(".git/shallow"), format!("{head}\n")).unwrap();
        assert!(
            repo.selected_commit_preview(vec![], vec![], vec![], Some(&head), None)
                .await
                .is_err()
        );
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn selected_root_amend_reviews_original_and_selected_files() {
        for format in ["sha1", "sha256"] {
            let root =
                std::env::temp_dir().join(format!("desktop-root-amend-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let init = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["init", "-q", &format!("--object-format={format}")])
                .output()
                .unwrap();
            if !init.status.success() && format == "sha256" {
                fs::remove_dir_all(root).unwrap();
                continue;
            }
            assert!(
                init.status.success(),
                "{}",
                String::from_utf8_lossy(&init.stderr)
            );
            let git = |arguments: &[&str]| {
                let result = std::process::Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(arguments)
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
                String::from_utf8(result.stdout).unwrap().trim().to_owned()
            };
            git(&["config", "user.name", "Test"]);
            git(&["config", "user.email", "test@example.invalid"]);
            fs::write(root.join("original"), b"first commit\n").unwrap();
            git(&["add", "--", "original"]);
            git(&["commit", "-qm", "initial"]);
            let head = git(&["rev-parse", "HEAD"]);
            fs::write(root.join("new"), b"selected\n").unwrap();
            fs::write(root.join("unrelated"), b"already staged\n").unwrap();
            git(&["add", "--", "unrelated"]);
            let index = fs::read(root.join(".git/index")).unwrap();
            let repo = Repository::open(&root).await.unwrap();
            let empty = repo
                .selected_commit_preview(vec![], vec![], vec![], Some(&head), None)
                .await
                .unwrap();
            assert_eq!(
                empty["review"]["files"],
                json!([{ "bytes": b"original".to_vec() }])
            );
            let selected = vec![GitPath::Text("new".into())];
            let reviewed = repo
                .selected_commit_preview(selected.clone(), vec![], vec![], Some(&head), None)
                .await
                .unwrap();
            assert_eq!(
                reviewed["review"]["files"],
                json!([{ "bytes": b"new".to_vec() }, { "bytes": b"original".to_vec() }])
            );
            let patch = repo
                .selected_worktree_patch(selected, vec![], vec![], Some(&head), None)
                .await
                .unwrap();
            let patch = String::from_utf8(patch).unwrap();
            assert!(patch.contains("+first commit\n") && patch.contains("+selected\n"));
            assert!(!patch.contains("already staged"));
            assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
            assert_eq!(git(&["rev-parse", "HEAD"]), head);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test]
    async fn partial_rename_uses_head_source_and_excludes_unselected_recreated_file() {
        let root =
            std::env::temp_dir().join(format!("desktop-partial-rename-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        let base = format!("old-first\n{}old-last\n", "context\n".repeat(10));
        fs::write(root.join("old"), &base).unwrap();
        fs::write(root.join("other"), &base).unwrap();
        git(&["add", "--all"]);
        git(&["commit", "-qm", "initial"]);
        git(&["mv", "--", "old", "new"]);
        fs::write(
            root.join("new"),
            format!("new-first\n{}new-last\n", "context\n".repeat(10)),
        )
        .unwrap();
        fs::write(root.join("old"), b"private replacement\n").unwrap();
        let index = fs::read(root.join(".git/index")).unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let destination = || GitPath::Text("new".into());
        let source = || GitPath::Text("old".into());
        let view = repo
            .copilot_commit_diff_detail(destination(), Some(source()))
            .await
            .unwrap();
        let ids: Vec<usize> = view["hunks"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|hunk| hunk["lines"].as_array().unwrap())
            .filter(|line| line["text"] == "old-first" || line["text"] == "new-first")
            .map(|line| line["id"].as_u64().unwrap() as usize)
            .collect();
        assert_eq!(ids.len(), 2);
        let alternate = repo
            .copilot_commit_diff_detail(destination(), Some(GitPath::Text("other".into())))
            .await
            .unwrap();
        assert_ne!(view["basis"], alternate["basis"]);
        let selection = || crate::CopilotLineSelection {
            path: destination(),
            original_path: Some(source()),
            basis: view["basis"].as_str().unwrap().into(),
            lines: ids.clone(),
        };
        let patch = repo
            .selected_worktree_patch(vec![], vec![], vec![selection()], None, None)
            .await
            .unwrap();
        let patch = String::from_utf8(patch).unwrap();
        assert!(patch.contains("+new-first"), "{patch}");
        assert!(!patch.contains("+new-last"), "{patch}");
        assert!(!patch.contains("private replacement"), "{patch}");
        let patch = repo
            .selected_worktree_patch(vec![source()], vec![], vec![selection()], None, None)
            .await
            .unwrap();
        assert!(
            String::from_utf8(patch)
                .unwrap()
                .contains("+private replacement")
        );
        fs::write(root.join("new"), b"changed since preview\n").unwrap();
        assert!(
            repo.selected_worktree_patch(vec![], vec![], vec![selection()], None, None)
                .await
                .is_err()
        );
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        fs::write(
            root.join("new"),
            format!("new-first\n{}new-last\n", "context\n".repeat(10)),
        )
        .unwrap();
        git(&["reset", "--quiet", "HEAD", "--", "."]);
        let before = fs::read(root.join(".git/index")).unwrap();
        let fresh = repo
            .copilot_commit_diff_detail(destination(), Some(source()))
            .await
            .unwrap();
        let lines: Vec<usize> = fresh["hunks"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|hunk| hunk["lines"].as_array().unwrap())
            .filter(|line| line["text"] == "old-first" || line["text"] == "new-first")
            .map(|line| line["id"].as_u64().unwrap() as usize)
            .collect();
        fs::write(root.join("new"), b"changed after review\n").unwrap();
        assert!(
            repo.select_renamed_diff(
                destination(),
                source(),
                fresh["basis"].as_str().unwrap(),
                lines.clone(),
                false
            )
            .await
            .is_err()
        );
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), before);
        fs::write(
            root.join("new"),
            format!("new-first\n{}new-last\n", "context\n".repeat(10)),
        )
        .unwrap();
        let fresh = repo
            .copilot_commit_diff_detail(destination(), Some(source()))
            .await
            .unwrap();
        assert_eq!(
            repo.select_renamed_diff(
                destination(),
                source(),
                fresh["basis"].as_str().unwrap(),
                lines.clone(),
                false
            )
            .await
            .unwrap()["code"],
            0
        );
        let show = |name: &str| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["show", name])
                .output()
                .unwrap()
                .stdout
        };
        let staged = String::from_utf8(show(":new")).unwrap();
        assert!(
            staged.contains("new-first")
                && staged.contains("old-last")
                && !staged.contains("new-last")
        );
        assert!(
            !std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["cat-file", "-e", ":old"])
                .output()
                .unwrap()
                .status
                .success()
        );
        assert_eq!(show(":other"), base.as_bytes());
        git(&["reset", "--quiet", "HEAD", "--", "."]);
        git(&["add", "--", "old"]);
        let fresh = repo
            .copilot_commit_diff_detail(destination(), Some(source()))
            .await
            .unwrap();
        assert_eq!(
            repo.select_renamed_diff(
                destination(),
                source(),
                fresh["basis"].as_str().unwrap(),
                lines,
                true
            )
            .await
            .unwrap()["code"],
            0
        );
        assert_eq!(show(":old"), b"private replacement\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preserves_final_newline_changes_in_both_directions() {
        let hunks = parse_hunks(
            b"--- a\n+++ b\n@@ -1,2 +1,2 @@\n a\n-old\n\\ No newline at end of file\n+new\n",
        )
        .unwrap();
        let selected = HashSet::from([1, 2]);
        assert!(hunks[0].lines[1].no_newline);
        assert_eq!(
            select_content(b"a\nold", &hunks, &selected, false).unwrap(),
            b"a\nnew\n"
        );
        assert_eq!(
            select_content(b"a\nold", &hunks, &selected, true).unwrap(),
            b"a\nold"
        );
    }

    #[test]
    fn handles_empty_base_and_zero_length_old_ranges() {
        let hunks =
            parse_hunks(b"@@ -0,0 +1,2 @@\n+first\n+last\n\\ No newline at end of file\n").unwrap();
        assert_eq!(
            select_content(b"", &hunks, &HashSet::from([1]), false).unwrap(),
            b"last"
        );
    }
}
