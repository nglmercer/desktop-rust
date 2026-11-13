//! Repository workflows shared by the CLI and browser/native transports.
pub mod application;
pub mod copilot;
mod copilot_bridge;
pub mod migration;
pub mod secret_store;
pub mod updater;
pub use application::{
    ApplicationService, BACKEND_VERSION, Capabilities, IBackendError, IRequest, JobState,
};
pub use secret_store::{SecretStore, StoreInfo, StoreKind};
mod accounts;
pub use accounts::{
    AccountAssignment, AccountTarget, RepositoryAccount, ScopedAssociation, StoredAccount,
};
mod repository_context;
pub use repository_context::{
    ForkContributionTarget, RepositoryContext, RepositoryIdentity, RepositoryMetadata,
};
mod repo_rules;
pub use repo_rules::{NameRuleFailure, NameRulesActor, NameRulesPolicy};
mod branch_policy;
pub mod cancellation;
pub mod comparison_snapshots;
pub mod hook_progress;
#[cfg(unix)]
mod hook_trace;
pub use branch_policy::{BranchPermissions, BranchPolicy, DefaultBranch};
mod commit;
mod commit_checkpoint;
mod commit_ref_guard;
pub use commit_ref_guard::CommitReferenceGuard;
mod diff;
mod editors;
pub use editors::{EditorLaunch, ExternalEditor, available_editors};
mod discard;
mod discard_fs;
mod fetch_error;
mod git_credentials;
mod github;
mod github_api;
mod github_checkout;
mod github_create;
mod lfs_progress;
pub use git_credentials::helper as git_credential_helper;
mod lfs;
mod lfs_attributes;
mod lfs_operations;
pub use lfs::LfsInstallScope;
pub use lfs_operations::{LfsTransferRequest, lfs_install, lfs_tool_status};
mod paths;
mod remote_discovery;
pub use remote_discovery::{ManagedRemoteAction, safe_fetch_remotes, safe_remote_url};
mod refs;
mod renderer_cherry_pick;
mod renderer_fast_forward;
mod renderer_fetch;
mod renderer_merge;
mod renderer_rebase;
mod renderer_reset;
mod renderer_revert;
pub mod renderer_worktree;
mod renderer_working_file;
mod resolution_diff;
pub use refs::symbolic_ref;
mod repository_discovery;
pub use repository_discovery::{repository_type, trust_repository};
mod creation;
mod gitignore;
pub use gitignore::GitIgnoreAction;
mod preferences;
mod publish;
mod publish_origin;
pub use publish_origin::publication_config_preflight;
mod protected_resources;
mod publish_receipt;
pub use protected_resources::{ProtectedResources, protected_resources};
mod settings;
mod shared_index_backup;
mod storage;
pub use paths::{canonical_mutation_path, directory_exists, directory_path, path_exists};
mod submodules;
mod worktrees;
pub use preferences::{Preferences, PreferencesPatch, PreferencesStore, Theme};
pub use settings::{
    ConfigScope, GitReadSettingKey, git_global_config_path, git_settings, git_version,
    set_git_setting,
};
pub use worktrees::{worktrees_at, worktrees_from_git_dir};
mod history;
mod index_object_backup;
mod stash;
mod stash_renderer;
pub use stash::remote_deletion::{RemoteBranchDeletion, UpstreamDeletion};
mod state;
pub use state::OperationState;
mod process;
mod stream_redaction;
pub use github::{GitHubAction, GitHubRequest, execute_github};
mod protocol;
mod registry;
mod status;
use anyhow::{Context, Result, bail};
pub use creation::CreationFile;
pub use protocol::*;
pub use registry::{
    CloneAccount, CloneOptions, Registry, SavedRepositories, SavedRepositoryRecord, clone_repository,
    clone_repository_selected, clone_repository_with_options, initialize, initialize_desktop_repository, recover_selected_clone,
};
use serde_json::Value;
use std::{
    ffi::OsString,
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;

type NativeSessionSlot = Arc<Mutex<Option<copilot::native_turn::NativeSession>>>;
type NativeSessions = Arc<Mutex<std::collections::HashMap<String, NativeSessionSlot>>>;

#[derive(Clone)]
pub struct Repository {
    path: PathBuf,
    gate: Arc<Mutex<()>>,
    saved_scope: Option<SavedRepositoryScope>,
    copilot_runtime_root: Option<PathBuf>,
    native_copilot_sessions: NativeSessions,
    native_copilot_slots: Arc<tokio::sync::Semaphore>,
}

#[derive(Clone)]
struct SavedRepositoryScope {
    registry: Registry,
    record_id: Option<String>,
}

impl Repository {
    /// Resolve the working tree root so all clients agree on relative paths.
    pub async fn open(path: &Path) -> Result<Self> {
        let path = path.canonicalize().context("resolve repository path")?;
        let output =
            process::run_bound_discovery(&path, &args(&["rev-parse", "--show-toplevel"])).await?;
        if output.code != 0 {
            bail!("not a working tree: {}", output.text().stderr.trim());
        }
        let root = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
        Ok(Self {
            path: PathBuf::from(os_from_bytes(root)?),
            gate: Arc::new(Mutex::new(())),
            saved_scope: None,
            copilot_runtime_root: None,
            native_copilot_sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
            native_copilot_slots: Arc::new(tokio::sync::Semaphore::new(
                copilot_bridge::MAX_NATIVE_SESSIONS,
            )),
        })
    }

    /// Associate this handle with a registry without reading or creating files.
    pub fn with_registry(mut self, registry: Registry) -> Self {
        self.saved_scope = Some(SavedRepositoryScope {
            registry,
            record_id: None,
        });
        self
    }

    /// Use the host-selected Copilot package without changing process environment.
    pub fn with_copilot_runtime_root(mut self, root: PathBuf) -> Self {
        self.copilot_runtime_root = Some(root);
        self
    }

    /// Bind a known saved record so forget/re-add cannot select a different account.
    pub fn with_saved_record(mut self, registry: Registry, id: String) -> Self {
        self.saved_scope = Some(SavedRepositoryScope {
            registry,
            record_id: Some(id),
        });
        self
    }

    /// Identify a fixed startup scope without reading account or registry files.
    pub fn fixed_saved_record(&self) -> Option<(String, &str)> {
        self.saved_scope.as_ref().and_then(|scope| {
            scope
                .record_id
                .as_deref()
                .map(|id| (scope.registry.namespace_id(), id))
        })
    }

    pub(crate) fn account_registry(&self) -> Option<&Registry> {
        self.saved_scope.as_ref().map(|scope| &scope.registry)
    }

    pub(crate) fn saved_record(&self) -> Result<Option<registry::SavedRepositoryRecord>> {
        let Some(scope) = &self.saved_scope else {
            return Ok(None);
        };
        if let Some(id) = &scope.record_id {
            let record = scope
                .registry
                .record_by_id(id)?
                .context("Saved repository changed; select it again")?;
            anyhow::ensure!(
                record.path == self.path,
                "Saved repository working tree changed; select it again"
            );
            Ok(Some(record))
        } else {
            scope.registry.record_for_path(&self.path)
        }
    }

    /// Select a native registered worktree, preserving or adopting its saved record.
    pub async fn open_worktree(&self, path: GitPath) -> Result<Self> {
        let registry = self
            .account_registry()
            .context("Select a repository registry before opening a worktree")?
            .clone();
        let requested = directory_path(path)?;
        let requested = if requested.is_absolute() {
            requested
        } else {
            self.path.join(requested)
        };
        let target = Self::open(&requested).await?;
        anyhow::ensure!(
            requested.canonicalize()? == target.path,
            "Worktree path must name its working tree root"
        );
        // A stale registered path may now contain a different repository.
        // Common-directory equality validates membership, never account scope.
        anyhow::ensure!(
            self.common_directory().await? == target.common_directory().await?,
            "Working tree belongs to another repository"
        );
        let worktrees = self.worktrees().await?;
        let mut known = false;
        for worktree in worktrees
            .as_array()
            .context("Invalid native working tree list")?
        {
            let path = directory_path(serde_json::from_value(worktree["path"].clone())?)?;
            if path.canonicalize().is_ok_and(|path| path == target.path) {
                known = worktree["bare"] == false && worktree["prunable"].is_null();
                break;
            }
        }
        anyhow::ensure!(known, "Path is not an available registered working tree");
        let source = self.saved_record()?;
        let root = target.path.clone();
        let saved_registry = registry.clone();
        let record = cancellation::blocking(move || {
            saved_registry.switch_worktree_record(source.as_ref(), root)
        })
        .await?;
        Ok(target.with_saved_record(registry, record.id))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Stable native-path identity shared by snapshots and transport selection fences.
    pub fn id(&self) -> String {
        use sha2::{Digest, Sha256};
        format!(
            "{:x}",
            Sha256::digest(self.path.as_os_str().as_encoded_bytes())
        )
    }

    /// Serialize operations across sessions sharing this repository handle.
    pub async fn execute(&self, operation: Operation) -> Result<Value> {
        let _guard = cancellation::wait(self.gate.lock()).await?;
        cancellation::Cancellation::current().check()?;
        if let Operation::ManageRemote { action } = &operation
            && !action.is_supported()
        {
            return Ok(serde_json::json!({"kind":"unsupported"}));
        }
        if matches!(
            operation,
            Operation::PublishContinuePreview { .. } | Operation::PublishContinue { .. }
        ) {
            publication_config_preflight(&self.path).await?;
        }
        git_credentials::scope(
            self,
            &operation,
            Box::pin(self.execute_inner(operation.clone())),
        )
        .await
    }

    async fn execute_inner(&self, operation: Operation) -> Result<Value> {
        if let Operation::Github { request } = operation {
            return self.execute_github_bound(request).await;
        }
        if matches!(operation, Operation::Snapshot) {
            return Ok(serde_json::to_value(self.snapshot().await?)?);
        }
        if let Operation::StatusDetailed { include_untracked } = operation {
            return Ok(serde_json::to_value(
                self.detailed_status(include_untracked).await?,
            )?);
        }
        match operation {
            Operation::CommitPreview => return self.commit_preview().await,
            Operation::MergeBase { first, second } => {
                return self.merge_base(&first, &second).await;
            }
            Operation::MergeTreePreview { ours, theirs } => {
                return self.merge_tree_preview(&ours, &theirs).await;
            }
            Operation::InterpretTrailers {
                message,
                parse,
                unfold,
                trailers,
            } => {
                return self
                    .interpret_trailers(message, parse, unfold, trailers)
                    .await;
            }
            Operation::SelectedCommitPreview {
                paths,
                remove_paths,
                partial_selections,
                allow_empty,
                amend_head,
                expected_head,
            } => {
                return self
                    .selected_commit_preview_with_empty(
                        paths,
                        remove_paths,
                        partial_selections,
                        allow_empty,
                        amend_head.as_deref(),
                        expected_head.as_ref(),
                    )
                    .await;
            }
            Operation::PrepareSelectedCommit {
                paths,
                remove_paths,
                partial_selections,
                allow_empty,
                amend_head,
                expected_head,
                basis,
            } => {
                return self
                    .prepare_selected_commit(
                        paths,
                        remove_paths,
                        partial_selections,
                        allow_empty,
                        amend_head.as_deref(),
                        expected_head.as_ref(),
                        &basis,
                    )
                    .await;
            }
            Operation::ListCommitCheckpoints => return self.list_commit_checkpoints().await,
            Operation::PublishSelectedCommit {
                checkpoint_id,
                message,
                no_verify,
                sign_off,
                allow_large_files,
            } => {
                return self
                    .publish_selected_checkpoint(
                        &checkpoint_id,
                        message,
                        no_verify,
                        sign_off,
                        allow_large_files,
                    )
                    .await;
            }
            Operation::InspectCommitCheckpoint { checkpoint_id } => {
                return self.inspect_commit_checkpoint(&checkpoint_id).await;
            }
            Operation::RevalidateCommitCheckpoint { checkpoint_id } => {
                return self.revalidate_commit_checkpoint(&checkpoint_id).await;
            }
            Operation::RecoverPublishedCommitCheckpoint { checkpoint_id } => {
                return self
                    .recover_published_commit_checkpoint(&checkpoint_id)
                    .await;
            }
            Operation::CloseUnchangedCommitCheckpoint { checkpoint_id } => {
                return self.close_unchanged_commit_checkpoint(&checkpoint_id).await;
            }
            Operation::ForgetCommitCheckpoint { checkpoint_id } => {
                return self.forget_commit_checkpoint(&checkpoint_id).await;
            }
            Operation::InitialCommit { paths } => return self.initial_commit(paths).await,
            Operation::RendererFinishMergeCommit => {
                return self.renderer_finish_merge_commit().await;
            }
            Operation::RendererAbortSquash {
                expected_head,
                expected_branch,
            } => {
                return self
                    .renderer_abort_squash(&expected_head, &expected_branch)
                    .await;
            }
            Operation::Commit {
                summary,
                description,
                allow_large_files,
                basis,
            } => {
                return self
                    .reviewed_commit(summary, description, allow_large_files, basis)
                    .await;
            }
            Operation::LfsTransfer { request } => return self.lfs_transfer(request).await,
            Operation::LfsCheckout { patterns } => return self.lfs_checkout(patterns).await,
            Operation::LfsTrack {
                patterns,
                filename,
                lockable,
            } => return self.lfs_track(patterns, filename, lockable, false).await,
            Operation::LfsUntrack { patterns } => {
                return self.lfs_track(patterns, false, None, true).await;
            }
            Operation::LfsToolStatus => return lfs_tool_status(&self.path).await,
            Operation::LfsStatus { paths } => return self.lfs_status(paths).await,
            Operation::LfsFileTracking { paths } => return self.lfs_file_tracking(paths).await,
            Operation::LfsInstall {
                scope,
                force,
                skip_repo,
                skip_smudge,
            } => return lfs_install(&self.path, scope, force, skip_repo, skip_smudge).await,
            Operation::Submodules => return self.submodules().await,
            Operation::RendererCheckoutIndex { paths } => {
                return self.renderer_checkout_index(paths).await;
            }
            Operation::RendererCheckoutHeadPaths { paths } => {
                return self.renderer_checkout_head_paths(paths).await;
            }
            Operation::RendererResetIndex { paths } => {
                return self.renderer_reset_index(paths).await;
            }
            Operation::RendererUpdateIndex {
                paths,
                force_remove,
            } => {
                return self.renderer_update_index(paths, force_remove).await;
            }
            Operation::RendererUnstageAll => {
                return self.renderer_unstage_all().await;
            }
            Operation::RendererRemoveIndexAll => {
                return self.renderer_remove_index_all().await;
            }
            Operation::RendererDeleteInitialHead { expected_oid } => {
                return self.renderer_delete_initial_head(&expected_oid).await;
            }
            Operation::VerifySelectedIndex { tree, head, branch } => {
                return self
                    .verify_selected_index(&tree, head.as_deref(), branch.as_deref())
                    .await;
            }
            Operation::RendererSubmoduleStatus => return self.renderer_submodule_status().await,
            Operation::RendererResetSubmodules { paths } => {
                return self.renderer_reset_submodules(paths).await;
            }
            Operation::RendererUpdateSubmodules {
                allow_file_protocol,
            } => {
                return self.renderer_update_submodules(allow_file_protocol).await;
            }
            Operation::SubmoduleDisplayUrl { path } => {
                return self.submodule_display_url(path).await;
            }
            Operation::SubmoduleUpdate { paths, recursive } => {
                return self
                    .submodule_command("update", paths, recursive, false)
                    .await;
            }
            Operation::SubmoduleSync { paths, recursive } => {
                return self
                    .submodule_command("sync", paths, recursive, false)
                    .await;
            }
            Operation::SubmoduleDeinit { paths, force } => {
                return self.submodule_command("deinit", paths, false, force).await;
            }
            Operation::SubmoduleAdd { url, path, name } => {
                return self.submodule_add(url, path, name).await;
            }

            Operation::GitSettings { scope } => return git_settings(&self.path, scope).await,
            Operation::GitDescription => return self.git_description().await,
            Operation::IndexChanges => return self.index_changes().await,
            Operation::RendererCommit { revision } => return self.renderer_commit(&revision).await,
            Operation::RendererAuthors { revisions } => {
                return self.renderer_authors(&revisions).await;
            }
            Operation::RendererHistory {
                revision,
                limit,
                skip,
                first_parent,
                exclude_remotes,
            } => {
                return self
                    .renderer_history(
                        revision.as_deref(),
                        limit,
                        skip,
                        first_parent,
                        exclude_remotes,
                    )
                    .await;
            }
            Operation::RendererRecentBranches => return self.renderer_reflog(false).await,
            Operation::RendererBranchCheckouts { after_date } => {
                return self.renderer_branch_checkouts(&after_date).await;
            }
            Operation::RendererStashLog => return self.renderer_reflog(true).await,
            Operation::RendererTagRefs => return self.renderer_tag_refs().await,
            Operation::RendererCommitChanges { revision } => {
                return self.renderer_commit_changes(&revision).await;
            }
            Operation::RendererLargeFiles { paths } => {
                return self.renderer_large_files(paths).await;
            }
            Operation::RendererCommitDiff {
                revision,
                path,
                old_path,
                hide_whitespace,
            } => {
                return self
                    .renderer_commit_diff(&revision, path, old_path, hide_whitespace)
                    .await;
            }
            Operation::RendererCompareDiff {
                before,
                after,
                merge_base,
                path,
                old_path,
                hide_whitespace,
                snapshot,
            } => {
                if snapshot {
                    return self
                        .renderer_compare_diff_snapshot(
                            &before,
                            &after,
                            merge_base,
                            path,
                            old_path,
                            hide_whitespace,
                        )
                        .await;
                }
                return self
                    .renderer_compare_diff(
                        &before,
                        &after,
                        merge_base,
                        path,
                        old_path,
                        hide_whitespace,
                    )
                    .await;
            }
            Operation::RendererCompareChanges { before, after, merge_base } => {
                return self.renderer_compare_changes(&before, &after, merge_base).await;
            }
            Operation::RendererResolutionBegin => {
                let permit = comparison_snapshots::reserve().await?;
                return comparison_snapshots::insert_upload(&self.id(), permit);
            }
            Operation::RendererResolutionAppend { snapshot_id, offset, data } => {
                return comparison_snapshots::append_upload(&self.id(), uuid::Uuid::parse_str(&snapshot_id)?, offset, &data);
            }
            Operation::RendererResolutionDiff { snapshot_id, size, path, stage, hide_whitespace } => {
                return self.renderer_resolution_diff(path, stage, hide_whitespace, &snapshot_id, size).await;
            }
            Operation::RendererCompareDiffChunk {
                snapshot_id,
                offset,
                length,
            } => {
                return comparison_snapshots::chunk(
                    &self.id(),
                    uuid::Uuid::parse_str(&snapshot_id)?,
                    offset,
                    length,
                );
            }
            Operation::RendererCompareDiffRelease { snapshot_id } => {
                return comparison_snapshots::release(
                    &self.id(),
                    uuid::Uuid::parse_str(&snapshot_id)?,
                );
            }
            Operation::RendererStagedDiff { amend_head } => {
                return self.renderer_staged_diff(amend_head.as_deref()).await;
            }
            Operation::RendererWorkingFile { path } => return self.renderer_working_file(path).await,
            Operation::RendererWorkingDiff {
                path,
                base,
                hide_whitespace,
                no_textconv,
            } => {
                return self
                    .renderer_working_diff(path, base, hide_whitespace, no_textconv)
                    .await;
            }
            Operation::RendererBlobPrefix {
                revision,
                path,
                length,
            } => {
                return self.renderer_blob_prefix(&revision, path, length).await;
            }
            Operation::RendererBlobChunk {
                revision,
                path,
                offset,
                length,
                expected_oid,
            } => {
                return self
                    .renderer_blob_chunk(&revision, path, offset, length, expected_oid.as_deref())
                    .await;
            }
            Operation::SafeFetchRemotes => return self.safe_fetch_remotes().await,
            Operation::ManageRemote { action } => return self.manage_remote(action).await,
            Operation::SetGitDescription { description } => {
                return self.set_git_description(description).await;
            }
            Operation::WriteCreationFile { kind, contents } => {
                return self.write_creation_file(kind, contents).await;
            }
            Operation::GitIgnore { action } => return self.gitignore(action).await,
            Operation::GitReadSetting {
                key,
                only_local,
                canonical_bool,
            } => {
                return settings::git_read_setting(&self.path, key, only_local, canonical_bool)
                    .await;
            }
            Operation::GitIdentityValue { key, only_local } => {
                return settings::git_identity_value(&self.path, &key, only_local).await;
            }
            Operation::GitEffectiveIdentity { role } => {
                return settings::git_effective_identity(&self.path, role).await;
            }
            Operation::SetGitSetting { scope, key, value } => {
                return set_git_setting(&self.path, scope, &key, value).await;
            }
            Operation::Worktrees => return self.worktrees().await,
            Operation::WorktreeAdd {
                path,
                branch,
                new_branch,
                start,
            } => return self.worktree_add(path, branch, new_branch, start).await,
            Operation::RendererWorktreePreview { action } => return self.renderer_worktree_preview(&action).await,
            Operation::RendererWorktreeApply { action, basis } => return self.renderer_worktree_apply(action, &basis).await,
            Operation::WorktreeRemove { path, force } => {
                return self.worktree_remove(path, force).await;
            }
            Operation::WorktreeMove { from, to } => return self.worktree_move(from, to).await,
            Operation::WorktreeLock {
                path,
                locked,
                reason,
            } => return self.worktree_lock(path, locked, reason).await,

            Operation::CheckoutPreview { branch } => return self.checkout_preview(&branch).await,
            Operation::RendererCheckoutDirect {
                branch,
                basis,
                allow_file_protocol,
            } => {
                return self
                    .checkout_direct_reviewed(&branch, &basis, allow_file_protocol)
                    .await;
            }
            Operation::RendererDetachPreview { commit } => {
                return self.detach_preview(&commit).await;
            }
            Operation::RendererDetach {
                commit,
                basis,
                allow_file_protocol,
            } => {
                return self
                    .detach_reviewed(&commit, &basis, allow_file_protocol)
                    .await;
            }
            Operation::RendererRevertPreview { commit } => {
                return self.renderer_revert_preview(&commit).await;
            }
            Operation::RendererCherryPickContinuePreview { expected } => {
                return self.renderer_cherry_pick_continue_preview(&expected).await;
            }
            Operation::RendererCherryPickStartPreview {
                commits,
                expected_head,
                expected_branch,
            } => {
                return self
                    .renderer_cherry_pick_start_preview(
                        &commits,
                        &expected_head,
                        expected_branch.as_deref(),
                    )
                    .await;
            }
            Operation::RendererCherryPickStart {
                commits,
                expected_head,
                expected_branch,
                basis,
            } => {
                return self
                    .renderer_cherry_pick_start(
                        &commits,
                        &expected_head,
                        expected_branch.as_deref(),
                        &basis,
                    )
                    .await;
            }
            Operation::RendererCherryPickContinue {
                expected,
                basis,
                empty,
            } => {
                return self
                    .renderer_cherry_pick_continue(&expected, &basis, empty)
                    .await;
            }
            Operation::RendererRebaseContinuePreview { expected } => {
                return self.renderer_rebase_continue_preview(&expected).await;
            }
            Operation::RendererRebaseStartPreview { request } => {
                return self.renderer_rebase_start_preview(&request).await;
            }
            Operation::RendererMergeStartPreview { request } => {
                return self.renderer_merge_start_preview(&request).await;
            }
            Operation::RendererMixedResetPreview { request } => {
                return self.renderer_mixed_reset_preview(&request).await;
            }
            Operation::RendererFastForwardPreview { request } => {
                return self.renderer_fast_forward_preview(&request).await;
            }
            Operation::RendererFetchPreview { request } => {
                return self.renderer_fetch_preview(&request).await;
            }
            Operation::RendererFetch { request, basis } => {
                return self.renderer_fetch(&request, &basis).await;
            }
            Operation::RendererRemoteHeadPreview { request } => {
                return self.renderer_remote_head_preview(&request).await;
            }
            Operation::RendererRemoteHead { request, basis } => {
                return self.renderer_remote_head(&request, &basis).await;
            }
            Operation::RendererFastForward { request, basis } => {
                return self.renderer_fast_forward(&request, &basis).await;
            }
            Operation::RendererMixedReset { request, basis } => {
                return self.renderer_mixed_reset(&request, &basis).await;
            }
            Operation::RendererMergeStart { request, basis } => {
                return self.renderer_merge_start(&request, &basis).await;
            }
            Operation::RendererRebaseStart { request, basis } => {
                return self.renderer_rebase_start(&request, &basis).await;
            }
            Operation::RendererRebaseContinue {
                expected,
                basis,
                empty,
                no_verify,
            } => {
                return self
                    .renderer_rebase_continue(&expected, &basis, empty, no_verify)
                    .await;
            }
            Operation::RendererRevert { commit, basis } => {
                return self.renderer_revert(&commit, &basis).await;
            }
            Operation::CheckoutHistory => return self.checkout_history().await,
            Operation::AcknowledgeCheckout { receipt_id } => {
                return self.acknowledge_checkout(&receipt_id).await;
            }
            Operation::Checkout {
                branch,
                strategy,
                basis,
                replace_stash,
                restore_index,
            } => {
                return self
                    .checkout_reviewed(
                        &branch,
                        strategy,
                        basis.as_deref(),
                        replace_stash,
                        restore_index,
                    )
                    .await;
            }
            Operation::StashList => return self.stash_list().await,
            Operation::RendererStashChanges { oid } => {
                return self.renderer_stash_changes(&oid).await;
            }
            Operation::RendererStashSave { branch } => {
                return self.renderer_stash_save(&branch).await;
            }
            Operation::RendererStashPop { oid, index, basis } => {
                return self.renderer_stash_action(&oid, index, &basis, true).await;
            }
            Operation::RendererStashDrop { oid, index, basis } => {
                return self.renderer_stash_action(&oid, index, &basis, false).await;
            }
            Operation::StashSave {
                message,
                include_untracked,
                desktop,
            } => return self.stash_save(message, include_untracked, desktop).await,
            Operation::StashFiles { oid } => return self.stash_files(&oid).await,
            Operation::StashApply {
                oid,
                index,
                basis,
                restore_index,
                pop,
            } => {
                return self
                    .stash_apply(&oid, index, basis.as_deref(), restore_index, pop)
                    .await;
            }
            Operation::StashDrop { oid, index, basis } => {
                return self.stash_drop(&oid, index, &basis).await;
            }
            Operation::StashMove {
                oid,
                index,
                basis,
                branch,
            } => return self.stash_move(&oid, index, &basis, &branch).await,
            Operation::History {
                revision,
                offset,
                limit,
            } => return self.history(revision, offset, limit).await,
            Operation::CommitFiles { commit, parent } => {
                return self.commit_files(&commit, parent).await;
            }
            Operation::CommitDiff {
                path,
                commit,
                parent,
            } => return self.commit_file_diff(path, &commit, parent).await,
            Operation::CreateBranchPreview {
                name,
                start_point,
                no_track,
                checkout,
            } => {
                return self
                    .create_branch_preview(&name, start_point, no_track, checkout)
                    .await;
            }
            Operation::CreateBranch {
                name,
                start_point,
                no_track,
                checkout,
                strategy,
                basis,
                replace_stash,
                restore_index,
            } => {
                return self
                    .create_branch_reviewed(
                        &name,
                        stash::checkout::CreationRequest {
                            start_point,
                            no_track,
                            track_direct: false,
                            checkout,
                        },
                        strategy,
                        basis.as_deref(),
                        replace_stash,
                        restore_index,
                    )
                    .await;
            }
            Operation::DeleteBranchPreview {
                name,
                to_checkout,
                upstream,
            } => {
                return self
                    .delete_branch_preview(&name, to_checkout.as_deref(), upstream.as_ref())
                    .await;
            }
            Operation::DeleteBranch {
                name,
                to_checkout,
                ref_only,
                force,
                strategy,
                basis,
                replace_stash,
                restore_index,
                upstream,
            } => {
                return self
                    .delete_branch_reviewed(
                        &name,
                        stash::checkout::DeletionRequest {
                            to_checkout,
                            ref_only,
                            force,
                            strategy,
                            basis,
                            replace_stash,
                            restore_index,
                            upstream,
                        },
                    )
                    .await;
            }
            Operation::RemoteDeletionTargets => return self.remote_deletion_targets().await,
            Operation::RemoteBranchDeletePreview { request } => {
                return self.remote_branch_delete_preview(request).await;
            }
            Operation::DeleteRemoteBranch { request, basis } => {
                return self.delete_remote_branch(request, basis.as_deref()).await;
            }
            Operation::RemoteDeletionHistory => return self.remote_deletion_history().await,
            Operation::RestoreRemoteTip {
                receipt_id,
                endpoint,
                name,
            } => return self.restore_remote_tip(&receipt_id, endpoint, &name).await,
            Operation::ForgetRemoteDeletion { receipt_id } => {
                return self.forget_remote_deletion(&receipt_id).await;
            }
            Operation::DeletedBranches => return self.deleted_branches().await,
            Operation::RestoreDeletedBranch { receipt_id, name } => {
                return self
                    .restore_deleted_branch(&receipt_id, name.as_deref())
                    .await;
            }
            Operation::ForgetDeletedBranch { receipt_id } => {
                return self.forget_deleted_branch(&receipt_id).await;
            }
            Operation::BranchList => return self.branch_list_with_policy().await,
            Operation::StatusContext => return self.status_context().await,
            Operation::RendererCherryPickState => return self.renderer_cherry_pick_state().await,
            Operation::RendererRebaseState => return self.renderer_rebase_state().await,
            Operation::StatusConflictDetails { kind } => {
                return self.status_conflict_details(kind).await;
            }
            Operation::BranchEnumeration { prefixes } => {
                return self.branch_enumeration(&prefixes).await;
            }
            Operation::BranchMerged { branch } => return self.branches_merged(&branch).await,
            Operation::BranchPointsAt { commitish } => {
                return self.branches_pointing_at(&commitish).await;
            }
            Operation::AheadBehind { range } => return self.ahead_behind(&range).await,
            Operation::RevisionList { range } => return self.revision_list(&range).await,
            Operation::HasMergeCommits { range } => return self.has_merge_commits(&range).await,
            Operation::UpstreamSetupPreview => return self.upstream_setup_preview().await,
            Operation::SetupUpstream { basis } => {
                return self.setup_upstream(basis.as_deref()).await;
            }
            Operation::PublishContext { host } => return self.publish_context(&host).await,
            Operation::PublishPreview { request } => return self.publish_preview(&request).await,
            Operation::PublishCreate {
                request,
                basis,
                acknowledge_partial,
            } => {
                return self
                    .publish_create(&request, &basis, acknowledge_partial)
                    .await;
            }
            Operation::PublishContinuePreview { receipt_id } => {
                return self.publish_continue_preview(&receipt_id).await;
            }
            Operation::PublishContinue {
                receipt_id,
                basis,
                acknowledge_partial,
            } => {
                return self
                    .publish_continue(&receipt_id, &basis, acknowledge_partial)
                    .await;
            }
            Operation::PublishHistory {} => return self.publish_history().await,
            Operation::AcknowledgePublish { receipt_id } => {
                return self.acknowledge_publish(&receipt_id).await;
            }
            Operation::PublishOrganizations { host, target } => {
                return self.publish_organizations(&host, target).await;
            }
            Operation::RepositoryAccount => return self.repository_account().await,
            Operation::RepositoryAccountCatalog => return self.repository_account_catalog().await,
            Operation::RepositoryAccountPreview { target } => {
                return self.repository_account_preview(target).await;
            }
            Operation::SetRepositoryAccount { target, basis } => {
                return self.set_repository_account(target, &basis).await;
            }
            Operation::RepositoryContext => return self.repository_context().await,
            Operation::RefreshRepositoryContext => return self.refresh_repository_context().await,
            Operation::SetForkContributionTarget { target, basis } => {
                return self
                    .set_fork_contribution_target(target, basis.as_deref())
                    .await;
            }
            Operation::BranchNameRules { name } => {
                return Ok(serde_json::to_value(self.branch_name_rules(&name).await?)?);
            }
            Operation::BranchPolicy => {
                return Ok(serde_json::to_value(self.branch_policy().await?)?);
            }
            Operation::RenameBranchPreview { name, new_name } => {
                return self.rename_branch_preview(&name, &new_name).await;
            }
            Operation::RenameBranch {
                name,
                new_name,
                basis,
            } => return self.rename_branch(&name, &new_name, basis.as_deref()).await,
            Operation::BranchRenameHistory => return self.branch_rename_history().await,
            Operation::AcknowledgeBranchRename { receipt_id } => {
                return self.acknowledge_branch_rename(&receipt_id).await;
            }
            Operation::ConflictDetail { path } => return self.conflict_detail(path).await,
            Operation::StageConflict {
                path,
                basis,
                allow_markers,
            } => return self.stage_conflict(path, &basis, allow_markers).await,
            Operation::ResolveConflict {
                path,
                choice,
                basis,
            } => return self.resolve_conflict(path, choice, &basis).await,

            Operation::DiscardPreview {
                path,
                scope,
                selection,
            } => return Box::pin(self.discard_preview(path, scope, selection)).await,
            Operation::DiscardBatchPreview { paths } => {
                return Box::pin(self.discard_batch_preview(paths)).await;
            }
            Operation::DiscardBatch { paths, basis } => {
                return Box::pin(self.discard_batch(paths, &basis)).await;
            }
            Operation::Discard {
                path,
                scope,
                selection,
                basis,
            } => return Box::pin(self.discard(path, scope, selection, &basis)).await,
            Operation::ForgetDiscard { undo_id } => return self.forget_discard(&undo_id).await,
            Operation::ListDiscards => return self.list_discards().await,
            Operation::UndoDiscard { undo_id } => {
                return Box::pin(self.undo_discard(&undo_id)).await;
            }
            Operation::DiffDetail { path, staged } => return self.diff_detail(path, staged).await,
            Operation::SelectDiff {
                path,
                staged,
                basis,
                lines,
            } => return self.select_diff(path, staged, &basis, lines).await,
            Operation::SelectRenamedDiff {
                path,
                original_path,
                basis,
                lines,
                preserve_source,
            } => {
                return Box::pin(self.select_renamed_diff(
                    path,
                    original_path,
                    &basis,
                    lines,
                    preserve_source,
                ))
                .await;
            }
            Operation::CopilotProbe { runtime_root } => {
                return self.copilot_probe(runtime_root.as_deref()).await;
            }
            Operation::CopilotCreateSession {
                model,
                reasoning_effort,
                account,
            } => {
                return self
                    .copilot_create_session(model, reasoning_effort, account)
                    .await;
            }
            Operation::CopilotSendMessage {
                session_id,
                message,
                timeout_ms,
            } => {
                return self
                    .copilot_send_message(&session_id, &message, timeout_ms)
                    .await;
            }
            Operation::CopilotCloseSession { session_id } => {
                return self.copilot_close_session(&session_id).await;
            }
            Operation::CopilotCommitDiffDetail {
                path,
                original_path,
            } => {
                return self.copilot_commit_diff_detail(path, original_path).await;
            }
            Operation::CopilotGenerateCommitMessage {
                paths,
                remove_paths,
                system_prompt,
                source,
                partial_selections,
                amend_head,
                rule_descriptions,
                account,
                model,
                reasoning_effort,
                timeout_ms,
                provider,
                expected_head,
            } => {
                return self
                    .copilot_generate_commit(
                        paths,
                        system_prompt,
                        source,
                        partial_selections,
                        amend_head,
                        copilot_bridge::CopilotCommitOptions {
                            remove_paths,
                            provider: provider.map(|value| *value),
                            expected_head: expected_head.map(|value| *value),
                            model,
                            reasoning_effort,
                            timeout_ms,
                            rule_descriptions,
                            account: account.map(|value| *value),
                        },
                    )
                    .await;
            }
            _ => {}
        }
        let command = self.command(operation).await?;
        Ok(serde_json::to_value(self.run(&command).await?.text())?)
    }

    async fn run(&self, command: &[OsString]) -> Result<process::RawOutput> {
        process::run(&self.path, command).await
    }

    async fn command(&self, operation: Operation) -> Result<Vec<OsString>> {
        use Operation::*;
        let command = match operation {
            RenameBranch { .. }
            | RenameBranchPreview { .. }
            | BranchRenameHistory
            | AcknowledgeBranchRename { .. } => unreachable!("handled by execute"),
            TrackBranch { name, upstream } => {
                self.validate_branch(&name).await?;
                self.resolve_commit(&upstream).await?;
                let full = self
                    .run(&args(&[
                        "rev-parse",
                        "--symbolic-full-name",
                        "--verify",
                        "--end-of-options",
                        &upstream,
                    ]))
                    .await?;
                let reference = String::from_utf8(full.stdout)?.trim().to_owned();
                if full.code != 0 || !reference.starts_with("refs/") {
                    bail!("upstream must name a branch ref");
                }
                args(&[
                    "branch",
                    &format!("--set-upstream-to={reference}"),
                    "--",
                    &name,
                ])
            }
            Detach { commit } => {
                args(&["switch", "--detach", &self.resolve_commit(&commit).await?])
            }
            Recover { kind, action } => {
                let expected = match kind {
                    RecoveryKind::Merge => "merge",
                    RecoveryKind::Rebase => "rebase",
                    RecoveryKind::CherryPick => "cherry_pick",
                    RecoveryKind::Revert => "revert",
                    RecoveryKind::Am => "am",
                };
                if self
                    .operation_state()
                    .await?
                    .is_none_or(|state| state.kind != expected)
                {
                    bail!("the requested Git operation is not in progress");
                }
                if matches!(kind, RecoveryKind::Merge) && matches!(action, RecoveryAction::Skip) {
                    bail!("merge has no skip action");
                }
                let command = if expected == "cherry_pick" {
                    "cherry-pick"
                } else {
                    expected
                };
                let flag = match action {
                    RecoveryAction::Continue => "--continue",
                    RecoveryAction::Abort => "--abort",
                    RecoveryAction::Skip => "--skip",
                    RecoveryAction::Quit => "--quit",
                };
                args(&[command, flag])
            }
            WorktreePrune { dry_run } => {
                let mut command = args(&["worktree", "prune", "--verbose"]);
                if dry_run {
                    command.push("--dry-run".into());
                }
                command
            }
            GitSettings { .. }
            | GitIdentityValue { .. }
            | GitEffectiveIdentity { .. }
            | GitDescription
            | IndexChanges
            | RendererCommit { .. }
            | RendererAuthors { .. }
            | RendererCompareChanges { .. }
            | RendererResolutionBegin
            | RendererResolutionAppend { .. }
            | RendererResolutionDiff { .. }
            | RendererHistory { .. }
            | RendererRecentBranches
            | RendererBranchCheckouts { .. }
            | RendererStashLog
            | RendererTagRefs
            | RendererCommitChanges { .. }
            | RendererLargeFiles { .. }
            | MergeBase { .. }
            | MergeTreePreview { .. }
            | InterpretTrailers { .. }
            | RendererCommitDiff { .. }
            | RendererCompareDiff { .. }
            | RendererCompareDiffChunk { .. }
            | RendererCompareDiffRelease { .. }
            | RendererStagedDiff { .. }
            | RendererWorkingDiff { .. }
            | RendererWorkingFile { .. }
            | RendererBlobPrefix { .. }
            | RendererBlobChunk { .. }
            | SafeFetchRemotes
            | ManageRemote { .. }
            | SetGitDescription { .. }
            | WriteCreationFile { .. }
            | GitIgnore { .. }
            | GitReadSetting { .. }
            | SetGitSetting { .. }
            | Worktrees
            | WorktreeAdd { .. }
            | RendererWorktreePreview { .. }
            | RendererWorktreeApply { .. }
            | WorktreeRemove { .. }
            | WorktreeMove { .. }
            | WorktreeLock { .. } => unreachable!("handled by execute"),
            Submodules
            | RendererCheckoutIndex { .. }
            | RendererCheckoutHeadPaths { .. }
            | RendererResetIndex { .. }
            | RendererUpdateIndex { .. }
            | RendererUnstageAll
            | RendererRemoveIndexAll
            | RendererDeleteInitialHead { .. }
            | VerifySelectedIndex { .. }
            | RendererSubmoduleStatus
            | RendererResetSubmodules { .. }
            | RendererUpdateSubmodules { .. }
            | SubmoduleDisplayUrl { .. }
            | SubmoduleUpdate { .. }
            | SubmoduleSync { .. }
            | SubmoduleDeinit { .. }
            | SubmoduleAdd { .. } => unreachable!("handled by execute"),
            Status => args(&[
                "status",
                "--porcelain=v1",
                "--branch",
                "--untracked-files=normal",
            ]),
            LfsTransfer { .. }
            | LfsCheckout { .. }
            | LfsTrack { .. }
            | LfsUntrack { .. }
            | LfsToolStatus
            | LfsStatus { .. }
            | LfsFileTracking { .. }
            | LfsInstall { .. } => unreachable!("handled by execute"),
            Snapshot
            | StatusDetailed { .. }
            | Github { .. }
            | DiffDetail { .. }
            | SelectDiff { .. }
            | SelectRenamedDiff { .. }
            | DiscardPreview { .. }
            | DiscardBatchPreview { .. }
            | DiscardBatch { .. }
            | Discard { .. }
            | ListDiscards
            | ForgetDiscard { .. }
            | UndoDiscard { .. }
            | History { .. }
            | CommitFiles { .. }
            | CommitDiff { .. }
            | BranchList
            | StatusContext
            | RendererCherryPickState
            | RendererRebaseState
            | StatusConflictDetails { .. }
            | BranchEnumeration { .. }
            | BranchMerged { .. }
            | BranchPointsAt { .. }
            | AheadBehind { .. }
            | RevisionList { .. }
            | HasMergeCommits { .. }
            | BranchPolicy
            | BranchNameRules { .. }
            | RepositoryContext
            | PublishContext { .. }
            | PublishPreview { .. }
            | PublishCreate { .. }
            | PublishContinuePreview { .. }
            | PublishContinue { .. }
            | PublishHistory {}
            | AcknowledgePublish { .. }
            | PublishOrganizations { .. }
            | RepositoryAccount
            | RepositoryAccountCatalog
            | RepositoryAccountPreview { .. }
            | SetRepositoryAccount { .. }
            | UpstreamSetupPreview
            | SetupUpstream { .. }
            | RefreshRepositoryContext
            | SetForkContributionTarget { .. }
            | ConflictDetail { .. }
            | StageConflict { .. }
            | ResolveConflict { .. } => {
                unreachable!("handled by execute")
            }
            Log => args(&[
                "log",
                "-30",
                "--oneline",
                "--decorate",
                "--no-show-signature",
            ]),
            Branches => args(&["branch", "--list", "--no-color"]),
            Diff { staged, path } => {
                let mut command = args(&["diff", "--no-ext-diff", "--no-textconv", "--no-color"]);
                if staged {
                    command.push("--cached".into());
                }
                command.push("--".into());
                if let Some(path) = path {
                    command.push(validate_path(path)?);
                }
                command
            }
            Stage { paths } => path_command(&["add", "--"], paths)?,
            Unstage { paths } => {
                // reset HEAD does not work before the first commit.
                let head = self.run(&args(&["rev-parse", "--verify", "HEAD"])).await?;
                if head.code == 0 {
                    path_command(&["reset", "--quiet", "HEAD", "--"], paths)?
                } else {
                    path_command(&["rm", "--cached", "--force", "--quiet", "--"], paths)?
                }
            }
            Commit { .. }
            | CommitPreview
            | SelectedCommitPreview { .. }
            | PrepareSelectedCommit { .. }
            | PublishSelectedCommit { .. }
            | ListCommitCheckpoints
            | InspectCommitCheckpoint { .. }
            | RevalidateCommitCheckpoint { .. }
            | RecoverPublishedCommitCheckpoint { .. }
            | CloseUnchangedCommitCheckpoint { .. }
            | ForgetCommitCheckpoint { .. }
            | InitialCommit { .. }
            | RendererFinishMergeCommit
            | RendererAbortSquash { .. } => {
                unreachable!("handled by execute")
            }
            Checkout { .. }
            | CheckoutPreview { .. }
            | RendererCheckoutDirect { .. }
            | RendererDetachPreview { .. }
            | RendererDetach { .. }
            | RendererRevertPreview { .. }
            | RendererRevert { .. }
            | RendererCherryPickContinuePreview { .. }
            | RendererCherryPickStartPreview { .. }
            | RendererCherryPickStart { .. }
            | RendererCherryPickContinue { .. }
            | RendererRebaseContinuePreview { .. }
            | RendererRebaseStartPreview { .. }
            | RendererMergeStartPreview { .. }
            | RendererMixedResetPreview { .. }
            | RendererFastForwardPreview { .. }
            | RendererFetchPreview { .. }
            | RendererFetch { .. }
            | RendererRemoteHeadPreview { .. }
            | RendererRemoteHead { .. }
            | RendererFastForward { .. }
            | RendererMixedReset { .. }
            | RendererMergeStart { .. }
            | RendererRebaseStart { .. }
            | RendererRebaseContinue { .. }
            | CheckoutHistory
            | AcknowledgeCheckout { .. } => unreachable!("handled by execute"),
            CreateBranch { .. } | CreateBranchPreview { .. } => unreachable!("handled by execute"),
            RemoteDeletionTargets
            | RemoteBranchDeletePreview { .. }
            | DeleteRemoteBranch { .. }
            | RemoteDeletionHistory
            | RestoreRemoteTip { .. }
            | ForgetRemoteDeletion { .. } => unreachable!("handled by execute"),
            DeleteBranch { .. }
            | DeleteBranchPreview { .. }
            | DeletedBranches
            | RestoreDeletedBranch { .. }
            | ForgetDeletedBranch { .. } => unreachable!("handled by execute"),
            Fetch { remote } => {
                validate_remote(&remote)?;
                args(&["fetch", "--", &remote])
            }
            Pull { remote, branch } => {
                validate_remote(&remote)?;
                self.validate_branch(&branch).await?;
                args(&["pull", "--ff-only", "--", &remote, &branch])
            }
            Push {
                remote,
                branch,
                set_upstream,
            } => {
                validate_remote(&remote)?;
                self.validate_branch(&branch).await?;
                let mut command = args(&["push"]);
                if set_upstream {
                    command.push("--set-upstream".into());
                }
                command.extend(args(&[
                    "--",
                    &remote,
                    &format!("refs/heads/{branch}:refs/heads/{branch}"),
                ]));
                command
            }
            Merge { branch } => args(&["merge", "--no-edit", &self.resolve_commit(&branch).await?]),
            MergeAbort => args(&["merge", "--abort"]),
            Rebase { branch } => args(&["rebase", &self.resolve_commit(&branch).await?]),
            RebaseAbort => args(&["rebase", "--abort"]),
            RebaseContinue => args(&["rebase", "--continue"]),
            CherryPick { commit, mainline } => {
                self.replay_command("cherry-pick", &commit, mainline)
                    .await?
            }
            Revert { commit, mainline } => self.replay_command("revert", &commit, mainline).await?,
            StashSave { .. }
            | StashList
            | RendererStashChanges { .. }
            | RendererStashSave { .. }
            | RendererStashPop { .. }
            | RendererStashDrop { .. }
            | StashFiles { .. }
            | StashApply { .. }
            | StashDrop { .. }
            | StashMove { .. } => unreachable!("handled by execute"),
            Tags => args(&["tag", "--list"]),
            TagCreate { name, target } => {
                self.validate_tag(&name).await?;
                let target = self
                    .resolve_commit(target.as_deref().unwrap_or("HEAD"))
                    .await?;
                args(&["tag", "--", &name, &target])
            }
            TagCreateAnnotated { name, target } => {
                self.validate_tag(&name).await?;
                anyhow::ensure!(
                    !target.is_empty() && target.len() <= 4096 && !target.contains('\0'),
                    "invalid target commit"
                );
                let target = self.resolve_commit(&target).await?;
                args(&["tag", "-a", "-m", "", "--", &name, &target])
            }
            TagDelete { name } => {
                self.validate_tag(&name).await?;
                args(&["tag", "-d", "--", &name])
            }
            Remotes => args(&["remote", "-v"]),
            AddRemote { name, url } => {
                validate_remote(&name)?;
                if url.is_empty()
                    || url.starts_with('-')
                    || url.contains(['\0', '\n', '\r'])
                    || url.starts_with("ext::")
                {
                    bail!("invalid remote URL");
                }
                args(&["remote", "add", "--", &name, &url])
            }
            RemoveRemote { name } => {
                validate_remote(&name)?;
                args(&["remote", "remove", "--", &name])
            }
            CopilotProbe { .. }
            | CopilotCreateSession { .. }
            | CopilotSendMessage { .. }
            | CopilotCloseSession { .. }
            | CopilotCommitDiffDetail { .. }
            | CopilotGenerateCommitMessage { .. } => {
                unreachable!("handled by execute")
            }
        };
        Ok(command)
    }

    async fn replay_command(
        &self,
        command: &str,
        commit: &str,
        mainline: Option<u32>,
    ) -> Result<Vec<OsString>> {
        let mut values = args(&[command]);
        if command == "revert" {
            values.push("--no-edit".into());
        }
        if let Some(mainline) = mainline {
            if mainline == 0 {
                bail!("mainline is one-based");
            }
            values.extend(args(&["--mainline", &mainline.to_string()]));
        }
        values.push(self.resolve_commit(commit).await?.into());
        Ok(values)
    }

    async fn validate_branch(&self, name: &str) -> Result<()> {
        if name.starts_with('-') || name.contains("@{") {
            bail!("invalid branch name");
        }
        let output = self
            .run(&args(&["check-ref-format", "--branch", name]))
            .await?;
        if output.code != 0 {
            bail!("invalid branch name");
        }
        Ok(())
    }

    async fn validate_tag(&self, name: &str) -> Result<()> {
        if name.starts_with('-') {
            bail!("invalid tag name");
        }
        let output = self
            .run(&args(&["check-ref-format", &format!("refs/tags/{name}")]))
            .await?;
        if output.code != 0 {
            bail!("invalid tag name");
        }
        Ok(())
    }

    async fn resolve_commit(&self, revision: &str) -> Result<String> {
        let output = self
            .run(&args(&[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{revision}^{{commit}}"),
            ]))
            .await?;
        if output.code != 0 {
            bail!("revision does not resolve to a commit");
        }
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    }
}

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn validate_remote(remote: &str) -> Result<()> {
    if remote.is_empty()
        || remote.starts_with(['-', '.'])
        || !remote
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        bail!("invalid configured remote name");
    }
    Ok(())
}

fn path_command(prefix: &[&str], paths: Vec<GitPath>) -> Result<Vec<OsString>> {
    if paths.is_empty() {
        bail!("select at least one file");
    }
    let mut command = args(prefix);
    for path in paths {
        command.push(validate_path(path)?);
    }
    Ok(command)
}

fn validate_path(path: GitPath) -> Result<OsString> {
    let value = match path {
        GitPath::Text(text) => OsString::from(text),
        GitPath::Bytes { bytes } => os_from_bytes(&bytes)?,
    };
    let path = Path::new(&value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("path must be a literal repository-relative file path");
    }
    // NUL cannot be passed to an operating system command.
    if value.as_encoded_bytes().contains(&0) {
        bail!("path contains NUL");
    }
    Ok(value)
}

fn os_from_bytes(bytes: &[u8]) -> Result<OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(OsString::from_vec(bytes.to_vec()))
    }
    #[cfg(not(unix))]
    {
        Ok(OsString::from(std::str::from_utf8(bytes)?))
    }
}

#[cfg(test)]
mod future_size_tests {
    use super::*;
    #[test]
    fn repository_operation_future_fits_normal_process_stacks() {
        let repository = Repository {
            path: PathBuf::from("."),
            gate: Arc::new(Mutex::new(())),
            saved_scope: None,
            copilot_runtime_root: None,
            native_copilot_sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
            native_copilot_slots: Arc::new(tokio::sync::Semaphore::new(
                copilot_bridge::MAX_NATIVE_SESSIONS,
            )),
        };
        let future = repository.execute(Operation::Status);
        let bytes = std::mem::size_of_val(&future);
        assert!(
            bytes <= 256 * 1024,
            "Repository operation future grew to {bytes} bytes; box large workflow boundaries before composing it into CLI/bridge futures"
        );
    }
}
