//! Durable, pre-publication snapshots for selected-commit recovery.
use crate::{
    CopilotExpectedHead, CopilotLineSelection, GitPath, Repository, args,
    cancellation::{self, Cancellation},
    diff::{IndexTransaction, read_index},
    discard_fs as work,
    index_object_backup::SavedObject,
    lfs::LfsObject,
    process,
    shared_index_backup::{SharedIndex, read_shared_index, validate_name as validate_shared_name},
};
use anyhow::{Context, Result};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const DIRECTORY: &str = "desktop-commit-checkpoints";
const MANIFEST_LIMIT: u64 = 1024 * 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SelectedRequest {
    paths: Vec<GitPath>,
    remove_paths: Vec<GitPath>,
    partial: Vec<CopilotLineSelection>,
    allow_empty: bool,
    amend_head: Option<String>,
    expected_head: Option<CopilotExpectedHead>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    version: u8,
    id: String,
    repository_id: String,
    state: String,
    basis: String,
    head: Option<String>,
    branch: Option<String>,
    operation: String,
    original_index: Option<String>,
    selected_index: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parents: Option<Vec<String>>,
    objects: Vec<SavedObject>,
    lfs: Vec<LfsObject>,
    shared: Vec<SharedIndex>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request: Option<SelectedRequest>,
}
struct CheckpointUse {
    path: PathBuf,
    file: Option<fs::File>,
}
impl Drop for CheckpointUse {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}
struct Preparing {
    path: PathBuf,
    retain: bool,
}
impl Drop for Preparing {
    fn drop(&mut self) {
        if !self.retain {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id)
}
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn private_directory(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    let created = match builder.create(path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(error.into()),
    };
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "commit checkpoint directory must be a real directory"
    );
    if created {
        sync_directory(
            path.parent()
                .context("checkpoint directory has no parent")?,
        )?;
    }
    Ok(())
}
fn read_checkpoint(base: &Path, id: &str, repository_id: &str) -> Result<Checkpoint> {
    anyhow::ensure!(valid_id(id), "invalid commit checkpoint ID");
    let base = Dir::open_ambient_dir(base, ambient_authority())?;
    let directory = base.open_dir_nofollow(id)?;
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = directory.open_with("manifest.json", &options)?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "invalid commit checkpoint manifest"
    );
    let mut bytes = Vec::new();
    file.take(MANIFEST_LIMIT + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MANIFEST_LIMIT,
        "commit checkpoint manifest too large"
    );
    let record: Checkpoint = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        record.version == 1
            && record.id == id
            && record.repository_id == repository_id
            && record.state == "prepared"
            && record.basis.len() == 64
            && record.basis.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid commit checkpoint manifest"
    );
    match (&record.tree, &record.parents) {
        (None, None) => {} // Checkpoints prepared before publication metadata was recorded.
        (Some(tree), Some(parents)) => {
            let length = tree.len();
            anyhow::ensure!(
                matches!(length, 40 | 64)
                    && tree.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && parents.len() <= 1024
                    && parents.iter().all(|parent| parent.len() == length
                        && parent.bytes().all(|byte| byte.is_ascii_hexdigit())),
                "invalid commit checkpoint publication metadata"
            );
        }
        _ => anyhow::bail!("incomplete commit checkpoint publication metadata"),
    }
    Ok(record)
}
fn index_entries(bytes: &[u8]) -> Result<Vec<(String, String)>> {
    anyhow::ensure!(
        bytes.is_empty() || bytes.ends_with(b"\0"),
        "incomplete index listing"
    );
    let mut entries = Vec::new();
    for row in bytes.split(|byte| *byte == 0).filter(|row| !row.is_empty()) {
        let separator = row
            .iter()
            .position(|byte| *byte == b'\t')
            .context("invalid index listing")?;
        anyhow::ensure!(separator + 1 < row.len(), "invalid index path");
        let fields: Vec<_> = std::str::from_utf8(&row[..separator])?
            .split_ascii_whitespace()
            .collect();
        anyhow::ensure!(fields.len() == 3, "invalid index entry");
        anyhow::ensure!(
            fields[0] != "040000",
            "sparse indexes require expansion before commit checkpoint preparation"
        );
        anyhow::ensure!(
            matches!(fields[0], "100644" | "100755" | "120000" | "160000")
                && matches!(fields[2], "0" | "1" | "2" | "3"),
            "invalid index entry"
        );
        entries.push((fields[0].into(), fields[1].into()));
        anyhow::ensure!(
            entries.len() <= 32768,
            "too many staged entries for commit recovery"
        );
    }
    Ok(entries)
}

/// Verify a saved regular file through a no-follow handle. Git object IDs
/// include the blob header, while the manifest checksum covers raw bytes.
fn verify_saved_file(
    directory: &Dir,
    name: &str,
    expected_sha256: &str,
    expected_size: Option<u64>,
    maximum: u64,
    git_oid: Option<&str>,
) -> Result<Vec<u8>> {
    let maximum = maximum.min(expected_size.unwrap_or(maximum));
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = directory
        .open_with(name, &options)
        .context("checkpoint file is missing or inaccessible")?;
    let before = file.metadata()?;
    anyhow::ensure!(
        before.is_file()
            && before.len() <= maximum
            && expected_size.is_none_or(|size| size == before.len()),
        "invalid checkpoint file size or type"
    );
    let mut checksum = Sha256::new();
    let mut git_sha1 = Sha1::new();
    let mut git_sha256 = Sha256::new();
    if git_oid.is_some() {
        let header = format!("blob {}\0", before.len());
        git_sha1.update(header.as_bytes());
        git_sha256.update(header.as_bytes());
    }
    let mut small = Vec::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        Cancellation::current().check()?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .context("checkpoint file size overflow")?;
        anyhow::ensure!(size <= maximum, "checkpoint file exceeds its limit");
        checksum.update(&buffer[..count]);
        if git_oid.is_some() {
            git_sha1.update(&buffer[..count]);
            git_sha256.update(&buffer[..count]);
        }
        if before.len() < 1024 {
            small.extend_from_slice(&buffer[..count]);
        }
    }
    let after = file.metadata()?;
    anyhow::ensure!(
        size == before.len()
            && after.len() == before.len()
            && after.modified().ok() == before.modified().ok(),
        "checkpoint file changed while reading"
    );
    anyhow::ensure!(
        format!("{:x}", checksum.finalize()) == expected_sha256,
        "checkpoint file checksum mismatch"
    );
    if let Some(oid) = git_oid {
        let actual = match oid.len() {
            40 => format!("{:x}", git_sha1.finalize()),
            64 => format!("{:x}", git_sha256.finalize()),
            _ => anyhow::bail!("invalid checkpoint Git object ID"),
        };
        anyhow::ensure!(actual == oid, "checkpoint Git object ID mismatch");
    }
    Ok(small)
}

fn verify_checkpoint_files(base: &Path, record: &Checkpoint, hash_length: usize) -> Result<()> {
    let base = Dir::open_ambient_dir(base, ambient_authority())?;
    let directory = base.open_dir_nofollow(&record.id)?;
    match &record.original_index {
        Some(expected) => {
            verify_saved_file(
                &directory,
                "index.before",
                expected,
                None,
                64 * 1024 * 1024,
                None,
            )?;
        }
        None => match directory.symlink_metadata("index.before") {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
            Ok(_) => anyhow::bail!("unexpected original index backup"),
        },
    }
    verify_saved_file(
        &directory,
        "index.selected",
        &record.selected_index,
        None,
        64 * 1024 * 1024,
        None,
    )?;
    let mut pointers = std::collections::BTreeMap::new();
    let mut seen = std::collections::HashSet::new();
    for object in &record.objects {
        anyhow::ensure!(
            object.oid.len() == hash_length
                && object.oid.bytes().all(|byte| byte.is_ascii_hexdigit())
                && seen.insert(object.oid.clone()),
            "invalid or duplicate saved Git object"
        );
        let small = verify_saved_file(
            &directory,
            &format!("object-{}", object.oid),
            &object.sha256,
            Some(object.size),
            u64::MAX,
            Some(&object.oid),
        )?;
        if let Some(pointer) = crate::lfs::pointer(&small).filter(|pointer| pointer.size != 0) {
            if let Some(size) = pointers.insert(pointer.oid, pointer.size) {
                anyhow::ensure!(size == pointer.size, "inconsistent saved LFS pointer");
            }
        }
    }
    let mut lfs_seen = std::collections::HashSet::new();
    for object in &record.lfs {
        object.validate()?;
        anyhow::ensure!(
            lfs_seen.insert(object.oid.clone())
                && pointers.remove(&object.oid) == Some(object.size),
            "invalid or unreferenced saved LFS payload"
        );
        verify_saved_file(
            &directory,
            &format!("lfs-{}", object.oid),
            &object.oid,
            Some(object.size),
            u64::MAX,
            None,
        )?;
    }
    anyhow::ensure!(pointers.is_empty(), "checkpoint is missing an LFS payload");
    let mut shared_seen = std::collections::HashSet::new();
    for shared in &record.shared {
        validate_shared_name(&shared.name)?;
        anyhow::ensure!(
            shared_seen.insert(shared.name.clone()),
            "duplicate shared index backup"
        );
        verify_saved_file(
            &directory,
            &shared.name,
            &shared.sha256,
            Some(shared.size),
            64 * 1024 * 1024,
            None,
        )?;
    }
    Ok(())
}

impl Repository {
    async fn reviewed_commit_parents(
        &self,
        head: Option<&str>,
        amend: bool,
    ) -> Result<Vec<String>> {
        let Some(head) = head else {
            anyhow::ensure!(!amend, "cannot amend an unborn HEAD");
            return Ok(Vec::new());
        };
        if !amend {
            return Ok(vec![head.to_owned()]);
        }
        let commit = self
            .run(&args(&["--no-replace-objects", "cat-file", "commit", head]))
            .await?;
        anyhow::ensure!(commit.code == 0, "cannot inspect commit parents");
        let mut parents = Vec::new();
        for line in commit
            .stdout
            .split(|byte| *byte == b'\n')
            .take_while(|line| !line.is_empty())
        {
            if let Some(parent) = line.strip_prefix(b"parent ") {
                let parent = std::str::from_utf8(parent)?;
                anyhow::ensure!(
                    parent.len() == head.len()
                        && parent.bytes().all(|byte| byte.is_ascii_hexdigit()),
                    "invalid commit parent"
                );
                parents.push(parent.to_owned());
                anyhow::ensure!(parents.len() <= 1024, "too many commit parents");
            }
        }
        Ok(parents)
    }

    async fn checkpoint_directory(&self) -> Result<PathBuf> {
        let base = self.git_directory().await?.join(DIRECTORY);
        private_directory(&base)?;
        Ok(base)
    }

    /// Have Git parse both verified indexes in an isolated Git directory using
    /// the saved shared-index files, even if the live shared files were pruned.
    async fn verify_index_coverage(
        &self,
        base: &Path,
        record: &Checkpoint,
        hash_length: usize,
    ) -> Result<()> {
        let scratch = self.git_directory().await?.join(format!(
            "desktop-checkpoint-inspect-{}",
            uuid::Uuid::new_v4()
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&scratch)?;
        let scratch = Preparing {
            path: scratch,
            retain: false,
        };
        fs::create_dir(scratch.path.join("objects"))?;
        fs::create_dir(scratch.path.join("refs"))?;
        fs::create_dir(scratch.path.join("refs/heads"))?;
        work::private_write(&scratch.path.join("HEAD"), b"ref: refs/heads/checkpoint\n")?;
        work::private_write(&scratch.path.join("empty-config"), b"")?;
        let config: &[u8] = if hash_length == 40 {
            b"[core]\nrepositoryformatversion = 0\nbare = false\n"
        } else {
            b"[core]\nrepositoryformatversion = 1\nbare = false\n[extensions]\nobjectformat = sha256\n"
        };
        work::private_write(&scratch.path.join("config"), config)?;
        let source = base.join(&record.id);
        for shared in &record.shared {
            validate_shared_name(&shared.name)?;
            let bytes = read_shared_index(&source.join(&shared.name))?
                .context("saved shared index disappeared")?;
            anyhow::ensure!(
                bytes.len() as u64 == shared.size && hash(&bytes) == shared.sha256,
                "saved shared index changed during inspection"
            );
            work::private_write(&scratch.path.join(&shared.name), &bytes)?;
        }
        let mut required = std::collections::HashSet::new();
        let mut indexes = vec![("index.selected", &record.selected_index)];
        if let Some(before) = &record.original_index {
            indexes.push(("index.before", before));
        }
        for (name, expected) in indexes {
            let bytes =
                read_shared_index(&source.join(name))?.context("saved index disappeared")?;
            anyhow::ensure!(
                hash(&bytes) == *expected,
                "saved index changed during inspection"
            );
            let copy = scratch.path.join(name);
            work::private_write(&copy, &bytes)?;
            let output = process::inspect_saved_index(&self.path, &scratch.path, &copy).await?;
            anyhow::ensure!(
                output.code == 0,
                "cannot parse saved index: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            for (mode, oid) in index_entries(&output.stdout)? {
                anyhow::ensure!(
                    oid.len() == hash_length && oid.bytes().all(|byte| byte.is_ascii_hexdigit()),
                    "invalid saved index object ID"
                );
                if mode != "160000" {
                    required.insert(oid);
                }
            }
        }
        let retained: std::collections::HashSet<_> = record
            .objects
            .iter()
            .map(|object| object.oid.clone())
            .collect();
        anyhow::ensure!(
            required == retained,
            "checkpoint does not retain exactly the Git blobs referenced by its indexes"
        );
        Ok(())
    }

    /// Retain the exact reviewed index and all original staged data. This is
    /// preparation only: it does not publish an index or create a commit.
    pub(crate) async fn prepare_selected_commit(
        &self,
        paths: Vec<GitPath>,
        remove_paths: Vec<GitPath>,
        partial: Vec<CopilotLineSelection>,
        allow_empty: bool,
        amend_head: Option<&str>,
        expected_head: Option<&CopilotExpectedHead>,
        basis: &str,
    ) -> Result<Value> {
        anyhow::ensure!(
            basis.len() == 64 && basis.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid selected commit basis"
        );
        let request = SelectedRequest {
            paths: paths.clone(),
            remove_paths: remove_paths.clone(),
            partial: partial.clone(),
            allow_empty,
            amend_head: amend_head.map(str::to_owned),
            expected_head: expected_head.cloned(),
        };
        let index_path = self.index_path().await?;
        let _index_lock = IndexTransaction::begin(&index_path)?;
        let (snapshot, review) = self
            .capture_selected_commit_review(
                paths,
                remove_paths,
                partial,
                allow_empty,
                amend_head,
                expected_head,
            )
            .await?;
        anyhow::ensure!(
            review["basis"].as_str() == Some(basis),
            "Selected changes, index, HEAD, or attributes changed; review the commit again"
        );
        let base = self.checkpoint_directory().await?;
        let id = uuid::Uuid::new_v4().to_string();
        let temporary = base.join(format!(".preparing-{id}"));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&temporary)?;
        let mut space = Preparing {
            path: temporary,
            retain: false,
        };
        if let Some(original) = &snapshot.live_index {
            work::private_write(&space.path.join("index.before"), original)?;
        }
        work::private_write(&space.path.join("index.selected"), &snapshot.index)?;
        let mut entries = Vec::new();
        for name in if snapshot.live_index.is_some() {
            vec!["index.before", "index.selected"]
        } else {
            vec!["index.selected"]
        } {
            let output = process::run_git(
                &self.path,
                &args(&["ls-files", "--sparse", "--stage", "-z"]),
                None,
                Some(&space.path.join(name)),
            )
            .await?;
            anyhow::ensure!(output.code == 0, "cannot enumerate saved index data");
            entries.extend(index_entries(&output.stdout)?);
        }
        let (objects, lfs, unavailable) =
            self.save_index_objects(&space.path, entries, false).await?;
        anyhow::ensure!(
            unavailable.is_empty(),
            "cannot retain all staged LFS payloads"
        );
        let shared = self
            .save_shared_indexes(&space.path, &snapshot.live_index, Some(&snapshot.index))
            .await?;
        let head = if snapshot.head.is_empty() {
            None
        } else {
            Some(std::str::from_utf8(&snapshot.head)?.trim().to_owned())
        };
        let branch = if snapshot.branch.is_empty() {
            None
        } else {
            Some(std::str::from_utf8(&snapshot.branch)?.trim().to_owned())
        };
        let tree = snapshot
            .tree
            .clone()
            .context("missing reviewed commit tree")?;
        let parents = self
            .reviewed_commit_parents(head.as_deref(), amend_head.is_some())
            .await?;
        let current_head = self
            .run(&args(&["rev-parse", "--verify", "--quiet", "HEAD"]))
            .await?;
        let current_branch = self
            .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
            .await?;
        anyhow::ensure!(
            matches!(current_head.code, 0 | 1)
                && matches!(current_branch.code, 0 | 1)
                && current_head.stdout == snapshot.head
                && current_branch.stdout == snapshot.branch
                && read_index(&index_path)? == snapshot.live_index,
            "Index or HEAD changed during checkpoint preparation"
        );
        let git_directory = self.git_directory().await?;
        let operation =
            cancellation::blocking(move || crate::discard::operation_certificate(&git_directory))
                .await?;
        anyhow::ensure!(
            operation == snapshot.operation,
            "Repository operation changed during checkpoint preparation"
        );
        let record = Checkpoint {
            version: 1,
            id: id.clone(),
            repository_id: self.id(),
            state: "prepared".into(),
            basis: basis.into(),
            head,
            branch,
            operation,
            original_index: snapshot.live_index.as_deref().map(hash),
            selected_index: hash(&snapshot.index),
            tree: Some(tree),
            parents: Some(parents),
            objects,
            lfs,
            shared,
            request: Some(request),
        };
        let manifest = serde_json::to_vec(&record)?;
        anyhow::ensure!(
            manifest.len() as u64 <= MANIFEST_LIMIT,
            "commit checkpoint is too large"
        );
        work::private_write(&space.path.join("manifest.json"), &manifest)?;
        sync_directory(&space.path)?;
        Cancellation::current().check()?;
        fs::rename(&space.path, base.join(&id))?;
        space.path = base.join(&id);
        space.retain = true;
        sync_directory(&base)
            .with_context(|| format!("Commit checkpoint {id} may require inspection"))?;
        Ok(json!({"id": id, "state":"prepared", "basis":basis}))
    }

    /// Recapture the selection as an observation, not a publication authorization.
    pub(crate) async fn revalidate_commit_checkpoint(&self, id: &str) -> Result<Value> {
        let base = self.checkpoint_directory().await?;
        read_checkpoint(&base, id, &self.id())?;
        let path = base.join(id).join(".use.lock");
        let _use_lock = CheckpointUse {
            file: Some(work::private_file(&path).context("commit checkpoint is in use")?),
            path,
        };
        let record = read_checkpoint(&base, id, &self.id())?;
        let manifest = serde_json::to_vec(&record)?;
        let request = record
            .request
            .clone()
            .context("Checkpoint does not contain its selected request; prepare it again")?;
        let index_path = self.index_path().await?;
        let _index_lock = IndexTransaction::begin(&index_path)?;
        let inspection = self.inspect_commit_checkpoint(id).await?;
        let unchanged = [
            "head_matches",
            "branch_matches",
            "index_matches",
            "operation_matches",
        ]
        .iter()
        .all(|field| inspection[*field] == true);
        let matches = if unchanged {
            let amend = request.amend_head.is_some();
            let (_, review) = self
                .capture_selected_commit_review(
                    request.paths,
                    request.remove_paths,
                    request.partial,
                    request.allow_empty,
                    request.amend_head.as_deref(),
                    request.expected_head.as_ref(),
                )
                .await?;
            let publication_matches = match (&record.tree, &record.parents) {
                (Some(tree), Some(parents)) => {
                    review["tree"].as_str() == Some(tree.as_str())
                        && self
                            .reviewed_commit_parents(record.head.as_deref(), amend)
                            .await?
                            == *parents
                }
                (None, None) => true,
                _ => false,
            };
            publication_matches && review["basis"].as_str() == Some(record.basis.as_str())
        } else {
            false
        };
        anyhow::ensure!(
            serde_json::to_vec(&read_checkpoint(&base, id, &self.id())?)? == manifest,
            "commit checkpoint changed during revalidation"
        );
        Ok(json!({"id": id, "basis": record.basis, "matches": matches}))
    }

    pub(crate) async fn list_commit_checkpoints(&self) -> Result<Value> {
        let base = self.checkpoint_directory().await?;
        let mut rows = Vec::new();
        for entry in fs::read_dir(&base)? {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if !valid_id(&id) {
                continue;
            }
            anyhow::ensure!(rows.len() < 1000, "too many commit checkpoints");
            match read_checkpoint(&base, &id, &self.id()) {
                Ok(record) => rows.push(
                    json!({"id":id,"state":record.state,"head":record.head,"branch":record.branch}),
                ),
                Err(_) => rows.push(json!({"id":id,"state":"invalid"})),
            }
        }
        Ok(json!({"checkpoints":rows}))
    }

    /// Check every recorded backup without modifying the repository. The
    /// current-state flags are observations, not publication locks.
    pub(crate) async fn inspect_commit_checkpoint(&self, id: &str) -> Result<Value> {
        let base = self.checkpoint_directory().await?;
        let record = read_checkpoint(&base, id, &self.id())?;
        let format = self
            .run(&args(&["rev-parse", "--show-object-format"]))
            .await?;
        anyhow::ensure!(format.code == 0, "cannot determine Git object format");
        let length = match format.stdout.strip_suffix(b"\n").unwrap_or(&format.stdout) {
            b"sha1" => 40,
            b"sha256" => 64,
            _ => anyhow::bail!("unsupported Git object format"),
        };
        let base_for_check = base.clone();
        let record = cancellation::blocking(move || {
            verify_checkpoint_files(&base_for_check, &record, length)?;
            Ok(record)
        })
        .await?;
        self.verify_index_coverage(&base, &record, length).await?;
        let reread = read_checkpoint(&base, id, &self.id())?;
        anyhow::ensure!(
            serde_json::to_vec(&record)? == serde_json::to_vec(&reread)?,
            "commit checkpoint changed during inspection"
        );
        let index_path = self.index_path().await?;
        let head = self
            .run(&args(&["rev-parse", "--verify", "--quiet", "HEAD"]))
            .await?;
        let branch = self
            .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
            .await?;
        anyhow::ensure!(
            matches!(head.code, 0 | 1) && matches!(branch.code, 0 | 1),
            "cannot inspect repository HEAD"
        );
        let current_head = if head.code == 0 {
            Some(std::str::from_utf8(&head.stdout)?.trim().to_owned())
        } else {
            None
        };
        let current_branch = if branch.code == 0 {
            Some(std::str::from_utf8(&branch.stdout)?.trim().to_owned())
        } else {
            None
        };
        let index = read_index(&index_path)?.as_deref().map(hash);
        let git_directory = self.git_directory().await?;
        let operation =
            cancellation::blocking(move || crate::discard::operation_certificate(&git_directory))
                .await?;
        Ok(
            json!({"id":id,"state":record.state,"basis":record.basis,"publication_metadata_present":record.tree.is_some() && record.parents.is_some(),"recorded_files_verified":true,"index_coverage_verified":true,"objects_verified":record.objects.len(),"lfs_verified":record.lfs.len(),"shared_indexes_verified":record.shared.len(),"head_matches":current_head == record.head,"branch_matches":current_branch == record.branch,"index_matches":index == record.original_index,"operation_matches":operation == record.operation}),
        )
    }

    /// Only pre-publication checkpoints can be removed. Any later phase must
    /// provide explicit guarded recovery instead of using this operation.
    pub(crate) async fn forget_commit_checkpoint(&self, id: &str) -> Result<Value> {
        let base = self.checkpoint_directory().await?;
        let record = read_checkpoint(&base, id, &self.id())?;
        anyhow::ensure!(
            record.state == "prepared",
            "commit checkpoint requires recovery"
        );
        let directory = base.join(id);
        let lock_path = directory.join(".use.lock");
        let _lock = work::private_file(&lock_path).context("commit checkpoint is in use")?;
        let record = read_checkpoint(&base, id, &self.id())?;
        anyhow::ensure!(
            record.state == "prepared",
            "commit checkpoint requires recovery"
        );
        fs::remove_dir_all(directory)?;
        sync_directory(&base)?;
        Ok(json!({"id":id,"forgotten":true}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn checkpoint_records_all_merge_parents_for_amend() {
        let root =
            std::env::temp_dir().join(format!("desktop-commit-merge-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
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
        fs::write(root.join("file"), b"initial\n").unwrap();
        git(&["add", "file"]);
        git(&["commit", "-qm", "initial"]);
        let first = git(&["rev-parse", "HEAD"]);
        let tree = git(&["write-tree"]);
        let second = git(&["commit-tree", &tree, "-p", &first, "-m", "second"]);
        let merge = git(&[
            "commit-tree",
            &tree,
            "-p",
            &first,
            "-p",
            &second,
            "-m",
            "merge",
        ]);
        git(&["update-ref", "HEAD", &merge, &first]);
        fs::write(root.join("file"), b"amended\n").unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let selection = || vec![GitPath::Text("file".into())];
        let preview = repo
            .selected_commit_preview_with_empty(
                selection(),
                vec![],
                vec![],
                false,
                Some(&merge),
                None,
            )
            .await
            .unwrap();
        let prepared = repo
            .prepare_selected_commit(
                selection(),
                vec![],
                vec![],
                false,
                Some(&merge),
                None,
                preview["basis"].as_str().unwrap(),
            )
            .await
            .unwrap();
        let id = prepared["id"].as_str().unwrap();
        let record = read_checkpoint(&root.join(".git").join(DIRECTORY), id, &repo.id()).unwrap();
        assert_eq!(record.tree.as_deref(), preview["tree"].as_str());
        assert_eq!(record.parents, Some(vec![first, second]));
        assert_eq!(git(&["rev-parse", "HEAD"]), merge);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn checkpoint_retains_staged_data_without_publishing_and_can_be_forgotten() {
        let root = std::env::temp_dir().join(format!(
            "desktop-commit-checkpoint-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&root).unwrap();
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
        fs::write(root.join("file"), b"initial\n").unwrap();
        git(&["add", "file"]);
        git(&["commit", "-qm", "initial"]);
        fs::write(root.join("file"), b"staged only\n").unwrap();
        git(&["add", "file"]);
        let staged_oid = git(&["rev-parse", ":file"]);
        git(&["update-index", "--split-index"]);
        fs::write(root.join("file"), b"selected worktree\n").unwrap();
        let selected_oid = git(&["hash-object", "file"]);
        let original = fs::read(root.join(".git/index")).unwrap();
        let head = git(&["rev-parse", "HEAD"]);
        let repo = Repository::open(&root).await.unwrap();
        let selected = || vec![GitPath::Text("file".into())];
        let preview = repo
            .selected_commit_preview_with_empty(selected(), vec![], vec![], false, None, None)
            .await
            .unwrap();
        let basis = preview["basis"].as_str().unwrap();
        fs::write(root.join("file"), b"changed after review\n").unwrap();
        assert!(
            repo.prepare_selected_commit(selected(), vec![], vec![], false, None, None, basis)
                .await
                .is_err()
        );
        fs::write(root.join("file"), b"selected worktree\n").unwrap();
        let preview = repo
            .selected_commit_preview_with_empty(selected(), vec![], vec![], false, None, None)
            .await
            .unwrap();
        let basis = preview["basis"].as_str().unwrap();
        let result = repo
            .prepare_selected_commit(selected(), vec![], vec![], false, None, None, basis)
            .await
            .unwrap();
        let id = result["id"].as_str().unwrap();
        assert_eq!(result["state"], "prepared");
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), original);
        assert_eq!(git(&["rev-parse", "HEAD"]), head);
        let base = root.join(".git").join(DIRECTORY);
        let record = read_checkpoint(&base, id, &repo.id()).unwrap();
        assert_eq!(record.tree.as_deref(), preview["tree"].as_str());
        assert_eq!(record.parents.as_deref(), Some(&[head.clone()][..]));
        assert!(!record.shared.is_empty());
        assert!(record.objects.iter().any(|object| object.oid == staged_oid));
        assert!(
            record
                .objects
                .iter()
                .any(|object| object.oid == selected_oid)
        );
        assert_eq!(
            fs::read(base.join(id).join("index.before")).unwrap(),
            original
        );
        assert_eq!(
            repo.list_commit_checkpoints().await.unwrap()["checkpoints"][0]["id"],
            id
        );
        let inspection = repo.inspect_commit_checkpoint(id).await.unwrap();
        assert_eq!(inspection["publication_metadata_present"], true);
        for field in [
            "recorded_files_verified",
            "head_matches",
            "branch_matches",
            "index_matches",
            "operation_matches",
        ] {
            assert_eq!(inspection[field], true, "{field}");
        }
        let checked = repo.revalidate_commit_checkpoint(id).await.unwrap();
        assert_eq!(checked["matches"], true);
        assert_eq!(checked["basis"], basis);
        fs::write(root.join("file"), b"changed after checkpoint\n").unwrap();
        assert_eq!(
            repo.revalidate_commit_checkpoint(id).await.unwrap()["matches"],
            false
        );
        fs::write(root.join("file"), b"selected worktree\n").unwrap();
        let manifest_path = base.join(id).join("manifest.json");
        let current_manifest = fs::read(&manifest_path).unwrap();
        let mut changed_parent: Value = serde_json::from_slice(&current_manifest).unwrap();
        changed_parent["parents"][0] = json!("0".repeat(head.len()));
        fs::write(&manifest_path, serde_json::to_vec(&changed_parent).unwrap()).unwrap();
        assert_eq!(
            repo.revalidate_commit_checkpoint(id).await.unwrap()["matches"],
            false
        );
        fs::write(&manifest_path, &current_manifest).unwrap();
        let mut old_manifest: Value = serde_json::from_slice(&current_manifest).unwrap();
        old_manifest.as_object_mut().unwrap().remove("request");
        fs::write(&manifest_path, serde_json::to_vec(&old_manifest).unwrap()).unwrap();
        assert!(repo.inspect_commit_checkpoint(id).await.is_ok());
        assert!(repo.revalidate_commit_checkpoint(id).await.is_err());
        assert!(!base.join(id).join(".use.lock").exists());
        fs::write(&manifest_path, current_manifest).unwrap();
        let live_lock = root.join(".git/index.lock");
        fs::write(&live_lock, b"other git operation").unwrap();
        assert!(repo.revalidate_commit_checkpoint(id).await.is_err());
        assert!(!base.join(id).join(".use.lock").exists());
        fs::remove_file(live_lock).unwrap();
        #[cfg(unix)]
        {
            git(&["config", "core.fsmonitor", "touch checkpoint-fsmonitor-ran"]);
            assert_eq!(
                repo.inspect_commit_checkpoint(id).await.unwrap()["index_coverage_verified"],
                true
            );
            assert!(!root.join("checkpoint-fsmonitor-ran").exists());
            git(&["config", "--unset", "core.fsmonitor"]);
        }
        let saved_blob = base.join(id).join(format!("object-{selected_oid}"));
        let bytes = fs::read(&saved_blob).unwrap();
        fs::write(&saved_blob, b"corrupted").unwrap();
        assert!(repo.inspect_commit_checkpoint(id).await.is_err());
        let manifest_path = base.join(id).join("manifest.json");
        let manifest_bytes = fs::read(&manifest_path).unwrap();
        let mut forged: Value = serde_json::from_slice(&manifest_bytes).unwrap();
        let object = forged["objects"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|entry| entry["oid"] == selected_oid)
            .unwrap();
        object["size"] = json!(b"corrupted".len());
        object["sha256"] = json!(hash(b"corrupted"));
        fs::write(&manifest_path, serde_json::to_vec(&forged).unwrap()).unwrap();
        assert!(repo.inspect_commit_checkpoint(id).await.is_err());
        fs::write(&manifest_path, manifest_bytes).unwrap();
        fs::write(&saved_blob, bytes).unwrap();
        let original_manifest = fs::read(&manifest_path).unwrap();
        let mut incomplete: Value = serde_json::from_slice(&original_manifest).unwrap();
        incomplete["objects"]
            .as_array_mut()
            .unwrap()
            .retain(|entry| entry["oid"] != selected_oid);
        fs::write(&manifest_path, serde_json::to_vec(&incomplete).unwrap()).unwrap();
        assert!(repo.inspect_commit_checkpoint(id).await.is_err());
        fs::write(&manifest_path, original_manifest).unwrap();
        let selected_path = base.join(id).join("index.selected");
        let saved_selected = fs::read(&selected_path).unwrap();
        let original_manifest = fs::read(&manifest_path).unwrap();
        let mut malformed: Value = serde_json::from_slice(&original_manifest).unwrap();
        fs::write(&selected_path, b"not a Git index").unwrap();
        malformed["selected_index"] = json!(hash(b"not a Git index"));
        fs::write(&manifest_path, serde_json::to_vec(&malformed).unwrap()).unwrap();
        assert!(repo.inspect_commit_checkpoint(id).await.is_err());
        fs::write(&selected_path, saved_selected).unwrap();
        fs::write(&manifest_path, original_manifest).unwrap();
        assert!(!fs::read_dir(root.join(".git")).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("desktop-checkpoint-inspect-")
        }));
        #[cfg(unix)]
        {
            let saved = base.join(id).join("index.selected");
            let bytes = fs::read(&saved).unwrap();
            fs::remove_file(&saved).unwrap();
            std::os::unix::fs::symlink(base.join(id).join("index.before"), &saved).unwrap();
            assert!(repo.inspect_commit_checkpoint(id).await.is_err());
            fs::remove_file(&saved).unwrap();
            fs::write(saved, bytes).unwrap();
        }
        let lock = base.join(id).join(".use.lock");
        fs::write(&lock, b"other operation").unwrap();
        assert!(repo.forget_commit_checkpoint(id).await.is_err());
        assert!(repo.revalidate_commit_checkpoint(id).await.is_err());
        fs::remove_file(lock).unwrap();
        let manifest = base.join(id).join("manifest.json");
        let original_manifest = fs::read(&manifest).unwrap();
        let mut interrupted: Value = serde_json::from_slice(&original_manifest).unwrap();
        interrupted["state"] = json!("invoking");
        fs::write(&manifest, serde_json::to_vec(&interrupted).unwrap()).unwrap();
        assert!(repo.forget_commit_checkpoint(id).await.is_err());
        assert_eq!(
            repo.list_commit_checkpoints().await.unwrap()["checkpoints"][0]["state"],
            "invalid"
        );
        fs::write(&manifest, original_manifest).unwrap();
        // Simulate losing the original index and its otherwise unreachable
        // objects after the checkpoint has become durable.
        git(&["reset", "--hard", "-q", "HEAD"]);
        git(&["prune", "--expire", "now"]);
        assert_eq!(
            repo.inspect_commit_checkpoint(id).await.unwrap()["index_matches"],
            false
        );
        for oid in [&staged_oid, &selected_oid] {
            assert!(
                !std::process::Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(["cat-file", "-e", oid])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        for entry in fs::read_dir(root.join(".git")).unwrap() {
            let path = entry.unwrap().path();
            if path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("sharedindex.")
            {
                fs::remove_file(path).unwrap();
            }
        }
        let after_prune = repo.inspect_commit_checkpoint(id).await.unwrap();
        assert_eq!(after_prune["index_coverage_verified"], true);
        repo.restore_shared_indexes(&base.join(id), &record.shared)
            .await
            .unwrap();
        repo.restore_index_objects(
            &base.join(id),
            &record.objects,
            &record.lfs,
            &[],
            staged_oid.len(),
            true,
        )
        .await
        .unwrap();
        let mut index_lock = IndexTransaction::begin(&root.join(".git/index")).unwrap();
        index_lock.commit(&original).unwrap();
        assert_eq!(git(&["show", ":file"]), "staged only");
        assert_eq!(
            git(&["cat-file", "blob", &selected_oid]),
            "selected worktree"
        );
        drop(repo);
        let reopened = Repository::open(&root).await.unwrap();
        assert_eq!(
            reopened.list_commit_checkpoints().await.unwrap()["checkpoints"][0]["state"],
            "prepared"
        );
        assert!(reopened.forget_commit_checkpoint("../bad").await.is_err());
        assert_eq!(
            reopened.forget_commit_checkpoint(id).await.unwrap()["forgotten"],
            true
        );
        assert_eq!(
            reopened.list_commit_checkpoints().await.unwrap()["checkpoints"],
            json!([])
        );
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), original);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn index_parser_rejects_truncated_and_invalid_records() {
        assert!(index_entries(b"100644 deadbeef 0\tfile").is_err());
        assert!(index_entries(b"040000 deadbeef 0\tfile\0").is_err());
        assert!(index_entries(b"100644 deadbeef 9\tfile\0").is_err());
        assert_eq!(
            index_entries(b"100644 deadbeef 0\tfile\0").unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn checkpoint_handles_an_unborn_empty_index() {
        let root =
            std::env::temp_dir().join(format!("desktop-empty-checkpoint-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        let repo = Repository::open(&root).await.unwrap();
        let preview = repo
            .selected_commit_preview_with_empty(vec![], vec![], vec![], true, None, None)
            .await
            .unwrap();
        let basis = preview["basis"].as_str().unwrap();
        let prepared = repo
            .prepare_selected_commit(vec![], vec![], vec![], true, None, None, basis)
            .await
            .unwrap();
        let id = prepared["id"].as_str().unwrap();
        let base = root.join(".git").join(DIRECTORY);
        let record = read_checkpoint(&base, id, &repo.id()).unwrap();
        assert!(record.head.is_none() && record.original_index.is_none());
        assert_eq!(
            repo.inspect_commit_checkpoint(id).await.unwrap()["recorded_files_verified"],
            true
        );
        assert!(!root.join(".git/index").exists());
        assert!(!base.join(id).join("index.before").exists());
        assert!(base.join(id).join("index.selected").exists());
        repo.forget_commit_checkpoint(id).await.unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn checkpoint_retains_staged_lfs_payloads() {
        let root =
            std::env::temp_dir().join(format!("desktop-checkpoint-lfs-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
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
        let payload = b"original staged LFS data\n";
        let oid = hash(payload);
        let storage = root.join("storage");
        let media = storage.join("objects");
        let target = media.join(&oid[..2]).join(&oid[2..4]).join(&oid);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, payload).unwrap();
        let alias = format!(
            "!f() {{ printf 'LocalMediaDir=%s\\nLfsStorageDir=%s\\n' '{}' '{}'; }}; f",
            media.display(),
            storage.display()
        );
        git(&["config", "alias.lfs", &alias]);
        let pointer = format!(
            "version https://git-lfs.github.com/spec/v1\noid sha256:{oid}\nsize {}\n",
            payload.len()
        );
        fs::write(root.join("file"), pointer).unwrap();
        git(&["add", "file"]);
        fs::write(root.join("file"), b"selected working file\n").unwrap();
        let original = fs::read(root.join(".git/index")).unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let selected = || vec![GitPath::Text("file".into())];
        let preview = repo
            .selected_commit_preview_with_empty(selected(), vec![], vec![], false, None, None)
            .await
            .unwrap();
        let basis = preview["basis"].as_str().unwrap();
        let prepared = repo
            .prepare_selected_commit(selected(), vec![], vec![], false, None, None, basis)
            .await
            .unwrap();
        let id = prepared["id"].as_str().unwrap();
        let base = root.join(".git").join(DIRECTORY);
        let record = read_checkpoint(&base, id, &repo.id()).unwrap();
        assert_eq!(record.lfs.len(), 1);
        assert_eq!(
            repo.inspect_commit_checkpoint(id).await.unwrap()["lfs_verified"],
            1
        );
        assert_eq!(
            fs::read(base.join(id).join(format!("lfs-{oid}"))).unwrap(),
            payload
        );
        let saved_payload = base.join(id).join(format!("lfs-{oid}"));
        fs::write(&saved_payload, b"tampered payload").unwrap();
        assert!(repo.inspect_commit_checkpoint(id).await.is_err());
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), original);
        repo.forget_commit_checkpoint(id).await.unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn checkpoint_inspects_sha256_git_objects() {
        let root = std::env::temp_dir().join(format!(
            "desktop-checkpoint-sha256-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&root).unwrap();
        let init = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["init", "-q", "--object-format=sha256"])
            .output()
            .unwrap();
        if !init.status.success() {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        fs::write(root.join("file"), b"sha256 object\n").unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let selected = || vec![GitPath::Text("file".into())];
        let preview = repo
            .selected_commit_preview_with_empty(selected(), vec![], vec![], false, None, None)
            .await
            .unwrap();
        let prepared = repo
            .prepare_selected_commit(
                selected(),
                vec![],
                vec![],
                false,
                None,
                None,
                preview["basis"].as_str().unwrap(),
            )
            .await
            .unwrap();
        let id = prepared["id"].as_str().unwrap();
        assert_eq!(
            repo.inspect_commit_checkpoint(id).await.unwrap()["objects_verified"],
            1
        );
        repo.forget_commit_checkpoint(id).await.unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn checkpoint_refuses_unretained_sparse_index_trees() {
        let root = std::env::temp_dir().join(format!(
            "desktop-sparse-checkpoint-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&root).unwrap();
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
            String::from_utf8(output.stdout).unwrap()
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.invalid"]);
        fs::create_dir(root.join("included")).unwrap();
        fs::create_dir(root.join("outside")).unwrap();
        fs::write(root.join("included/file"), b"base\n").unwrap();
        fs::write(root.join("outside/file"), b"outside\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "initial"]);
        git(&["sparse-checkout", "init", "--cone", "--sparse-index"]);
        git(&["sparse-checkout", "set", "included"]);
        assert!(git(&["ls-files", "--sparse", "--stage"]).contains("040000 "));
        fs::write(root.join("included/file"), b"selected\n").unwrap();
        let index = fs::read(root.join(".git/index")).unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let selected = || vec![GitPath::Text("included/file".into())];
        let preview = repo
            .selected_commit_preview_with_empty(selected(), vec![], vec![], false, None, None)
            .await
            .unwrap();
        let result = repo
            .prepare_selected_commit(
                selected(),
                vec![],
                vec![],
                false,
                None,
                None,
                preview["basis"].as_str().unwrap(),
            )
            .await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("sparse indexes require expansion")
        );
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        fs::remove_dir_all(root).unwrap();
    }
}
