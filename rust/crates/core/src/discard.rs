//! Certified discards and conflict resolutions with durable file/index Undo.
use crate::{
    DiscardScope, DiscardSelection, GitPath, Repository, args,
    cancellation::{self, Cancellation},
    diff::{IndexTransaction, read_index},
    discard_fs::{self as work, FileState, Kind, WorkingFile},
    index_object_backup::SavedObject,
    shared_index_backup::SharedIndex,
};
use anyhow::{Context, Result, bail};
use cap_std::{ambient_authority, fs::Dir};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct IndexEntry {
    mode: String,
    oid: String,
    stage: u8,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Change {
    #[serde(default, skip_serializing_if = "is_false")]
    skip_working: bool,
    path: GitPath,
    before: FileState,
    after: FileState,
    before_index: Vec<IndexEntry>,
    after_index: Vec<IndexEntry>,
}
#[derive(Clone, Copy, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Purpose {
    #[default]
    Discard,
    ConflictChoice,
    ConflictStage,
}
impl Purpose {
    fn is_discard(&self) -> bool {
        *self == Self::Discard
    }
}
fn is_false(value: &bool) -> bool {
    !*value
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Record {
    #[serde(default, skip_serializing_if = "Purpose::is_discard")]
    purpose: Purpose,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    unavailable_lfs: Vec<crate::lfs::LfsObject>,
    version: u8,
    id: String,
    repository_id: String,
    created_ns: u128,
    state: String,
    head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    branch: Option<Vec<u8>>,
    scope: DiscardScope,
    changes: Vec<Change>,
    hash_length: usize,
    exact_before: Option<String>,
    exact_after: Option<String>,
    #[serde(default)]
    objects: Vec<SavedObject>,
    #[serde(default)]
    lfs: Vec<crate::lfs::LfsObject>,
    #[serde(default)]
    shared: Vec<SharedIndex>,
    #[serde(default)]
    operation: Option<String>,
}
struct Plan {
    record: Record,
    original_index: Option<Vec<u8>>,
    replacement_index: Option<Vec<u8>>,
    basis: String,
    space: Space,
}
struct Space {
    directory: PathBuf,
    retain: bool,
}
impl Space {
    fn new(base: &Path) -> Result<Self> {
        let directory = base.join(format!(".preparing-{}", uuid::Uuid::new_v4()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
        Ok(Self {
            directory,
            retain: false,
        })
    }
    fn file(&self, position: usize, side: &str) -> PathBuf {
        self.directory.join(format!("{position}.{side}"))
    }
}
impl Drop for Space {
    fn drop(&mut self) {
        if !self.retain {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
}

impl Repository {
    async fn discard_directory(&self) -> Result<PathBuf> {
        let base = self.git_directory().await?.join("desktop-undo");
        create_private_dir(&base)?;
        anyhow::ensure!(
            !fs::symlink_metadata(&base)?.file_type().is_symlink(),
            "undo directory cannot be a symlink"
        );
        Ok(base)
    }
    async fn head_oid(&self) -> Result<Option<String>> {
        let result = self
            .run(&args(&["rev-parse", "--verify", "--quiet", "HEAD"]))
            .await?;
        if result.code == 0 {
            Ok(Some(String::from_utf8(result.stdout)?.trim().into()))
        } else if matches!(result.code, 1 | 128) {
            Ok(None)
        } else {
            success(result).map(|_| None)
        }
    }
    async fn indexed(&self, path: &OsStr) -> Result<Vec<IndexEntry>> {
        let mut command = args(&["ls-files", "--stage", "-z", "--"]);
        command.push(path.to_owned());
        let result = success(self.run(&command).await?)?;
        let mut entries = Vec::new();
        for record in result
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
        {
            let (fields, name) = split(record)?;
            if name != path.as_encoded_bytes() {
                continue;
            }
            let fields: Vec<_> = fields.split_whitespace().collect();
            anyhow::ensure!(fields.len() == 3, "invalid index record");
            entries.push(IndexEntry {
                mode: fields[0].into(),
                oid: fields[1].into(),
                stage: fields[2].parse()?,
            });
        }
        Ok(entries)
    }
    async fn at_head(&self, head: Option<&str>, path: &OsStr) -> Result<Option<IndexEntry>> {
        let Some(head) = head else {
            return Ok(None);
        };
        let mut command = args(&["ls-tree", "-z", head, "--"]);
        command.push(path.to_owned());
        let result = success(self.run(&command).await?)?;
        for record in result
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
        {
            let (fields, name) = split(record)?;
            if name != path.as_encoded_bytes() {
                continue;
            }
            let fields: Vec<_> = fields.split_whitespace().collect();
            anyhow::ensure!(
                fields.len() == 3 && fields[1] != "tree",
                "cannot discard a tree as a file"
            );
            return Ok(Some(IndexEntry {
                mode: fields[0].into(),
                oid: fields[2].into(),
                stage: 0,
            }));
        }
        Ok(None)
    }

    async fn restore_bytes(
        &self,
        path: &OsStr,
        entry: Option<&IndexEntry>,
        destination: &Path,
        before: &FileState,
    ) -> Result<FileState> {
        let Some(entry) = entry else {
            return Ok(FileState::absent());
        };
        anyhow::ensure!(
            matches!(entry.mode.as_str(), "100644" | "100755" | "120000"),
            "submodules cannot be discarded as files"
        );
        let command = if entry.mode == "120000" {
            args(&["cat-file", "blob", &entry.oid])
        } else {
            let mut option = OsString::from("--path=");
            option.push(path);
            let mut command = args(&["cat-file", "--filters"]);
            command.push(option);
            command.push(entry.oid.clone().into());
            command
        };
        success(crate::process::git_to_file(&self.path, &command, destination).await?)?;
        let symlinks = self
            .run(&args(&["config", "--type=bool", "core.symlinks"]))
            .await?;
        let mut state = cancellation::blocking({
            let destination = destination.to_owned();
            move || work::copy_state(fs::File::open(destination)?, None)
        })
        .await?;
        if entry.mode == "120000" && !symlinks.stdout.starts_with(b"false") {
            anyhow::ensure!(state.size <= 65536, "symlink target exceeds size limit");
            state.kind = Kind::Symlink;
            state.permissions = 0o777;
        } else {
            let defaults =
                checkout_permissions(destination.parent().context("restore file has no parent")?)?;
            let executable = entry.mode == "100755";
            state.permissions = if before.kind == Kind::Regular {
                if executable == (before.permissions & 0o111 != 0) {
                    before.permissions
                } else {
                    (before.permissions & 0o666)
                        | if executable {
                            ((before.permissions & 0o444) >> 2) & defaults & 0o111
                        } else {
                            0
                        }
                }
            } else if executable {
                defaults
            } else {
                defaults & 0o666
            };
        }
        Ok(state)
    }

    async fn prepare_discard(
        &self,
        requested: Vec<GitPath>,
        scope: DiscardScope,
        selection: Option<DiscardSelection>,
    ) -> Result<Plan> {
        anyhow::ensure!(
            !requested.is_empty() && requested.len() <= 4096,
            "Select between 1 and 4096 files"
        );
        anyhow::ensure!(
            selection.is_none() || scope == DiscardScope::WorkingTree && requested.len() == 1,
            "selected lines require one working-tree file"
        );
        let snapshot = Box::pin(self.snapshot()).await?;
        let visible: std::collections::HashMap<_, _> = snapshot
            .files
            .iter()
            .filter_map(|change| {
                work::checked_path(change.path.clone())
                    .ok()
                    .map(|path| (path, change))
            })
            .collect();
        let mut paths = Vec::new();
        let mut selected = std::collections::HashSet::new();
        let mut affected = std::collections::HashSet::new();
        let mut path_bytes = 0usize;
        for path in requested {
            let path = work::checked_path(path)?;
            path_bytes = path_bytes.saturating_add(path.as_encoded_bytes().len());
            anyhow::ensure!(path_bytes <= 1024 * 1024, "Selected paths exceed 1 MiB");
            anyhow::ensure!(
                selected.insert(path.clone()),
                "Selected paths must not repeat"
            );
            let changed = visible
                .get(&path)
                .context("file has no visible changes to discard")?;
            if affected.insert(path.clone()) {
                paths.push(path);
            }
            if scope == DiscardScope::AllChanges
                && changed.index == 'R'
                && let Some(original) = &changed.original_path
            {
                let original = work::checked_path(original.clone())?;
                if affected.insert(original.clone()) {
                    path_bytes = path_bytes.saturating_add(original.as_encoded_bytes().len());
                    anyhow::ensure!(path_bytes <= 1024 * 1024, "Affected paths exceed 1 MiB");
                    paths.push(original);
                }
            }
        }
        anyhow::ensure!(paths.len() <= 8192, "Too many affected paths");
        let index_path = self.index_path().await?;
        let original_index = read_index(&index_path)?;
        let head = self.head_oid().await?;
        let git_directory = self.git_directory().await?;
        let operation = operation_certificate(&git_directory)?;
        let format = success(
            self.run(&args(&["rev-parse", "--show-object-format"]))
                .await?,
        )?;
        let hash_length = if format.stdout.starts_with(b"sha256") {
            64
        } else {
            40
        };
        let space = Space::new(&self.discard_directory().await?)?;
        let mut changes = Vec::new();
        let mut exact_index = false;
        for (position, path) in paths.iter().enumerate() {
            Cancellation::current().check()?;
            let before = cancellation::blocking({
                let root = self.path.clone();
                let path = path.clone();
                let backup = space.file(position, "before");
                move || {
                    work::capture(
                        &Dir::open_ambient_dir(root, ambient_authority())?,
                        Path::new(&path),
                        Some(&backup),
                    )
                }
            })
            .await?;
            let before_index = self.indexed(path).await?;
            exact_index |= self
                .special_index(path, &before_index, head.as_deref())
                .await?;
            anyhow::ensure!(
                !before_index.iter().any(|entry| entry.mode == "160000"),
                "submodules require their own recovery workflow"
            );
            let desired = if let Some(selection) = &selection {
                Box::pin(self.discarded_selection(path, &selection.basis, &selection.lines))
                    .await?
                    .map(|(mode, oid)| IndexEntry {
                        mode,
                        oid,
                        stage: 0,
                    })
            } else if scope == DiscardScope::AllChanges {
                self.at_head(head.as_deref(), path).await?
            } else {
                anyhow::ensure!(
                    before_index.iter().all(|entry| entry.stage == 0),
                    "resolve the conflict or explicitly discard all changes from HEAD"
                );
                before_index.first().cloned()
            };
            let after_index = if scope == DiscardScope::AllChanges {
                desired.iter().cloned().collect()
            } else {
                before_index.clone()
            };
            let after = Box::pin(self.restore_bytes(
                path,
                desired.as_ref(),
                &space.file(position, "after"),
                &before,
            ))
            .await?;
            let current = cancellation::blocking({
                let root = self.path.clone();
                let path = path.clone();
                move || {
                    work::capture(
                        &Dir::open_ambient_dir(root, ambient_authority())?,
                        Path::new(&path),
                        None,
                    )
                }
            })
            .await?;
            anyhow::ensure!(
                before == current,
                "file changed while preparing discard; reload before retrying"
            );
            changes.push(Change {
                skip_working: false,
                path: GitPath::Bytes {
                    bytes: path.as_encoded_bytes().to_vec(),
                },
                before,
                after,
                before_index,
                after_index,
            });
        }
        anyhow::ensure!(
            read_index(&index_path)? == original_index && self.head_oid().await? == head,
            "HEAD or index changed while preparing discard"
        );
        anyhow::ensure!(
            changes
                .iter()
                .any(|change| change.before != change.after
                    || change.before_index != change.after_index),
            "no changes to discard in the selected scope"
        );
        let mut digest = Sha256::new();
        digest.update(serde_json::to_vec(&(
            self.id(),
            &head,
            scope,
            &changes,
            &operation,
        ))?);
        digest.update(original_index.as_deref().unwrap_or_default());
        let basis = format!("{:x}", digest.finalize());
        let replacement_index = self
            .replacement_index(
                &space.directory,
                &original_index,
                &changes,
                false,
                hash_length,
            )
            .await?;
        let (objects, lfs, unavailable_lfs) = self
            .save_index_objects(
                &space.directory,
                changes
                    .iter()
                    .flat_map(|change| &change.before_index)
                    .map(|entry| (entry.mode.clone(), entry.oid.clone()))
                    .collect(),
                false,
            )
            .await?;
        debug_assert!(unavailable_lfs.is_empty());
        let shared = self
            .save_shared_indexes(
                &space.directory,
                &original_index,
                replacement_index.as_ref(),
            )
            .await?;
        let (exact_before, exact_after) = if exact_index {
            let before = original_index
                .as_deref()
                .context("special entry without an index")?;
            let after = replacement_index.as_deref().unwrap_or(before);
            work::private_write(&space.directory.join("index.before"), before)?;
            work::private_write(&space.directory.join("index.after"), after)?;
            (Some(checksum(before)), Some(checksum(after)))
        } else {
            (None, None)
        };
        let plan = Plan {
            record: Record {
                purpose: Purpose::Discard,
                unavailable_lfs: Vec::new(),
                version: 2,
                id: uuid::Uuid::new_v4().to_string(),
                repository_id: self.id(),
                created_ns: SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
                state: "prepared".into(),
                head,
                branch: None,
                scope,
                changes,
                hash_length,
                exact_before,
                exact_after,
                objects,
                lfs,
                shared,
                operation: Some(operation),
            },
            original_index,
            replacement_index,
            basis,
            space,
        };
        anyhow::ensure!(
            serde_json::to_vec_pretty(&plan.record)?.len() <= MAX_MANIFEST_BYTES,
            "Recovery manifest exceeds 1 MiB; select fewer files"
        );
        Ok(plan)
    }

    pub(crate) async fn common_directory(&self) -> Result<PathBuf> {
        let result = success(
            self.run(&args(&[
                "rev-parse",
                "--path-format=absolute",
                "--git-common-dir",
            ]))
            .await?,
        )?;
        Ok(PathBuf::from(crate::os_from_bytes(
            result.stdout.strip_suffix(b"\n").unwrap_or(&result.stdout),
        )?)
        .canonicalize()?)
    }

    async fn restore_saved_data(&self, directory: &Path, record: &Record) -> Result<()> {
        self.restore_shared_indexes(directory, &record.shared)
            .await?;
        self.restore_index_objects(
            directory,
            &record.objects,
            &record.lfs,
            &record.unavailable_lfs,
            record.hash_length,
            record.version >= 2,
        )
        .await
    }

    async fn special_index(
        &self,
        path: &OsStr,
        entries: &[IndexEntry],
        head: Option<&str>,
    ) -> Result<bool> {
        if entries.is_empty() {
            return Ok(false);
        }
        let mut command = args(&["ls-files", "-v", "-z", "--"]);
        command.push(path.to_owned());
        let flags = success(self.run(&command).await?)?;
        if flags
            .stdout
            .split(|byte| *byte == 0)
            .filter_map(|record| record.first())
            .any(|tag| tag.is_ascii_lowercase() || *tag == b'S')
        {
            return Ok(true);
        }
        if entries.len() != 1 || entries[0].stage != 0 {
            return Ok(false);
        }
        let mut command = args(&[
            "diff",
            "--cached",
            "--raw",
            "--no-abbrev",
            "--no-renames",
            "--ita-invisible-in-index",
            "-z",
            "--",
        ]);
        command.push(path.to_owned());
        let effective = success(self.run(&command).await?)?;
        let absent_in_effective_index = effective
            .stdout
            .split(|byte| *byte == 0)
            .next()
            .and_then(|header| header.split(|byte| *byte == b' ').nth(1))
            == Some(b"000000".as_slice());
        Ok(absent_in_effective_index
            || effective.stdout.is_empty() && self.at_head(head, path).await?.is_none())
    }

    async fn lock_discard_refs(&self) -> Result<Vec<RefLock>> {
        let format = self.run(&args(&["rev-parse", "--show-ref-format"])).await?;
        anyhow::ensure!(
            !format.stdout.starts_with(b"reftable"),
            "discard ref locking for reftable repositories is not implemented yet"
        );
        let head = self.head_oid().await?;
        let symbolic = self
            .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
            .await?;
        let directory = self.git_directory().await?;
        let mut locks = vec![RefLock::new(directory.join("HEAD"))?];
        if symbolic.code == 0 {
            let name = symbolic
                .stdout
                .strip_suffix(b"\n")
                .unwrap_or(&symbolic.stdout);
            let reference = crate::os_from_bytes(name)?;
            anyhow::ensure!(
                Path::new(&reference)
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_)))
                    && name.starts_with(b"refs/"),
                "invalid symbolic HEAD"
            );
            let common = success(
                self.run(&args(&[
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-common-dir",
                ]))
                .await?,
            )?;
            let common =
                crate::os_from_bytes(common.stdout.strip_suffix(b"\n").unwrap_or(&common.stdout))?;
            locks.push(RefLock::new(PathBuf::from(common).join(reference))?);
        }
        let current = self
            .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
            .await?;
        anyhow::ensure!(
            current.stdout == symbolic.stdout
                && current.code == symbolic.code
                && self.head_oid().await? == head,
            "HEAD changed while acquiring discard locks"
        );
        Ok(locks)
    }

    async fn replacement_index(
        &self,
        directory: &Path,
        original: &Option<Vec<u8>>,
        changes: &[Change],
        undo: bool,
        hash_length: usize,
    ) -> Result<Option<Vec<u8>>> {
        if !changes
            .iter()
            .any(|change| change.before_index != change.after_index)
        {
            return Ok(None);
        }
        let alternate = directory.join(format!("index-{}", uuid::Uuid::new_v4()));
        let _cleanup = AlternateIndex(alternate.clone());
        if let Some(bytes) = original {
            work::private_write(&alternate, bytes)?;
        } else {
            success(
                crate::process::run_git(
                    &self.path,
                    &args(&["read-tree", "--empty"]),
                    None,
                    Some(&alternate),
                )
                .await?,
            )?;
        }
        let mut instructions = Vec::new();
        for change in changes {
            let path = work::checked_path(change.path.clone())?;
            let entries = if undo {
                &change.before_index
            } else {
                &change.after_index
            };
            instructions.extend(format!("0 {}\t", "0".repeat(hash_length)).as_bytes());
            instructions.extend(path.as_encoded_bytes());
            instructions.push(0);
            for entry in entries {
                validate_entry(entry, hash_length)?;
                instructions
                    .extend(format!("{} {} {}\t", entry.mode, entry.oid, entry.stage).as_bytes());
                instructions.extend(path.as_encoded_bytes());
                instructions.push(0);
            }
        }
        success(
            crate::process::run_git(
                &self.path,
                &args(&["update-index", "-z", "--index-info"]),
                Some(instructions),
                Some(&alternate),
            )
            .await?,
        )?;
        read_index(&alternate)
    }

    async fn resolution_branch(&self) -> Result<Option<Vec<u8>>> {
        let output = self
            .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
            .await?;
        anyhow::ensure!(
            matches!(output.code, 0 | 1),
            "cannot inspect conflict branch"
        );
        Ok((output.code == 0).then(|| {
            output
                .stdout
                .strip_suffix(b"\n")
                .unwrap_or(&output.stdout)
                .to_vec()
        }))
    }

    async fn lock_resolution_refs(&self) -> Result<Vec<RefLock>> {
        let format = success(self.run(&args(&["rev-parse", "--show-ref-format"])).await?)?;
        if format.stdout.starts_with(b"reftable") {
            // The index lock excludes ordinary checkout/merge writers. Reftable
            // refs are inspected again before publication; direct ref writers
            // are outside this working-tree transaction's reservation.
            Ok(Vec::new())
        } else {
            self.lock_discard_refs().await
        }
    }

    async fn conflict_journal_plan(
        &self,
        path: GitPath,
        basis: &str,
        original_index: Vec<u8>,
        choice: Option<crate::ConflictChoice>,
        staged_index: Option<Vec<u8>>,
    ) -> Result<Plan> {
        let checked = work::checked_path(path.clone())?;
        let detail = self.conflict_detail(path.clone()).await?;
        anyhow::ensure!(
            detail["basis"].as_str() == Some(basis),
            "Conflict changed; refresh before resolving"
        );
        let head = self.head_oid().await?;
        let git_directory = self.git_directory().await?;
        let operation = operation_certificate(&git_directory)?;
        let before_index = self.indexed(&checked).await?;
        anyhow::ensure!(
            !before_index.is_empty() && before_index.iter().all(|entry| entry.stage != 0),
            "file is no longer unmerged"
        );
        let gitlink = before_index.iter().any(|entry| entry.mode == "160000");
        anyhow::ensure!(
            !gitlink || before_index.iter().all(|entry| entry.mode == "160000"),
            "mixed file/submodule conflicts require their dedicated workflow"
        );
        let branch = self.resolution_branch().await?;
        let purpose = if choice.is_some() {
            Purpose::ConflictChoice
        } else {
            Purpose::ConflictStage
        };
        let space = Space::new(&self.discard_directory().await?)?;
        let before = if gitlink {
            FileState::absent()
        } else {
            cancellation::blocking({
                let root = self.path.clone();
                let path = checked.clone();
                let backup = space.file(0, "before");
                move || {
                    work::capture(
                        &Dir::open_ambient_dir(root, ambient_authority())?,
                        Path::new(&path),
                        Some(&backup),
                    )
                }
            })
            .await?
        };
        let (after, after_index) = if let Some(choice) = choice {
            let stage = match choice {
                crate::ConflictChoice::Ours => Some(2),
                crate::ConflictChoice::Theirs => Some(3),
                crate::ConflictChoice::Delete => None,
            };
            let mut desired = before_index
                .iter()
                .find(|entry| Some(entry.stage) == stage)
                .cloned();
            if let Some(entry) = &mut desired {
                entry.stage = 0;
            }
            let after = if gitlink {
                FileState::absent()
            } else {
                self.restore_bytes(&checked, desired.as_ref(), &space.file(0, "after"), &before)
                    .await?
            };
            if let Some(entry) = &mut desired
                && matches!(entry.mode.as_str(), "100644" | "100755")
            {
                // Apply clean/encoding conversion exactly where the real path
                // selects attributes, without publishing the checkout first.
                let mut command = args(&["hash-object", "-w", "--path"]);
                command.extend([
                    checked.clone(),
                    "--".into(),
                    space.file(0, "after").into_os_string(),
                ]);
                entry.oid = String::from_utf8(success(self.run(&command).await?)?.stdout)?
                    .trim()
                    .to_owned();
            }
            (after, desired.into_iter().collect())
        } else {
            let replacement = staged_index.as_ref().context("missing edited index")?;
            let alternate = space.directory.join("resolved-index");
            work::private_write(&alternate, replacement)?;
            let mut command = args(&["ls-files", "--stage", "-z", "--"]);
            command.push(checked.clone());
            let output = success(
                crate::process::run_git(&self.path, &command, None, Some(&alternate)).await?,
            )?;
            let mut entries = Vec::new();
            for record in output
                .stdout
                .split(|byte| *byte == 0)
                .filter(|record| !record.is_empty())
            {
                let (fields, name) = split(record)?;
                anyhow::ensure!(
                    name == checked.as_encoded_bytes(),
                    "unexpected resolved path"
                );
                let fields: Vec<_> = fields.split_whitespace().collect();
                anyhow::ensure!(
                    fields.len() == 3 && fields[2] == "0",
                    "edited index is still conflicted"
                );
                entries.push(IndexEntry {
                    mode: fields[0].into(),
                    oid: fields[1].into(),
                    stage: 0,
                });
            }
            (before.clone(), entries)
        };
        let hash_length = before_index[0].oid.len();
        let change = Change {
            skip_working: gitlink || purpose == Purpose::ConflictStage,
            path,
            before,
            after,
            before_index,
            after_index,
        };
        let changes = vec![change];
        let replacement_index = if let Some(index) = staged_index {
            Some(index)
        } else {
            self.replacement_index(
                &space.directory,
                &Some(original_index.clone()),
                &changes,
                false,
                hash_length,
            )
            .await?
        };
        let (objects, lfs, unavailable_lfs) = self
            .save_index_objects(
                &space.directory,
                changes
                    .iter()
                    .flat_map(|change| &change.before_index)
                    .map(|entry| (entry.mode.clone(), entry.oid.clone()))
                    .collect(),
                true,
            )
            .await?;
        let shared = self
            .save_shared_indexes(
                &space.directory,
                &Some(original_index.clone()),
                replacement_index.as_ref(),
            )
            .await?;
        let exact = self
            .special_index(&checked, &changes[0].before_index, head.as_deref())
            .await?;
        let (exact_before, exact_after) = if exact {
            let after = replacement_index
                .as_ref()
                .context("missing resolved index")?;
            work::private_write(&space.directory.join("index.before"), &original_index)?;
            work::private_write(&space.directory.join("index.after"), after)?;
            (Some(checksum(&original_index)), Some(checksum(after)))
        } else {
            (None, None)
        };
        anyhow::ensure!(
            self.conflict_detail(changes[0].path.clone()).await?["basis"].as_str() == Some(basis)
                && read_index(&self.index_path().await?)?.as_ref() == Some(&original_index)
                && self.head_oid().await? == head
                && operation_certificate(&git_directory)? == operation,
            "Conflict, working file, index or filters changed while preparing undo"
        );
        Ok(Plan {
            record: Record {
                version: 3,
                purpose,
                unavailable_lfs,
                id: uuid::Uuid::new_v4().to_string(),
                repository_id: self.id(),
                created_ns: SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
                state: "prepared".into(),
                head,
                branch,
                scope: DiscardScope::AllChanges,
                changes,
                hash_length,
                exact_before,
                exact_after,
                objects,
                lfs,
                shared,
                operation: Some(operation),
            },
            original_index: Some(original_index),
            replacement_index,
            basis: basis.into(),
            space,
        })
    }

    pub(crate) async fn resolve_conflict_with_undo(
        &self,
        path: GitPath,
        choice: crate::ConflictChoice,
        basis: &str,
    ) -> Result<Value> {
        let index_path = self.index_path().await?;
        let transaction = IndexTransaction::begin(&index_path)?;
        let references = self.lock_resolution_refs().await?;
        let original = read_index(&index_path)?.context("missing conflicted index")?;
        let plan = self
            .conflict_journal_plan(path, basis, original, Some(choice), None)
            .await?;
        self.publish_plan(plan, transaction, references, index_path)
            .await
    }

    pub(crate) async fn publish_edited_conflict_with_undo(
        &self,
        path: GitPath,
        basis: &str,
        original: Vec<u8>,
        replacement: Vec<u8>,
        transaction: IndexTransaction,
    ) -> Result<Value> {
        let index_path = self.index_path().await?;
        let references = self.lock_resolution_refs().await?;
        let plan = self
            .conflict_journal_plan(path, basis, original, None, Some(replacement))
            .await?;
        self.publish_plan(plan, transaction, references, index_path)
            .await
    }

    pub(crate) async fn discard_preview(
        &self,
        path: GitPath,
        scope: DiscardScope,
        selection: Option<DiscardSelection>,
    ) -> Result<Value> {
        let plan = Box::pin(self.prepare_discard(vec![path], scope, selection)).await?;
        Ok(
            json!({"basis":plan.basis,"scope":scope,"files":display_changes(&plan.record.changes),"index_changes":plan.replacement_index.is_some(),"undo_requires_unchanged_index":plan.record.exact_before.is_some(),"backup_bytes":plan.record.changes.iter().map(|change|change.before.size).sum::<u64>()+plan.record.objects.iter().map(|object|object.size).sum::<u64>()+plan.record.shared.iter().map(|index|index.size).sum::<u64>()+plan.record.lfs.iter().map(|object|object.size).sum::<u64>()}),
        )
    }

    pub(crate) async fn discard(
        &self,
        path: GitPath,
        scope: DiscardScope,
        selection: Option<DiscardSelection>,
        basis: &str,
    ) -> Result<Value> {
        let index_path = self.index_path().await?;
        let transaction = IndexTransaction::begin(&index_path)?;
        let references = self.lock_discard_refs().await?;
        let plan = Box::pin(self.prepare_discard(vec![path], scope, selection)).await?;
        anyhow::ensure!(
            plan.basis == basis,
            "File, index, HEAD, or restore filters changed; review discard again"
        );
        Box::pin(self.publish_plan(plan, transaction, references, index_path)).await
    }

    pub(crate) async fn discard_batch_preview(&self, paths: Vec<GitPath>) -> Result<Value> {
        let plan = self
            .prepare_discard(paths, DiscardScope::AllChanges, None)
            .await?;
        Ok(
            json!({"basis":plan.basis,"files":display_changes(&plan.record.changes),"index_changes":plan.replacement_index.is_some(),"backup_bytes":plan.record.changes.iter().map(|change| change.before.size).sum::<u64>() + plan.record.objects.iter().map(|object| object.size).sum::<u64>() + plan.record.shared.iter().map(|index| index.size).sum::<u64>() + plan.record.lfs.iter().map(|object| object.size).sum::<u64>()}),
        )
    }

    pub(crate) async fn discard_batch(&self, paths: Vec<GitPath>, basis: &str) -> Result<Value> {
        let index_path = self.index_path().await?;
        let transaction = IndexTransaction::begin(&index_path)?;
        let references = self.lock_discard_refs().await?;
        let plan = self
            .prepare_discard(paths, DiscardScope::AllChanges, None)
            .await?;
        anyhow::ensure!(
            plan.basis == basis,
            "Files, index, HEAD, or restore filters changed; review discard again"
        );
        self.publish_plan(plan, transaction, references, index_path)
            .await
    }

    async fn publish_plan(
        &self,
        mut plan: Plan,
        transaction: IndexTransaction,
        references: Vec<RefLock>,
        index_path: PathBuf,
    ) -> Result<Value> {
        let root = self.path.clone();
        let git_directory = self.git_directory().await?;
        anyhow::ensure!(
            plan.record.version < 3 || self.resolution_branch().await? == plan.record.branch,
            "Branch changed before file publication"
        );
        anyhow::ensure!(
            self.head_oid().await? == plan.record.head,
            "HEAD changed before file publication"
        );
        cancellation::blocking(move || {
            let _references = references;
            let root = Dir::open_ambient_dir(root, ambient_authority())?;
            let mut staged = Vec::new();
            for (position, change) in plan.record.changes.iter().enumerate().filter(|(_,change)| !change.skip_working && change.before != change.after) {
                let path = work::checked_path(change.path.clone())?;
                let file = WorkingFile::open(&root, Path::new(&path), true)?.context("missing parent directory")?;
                staged.push(file.stage(&plan.space.file(position, "after"), &change.after)?);
            }
            Cancellation::current().check()?;
            anyhow::ensure!(read_index(&index_path)? == plan.original_index, "Index changed before discard");
            for change in plan.record.changes.iter().filter(|change| !change.skip_working || plan.record.purpose == Purpose::ConflictStage) {
                let path = work::checked_path(change.path.clone())?;
                anyhow::ensure!(work::capture(&root, Path::new(&path), None)? == change.before, "File changed before discard; review again");
            }
            anyhow::ensure!(plan.record.operation.as_deref() == Some(operation_certificate(&git_directory)?.as_str()), "Repository operation changed; review discard again");
            let destination = plan.space.directory.parent().context("undo directory has no parent")?.join(&plan.record.id);
            write_record(&plan.space.directory, &plan.record)?;
            fs::rename(&plan.space.directory, &destination)?;
            plan.space.directory = destination; plan.space.retain = true;
            sync_directory(plan.space.directory.parent().context("missing undo parent")?)?;
            // No cancellation points after the first publication. The durable prepared
            // record can recover any prefix if an OS operation or the process fails.
            let mut transaction = transaction;
            let result = (|| -> Result<()> {
                for file in &mut staged { file.publish()?; }
                if let Some(index) = &plan.replacement_index { transaction.commit(index)?; }
                plan.record.state = "applied".into(); write_record(&plan.space.directory, &plan.record)?;
                Ok(())
            })();
            result.with_context(|| format!("File operation interrupted; recover with discard-undo {}", plan.record.id))?;
            Ok(json!({"code":0,"stdout":"","stderr":"","undo_id":plan.record.id,"purpose":plan.record.purpose,"lfs_payloads_unavailable":plan.record.unavailable_lfs.len(),"files":display_changes(&plan.record.changes)}))
        }).await
    }

    pub(crate) async fn list_discards(&self) -> Result<Value> {
        let base = self.discard_directory().await?;
        cancellation::blocking(move || {
            let mut records = Vec::new();
            for entry in fs::read_dir(base)? {
                Cancellation::current().check()?;
                let entry = entry?; let id = entry.file_name().to_string_lossy().into_owned();
                if validate_id(&id).is_err() { continue; }
                anyhow::ensure!(records.len() < 1000, "undo history exceeds 1000 entries; inspect the Git directory");
                match read_record(&entry.path(), &id) {
                    Ok(record) => records.push(json!({"id":id,"created_ns":record.created_ns,"state":record.state,"scope":record.scope,"purpose":record.purpose,"lfs_payloads_unavailable":record.unavailable_lfs.len(),"files":display_changes(&record.changes)})),
                    Err(error) => records.push(json!({"id":id,"state":"invalid","error":error.to_string(),"created_ns":0})),
                }
            }
            records.sort_by_key(|record| std::cmp::Reverse(record["created_ns"].as_u64().unwrap_or_default()));
            Ok(json!(records))
        }).await
    }

    pub(crate) async fn forget_discard(&self, id: &str) -> Result<Value> {
        validate_id(id)?;
        let index_path = self.index_path().await?;
        let transaction = IndexTransaction::begin(&index_path)?;
        let directory = self.discard_directory().await?.join(id);
        let record = read_record(&directory, id)?;
        anyhow::ensure!(
            record.repository_id == self.id()
                && matches!(record.state.as_str(), "applied" | "undone"),
            "recover an interrupted discard before deleting its backup"
        );
        cancellation::blocking(move || {
            let _transaction = transaction;
            fs::remove_dir_all(&directory)?;
            sync_directory(
                directory
                    .parent()
                    .context("missing undo directory parent")?,
            )?;
            Ok(json!({"code":0,"stdout":"","stderr":""}))
        })
        .await
    }

    pub(crate) async fn undo_discard(&self, id: &str) -> Result<Value> {
        validate_id(id)?;
        let directory = self.discard_directory().await?.join(id);
        let record = read_record(&directory, id)?;
        anyhow::ensure!(
            record.repository_id == self.id()
                && matches!(record.state.as_str(), "applied" | "prepared"),
            "discard is not available for this working tree"
        );
        anyhow::ensure!(
            self.head_oid().await? == record.head,
            "HEAD changed since discard; undo data was retained"
        );
        anyhow::ensure!(
            record.version < 3 || self.resolution_branch().await? == record.branch,
            "Branch changed since resolution; undo data was retained"
        );
        let index_path = self.index_path().await?;
        let transaction = IndexTransaction::begin(&index_path)?;
        let references = if record.purpose == Purpose::Discard {
            self.lock_discard_refs().await?
        } else {
            self.lock_resolution_refs().await?
        };
        anyhow::ensure!(
            self.head_oid().await? == record.head,
            "HEAD changed before undo"
        );
        let original_index = read_index(&index_path)?;
        let git_directory = self.git_directory().await?;
        if let Some(expected) = &record.operation {
            anyhow::ensure!(
                *expected == operation_certificate(&git_directory)?,
                "Repository operation changed since discard; backup retained"
            );
        }
        self.restore_saved_data(&directory, &record).await?;
        let mut need_index = false;
        for change in &record.changes {
            let path = work::checked_path(change.path.clone())?;
            let indexed = self.indexed(&path).await?;
            anyhow::ensure!(
                indexed == change.after_index
                    || record.state == "prepared" && indexed == change.before_index,
                "Index entries changed since discard; undo data was retained"
            );
            need_index |= indexed != change.before_index;
        }
        let replacement = if let (Some(before_hash), Some(after_hash)) =
            (&record.exact_before, &record.exact_after)
        {
            let before =
                read_index(&directory.join("index.before"))?.context("missing saved index")?;
            let after =
                read_index(&directory.join("index.after"))?.context("missing saved index")?;
            anyhow::ensure!(
                checksum(&before) == *before_hash && checksum(&after) == *after_hash,
                "saved index was changed; undo data retained"
            );
            anyhow::ensure!(
                original_index.as_deref() == Some(after.as_slice())
                    || record.state == "prepared"
                        && original_index.as_deref() == Some(before.as_slice()),
                "Index changed since discard; this entry requires the original index state"
            );
            (original_index.as_deref() != Some(before.as_slice())).then_some(before)
        } else if need_index {
            self.replacement_index(
                &directory,
                &original_index,
                &record.changes,
                true,
                record.hash_length,
            )
            .await?
        } else {
            None
        };
        // Object recovery and rebuilding the index can run filters or take time.
        // Recheck the repository context immediately before publication as well.
        anyhow::ensure!(
            self.head_oid().await? == record.head,
            "HEAD changed before undo; backup retained"
        );
        anyhow::ensure!(
            record.version < 3 || self.resolution_branch().await? == record.branch,
            "Branch changed before undo; backup retained"
        );
        let root = self.path.clone();
        cancellation::blocking(move || {
            let _references = references;
            let mut record = record;
            let root = Dir::open_ambient_dir(root, ambient_authority())?;
            let mut staged = Vec::new();
            for (position, change) in record
                .changes
                .iter()
                .enumerate()
                .filter(|(_, change)| !change.skip_working)
            {
                let path = work::checked_path(change.path.clone())?;
                let file = WorkingFile::open(&root, Path::new(&path), true)?
                    .context("missing parent directory")?;
                let current = file.capture(None)?;
                anyhow::ensure!(
                    current == change.after
                        || record.state == "prepared" && current == change.before,
                    "Working file changed since discard; undo data was retained"
                );
                if change.before != change.after {
                    staged.push(file.stage(
                        &directory.join(format!("{position}.before")),
                        &change.before,
                    )?);
                }
            }
            Cancellation::current().check()?;
            anyhow::ensure!(
                read_index(&index_path)? == original_index,
                "Index changed before undo"
            );
            for change in record.changes.iter().filter(|change| !change.skip_working) {
                let path = work::checked_path(change.path.clone())?;
                let current = work::capture(&root, Path::new(&path), None)?;
                anyhow::ensure!(
                    current == change.after
                        || record.state == "prepared" && current == change.before,
                    "Working file changed before undo; backup retained"
                );
            }
            if let Some(expected) = &record.operation {
                anyhow::ensure!(
                    *expected == operation_certificate(&git_directory)?,
                    "Repository operation changed before undo; backup retained"
                );
            }
            let mut transaction = transaction;
            // A prepared record also describes partially completed undo operations.
            record.state = "prepared".into();
            write_record(&directory, &record)?;
            for file in &mut staged {
                file.publish()?;
            }
            if let Some(index) = replacement {
                transaction.commit(&index)?;
            }
            record.state = "undone".into();
            write_record(&directory, &record)?;
            Ok(json!({"code":0,"stdout":"","stderr":"","undo_id":record.id}))
        })
        .await
    }
}

fn validate_entry(entry: &IndexEntry, length: usize) -> Result<()> {
    anyhow::ensure!(
        matches!(
            entry.mode.as_str(),
            "100644" | "100755" | "120000" | "160000"
        ) && entry.stage <= 3
            && entry.oid.len() == length
            && entry.oid.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid undo index entry"
    );
    Ok(())
}
fn validate_id(id: &str) -> Result<()> {
    anyhow::ensure!(
        uuid::Uuid::parse_str(id)?.to_string() == id,
        "invalid discard ID"
    );
    Ok(())
}
fn split(record: &[u8]) -> Result<(&str, &[u8])> {
    let tab = record
        .iter()
        .position(|byte| *byte == b'\t')
        .context("invalid Git entry")?;
    Ok((std::str::from_utf8(&record[..tab])?, &record[tab + 1..]))
}
fn success(output: crate::process::RawOutput) -> Result<crate::process::RawOutput> {
    if output.code != 0 {
        bail!("{}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(output)
}
fn display_changes(changes: &[Change]) -> Vec<Value> {
    changes.iter().map(|change| json!({"path":change.path,"display_path":match &change.path {GitPath::Text(value)=>value.clone(),GitPath::Bytes{bytes}=>String::from_utf8_lossy(bytes).into_owned()},"before":change.before,"after":change.after,"index_only":change.skip_working})).collect()
}
fn create_private_dir(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        fs::symlink_metadata(path)?.is_dir()
            && !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "undo path must be a directory"
    );
    Ok(())
}
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn write_record(directory: &Path, record: &Record) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(record)?;
    anyhow::ensure!(
        bytes.len() <= MAX_MANIFEST_BYTES,
        "Recovery manifest exceeds 1 MiB; select fewer files"
    );
    let temporary = directory.join(format!("manifest-{}.tmp", uuid::Uuid::new_v4()));
    work::private_write(&temporary, &bytes)?;
    fs::rename(&temporary, directory.join("manifest.json"))?;
    sync_directory(directory)
}
fn read_record(directory: &Path, id: &str) -> Result<Record> {
    anyhow::ensure!(
        !fs::symlink_metadata(directory)?.file_type().is_symlink(),
        "undo record directory cannot be a symlink"
    );
    let mut bytes = Vec::new();
    fs::File::open(directory.join("manifest.json"))?
        .take(MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= MAX_MANIFEST_BYTES,
        "undo manifest exceeds size limit"
    );
    let record: Record = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        matches!(record.version, 1..=3)
            && record.id == id
            && matches!(record.hash_length, 40 | 64)
            && !record.changes.is_empty()
            && record.changes.len()
                <= if record.purpose == Purpose::Discard {
                    8192
                } else {
                    2
                },
        "unsupported undo record"
    );
    anyhow::ensure!(
        record.lfs.len() + record.unavailable_lfs.len() <= record.objects.len(),
        "invalid LFS undo record"
    );
    anyhow::ensure!(
        record.version == 3 && record.purpose != Purpose::Discard
            || record.version < 3
                && record.purpose == Purpose::Discard
                && record.unavailable_lfs.is_empty()
                && record.changes.iter().all(|change| !change.skip_working),
        "unsupported legacy undo features"
    );
    for object in record.lfs.iter().chain(&record.unavailable_lfs) {
        object.validate()?;
    }
    let mut paths = std::collections::HashSet::new();
    for change in &record.changes {
        anyhow::ensure!(
            paths.insert(work::checked_path(change.path.clone())?),
            "duplicate path in undo record"
        );
        anyhow::ensure!(
            !change.skip_working
                || record.purpose == Purpose::ConflictStage && change.before == change.after
                || record.purpose == Purpose::ConflictChoice
                    && change.before.kind == Kind::Absent
                    && change.after.kind == Kind::Absent,
            "invalid index-only undo change"
        );
        for entry in change.before_index.iter().chain(&change.after_index) {
            anyhow::ensure!(
                entry.mode != "160000"
                    || change.skip_working && record.purpose == Purpose::ConflictChoice,
                "gitlink undo must leave the nested checkout intact"
            );
            validate_entry(entry, record.hash_length)?;
        }
    }
    Ok(record)
}

struct RefLock {
    path: PathBuf,
    _file: fs::File,
}
impl RefLock {
    fn new(path: PathBuf) -> Result<Self> {
        let mut lock = path.as_os_str().to_owned();
        lock.push(".lock");
        let path = PathBuf::from(lock);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file =
            work::private_file(&path).context("HEAD or branch is locked by another operation")?;
        Ok(Self { path, _file: file })
    }
}
impl Drop for RefLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod batch_journal_tests {
    use super::*;

    fn record() -> Record {
        let absent = FileState::absent();
        Record {
            purpose: Purpose::Discard,
            unavailable_lfs: vec![],
            version: 2,
            id: uuid::Uuid::new_v4().to_string(),
            repository_id: "test".into(),
            created_ns: 0,
            state: "prepared".into(),
            head: None,
            branch: None,
            scope: DiscardScope::AllChanges,
            changes: vec![Change {
                skip_working: false,
                path: GitPath::Text("file".into()),
                before: absent.clone(),
                after: absent,
                before_index: vec![],
                after_index: vec![],
            }],
            hash_length: 40,
            exact_before: None,
            exact_after: None,
            objects: vec![],
            lfs: vec![],
            shared: vec![],
            operation: None,
        }
    }

    #[test]
    fn refuses_oversized_or_duplicate_recovery_records() {
        let dir =
            std::env::temp_dir().join(format!("desktop-batch-record-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let mut oversized = record();
        oversized.operation = Some("x".repeat(MAX_MANIFEST_BYTES));
        assert!(write_record(&dir, &oversized).is_err());
        assert!(!dir.join("manifest.json").exists());
        let mut duplicate = record();
        duplicate.changes.push(duplicate.changes[0].clone());
        write_record(&dir, &duplicate).unwrap();
        assert!(read_record(&dir, &duplicate.id).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}

struct AlternateIndex(PathBuf);
impl Drop for AlternateIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let mut lock = self.0.as_os_str().to_owned();
        lock.push(".lock");
        let _ = fs::remove_file(PathBuf::from(lock));
    }
}

fn checkout_permissions(directory: &Path) -> Result<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let path = directory.join(format!("permission-{}", uuid::Uuid::new_v4()));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o777)
            .open(&path)?;
        let mode = file.metadata()?.permissions().mode() & 0o777;
        drop(file);
        fs::remove_file(path)?;
        Ok(mode)
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        Ok(0o644)
    }
}

#[cfg(test)]
mod future_stack_tests {
    use super::*;

    #[tokio::test]
    async fn discard_preparation_future_stays_bounded() {
        let root =
            std::env::temp_dir().join(format!("desktop-discard-stack-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        assert!(
            std::process::Command::new("git")
                .arg("init")
                .arg("-q")
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        let repository = Repository::open(&root).await.unwrap();
        let future = repository.prepare_discard(
            vec![GitPath::Text("file".into())],
            DiscardScope::WorkingTree,
            None,
        );
        let size = std::mem::size_of_val(&future);
        assert!(
            size <= 64 * 1024,
            "discard preparation future grew to {size} bytes"
        );
        drop(future);
        fs::remove_dir_all(root).unwrap();
    }
}

pub(crate) fn operation_certificate(directory: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    for name in [
        "MERGE_HEAD",
        "MERGE_MSG",
        "MERGE_MODE",
        "AUTO_MERGE",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "REBASE_HEAD",
        "sequencer/head",
        "sequencer/abort-safety",
        "sequencer/todo",
        "rebase-merge/head-name",
        "rebase-merge/onto",
        "rebase-merge/orig-head",
        "rebase-merge/msgnum",
        "rebase-merge/end",
        "rebase-merge/done",
        "rebase-merge/git-rebase-todo",
        "rebase-apply/head-name",
        "rebase-apply/onto",
        "rebase-apply/orig-head",
        "rebase-apply/next",
        "rebase-apply/last",
        "rebase-apply/applying",
    ] {
        digest.update(name.as_bytes());
        match fs::File::open(directory.join(name)) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
                anyhow::ensure!(
                    bytes.len() <= 1024 * 1024,
                    "repository operation state exceeds supported size"
                );
                digest.update([1]);
                digest.update((bytes.len() as u64).to_le_bytes());
                digest.update(bytes);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                digest.update([0]);
            }
            Err(error) => return Err(error.into()),
        }
    }
    for name in ["sequencer", "rebase-merge", "rebase-apply"] {
        digest.update([u8::from(directory.join(name).exists())]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
