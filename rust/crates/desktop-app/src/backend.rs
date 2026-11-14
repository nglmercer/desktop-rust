//! Typed, repository-scoped operations exposed by the Wry host.
//!
//! Cancellable reads and explicit repository-trust requests.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use desktop_backend::{Operation, Repository, cancellation::Cancellation};
use desktop_core::error::{AppError, Result};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadRequest {
    repository_path: String,
    method: ReadMethod,
    #[serde(default)]
    include_untracked: Option<bool>,
    #[serde(default)]
    prefixes: Option<Vec<String>>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    commitish: Option<String>,
    #[serde(default)]
    revisions: Option<Vec<String>>,
    #[serde(default)]
    after_date: Option<String>,
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    skip: Option<u64>,
    #[serde(default)]
    first_parent: Option<bool>,
    #[serde(default)]
    exclude_remotes: Option<bool>,
    #[serde(default)]
    range: Option<String>,
    #[serde(default)]
    kind: Option<desktop_backend::StatusConflictKind>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    only_local: Option<bool>,
    #[serde(default)]
    canonical_bool: Option<bool>,
    #[serde(default)]
    paths: Option<Vec<desktop_backend::GitPath>>,
    #[serde(default)]
    submodule_path: Option<desktop_backend::GitPath>,
    #[serde(default)]
    remote_name: Option<String>,
    #[serde(default)]
    reference: Option<String>,
    #[serde(default)]
    request_id: Option<uuid::Uuid>,
    #[serde(default)]
    deadline_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CopilotRequest {
    repository_path: String,
    operation: Operation,
    request_id: Option<uuid::Uuid>,
    deadline_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitInfoRequest {
    operation: GitInfoOperation,
    request_id: Option<uuid::Uuid>,
    deadline_ms: Option<u64>,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum GitInfoOperation {
    Read {},
    Version {},
    GlobalConfigPath {},
    AvailableEditors {},
    AvailableTerminals {},
    OpenTerminal {
        repository_path: String,
        name: String,
        directory: String,
    },
    OpenGlobalGitConfigEditor {
        name: String,
    },
    OpenNamedEditor {
        repository_path: String,
        path: Option<desktop_backend::GitPath>,
        name: String,
    },
    CloneSelected {
        url: String,
        path: desktop_backend::GitPath,
        default_branch: Option<String>,
        account: desktop_backend::CloneAccount,
    },
    ListCloneRecoveries {},
    RecoverClone {
        recovery_id: String,
    },
    ReleaseComparisonSnapshot {
        snapshot_id: String,
    },
    LfsStatus {},
    LfsInstall {
        force: bool,
    },
    LfsInstallRepository {
        repository_path: String,
        force: bool,
    },
    SetDescription {
        repository_path: String,
        description: String,
    },
    WriteCreationFile {
        repository_path: String,
        kind: desktop_backend::CreationFile,
        contents: String,
    },
    GitIgnore {
        repository_path: String,
        action: desktop_backend::GitIgnoreAction,
    },
    ReviewedDiscard {
        repository_path: String,
        operation: Operation,
    },
    BranchCreation {
        repository_path: String,
        operation: Operation,
    },
    BranchDeletion {
        repository_path: String,
        operation: Operation,
    },
    RemoteBranchDeletion {
        repository_path: String,
        operation: Operation,
    },
    BranchRename {
        repository_path: String,
        operation: Operation,
    },
    StashWorkflow {
        repository_path: String,
        operation: Operation,
    },
    BranchCheckout {
        repository_path: String,
        operation: Operation,
    },
    RevertCommit {
        repository_path: String,
        operation: Operation,
    },
    AbortGitOperation {
        repository_path: String,
        operation: Operation,
    },
    FinishMergeCommit {
        repository_path: String,
        operation: Operation,
    },
    ContinueCherryPick {
        repository_path: String,
        operation: Operation,
    },
    StartCherryPick {
        repository_path: String,
        operation: Operation,
    },
    ContinueRebase {
        repository_path: String,
        operation: Operation,
    },
    StartRebase {
        repository_path: String,
        operation: Operation,
    },
    StartMerge {
        repository_path: String,
        operation: Operation,
    },
    MixedReset {
        repository_path: String,
        operation: Operation,
    },
    FastForwardBranches {
        repository_path: String,
        operation: Operation,
    },
    FetchRemote {
        repository_path: String,
        operation: Operation,
    },
    WorktreeMutation {
        repository_path: String,
        operation: Operation,
    },
    SelectedCommitPreview {
        repository_path: String,
        operation: Operation,
    },
    CommitCheckpoint {
        repository_path: String,
        operation: Operation,
    },
    InterpretTrailers {
        repository_path: String,
        operation: Operation,
    },
    MergeTreePreview {
        repository_path: String,
        operation: Operation,
    },
    MergeBase {
        repository_path: String,
        operation: Operation,
    },
    SubmoduleStatus {
        repository_path: String,
        operation: Operation,
    },
    CheckoutIndex {
        repository_path: String,
        operation: Operation,
    },
    ResetIndex {
        repository_path: String,
        operation: Operation,
    },
    UpdateIndex {
        repository_path: String,
        operation: Operation,
    },
    DeleteInitialHead {
        repository_path: String,
        operation: Operation,
    },
    ResetSubmodules {
        repository_path: String,
        operation: Operation,
    },
    UpdateSubmodules {
        repository_path: String,
        operation: Operation,
    },
    TagMutation {
        repository_path: String,
        operation: Operation,
    },
    InitialCommit {
        repository_path: String,
        paths: Vec<desktop_backend::GitPath>,
    },
    ManageRemote {
        repository_path: String,
        action: desktop_backend::ManagedRemoteAction,
    },
    InitializeRepository {
        repository_path: String,
        default_branch: String,
    },
    Set {
        key: GlobalGitKey,
        value: String,
    },
    SetRepository {
        repository_path: String,
        key: GitIdentityKey,
        value: String,
    },
    UnsetRepository {
        repository_path: String,
        key: GitIdentityKey,
    },
}

#[derive(Clone, Deserialize)]
enum GlobalGitKey {
    #[serde(rename = "user.name")]
    Name,
    #[serde(rename = "user.email")]
    Email,
    #[serde(rename = "init.defaultbranch")]
    DefaultBranch,
}

impl GlobalGitKey {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Name => "user.name",
            Self::Email => "user.email",
            Self::DefaultBranch => "init.defaultbranch",
        }
    }
}

#[derive(Clone, Deserialize)]
enum GitIdentityKey {
    #[serde(rename = "user.name")]
    Name,
    #[serde(rename = "user.email")]
    Email,
}

impl GitIdentityKey {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Name => "user.name",
            Self::Email => "user.email",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReadMethod {
    RepositoryType,
    DirectoryExists,
    PathExists,
    CanonicalMutationPath,
    TrustRepository,
    Snapshot,
    Status,
    StatusDetailed,
    IndexChanges,
    RendererCommit,
    RendererAuthors,
    RendererHistory,
    RendererRecentBranches,
    RendererBranchCheckouts,
    RendererStashLog,
    RendererTagRefs,
    RendererCommitChanges,
    RendererLargeFiles,
    Worktrees,
    WorktreesFromGitDir,
    ProtectedResources,
    StatusContext,
    RendererLastFetch,
    RendererCherryPickState,
    RendererRebaseState,
    StatusConflictDetails,
    BranchEnumeration,
    BranchMerged,
    BranchPointsAt,
    AheadBehind,
    RevisionList,
    HasMergeCommits,
    GitIdentityValue,
    GitAuthorIdentity,
    GitCommitterIdentity,
    GitDescription,
    SafeFetchRemotes,
    SafeRemoteUrl,
    SymbolicRef,
    SubmoduleDisplayUrl,
    GitReadSetting,
    LfsStatus,
    LfsFileTracking,
}

pub struct Backend {
    runtime: tokio::runtime::Runtime,
    jobs: Arc<Mutex<HashMap<uuid::Uuid, Cancellation>>>,
    capacity: usize,
    registry: Option<desktop_backend::Registry>,
    copilot_runtime_root: Option<PathBuf>,
    comparison_owner: Arc<desktop_backend::comparison_snapshots::Owner>,
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.comparison_owner.close();
    }
}

pub struct ReadJob {
    path: String,
    operation: BackendOperation,
    token: Cancellation,
    id: Option<uuid::Uuid>,
    jobs: Arc<Mutex<HashMap<uuid::Uuid, Cancellation>>>,
    pub deadline: Instant,
}

#[derive(Clone)]
enum BackendOperation {
    RepositoryType,
    DirectoryExists,
    PathExists,
    CanonicalMutationPath,
    TrustRepository,
    SafeFetchRemotes,
    Worktrees,
    WorktreesFromGitDir,
    ProtectedResources,
    SafeRemoteUrl(String),
    SymbolicRef(String),
    GitInfo(GitInfoOperation),
    Repository(Operation),
    Copilot(Operation),
}

impl Drop for ReadJob {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            self.jobs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
        }
    }
}

fn retained_clone_path(path: &Path) -> Value {
    let value = match path.to_str() {
        Some(text) => desktop_backend::GitPath::Text(text.to_owned()),
        None => desktop_backend::GitPath::Bytes {
            bytes: path.as_os_str().as_encoded_bytes().to_vec(),
        },
    };
    serde_json::json!(value)
}

impl Backend {
    async fn clone_selected(
        &self,
        url: String,
        path: desktop_backend::GitPath,
        default_branch: Option<String>,
        account: desktop_backend::CloneAccount,
    ) -> Result<Value> {
        let registry = self
            .registry
            .clone()
            .ok_or_else(|| AppError::msg("Clone registry is unavailable"))?;
        let destination = desktop_backend::directory_path(path)
            .map_err(|error| AppError::msg(error.to_string()))?;
        let assigned = desktop_backend::AccountAssignment::Assigned {
            host: account.host.to_ascii_lowercase(),
            login: account.login.clone(),
            id: account.id,
        };
        let repo = desktop_backend::clone_repository_selected(
            registry.clone(),
            &url,
            &destination,
            desktop_backend::CloneOptions {
                branch: None,
                default_branch,
                recursive: false,
                report_progress: true,
            },
            account,
        )
        .await
        .map_err(|error| AppError::msg(error.to_string()))?;
        let finish: Result<Value> = async {
            let account_view = repo.execute(Operation::RepositoryAccount).await.map_err(|error| AppError::msg(error.to_string()))?;
            let bound = account_view["scope_id"].as_str().ok_or_else(|| AppError::msg("Clone account binding is unavailable"))?;
            let record_root = repo.path().to_owned();
            let record = desktop_backend::cancellation::blocking(move || registry.record_for_path(&record_root)).await.map_err(|error| AppError::msg(error.to_string()))?
                .ok_or_else(|| AppError::msg("Clone account is unavailable"))?;
            if record.id != bound || record.account.assignment != assigned {
                return Err(AppError::msg("Clone account changed after completion"));
            }
            let staging = repo.retained_clone_staging().ok_or_else(|| AppError::msg("Clone staging path is unavailable"))?;
            Ok(serde_json::json!({"kind":"clone_completed_unselected", "record":record, "retained_staging":retained_clone_path(staging)}))
        }.await;
        finish.map_err(|_| {
            AppError::msg("Clone completed and was saved; inspect the destination before retrying")
        })
    }

    #[cfg(test)]
    fn read_args(&self, args: Vec<Value>) -> Result<Value> {
        self.read(self.admit(args)?)
    }
    pub fn new(capacity: usize) -> std::io::Result<Self> {
        Ok(Self {
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?,
            jobs: Arc::new(Mutex::new(HashMap::new())),
            capacity,
            registry: None,
            copilot_runtime_root: None,
            comparison_owner: desktop_backend::comparison_snapshots::Owner::new(),
        })
    }

    pub fn with_registry(mut self, registry: desktop_backend::Registry) -> Self {
        self.registry = Some(registry);
        self
    }

    pub fn with_copilot_runtime_root(mut self, root: Option<PathBuf>) -> Self {
        self.copilot_runtime_root = root;
        self
    }

    pub fn admit_git_info(&self, args: Vec<Value>) -> Result<ReadJob> {
        let [value]: [Value; 1] = args
            .try_into()
            .map_err(|_| AppError::BadArgs("Git identity requires exactly one request".into()))?;
        let request: GitInfoRequest = serde_json::from_value(value)
            .map_err(|_| AppError::BadArgs("invalid Git identity request".into()))?;
        if let GitInfoOperation::SetRepository {
            repository_path, ..
        }
        | GitInfoOperation::UnsetRepository {
            repository_path, ..
        }
        | GitInfoOperation::LfsInstallRepository {
            repository_path, ..
        }
        | GitInfoOperation::SetDescription {
            repository_path, ..
        }
        | GitInfoOperation::WriteCreationFile {
            repository_path, ..
        }
        | GitInfoOperation::GitIgnore {
            repository_path, ..
        }
        | GitInfoOperation::ReviewedDiscard {
            repository_path, ..
        }
        | GitInfoOperation::BranchCreation {
            repository_path, ..
        }
        | GitInfoOperation::BranchDeletion {
            repository_path, ..
        }
        | GitInfoOperation::RemoteBranchDeletion {
            repository_path, ..
        }
        | GitInfoOperation::BranchRename {
            repository_path, ..
        }
        | GitInfoOperation::StashWorkflow {
            repository_path, ..
        }
        | GitInfoOperation::BranchCheckout {
            repository_path, ..
        }
        | GitInfoOperation::RevertCommit {
            repository_path, ..
        }
        | GitInfoOperation::AbortGitOperation {
            repository_path, ..
        }
        | GitInfoOperation::FinishMergeCommit {
            repository_path, ..
        }
        | GitInfoOperation::ContinueCherryPick {
            repository_path, ..
        }
        | GitInfoOperation::StartCherryPick {
            repository_path, ..
        }
        | GitInfoOperation::ContinueRebase {
            repository_path, ..
        }
        | GitInfoOperation::StartRebase {
            repository_path, ..
        }
        | GitInfoOperation::StartMerge {
            repository_path, ..
        }
        | GitInfoOperation::MixedReset {
            repository_path, ..
        }
        | GitInfoOperation::FastForwardBranches {
            repository_path, ..
        }
        | GitInfoOperation::OpenNamedEditor {
            repository_path, ..
        }
        | GitInfoOperation::OpenTerminal {
            repository_path, ..
        }
        | GitInfoOperation::FetchRemote {
            repository_path, ..
        }
        | GitInfoOperation::WorktreeMutation {
            repository_path, ..
        }
        | GitInfoOperation::SelectedCommitPreview {
            repository_path, ..
        }
        | GitInfoOperation::CommitCheckpoint {
            repository_path, ..
        }
        | GitInfoOperation::InterpretTrailers {
            repository_path, ..
        }
        | GitInfoOperation::MergeTreePreview {
            repository_path, ..
        }
        | GitInfoOperation::MergeBase {
            repository_path, ..
        }
        | GitInfoOperation::SubmoduleStatus {
            repository_path, ..
        }
        | GitInfoOperation::CheckoutIndex {
            repository_path, ..
        }
        | GitInfoOperation::ResetIndex {
            repository_path, ..
        }
        | GitInfoOperation::UpdateIndex {
            repository_path, ..
        }
        | GitInfoOperation::DeleteInitialHead {
            repository_path, ..
        }
        | GitInfoOperation::ResetSubmodules {
            repository_path, ..
        }
        | GitInfoOperation::UpdateSubmodules {
            repository_path, ..
        }
        | GitInfoOperation::TagMutation {
            repository_path, ..
        }
        | GitInfoOperation::InitialCommit {
            repository_path, ..
        }
        | GitInfoOperation::ManageRemote {
            repository_path, ..
        }
        | GitInfoOperation::InitializeRepository {
            repository_path, ..
        } = &request.operation
            && !Path::new(repository_path).is_absolute()
        {
            return Err(AppError::BadArgs("repository path must be absolute".into()));
        }
        if let GitInfoOperation::ReviewedDiscard { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::DiffDetail { staged: false, .. }
                    | Operation::SelectDiff { staged: false, .. }
                    | Operation::SelectRenamedDiff { .. }
                    | Operation::RendererStagedDiff { .. }
                    | Operation::RendererWorkingDiff { .. }
                    | Operation::RendererWorkingFile { .. }
                    | Operation::RendererBlobPrefix { .. }
                    | Operation::RendererBlobSnapshot { .. }
                    | Operation::RendererBlobChunk { .. }
                    | Operation::RendererCommitDiff { .. }
                    | Operation::RendererCompareDiff { .. }
                    | Operation::RendererCompareChanges { .. }
                    | Operation::RendererResolutionBegin
                    | Operation::RendererResolutionAppend { .. }
                    | Operation::RendererResolutionDiff { .. }
                    | Operation::RendererCompareDiffChunk { .. }
                    | Operation::RendererCompareDiffRelease { .. }
                    | Operation::DiscardPreview {
                        scope: desktop_backend::DiscardScope::WorkingTree,
                        selection: Some(_),
                        ..
                    }
                    | Operation::Discard {
                        scope: desktop_backend::DiscardScope::WorkingTree,
                        selection: Some(_),
                        ..
                    }
                    | Operation::ListDiscards
                    | Operation::DiscardBatchPreview { .. }
                    | Operation::DiscardBatch { .. }
                    | Operation::UndoDiscard { .. }
                    | Operation::ForgetDiscard { .. }
                    | Operation::ConflictDetail { .. }
                    | Operation::StageConflict { .. }
                    | Operation::ResolveConflict { .. }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported reviewed discard operation".into(),
            ));
        }
        if let GitInfoOperation::AbortGitOperation { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::Recover {
                    kind: desktop_backend::RecoveryKind::Merge
                        | desktop_backend::RecoveryKind::Rebase
                        | desktop_backend::RecoveryKind::CherryPick,
                    action: desktop_backend::RecoveryAction::Abort,
                } | Operation::RendererAbortSquash { .. }
            )
        {
            return Err(AppError::BadArgs("unsupported Git abort operation".into()));
        }
        if let GitInfoOperation::BranchCreation { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::CreateBranchPreview {
                    checkout: false,
                    ..
                } | Operation::CreateBranch {
                    checkout: false,
                    basis: Some(_),
                    replace_stash: false,
                    restore_index: false,
                    ..
                }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported branch creation operation".into(),
            ));
        }
        if let GitInfoOperation::BranchDeletion { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::DeleteBranchPreview {
                    to_checkout: None,
                    upstream: None,
                    ..
                } | Operation::DeleteBranchPreview {
                    upstream: Some(_),
                    ..
                } | Operation::DeleteBranch {
                    to_checkout: None,
                    upstream: None,
                    ref_only: true,
                    basis: Some(_),
                    replace_stash: false,
                    restore_index: false,
                    ..
                } | Operation::DeleteBranch {
                    upstream: Some(_),
                    basis: Some(_),
                    replace_stash: false,
                    restore_index: false,
                    ..
                }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported branch deletion operation".into(),
            ));
        }
        if let GitInfoOperation::RemoteBranchDeletion { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::RemoteBranchDeletePreview { .. }
                    | Operation::DeleteRemoteBranch { basis: Some(_), .. }
                    | Operation::RemoteDeletionHistory
                    | Operation::RestoreRemoteTip { .. }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported remote branch deletion operation".into(),
            ));
        }
        if let GitInfoOperation::BranchRename { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::RenameBranchPreview { .. }
                    | Operation::RenameBranch { basis: Some(_), .. }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported branch rename operation".into(),
            ));
        }
        if let GitInfoOperation::StashWorkflow { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::StashList
                    | Operation::RendererStashChanges { .. }
                    | Operation::RendererStashSave { .. }
                    | Operation::RendererStashPop { .. }
                    | Operation::RendererStashDrop { .. }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported Desktop stash operation".into(),
            ));
        }
        if let GitInfoOperation::BranchCheckout { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::CheckoutPreview { .. }
                    | Operation::Checkout { basis: Some(_), .. }
                    | Operation::RendererCheckoutDirect { .. }
                    | Operation::RendererDetachPreview { .. }
                    | Operation::RendererDetach { .. }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported branch checkout operation".into(),
            ));
        }
        if let GitInfoOperation::RevertCommit { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::RendererRevertPreview { .. } | Operation::RendererRevert { .. }
            )
        {
            return Err(AppError::BadArgs("unsupported revert operation".into()));
        }
        if let GitInfoOperation::FinishMergeCommit { operation, .. } = &request.operation
            && !matches!(operation, Operation::RendererFinishMergeCommit)
        {
            return Err(AppError::BadArgs(
                "unsupported merge completion operation".into(),
            ));
        }
        if let GitInfoOperation::ContinueCherryPick { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::RendererCherryPickContinuePreview { .. }
                    | Operation::RendererCherryPickContinue { .. }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported cherry-pick continuation operation".into(),
            ));
        }
        if let GitInfoOperation::StartCherryPick { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::RendererCherryPickStartPreview { .. }
                    | Operation::RendererCherryPickStart { .. }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported cherry-pick start operation".into(),
            ));
        }
        if let GitInfoOperation::ContinueRebase { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::RendererRebaseContinuePreview { .. }
                    | Operation::RendererRebaseContinue { .. }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported rebase continuation operation".into(),
            ));
        }
        if let GitInfoOperation::StartRebase { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::RendererRebaseStartPreview { .. }
                    | Operation::RendererRebaseStart { .. }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported rebase start operation".into(),
            ));
        }
        if let GitInfoOperation::StartMerge { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::RendererMergeStartPreview { .. } | Operation::RendererMergeStart { .. }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported merge start operation".into(),
            ));
        }
        if let GitInfoOperation::MixedReset { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::RendererMixedResetPreview { .. } | Operation::RendererMixedReset { .. }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported mixed reset operation".into(),
            ));
        }
        if let GitInfoOperation::FastForwardBranches { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::RendererFastForwardPreview { .. }
                    | Operation::RendererFastForward { .. }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported branch fast-forward operation".into(),
            ));
        }
        if let GitInfoOperation::FetchRemote { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::RendererFetchPreview { .. }
                    | Operation::RendererFetch { .. }
                    | Operation::RendererPullPreview { .. }
                    | Operation::RendererPull { .. }
                    | Operation::RendererPushPreview { .. }
                    | Operation::RendererPush { .. }
                    | Operation::RendererRemoteHeadPreview { .. }
                    | Operation::RendererRemoteHead { .. }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported remote fetch operation".into(),
            ));
        }
        if let GitInfoOperation::WorktreeMutation { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::RendererWorktreePreview { .. } | Operation::RendererWorktreeApply { .. }
            )
        {
            return Err(AppError::BadArgs("unsupported worktree operation".into()));
        }
        if let GitInfoOperation::SelectedCommitPreview { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::SelectedCommitPreview { .. } | Operation::VerifySelectedIndex { .. }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported selected commit preview operation".into(),
            ));
        }
        if let GitInfoOperation::CommitCheckpoint { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::PrepareSelectedCommit { .. }
                    | Operation::PublishSelectedCommit { .. }
                    | Operation::ListCommitCheckpoints
                    | Operation::InspectCommitCheckpoint { .. }
                    | Operation::RevalidateCommitCheckpoint { .. }
                    | Operation::RecoverPublishedCommitCheckpoint { .. }
                    | Operation::CloseUnchangedCommitCheckpoint { .. }
                    | Operation::ForgetCommitCheckpoint { .. }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported commit checkpoint operation".into(),
            ));
        }
        if let GitInfoOperation::InterpretTrailers { operation, .. } = &request.operation
            && !matches!(operation, Operation::InterpretTrailers { .. })
        {
            return Err(AppError::BadArgs("unsupported trailer operation".into()));
        }
        if let GitInfoOperation::MergeTreePreview { operation, .. } = &request.operation
            && !matches!(operation, Operation::MergeTreePreview { .. })
        {
            return Err(AppError::BadArgs(
                "unsupported merge preview operation".into(),
            ));
        }
        if let GitInfoOperation::MergeBase { operation, .. } = &request.operation
            && !matches!(operation, Operation::MergeBase { .. })
        {
            return Err(AppError::BadArgs("unsupported merge-base operation".into()));
        }
        if let GitInfoOperation::SubmoduleStatus { operation, .. } = &request.operation
            && !matches!(operation, Operation::RendererSubmoduleStatus)
        {
            return Err(AppError::BadArgs(
                "unsupported submodule status operation".into(),
            ));
        }
        if let GitInfoOperation::CheckoutIndex { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::RendererCheckoutIndex { .. }
                    | Operation::RendererCheckoutHeadPaths { .. }
            )
        {
            return Err(AppError::BadArgs(
                "unsupported index checkout operation".into(),
            ));
        }
        if let GitInfoOperation::ResetIndex { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::RendererResetIndex { .. }
                    | Operation::RendererUnstageAll
                    | Operation::RendererRemoveIndexAll
            )
        {
            return Err(AppError::BadArgs(
                "unsupported index reset operation".into(),
            ));
        }
        if let GitInfoOperation::DeleteInitialHead { operation, .. } = &request.operation
            && !matches!(operation, Operation::RendererDeleteInitialHead { .. })
        {
            return Err(AppError::BadArgs(
                "unsupported initial HEAD deletion".into(),
            ));
        }
        if let GitInfoOperation::UpdateIndex { operation, .. } = &request.operation
            && !matches!(operation, Operation::RendererUpdateIndex { .. })
        {
            return Err(AppError::BadArgs(
                "unsupported index update operation".into(),
            ));
        }
        if let GitInfoOperation::ResetSubmodules { operation, .. } = &request.operation
            && !matches!(operation, Operation::RendererResetSubmodules { .. })
        {
            return Err(AppError::BadArgs(
                "unsupported submodule reset operation".into(),
            ));
        }
        if let GitInfoOperation::UpdateSubmodules { operation, .. } = &request.operation
            && !matches!(operation, Operation::RendererUpdateSubmodules { .. })
        {
            return Err(AppError::BadArgs(
                "unsupported submodule update operation".into(),
            ));
        }
        if let GitInfoOperation::TagMutation { operation, .. } = &request.operation
            && !matches!(
                operation,
                Operation::TagCreateAnnotated { .. } | Operation::TagDelete { .. }
            )
        {
            return Err(AppError::BadArgs("unsupported tag operation".into()));
        }
        if matches!(
            request.operation,
            GitInfoOperation::CloneSelected { .. }
                | GitInfoOperation::ListCloneRecoveries {}
                | GitInfoOperation::RecoverClone { .. }
        ) && self.registry.is_none()
        {
            return Err(AppError::BadArgs("Clone registry is unavailable".into()));
        }
        if let GitInfoOperation::RecoverClone { recovery_id } = &request.operation {
            if !uuid::Uuid::parse_str(recovery_id)
                .is_ok_and(|value| value.to_string() == *recovery_id)
            {
                return Err(AppError::BadArgs("Invalid clone recovery ID".into()));
            }
        }
        let is_fetch = matches!(
            &request.operation,
            GitInfoOperation::FetchRemote {
                operation: Operation::RendererFetch { .. }
                    | Operation::RendererPull { .. }
                    | Operation::RendererPush { .. }
                    | Operation::RendererRemoteHead { .. },
                ..
            } | GitInfoOperation::CloneSelected { .. }
                | GitInfoOperation::RecoverClone { .. }
        );
        let maximum = if is_fetch { 3_600_000 } else { 60_000 };
        let duration = request.deadline_ms.unwrap_or(maximum);
        if !(1..=maximum).contains(&duration) {
            return Err(AppError::BadArgs(format!(
                "deadline_ms must be between 1 and {maximum}"
            )));
        }
        let token = Cancellation::default();
        if let Some(id) = request.request_id {
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            if jobs.len() >= self.capacity || jobs.contains_key(&id) {
                return Err(AppError::Coded {
                    code: "EBUSY".into(),
                    message: "backend queue is full or request ID is already active".into(),
                });
            }
            jobs.insert(id, token.clone());
        }
        Ok(ReadJob {
            path: String::new(),
            operation: BackendOperation::GitInfo(request.operation),
            token,
            id: request.request_id,
            jobs: self.jobs.clone(),
            deadline: Instant::now() + Duration::from_millis(duration),
        })
    }

    pub fn admit_copilot(&self, args: Vec<Value>) -> Result<ReadJob> {
        let [value]: [Value; 1] = args
            .try_into()
            .map_err(|_| AppError::BadArgs("Copilot requires exactly one request".into()))?;
        // Serde diagnostics may contain malformed credential values.
        let request: CopilotRequest = serde_json::from_value(value)
            .map_err(|_| AppError::BadArgs("invalid Copilot request".into()))?;
        if !Path::new(&request.repository_path).is_absolute() {
            return Err(AppError::BadArgs("repository path must be absolute".into()));
        }
        if self.registry.is_none() {
            return Err(AppError::BadArgs(
                "Copilot account registry is unavailable".into(),
            ));
        }
        match &request.operation {
            Operation::CopilotCommitDiffDetail { .. } => {}
            Operation::CopilotGenerateCommitMessage {
                account: Some(_), ..
            } => {}
            Operation::CopilotGenerateCommitMessage { account: None, .. } => {
                return Err(AppError::BadArgs(
                    "choose a GitHub account for Copilot".into(),
                ));
            }
            _ => return Err(AppError::BadArgs("unsupported Copilot operation".into())),
        }
        let duration = request.deadline_ms.unwrap_or(170_000);
        if !(1..=330_000).contains(&duration) {
            return Err(AppError::BadArgs(
                "deadline_ms must be between 1 and 330000".into(),
            ));
        }
        let token = Cancellation::default();
        if let Some(id) = request.request_id {
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            if jobs.len() >= self.capacity || jobs.contains_key(&id) {
                return Err(AppError::Coded {
                    code: "EBUSY".into(),
                    message: "backend queue is full or request ID is already active".into(),
                });
            }
            jobs.insert(id, token.clone());
        }
        Ok(ReadJob {
            path: request.repository_path,
            operation: BackendOperation::Copilot(request.operation),
            token,
            id: request.request_id,
            jobs: self.jobs.clone(),
            deadline: Instant::now() + Duration::from_millis(duration),
        })
    }

    pub fn admit(&self, args: Vec<Value>) -> Result<ReadJob> {
        let [value]: [Value; 1] = args
            .try_into()
            .map_err(|_| AppError::BadArgs("backend read requires exactly one request".into()))?;
        let request: ReadRequest = serde_json::from_value(value)?;
        if !matches!(&request.method, ReadMethod::RendererAuthors) && request.revisions.is_some() {
            return Err(AppError::BadArgs(
                "revisions only apply to author reads".into(),
            ));
        }
        if !matches!(&request.method, ReadMethod::RendererBranchCheckouts)
            && request.after_date.is_some()
        {
            return Err(AppError::BadArgs(
                "after_date only applies to checkout history".into(),
            ));
        }
        if !matches!(&request.method, ReadMethod::RendererHistory)
            && (request.limit.is_some()
                || request.skip.is_some()
                || request.first_parent.is_some()
                || request.exclude_remotes.is_some())
        {
            return Err(AppError::BadArgs(
                "history options only apply to history reads".into(),
            ));
        }
        if !Path::new(&request.repository_path).is_absolute() {
            return Err(AppError::BadArgs("repository path must be absolute".into()));
        }
        if !matches!(
            &request.method,
            ReadMethod::AheadBehind | ReadMethod::RevisionList | ReadMethod::HasMergeCommits
        ) && request.range.is_some()
        {
            return Err(AppError::BadArgs(
                "range only applies to revision reads".into(),
            ));
        }
        if !matches!(&request.method, ReadMethod::StatusConflictDetails) && request.kind.is_some() {
            return Err(AppError::BadArgs(
                "kind only applies to conflict detail reads".into(),
            ));
        }
        if !matches!(
            &request.method,
            ReadMethod::GitIdentityValue | ReadMethod::GitReadSetting
        ) && (request.key.is_some() || request.only_local.is_some())
        {
            return Err(AppError::BadArgs(
                "identity options only apply to Git identity reads".into(),
            ));
        }
        if !matches!(&request.method, ReadMethod::GitReadSetting)
            && request.canonical_bool.is_some()
        {
            return Err(AppError::BadArgs(
                "boolean parsing only applies to Git setting reads".into(),
            ));
        }
        if !matches!(
            &request.method,
            ReadMethod::LfsFileTracking | ReadMethod::RendererLargeFiles
        ) && request.paths.is_some()
        {
            return Err(AppError::BadArgs(
                "paths only apply to file-tracking or size reads".into(),
            ));
        }
        if !matches!(&request.method, ReadMethod::SubmoduleDisplayUrl)
            && request.submodule_path.is_some()
        {
            return Err(AppError::BadArgs(
                "submodule_path only applies to submodule URL reads".into(),
            ));
        }
        if !matches!(&request.method, ReadMethod::SafeRemoteUrl) && request.remote_name.is_some() {
            return Err(AppError::BadArgs(
                "remote_name only applies to remote URL reads".into(),
            ));
        }
        if !matches!(&request.method, ReadMethod::SymbolicRef) && request.reference.is_some() {
            return Err(AppError::BadArgs(
                "reference only applies to symbolic ref reads".into(),
            ));
        }
        let operation = if matches!(
            &request.method,
            ReadMethod::RepositoryType
                | ReadMethod::DirectoryExists
                | ReadMethod::PathExists
                | ReadMethod::CanonicalMutationPath
                | ReadMethod::TrustRepository
                | ReadMethod::SafeFetchRemotes
                | ReadMethod::Worktrees
                | ReadMethod::WorktreesFromGitDir
                | ReadMethod::ProtectedResources
                | ReadMethod::SafeRemoteUrl
                | ReadMethod::SymbolicRef
        ) && request.include_untracked.is_none()
            && request.prefixes.is_none()
            && request.branch.is_none()
            && request.commitish.is_none()
        {
            match request.method {
                ReadMethod::RepositoryType => BackendOperation::RepositoryType,
                ReadMethod::DirectoryExists => BackendOperation::DirectoryExists,
                ReadMethod::PathExists => BackendOperation::PathExists,
                ReadMethod::CanonicalMutationPath => BackendOperation::CanonicalMutationPath,
                ReadMethod::TrustRepository => BackendOperation::TrustRepository,
                ReadMethod::SafeFetchRemotes => BackendOperation::SafeFetchRemotes,
                ReadMethod::Worktrees => BackendOperation::Worktrees,
                ReadMethod::WorktreesFromGitDir => BackendOperation::WorktreesFromGitDir,
                ReadMethod::ProtectedResources => BackendOperation::ProtectedResources,
                ReadMethod::SafeRemoteUrl => BackendOperation::SafeRemoteUrl(
                    request
                        .remote_name
                        .ok_or_else(|| AppError::BadArgs("remote name is required".into()))?,
                ),
                ReadMethod::SymbolicRef => BackendOperation::SymbolicRef(
                    request
                        .reference
                        .ok_or_else(|| AppError::BadArgs("symbolic ref is required".into()))?,
                ),
                _ => unreachable!(),
            }
        } else {
            BackendOperation::Repository(match request.method {
                ReadMethod::GitAuthorIdentity | ReadMethod::GitCommitterIdentity
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::GitEffectiveIdentity {
                        role: match request.method {
                            ReadMethod::GitAuthorIdentity => {
                                desktop_backend::GitIdentityRole::Author
                            }
                            ReadMethod::GitCommitterIdentity => {
                                desktop_backend::GitIdentityRole::Committer
                            }
                            _ => unreachable!(),
                        },
                    }
                }
                ReadMethod::SubmoduleDisplayUrl
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::SubmoduleDisplayUrl {
                        path: request.submodule_path.ok_or_else(|| {
                            AppError::BadArgs("submodule path is required".into())
                        })?,
                    }
                }
                ReadMethod::GitDescription
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::GitDescription
                }
                ReadMethod::GitReadSetting
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    let key = request
                        .key
                        .ok_or_else(|| AppError::BadArgs("setting key is required".into()))?;
                    let key = serde_json::from_value(Value::String(key))
                        .map_err(|_| AppError::BadArgs("unsupported Git setting".into()))?;
                    Operation::GitReadSetting {
                        key,
                        only_local: request.only_local.unwrap_or(false),
                        canonical_bool: request.canonical_bool.unwrap_or(false),
                    }
                }
                ReadMethod::LfsStatus
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::LfsStatus { paths: vec![] }
                }
                ReadMethod::LfsFileTracking
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::LfsFileTracking {
                        paths: request
                            .paths
                            .ok_or_else(|| AppError::BadArgs("paths are required".into()))?,
                    }
                }
                ReadMethod::Snapshot
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::Snapshot
                }
                ReadMethod::GitIdentityValue
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    let key = request
                        .key
                        .ok_or_else(|| AppError::BadArgs("identity key is required".into()))?;
                    if !matches!(key.as_str(), "user.name" | "user.email") {
                        return Err(AppError::BadArgs("unsupported Git identity field".into()));
                    }
                    Operation::GitIdentityValue {
                        key,
                        only_local: request.only_local.unwrap_or(false),
                    }
                }
                ReadMethod::Status
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::Status
                }
                ReadMethod::StatusDetailed
                    if request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::StatusDetailed {
                        include_untracked: request.include_untracked.unwrap_or(true),
                    }
                }
                ReadMethod::IndexChanges
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::IndexChanges
                }
                ReadMethod::RendererRecentBranches | ReadMethod::RendererStashLog
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    match request.method {
                        ReadMethod::RendererRecentBranches => Operation::RendererRecentBranches,
                        ReadMethod::RendererStashLog => Operation::RendererStashLog,
                        _ => unreachable!(),
                    }
                }
                ReadMethod::RendererBranchCheckouts
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::RendererBranchCheckouts {
                        after_date: request
                            .after_date
                            .ok_or_else(|| AppError::BadArgs("after_date is required".into()))?,
                    }
                }
                ReadMethod::RendererTagRefs
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::RendererTagRefs
                }
                ReadMethod::RendererCommitChanges
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none() =>
                {
                    Operation::RendererCommitChanges {
                        revision: request
                            .commitish
                            .ok_or_else(|| AppError::BadArgs("commitish is required".into()))?,
                    }
                }
                ReadMethod::RendererLargeFiles
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::RendererLargeFiles {
                        paths: request
                            .paths
                            .ok_or_else(|| AppError::BadArgs("paths are required".into()))?,
                    }
                }
                ReadMethod::RendererCommit
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none() =>
                {
                    Operation::RendererCommit {
                        revision: request
                            .commitish
                            .ok_or_else(|| AppError::BadArgs("commitish is required".into()))?,
                    }
                }
                ReadMethod::RendererAuthors
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::RendererAuthors {
                        revisions: request
                            .revisions
                            .ok_or_else(|| AppError::BadArgs("revisions are required".into()))?,
                    }
                }
                ReadMethod::RendererHistory
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none() =>
                {
                    Operation::RendererHistory {
                        revision: request.commitish,
                        limit: request.limit,
                        skip: request.skip,
                        first_parent: request.first_parent.unwrap_or(false),
                        exclude_remotes: request.exclude_remotes.unwrap_or(false),
                    }
                }
                ReadMethod::StatusContext
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::StatusContext
                }
                ReadMethod::RendererLastFetch
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::RendererLastFetch
                }
                ReadMethod::RendererCherryPickState
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::RendererCherryPickState
                }
                ReadMethod::RendererRebaseState
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::RendererRebaseState
                }
                ReadMethod::StatusConflictDetails
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::StatusConflictDetails {
                        kind: request
                            .kind
                            .ok_or_else(|| AppError::BadArgs("kind is required".into()))?,
                    }
                }
                ReadMethod::BranchEnumeration
                    if request.include_untracked.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::BranchEnumeration {
                        prefixes: request.prefixes.unwrap_or_default(),
                    }
                }
                ReadMethod::BranchMerged
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::BranchMerged {
                        branch: request
                            .branch
                            .ok_or_else(|| AppError::BadArgs("branch is required".into()))?,
                    }
                }
                ReadMethod::BranchPointsAt
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none() =>
                {
                    Operation::BranchPointsAt {
                        commitish: request
                            .commitish
                            .ok_or_else(|| AppError::BadArgs("commitish is required".into()))?,
                    }
                }
                ReadMethod::AheadBehind
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::AheadBehind {
                        range: request
                            .range
                            .ok_or_else(|| AppError::BadArgs("range is required".into()))?,
                    }
                }
                ReadMethod::RevisionList
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::RevisionList {
                        range: request
                            .range
                            .ok_or_else(|| AppError::BadArgs("range is required".into()))?,
                    }
                }
                ReadMethod::HasMergeCommits
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::HasMergeCommits {
                        range: request
                            .range
                            .ok_or_else(|| AppError::BadArgs("range is required".into()))?,
                    }
                }
                _ => {
                    return Err(AppError::BadArgs(
                        "invalid options for repository read".into(),
                    ));
                }
            })
        };
        let duration = request.deadline_ms.unwrap_or(60_000);
        if !(1..=60_000).contains(&duration) {
            return Err(AppError::BadArgs(
                "deadline_ms must be between 1 and 60000".into(),
            ));
        }
        let token = Cancellation::default();
        if let Some(id) = request.request_id {
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            if jobs.len() >= self.capacity || jobs.contains_key(&id) {
                return Err(AppError::Coded {
                    code: "EBUSY".into(),
                    message: "backend read queue is full or request ID is already active".into(),
                });
            }
            jobs.insert(id, token.clone());
        }
        Ok(ReadJob {
            path: request.repository_path,
            operation,
            token,
            id: request.request_id,
            jobs: self.jobs.clone(),
            deadline: Instant::now() + Duration::from_millis(duration),
        })
    }

    pub fn cancel(&self, args: &[Value]) -> Result<()> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Cancel {
            request_id: uuid::Uuid,
        }
        let [value] = args else {
            return Err(AppError::BadArgs(
                "backend cancel requires one request".into(),
            ));
        };
        let request: Cancel = serde_json::from_value(value.clone())?;
        if let Some(token) = self
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&request.request_id)
        {
            token.cancel();
        }
        Ok(())
    }

    pub fn cancel_all(&self) {
        for token in self.jobs.lock().unwrap_or_else(|e| e.into_inner()).values() {
            token.cancel();
        }
    }

    #[cfg(test)]
    pub fn read(&self, job: ReadJob) -> Result<Value> {
        self.read_with_progress(job, None)
    }

    pub fn read_with_progress(
        &self,
        job: ReadJob,
        progress: Option<Arc<dyn Fn(desktop_backend::hook_progress::CommitEvent) + Send + Sync>>,
    ) -> Result<Value> {
        let progress = if matches!(
            &job.operation,
            BackendOperation::GitInfo(GitInfoOperation::CommitCheckpoint {
                operation: Operation::PublishSelectedCommit { .. },
                ..
            }) | BackendOperation::GitInfo(GitInfoOperation::FetchRemote {
                operation: Operation::RendererFetch { .. }
                    | Operation::RendererPull { .. }
                    | Operation::RendererPush { .. },
                ..
            }) | BackendOperation::GitInfo(GitInfoOperation::CloneSelected { .. })
        ) {
            progress
        } else {
            None
        };
        let token = job.token.clone();
        let identity_write = matches!(
            job.operation,
            BackendOperation::GitInfo(
                GitInfoOperation::Set {
                    key: GlobalGitKey::Name | GlobalGitKey::Email,
                    ..
                } | GitInfoOperation::SetRepository { .. }
                    | GitInfoOperation::UnsetRepository { .. }
            )
        );
        let settings_write = matches!(
            job.operation,
            BackendOperation::GitInfo(GitInfoOperation::Set {
                key: GlobalGitKey::DefaultBranch,
                ..
            })
        );
        let config_path_write = matches!(
            job.operation,
            BackendOperation::GitInfo(GitInfoOperation::GlobalConfigPath {})
        );
        let description_write = matches!(
            job.operation,
            BackendOperation::GitInfo(GitInfoOperation::SetDescription { .. })
        );
        let creation_write = matches!(
            job.operation,
            BackendOperation::GitInfo(
                GitInfoOperation::WriteCreationFile { .. } | GitInfoOperation::InitialCommit { .. }
            )
        );
        let remote_write = matches!(
            job.operation,
            BackendOperation::GitInfo(GitInfoOperation::ManageRemote { .. })
        );
        let gitignore_write = matches!(
            job.operation,
            BackendOperation::GitInfo(GitInfoOperation::GitIgnore {
                action: desktop_backend::GitIgnoreAction::Save { .. }
                    | desktop_backend::GitIgnoreAction::Append { .. },
                ..
            })
        );
        let discard_write = matches!(
            job.operation,
            BackendOperation::GitInfo(GitInfoOperation::ReviewedDiscard {
                operation: Operation::Discard { .. }
                    | Operation::DiscardBatch { .. }
                    | Operation::UndoDiscard { .. }
                    | Operation::ForgetDiscard { .. }
                    | Operation::StageConflict { .. }
                    | Operation::ResolveConflict { .. },
                ..
            })
        );
        let recovery_write = matches!(
            job.operation,
            BackendOperation::GitInfo(
                GitInfoOperation::AbortGitOperation { .. }
                    | GitInfoOperation::FinishMergeCommit { .. }
                    | GitInfoOperation::ContinueCherryPick {
                        operation: Operation::RendererCherryPickContinue { .. },
                        ..
                    }
                    | GitInfoOperation::StartCherryPick {
                        operation: Operation::RendererCherryPickStart { .. },
                        ..
                    }
                    | GitInfoOperation::ContinueRebase {
                        operation: Operation::RendererRebaseContinue { .. },
                        ..
                    }
                    | GitInfoOperation::StartRebase {
                        operation: Operation::RendererRebaseStart { .. },
                        ..
                    }
                    | GitInfoOperation::StartMerge {
                        operation: Operation::RendererMergeStart { .. },
                        ..
                    }
                    | GitInfoOperation::MixedReset {
                        operation: Operation::RendererMixedReset { .. },
                        ..
                    }
                    | GitInfoOperation::FastForwardBranches {
                        operation: Operation::RendererFastForward { .. },
                        ..
                    }
                    | GitInfoOperation::FetchRemote {
                        operation: Operation::RendererFetch { .. }
                            | Operation::RendererPull { .. }
                            | Operation::RendererPush { .. }
                            | Operation::RendererRemoteHead { .. },
                        ..
                    }
                    | GitInfoOperation::WorktreeMutation {
                        operation: Operation::RendererWorktreeApply { .. },
                        ..
                    }
                    | GitInfoOperation::BranchCreation {
                        operation: Operation::CreateBranch { .. },
                        ..
                    }
                    | GitInfoOperation::BranchDeletion {
                        operation: Operation::DeleteBranch { .. },
                        ..
                    }
                    | GitInfoOperation::RemoteBranchDeletion {
                        operation: Operation::DeleteRemoteBranch { .. }
                            | Operation::RestoreRemoteTip { .. },
                        ..
                    }
                    | GitInfoOperation::BranchRename {
                        operation: Operation::RenameBranch { .. },
                        ..
                    }
                    | GitInfoOperation::StashWorkflow {
                        operation: Operation::RendererStashSave { .. }
                            | Operation::RendererStashPop { .. }
                            | Operation::RendererStashDrop { .. },
                        ..
                    }
                    | GitInfoOperation::BranchCheckout {
                        operation: Operation::Checkout { .. }
                            | Operation::RendererCheckoutDirect { .. }
                            | Operation::RendererDetach { .. },
                        ..
                    }
                    | GitInfoOperation::RevertCommit {
                        operation: Operation::RendererRevert { .. },
                        ..
                    }
            )
        );
        let checkpoint_write = matches!(
            job.operation,
            BackendOperation::GitInfo(GitInfoOperation::CommitCheckpoint {
                operation: Operation::PrepareSelectedCommit { .. }
                    | Operation::PublishSelectedCommit { .. }
                    | Operation::RecoverPublishedCommitCheckpoint { .. }
                    | Operation::CloseUnchangedCommitCheckpoint { .. }
                    | Operation::ForgetCommitCheckpoint { .. },
                ..
            })
        );
        let initialization = matches!(
            job.operation,
            BackendOperation::GitInfo(GitInfoOperation::InitializeRepository { .. })
        );
        let clone_write = matches!(
            job.operation,
            BackendOperation::GitInfo(
                GitInfoOperation::CloneSelected { .. } | GitInfoOperation::RecoverClone { .. }
            )
        );
        let lfs_write = matches!(
            job.operation,
            BackendOperation::GitInfo(
                GitInfoOperation::LfsInstall { .. } | GitInfoOperation::LfsInstallRepository { .. }
            )
        );
        let work = token.clone().scope(async {
            if token.is_cancelled() { return Err(AppError::Coded { code: "cancelled".into(), message: "backend operation cancelled before start".into() }); }
            let remaining = job.deadline.saturating_duration_since(Instant::now());
            let work = async {
                let (operation, scoped) = match job.operation.clone() {
                    BackendOperation::RepositoryType => return desktop_backend::repository_type(Path::new(&job.path)).await.map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::DirectoryExists => return desktop_backend::directory_exists(Path::new(&job.path)).await.map(serde_json::Value::Bool).map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::PathExists => return desktop_backend::path_exists(Path::new(&job.path)).await.map(serde_json::Value::Bool).map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::CanonicalMutationPath => return desktop_backend::canonical_mutation_path(Path::new(&job.path)).await.map(serde_json::Value::String).map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::TrustRepository => return desktop_backend::trust_repository(Path::new(&job.path)).await.map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::SafeFetchRemotes => return desktop_backend::safe_fetch_remotes(Path::new(&job.path)).await.map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::Worktrees => return desktop_backend::worktrees_at(Path::new(&job.path)).await.map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::WorktreesFromGitDir => return desktop_backend::worktrees_from_git_dir(Path::new(&job.path)).await.map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::ProtectedResources => return desktop_backend::protected_resources(Path::new(&job.path)).await.and_then(|resources| serde_json::to_value(resources).map_err(Into::into)).map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::SafeRemoteUrl(name) => return desktop_backend::safe_remote_url(Path::new(&job.path), &name).await.map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::SymbolicRef(reference) => return desktop_backend::symbolic_ref(Path::new(&job.path), &reference).await.map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::GitInfo(operation) => {
                        return match operation {
                            GitInfoOperation::Version {} => desktop_backend::git_version().await.map(serde_json::Value::String),
                            GitInfoOperation::GlobalConfigPath {} => desktop_backend::git_global_config_path().await.map(serde_json::Value::String),
                            GitInfoOperation::AvailableEditors {} => desktop_backend::available_editors(desktop_backend::ExternalEditor::Automatic).await,
                            GitInfoOperation::AvailableTerminals {} => desktop_backend::available_terminals().await,
                            GitInfoOperation::OpenTerminal { repository_path, name, directory } => {
                                let requested = PathBuf::from(repository_path);
                                let canonical = desktop_backend::cancellation::blocking(move || Ok(requested.canonicalize()?)).await.map_err(|error| AppError::msg(error.to_string()))?;
                                let repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                repo.open_terminal(name, directory.into()).await
                            }
                            GitInfoOperation::OpenGlobalGitConfigEditor { name } => desktop_backend::open_global_git_config_editor(name).await,
                            GitInfoOperation::OpenNamedEditor { repository_path, path, name } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                repo.open_editor(path, desktop_backend::ExternalEditor::Named { name }).await
                            }
                            GitInfoOperation::CloneSelected { url, path, default_branch, account } => {
                                return self.clone_selected(url, path, default_branch, account).await;
                            }
                            GitInfoOperation::ListCloneRecoveries {} => {
                                let registry = self.registry.clone().ok_or_else(|| AppError::msg("Clone registry is unavailable"))?;
                                desktop_backend::cancellation::blocking(move || registry.clone_recovery_records()).await
                            }
                            GitInfoOperation::RecoverClone { recovery_id } => {
                                let registry = self.registry.clone().ok_or_else(|| AppError::msg("Clone registry is unavailable"))?;
                                let (record, staging) = desktop_backend::recover_selected_clone_with_staging(registry, &recovery_id).await.map_err(|error| AppError::msg(error.to_string()))?;
                                let mut value = serde_json::to_value(record).map_err(|_| AppError::msg("Clone was recovered and saved; inspect the destination before retrying"))?;
                                value.as_object_mut().ok_or_else(|| AppError::msg("Clone was recovered and saved; inspect the destination before retrying"))?
                                    .insert("retained_staging".into(), retained_clone_path(&staging));
                                return Ok(value);
                            }
                            GitInfoOperation::LfsStatus {} => {
                                let cwd = std::env::current_dir().map_err(|error| AppError::msg(error.to_string()))?;
                                desktop_backend::lfs_tool_status(&cwd).await
                            }
                            GitInfoOperation::LfsInstall { force } => {
                                let cwd = std::env::current_dir().map_err(|error| AppError::msg(error.to_string()))?;
                                desktop_backend::lfs_install(&cwd, desktop_backend::LfsInstallScope::Global, force, true, false).await
                            }
                            GitInfoOperation::LfsInstallRepository { repository_path, force } => {
                                let repo = Repository::open(Path::new(&repository_path)).await.map_err(|error| AppError::msg(error.to_string()))?;
                                repo.execute(Operation::LfsInstall { scope: desktop_backend::LfsInstallScope::Global, force, skip_repo: false, skip_smudge: false }).await
                            }
                            GitInfoOperation::SetDescription { repository_path, description } => {
                                let repo = Repository::open(Path::new(&repository_path)).await.map_err(|error| AppError::msg(error.to_string()))?;
                                repo.execute(Operation::SetGitDescription { description }).await
                            }
                            GitInfoOperation::WriteCreationFile { repository_path, kind, contents } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical {
                                    return Err(AppError::msg("select a repository root, not a nested path"));
                                }
                                repo.execute(Operation::WriteCreationFile { kind, contents }).await
                            }
                            GitInfoOperation::GitIgnore { repository_path, action } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                repo.execute(Operation::GitIgnore { action }).await
                            }
                            GitInfoOperation::ReleaseComparisonSnapshot { snapshot_id } => {
                                desktop_backend::comparison_snapshots::release_owned(&snapshot_id)
                            }
                            GitInfoOperation::ReviewedDiscard { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if matches!(operation, Operation::ConflictDetail { .. } | Operation::StageConflict { .. } | Operation::ResolveConflict { .. }) {
                                    if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::BranchCreation { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::BranchDeletion { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::RemoteBranchDeletion { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::BranchRename { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::StashWorkflow { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::BranchCheckout { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::RevertCommit { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::AbortGitOperation { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::FinishMergeCommit { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::ContinueCherryPick { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::StartCherryPick { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::ContinueRebase { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::StartRebase { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::StartMerge { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::MixedReset { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::FastForwardBranches { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::FetchRemote { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::WorktreeMutation { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::SelectedCommitPreview { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::CommitCheckpoint { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::InterpretTrailers { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::MergeTreePreview { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::MergeBase { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::SubmoduleStatus { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::CheckoutIndex { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::ResetIndex { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::UpdateIndex { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::DeleteInitialHead { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::ResetSubmodules { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::UpdateSubmodules { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::TagMutation { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::InitialCommit { repository_path, paths } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical {
                                    return Err(AppError::msg("select a repository root, not a nested path"));
                                }
                                if let Some(registry) = &self.registry {
                                    repo = repo.with_registry(registry.clone());
                                }
                                repo.execute(Operation::InitialCommit { paths }).await
                            }
                            GitInfoOperation::ManageRemote { repository_path, action } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let mut repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                if let Some(registry) = &self.registry { repo = repo.with_registry(registry.clone()); }
                                repo.execute(Operation::ManageRemote { action }).await
                            }
                            GitInfoOperation::InitializeRepository { repository_path, default_branch } => {
                                desktop_backend::initialize_desktop_repository(Path::new(&repository_path), &default_branch).await.map(|()| Value::Null)
                            }
                            GitInfoOperation::Read {} => {
                                let cwd = std::env::current_dir().map_err(|error| AppError::msg(error.to_string()))?;
                                desktop_backend::git_settings(&cwd, desktop_backend::ConfigScope::Global).await
                            }
                            GitInfoOperation::Set { key, value } => {
                                let cwd = std::env::current_dir().map_err(|error| AppError::msg(error.to_string()))?;
                                desktop_backend::set_git_setting(&cwd, desktop_backend::ConfigScope::Global, key.as_str(), Some(value)).await
                            }
                            GitInfoOperation::SetRepository { repository_path, key, value } => {
                                let repo = Repository::open(Path::new(&repository_path)).await.map_err(|error| AppError::msg(error.to_string()))?;
                                repo.execute(Operation::SetGitSetting { scope: desktop_backend::ConfigScope::Local, key: key.as_str().into(), value: Some(value) }).await
                            }
                            GitInfoOperation::UnsetRepository { repository_path, key } => {
                                let repo = Repository::open(Path::new(&repository_path)).await.map_err(|error| AppError::msg(error.to_string()))?;
                                repo.execute(Operation::SetGitSetting { scope: desktop_backend::ConfigScope::Local, key: key.as_str().into(), value: None }).await
                            }
                        }.map_err(|error| AppError::msg(error.to_string()));
                    }
                    BackendOperation::Repository(operation) => (operation, false),
                    BackendOperation::Copilot(operation) => (operation, true),
                };
                let mut repo = Repository::open(Path::new(&job.path))
                    .await
                    .map_err(|error| AppError::msg(error.to_string()))?;
                if scoped {
                    repo = repo.with_registry(self.registry.clone().ok_or_else(|| AppError::BadArgs("Copilot account registry is unavailable".into()))?);
                    if let Some(root) = &self.copilot_runtime_root {
                        repo = repo.with_copilot_runtime_root(root.clone());
                    }
                }
                repo.execute(operation)
                    .await
                    .map_err(|error| AppError::msg(error.to_string()))
            };
            tokio::pin!(work);
            tokio::select! {
                result = &mut work => if token.is_cancelled() { Err(AppError::Coded { code: "cancelled".into(), message: if recovery_write { "Git operation cancelled; inspect the repository before retrying" } else if identity_write { "Git identity update cancelled; inspect Git settings before retrying" } else if settings_write { "Git setting update cancelled; inspect Git settings before retrying" } else if config_path_write { "Git config preparation cancelled; check the configuration file before retrying" } else if description_write { "Description update cancelled; inspect the repository description before retrying" } else if creation_write { "Repository creation operation cancelled; inspect the repository and index before retrying" } else if discard_write { "Discard operation cancelled; inspect the file and discard recovery history before retrying" } else if checkpoint_write { "Commit operation cancelled; inspect commit checkpoints before retrying" } else if gitignore_write { "Ignore file update cancelled; inspect .gitignore before retrying" } else if remote_write { "Remote update cancelled; inspect Git configuration before retrying" } else if clone_write { "Clone cancelled; inspect the destination and recovery history before retrying" } else if initialization { "Repository initialization cancelled; inspect the destination before retrying" } else if lfs_write { "Git LFS setup cancelled; inspect Git settings and hooks before retrying" } else { "backend operation cancelled" }.into() }) } else { result },
                _ = tokio::time::sleep(remaining) => {
                    token.cancel();
                    let _ = tokio::time::timeout(Duration::from_secs(3), &mut work).await;
                    Err(AppError::Coded { code: "ETIMEDOUT".into(), message: if recovery_write { "Git operation timed out; inspect the repository before retrying" } else if identity_write { "Git identity update timed out; inspect Git settings before retrying" } else if settings_write { "Git setting update timed out; inspect Git settings before retrying" } else if config_path_write { "Git config preparation timed out; check the configuration file before retrying" } else if description_write { "Description update timed out; inspect the repository description before retrying" } else if creation_write { "Repository creation operation timed out; inspect the repository and index before retrying" } else if discard_write { "Discard operation timed out; inspect the file and discard recovery history before retrying" } else if checkpoint_write { "Commit operation timed out; inspect commit checkpoints before retrying" } else if gitignore_write { "Ignore file update timed out; inspect .gitignore before retrying" } else if remote_write { "Remote update timed out; inspect Git configuration before retrying" } else if clone_write { "Clone timed out; inspect the destination and recovery history before retrying" } else if initialization { "Repository initialization timed out; inspect the destination before retrying" } else if lfs_write { "Git LFS setup timed out; inspect Git settings and hooks before retrying" } else { "backend operation timed out" }.into() })
                }
            }
        });
        let work =
            desktop_backend::comparison_snapshots::scope(self.comparison_owner.clone(), work);
        if let Some(progress) = progress {
            let (sender, mut receiver) = tokio::sync::mpsc::channel::<(
                u64,
                desktop_backend::hook_progress::CommitEvent,
            )>(64);
            let forward = self.runtime.spawn(async move {
                while let Some((_, event)) = receiver.recv().await {
                    progress(event);
                }
            });
            let result = self
                .runtime
                .block_on(desktop_backend::hook_progress::scope(sender, 0, work));
            let _ = self.runtime.block_on(forward);
            result
        } else {
            self.runtime.block_on(work)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wry_editor_rejects_unknown_names_and_repository_escape() {
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
        let root = std::env::temp_dir().join(format!("desktop-wry-editor-{}", uuid::Uuid::new_v4()));
        assert!(std::process::Command::new("git").args(["init", "-q"]).arg(&root).status().unwrap().success());
        std::fs::write(root.join("file"), "test").unwrap();
        let backend = Backend::new(8).unwrap();
        let invoke = |path: &str, name: &str| {
            let job = backend.admit_git_info(vec![json!({"operation":{"type":"open_named_editor","repository_path":root,"path":path,"name":name}})]).unwrap();
            backend.read(job)
        };
        assert!(invoke("file", "/bin/sh").is_err());
        assert!(invoke("../outside", "Visual Studio Code").is_err());
        std::fs::remove_dir_all(root).unwrap();
        }).unwrap().join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn wry_editor_launches_detected_editor_in_real_repository() {
        use std::os::unix::fs::PermissionsExt;
        const ROOT_ENV: &str = "DESKTOP_WRY_EDITOR_TEST_ROOT";
        if let Some(root) = std::env::var_os(ROOT_ENV) {
            std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(move || {
                let root = std::path::PathBuf::from(root);
                let repo = root.join("repo");
                let backend = Backend::new(8).unwrap();
                let invalid = backend.admit_git_info(vec![json!({"operation":{"type":"open_global_git_config_editor","name":"/bin/sh"}})]).unwrap();
                assert!(backend.read(invalid).is_err());
                assert!(!root.join("git config").exists());
                assert!(backend.admit_git_info(vec![json!({"operation":{"type":"open_global_git_config_editor","name":"Visual Studio Code","path":"/etc/passwd"}})]).is_err());
                let global = backend.admit_git_info(vec![json!({"operation":{"type":"open_global_git_config_editor","name":"Visual Studio Code"}})]).unwrap();
                assert_eq!(backend.read(global).unwrap()["code"], 0);
                assert_eq!(std::fs::read_to_string(root.join("capture")).unwrap(), format!("--\n{}\n", root.join("git config").display()));
                let job = backend.admit_git_info(vec![json!({"operation":{"type":"open_named_editor","repository_path":repo,"path":"file with spaces","name":"Visual Studio Code"}})]).unwrap();
                assert_eq!(backend.read(job).unwrap()["code"], 0);
                assert_eq!(std::fs::read_to_string(root.join("capture")).unwrap(), format!("--\n{}\n", repo.join("file with spaces").display()));
            }).unwrap().join().unwrap();
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "desktop-wry-editor-launch-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join("bin")).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(root.join("repo"))
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(root.join("repo/file with spaces"), "test").unwrap();
        let editor = root.join("bin/code");
        std::fs::write(
            &editor,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$DESKTOP_WRY_EDITOR_TEST_ROOT/capture\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();
        let paths = std::iter::once(root.join("bin"))
            .chain(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ))
            .collect::<Vec<_>>();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "backend::tests::wry_editor_launches_detected_editor_in_real_repository",
                "--nocapture",
            ])
            .env(ROOT_ENV, &root)
            .env("GIT_CONFIG_GLOBAL", root.join("git config"))
            .env("PATH", std::env::join_paths(paths).unwrap())
            .status()
            .unwrap();
        let launched = root.join("capture").is_file();
        std::fs::remove_dir_all(root).unwrap();
        assert!(status.success() && launched);
    }

    #[test]
    fn wry_reviews_and_recovers_remote_branch_deletion() {
        use std::{fs, process::Command};
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
            let root = std::env::temp_dir().join(format!("desktop-wry-remote-delete-{}", uuid::Uuid::new_v4()));
            let repo = root.join("repo");
            let first = root.join("first.git");
            let second = root.join("second.git");
            fs::create_dir_all(&repo).unwrap();
            let git = |directory: &Path, arguments: &[&str]| {
                let output = Command::new("git").arg("-C").arg(directory).args(arguments).env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
                assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
                output.stdout
            };
            git(&root, &["init", "--bare", "-q", first.to_str().unwrap()]);
            git(&root, &["init", "--bare", "-q", second.to_str().unwrap()]);
            git(&repo, &["init", "-q", "-b", "main"]);
            git(&repo, &["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-qm", "first"]);
            git(&repo, &["push", "-q", first.to_str().unwrap(), "HEAD:refs/heads/topic"]);
            git(&repo, &["push", "-q", second.to_str().unwrap(), "HEAD:refs/heads/topic"]);
            git(&repo, &["remote", "add", "origin", first.to_str().unwrap()]);
            git(&repo, &["remote", "set-url", "--add", "--push", "origin", first.to_str().unwrap()]);
            git(&repo, &["remote", "set-url", "--add", "--push", "origin", second.to_str().unwrap()]);
            let backend = Backend::new(8).unwrap();
            let invoke = |operation: Value| backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"remote_branch_deletion","repository_path":repo,"operation":operation}})]).unwrap()).unwrap();
            let request = json!({"remote":"origin","branch":"topic","expected":null,"fetch_backup":false});
            let preview = invoke(json!({"method":"remote_branch_delete_preview","request":request}));
            assert_eq!(preview["endpoints"].as_array().unwrap().len(), 2);
            let deletion = invoke(json!({"method":"delete_remote_branch","request":request,"basis":preview["basis"]}));
            assert_eq!(deletion["code"], 0);
            for remote in [&first, &second] {
                assert!(!Command::new("git").arg("-C").arg(remote).args(["show-ref", "--verify", "--quiet", "refs/heads/topic"]).status().unwrap().success());
            }
            let history = invoke(json!({"method":"remote_deletion_history"}));
            assert_eq!(history[0]["id"], deletion["receipt_id"]);
            let restored = invoke(json!({"method":"restore_remote_tip","receipt_id":deletion["receipt_id"],"endpoint":0,"name":"recovered-topic"}));
            assert_eq!(restored["code"], 0);
            git(&repo, &["show-ref", "--verify", "--quiet", "refs/heads/recovered-topic"]);
            fs::remove_dir_all(root).unwrap();
        }).unwrap().join().unwrap();
    }

    #[test]
    fn wry_comparison_diff_returns_large_raw_response() {
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
            use std::{fs, process::Command};
            let root = std::env::temp_dir().join(format!("desktop-wry-compare-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let git = |args: &[&str]| {
                let output = Command::new("git").arg("-C").arg(&root).args(args).output().unwrap();
                assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
                output.stdout
            };
            git(&["init", "-q", "-b", "main"]);
            git(&["config", "user.name", "Test"]); git(&["config", "user.email", "test@example.invalid"]); git(&["config", "commit.gpgsign", "false"]);
            fs::write(root.join("[a].txt"), "first\n").unwrap(); git(&["add", "--", "."]); git(&["commit", "-qm", "first"]);
            let first = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap().trim().to_owned();
            fs::write(root.join("[a].txt"), format!("{}\n", "a".repeat(120)).repeat(20_000)).unwrap(); git(&["add", "--", "."]); git(&["commit", "-qm", "second"]);
            let head = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap().trim().to_owned();
            let backend = Backend::new(16).unwrap();
            let commit = backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":root,"operation":{"method":"renderer_commit_diff","revision":head,"path":"[a].txt","old_path":null,"hide_whitespace":false,"snapshot":true}}})]).unwrap();
            let commit = backend.read(commit).unwrap();
            assert_eq!(commit["kind"], "snapshot");
            assert!(commit["size"].as_u64().unwrap() > 2 * 1024 * 1024);
            let release = backend.admit_git_info(vec![json!({"operation":{"type":"release_comparison_snapshot","snapshot_id":commit["id"]}})]).unwrap();
            assert_eq!(backend.read(release).unwrap()["released"], true);
            let request = backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":root,"operation":{"method":"renderer_compare_diff","before":first,"after":head,"merge_base":true,"path":"[a].txt","old_path":null,"hide_whitespace":false,"snapshot":true}}})]).unwrap();
            let response = backend.read(request).unwrap();
            assert_eq!(response["kind"], "snapshot");
            let size = response["size"].as_u64().unwrap();
            assert!(size > 2 * 1024 * 1024);
            let id = response["id"].as_str().unwrap();
            let mut offset = 0;
            while offset < size {
                let request = backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":root,"operation":{"method":"renderer_compare_diff_chunk","snapshot_id":id,"offset":offset,"length":524288}}})]).unwrap();
                let chunk = backend.read(request).unwrap();
                assert_eq!(chunk["offset"], offset);
                assert_eq!(chunk["size"], size);
                assert!(!chunk["data"].as_str().unwrap().is_empty());
                offset = (offset + 524288).min(size);
            }
            let release = backend.admit_git_info(vec![json!({"operation":{"type":"release_comparison_snapshot","snapshot_id":id}})]).unwrap();
            assert_eq!(backend.read(release).unwrap()["released"], true);
            let request = backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":root,"operation":{"method":"renderer_compare_changes","before":first,"after":head,"merge_base":true}}})]).unwrap();
            let changes = backend.read(request).unwrap();
            assert_eq!(changes["kind"], "snapshot");
            let changes_id = changes["id"].as_str().unwrap();
            let chunk = backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":root,"operation":{"method":"renderer_compare_diff_chunk","snapshot_id":changes_id,"offset":0,"length":524288}}})]).unwrap();
            let chunk = backend.read(chunk).unwrap();
            assert!(!chunk["data"].as_str().unwrap().is_empty());
            fs::remove_dir_all(&root).unwrap();
            let request = backend.admit_git_info(vec![json!({"operation":{"type":"release_comparison_snapshot","snapshot_id":changes_id}})]).unwrap();
            assert_eq!(backend.read(request).unwrap()["released"], true);
        }).unwrap().join().unwrap();
    }

    #[test]
    fn wry_resolution_preview_upload_and_read() {
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
            let root = std::env::temp_dir().join(format!("desktop-wry-resolution-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let result = Command::new("git").arg("-C").arg(&root).args(["init", "-q"]).output().unwrap(); assert!(result.status.success());
            fs::write(root.join("[a].txt"), "working\n").unwrap();
            let backend = Backend::new(16).unwrap();
            let route = |operation: Value| backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":root,"operation":operation}})]).unwrap();
            let upload = backend.read(route(json!({"method":"renderer_resolution_begin"}))).unwrap();
            let id = upload["id"].as_str().unwrap();
            assert_eq!(backend.read(route(json!({"method":"renderer_resolution_append","snapshot_id":id,"offset":0,"data":"b3Vycwo="}))).unwrap()["size"], 5);
            let preview = backend.read(route(json!({"method":"renderer_resolution_diff","snapshot_id":id,"size":5,"path":"[a].txt","stage":null,"hide_whitespace":false}))).unwrap();
            assert_eq!(preview["kind"], "snapshot");
            let result_id = preview["id"].as_str().unwrap();
            let chunk = backend.read(route(json!({"method":"renderer_compare_diff_chunk","snapshot_id":result_id,"offset":0,"length":524288}))).unwrap();
            assert!(!chunk["data"].as_str().unwrap().is_empty());
            let release = backend.admit_git_info(vec![json!({"operation":{"type":"release_comparison_snapshot","snapshot_id":result_id}})]).unwrap();
            assert_eq!(backend.read(release).unwrap()["released"], true);
            fs::remove_dir_all(root).unwrap();
        }).unwrap().join().unwrap();
    }

    #[test]
    fn wry_reads_working_image_bytes() {
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
            let root = std::env::temp_dir().join(format!("desktop-wry-image-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let init = Command::new("git").arg("-C").arg(&root).args(["init", "-q"]).output().unwrap(); assert!(init.status.success());
            fs::write(root.join("[a].png"), [0, 255, 0, 128, 42, 10]).unwrap();
            let backend = Backend::new(16).unwrap();
            let route = |operation: Value| backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":root,"operation":operation}})]).unwrap();
            let preview = backend.read(route(json!({"method":"renderer_working_file","path":"[a].png"}))).unwrap();
            let id = preview["id"].as_str().unwrap();
            let chunk = backend.read(route(json!({"method":"renderer_compare_diff_chunk","snapshot_id":id,"offset":0,"length":64}))).unwrap();
            assert_eq!(chunk["data"], "AP8AgCoK");
            let release = backend.admit_git_info(vec![json!({"operation":{"type":"release_comparison_snapshot","snapshot_id":id}})]).unwrap();
            assert_eq!(backend.read(release).unwrap()["released"], true);
            let preview = backend.read(route(json!({"method":"renderer_working_file","path":"[a].png","max_bytes":6}))).unwrap();
            let id = preview["id"].as_str().unwrap();
            let release = backend.admit_git_info(vec![json!({"operation":{"type":"release_comparison_snapshot","snapshot_id":id}})]).unwrap();
            assert_eq!(backend.read(release).unwrap()["released"], true);
            assert_eq!(backend.read(route(json!({"method":"renderer_working_file","path":"[a].png","max_bytes":5}))).unwrap()["kind"], "too_large");
            fs::remove_dir_all(root).unwrap();
        }).unwrap().join().unwrap();
    }

    #[test]
    fn wry_reviews_and_mutates_worktrees() {
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
            let root = std::env::temp_dir().join(format!("desktop-wry-worktree-{}", uuid::Uuid::new_v4()));
            let main = root.join("main"); let linked = root.join("linked");
            fs::create_dir_all(&main).unwrap();
            let git = |args: &[&str]| { let out = Command::new("git").arg("-C").arg(&main).args(args).output().unwrap(); assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr)); };
            git(&["init", "-q", "-b", "main"]); git(&["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-qm", "initial"]);
            let backend = Backend::new(16).unwrap();
            let route = |operation: Value| backend.admit_git_info(vec![json!({"operation":{"type":"worktree_mutation","repository_path":main,"operation":operation}})]).unwrap();
            let action = json!({"kind":"add","path":linked,"create_branch":"feature","commitish":null});
            let preview = backend.read(route(json!({"method":"renderer_worktree_preview","action":action}))).unwrap();
            assert_eq!(backend.read(route(json!({"method":"renderer_worktree_apply","action":action,"basis":preview["basis"]}))).unwrap()["code"], 0);
            fs::write(linked.join("dirty"), "one").unwrap();
            let remove = json!({"kind":"remove","path":linked,"force":false});
            let preview = backend.read(route(json!({"method":"renderer_worktree_preview","action":remove}))).unwrap();
            assert_eq!(backend.read(route(json!({"method":"renderer_worktree_apply","action":remove,"basis":preview["basis"]}))).unwrap()["kind"], "refused");
            let force = json!({"kind":"remove","path":linked,"force":true});
            let preview = backend.read(route(json!({"method":"renderer_worktree_preview","action":force}))).unwrap();
            fs::write(linked.join("dirty"), "two").unwrap();
            assert!(backend.read(route(json!({"method":"renderer_worktree_apply","action":force,"basis":preview["basis"]}))).is_err());
            let preview = backend.read(route(json!({"method":"renderer_worktree_preview","action":force}))).unwrap();
            assert_eq!(backend.read(route(json!({"method":"renderer_worktree_apply","action":force,"basis":preview["basis"]}))).unwrap()["code"], 0);
            assert!(!linked.exists()); fs::remove_dir_all(root).unwrap();
        }).unwrap().join().unwrap();
    }

    #[test]
    fn wry_reverts_a_reviewed_commit() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use std::{fs, process::Command};
                let root = std::env::temp_dir()
                    .join(format!("desktop-wry-revert-{}", uuid::Uuid::new_v4()));
                fs::create_dir(&root).unwrap();
                let git = |args: &[&str]| {
                    let output = Command::new("git")
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
                    String::from_utf8(output.stdout).unwrap().trim().to_owned()
                };
                git(&["init", "-q"]);
                git(&["config", "user.name", "Test"]);
                git(&["config", "user.email", "test@example.invalid"]);
                fs::write(root.join("file"), "base\n").unwrap();
                git(&["add", "file"]);
                git(&["commit", "-qm", "base"]);
                fs::write(root.join("file"), "change\n").unwrap();
                git(&["commit", "-qam", "change"]);
                let commit = git(&["rev-parse", "HEAD"]);
                let backend = Backend::new(16).unwrap();
                let route = |operation: Value| {
                    backend.admit_git_info(vec![json!({"operation":{
                        "type":"revert_commit", "repository_path":root, "operation":operation
                    }})])
                };
                assert!(route(json!({"method":"stage", "paths":["file"]})).is_err());
                let preview = backend
                    .read(
                        route(json!({
                            "method":"renderer_revert_preview", "commit":commit
                        }))
                        .unwrap(),
                    )
                    .unwrap();
                assert_eq!(preview["commit"], commit);
                let basis = preview["basis"].as_str().unwrap();
                assert!(
                    backend
                        .read(
                            route(json!({
                                "method":"renderer_revert", "commit":commit, "basis":"invalid"
                            }))
                            .unwrap()
                        )
                        .is_err()
                );
                assert_eq!(git(&["rev-parse", "HEAD"]), commit);
                let result = backend
                    .read(
                        route(json!({
                            "method":"renderer_revert", "commit":commit, "basis":basis
                        }))
                        .unwrap(),
                    )
                    .unwrap();
                assert_eq!(result["outcome"], "complete");
                assert_eq!(result["head_before"], commit);
                assert_ne!(git(&["rev-parse", "HEAD"]), commit);
                assert_eq!(fs::read(root.join("file")).unwrap(), b"base\n");
                fs::remove_dir_all(root).unwrap();
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn wry_fetch_deadlines_are_bounded_and_mutation_specific() {
        let backend = Backend::new(16).unwrap();
        let request =
            json!({"remote":"origin", "expected_url":"/source", "recurse_submodules":false});
        let route = |method: &str, deadline: u64| {
            let operation = if method == "renderer_fetch" {
                json!({"method":method, "request":request, "basis":"test"})
            } else {
                json!({"method":method, "request":request})
            };
            backend.admit_git_info(vec![json!({
                "deadline_ms": deadline,
                "operation":{"type":"fetch_remote", "repository_path":"/repo", "operation":operation}
            })])
        };
        assert!(route("renderer_fetch_preview", 60_000).is_ok());
        assert!(route("renderer_fetch", 3_600_000).is_ok());
        assert!(route("renderer_fetch", 3_600_001).is_err());
        assert!(route("renderer_fetch_preview", 60_001).is_err());
        assert!(
            backend
                .admit_git_info(vec![
                    json!({"deadline_ms":60_001, "operation":{"type":"version"}})
                ])
                .is_err()
        );
    }

    #[test]
    #[cfg(unix)]
    fn wry_refreshes_reviewed_remote_head() {
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
            let root = std::env::temp_dir().join(format!("desktop-wry-remote-head-{}", uuid::Uuid::new_v4()));
            let source = root.join("source"); let target = root.join("target");
            fs::create_dir_all(&source).unwrap(); fs::create_dir(&target).unwrap();
            let git = |dir: &Path, args: &[&str]| { let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap(); assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr)); String::from_utf8(out.stdout).unwrap().trim().to_owned() };
            git(&source, &["init", "-q", "-b", "main"]);
            git(&source, &["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-qm", "base"]);
            git(&target, &["init", "-q"]); let url = source.to_str().unwrap();
            git(&target, &["remote", "add", "--", "--test", url]);
            git(&target, &["fetch", "--", "--test"]);
            let request = json!({"remote":"--test", "expected_url":url, "recurse_submodules":false, "is_background_task":true});
            let backend = Backend::new(16).unwrap();
            let route = |operation: Value| backend.admit_git_info(vec![json!({"operation":{"type":"fetch_remote", "repository_path":target, "operation":operation}})]);
            let preview = backend.read(route(json!({"method":"renderer_remote_head_preview", "request":request})).unwrap()).unwrap();
            assert!(backend.read(route(json!({"method":"renderer_remote_head", "request":request, "basis":"bad"})).unwrap()).is_err());
            let result = backend.read(route(json!({"method":"renderer_remote_head", "request":request, "basis":preview["basis"]})).unwrap()).unwrap();
            assert_eq!(result["code"], 0);
            assert_eq!(git(&target, &["symbolic-ref", "refs/remotes/--test/HEAD"]), "refs/remotes/--test/main");
            let unsupported = json!({"remote":"--test", "expected_url":"https://user:password@example.invalid/repo", "recurse_submodules":false});
            assert_eq!(backend.read(route(json!({"method":"renderer_remote_head_preview", "request":unsupported})).unwrap()).unwrap()["kind"], "unsupported");
            assert!(route(json!({"method":"stage", "paths":["file"]})).is_err());
            fs::write(target.join(".git/config"), "[broken\n").unwrap();
            assert!(backend.read(route(json!({"method":"renderer_remote_head_preview", "request":request})).unwrap()).is_err());
            fs::remove_dir_all(root).unwrap();
        }).unwrap().join().unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn wry_fetches_reviewed_remote() {
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
            let root = std::env::temp_dir().join(format!("desktop-wry-fetch-{}", uuid::Uuid::new_v4()));
            let source = root.join("source"); let target = root.join("target");
            fs::create_dir_all(&source).unwrap(); fs::create_dir(&target).unwrap();
            let git = |dir: &Path, args: &[&str]| { let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap(); assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr)); String::from_utf8(out.stdout).unwrap().trim().to_owned() };
            git(&source, &["init", "-q", "-b", "main"]); git(&source, &["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-qm", "base"]);
            git(&target, &["init", "-q"]); let url = source.to_str().unwrap(); git(&target, &["remote", "add", "--", "--test", url]);
            let request = json!({"remote":"--test", "expected_url":url, "recurse_submodules":false, "report_progress":true});
            let backend = Backend::new(16).unwrap();
            let route = |operation: Value| backend.admit_git_info(vec![json!({"operation":{"type":"fetch_remote", "repository_path":target, "operation":operation}})]);
            assert!(route(json!({"method":"stage", "paths":["file"]})).is_err());
            let preview = backend.read(route(json!({"method":"renderer_fetch_preview", "request":request})).unwrap()).unwrap();
            assert!(backend.read(route(json!({"method":"renderer_fetch", "request":request, "basis":"bad"})).unwrap()).is_err());
            let (sender, receiver) = std::sync::mpsc::channel();
            let progress = Arc::new(move |event| { let _ = sender.send(event); });
            let result = backend.read_with_progress(route(json!({"method":"renderer_fetch", "request":request, "basis":preview["basis"]})).unwrap(), Some(progress)).unwrap();
            assert_eq!(result["code"], 0);
            assert!(receiver.try_iter().any(|event| matches!(event, desktop_backend::hook_progress::CommitEvent::FetchOutput { stream: "stderr", .. })));
            assert_eq!(git(&target, &["rev-parse", "refs/remotes/--test/main"]), git(&source, &["rev-parse", "HEAD"]));
            fs::remove_dir_all(root).unwrap();
        }).unwrap().join().unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn wry_pushes_reviewed_branch_and_tag() {
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
            let root = std::env::temp_dir().join(format!("desktop-wry-push-{}", uuid::Uuid::new_v4()));
            let source = root.join("source"); let target = root.join("remote.git");
            fs::create_dir_all(&source).unwrap(); fs::create_dir(&target).unwrap();
            let git = |dir: &Path, args: &[&str]| { let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap(); assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr)); String::from_utf8(out.stdout).unwrap().trim().to_owned() };
            git(&source, &["init", "-q", "-b", "main"]); git(&source, &["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-qm", "base"]); git(&source, &["tag", "v1"]);
            git(&target, &["init", "--bare", "-q"]); let url = target.to_str().unwrap(); git(&source, &["remote", "add", "origin", url]);
            let request = json!({"remote":"origin", "expected_url":url, "local_branch":"main", "remote_branch":null, "tags":["v1"], "force_with_lease":false, "no_verify":false, "report_progress":true, "require_hook_callbacks":true});
            let backend = Backend::new(16).unwrap();
            let route = |operation: Value| backend.admit_git_info(vec![json!({"operation":{"type":"fetch_remote", "repository_path":source, "operation":operation}})]);
            let preview = backend.read(route(json!({"method":"renderer_push_preview", "request":request})).unwrap()).unwrap();
            assert!(backend.read(route(json!({"method":"renderer_push", "request":request, "basis":"bad"})).unwrap()).is_err());
            let (sender, receiver) = std::sync::mpsc::channel();
            let progress = Arc::new(move |event| { let _ = sender.send(event); });
            let result = backend.read_with_progress(route(json!({"method":"renderer_push", "request":request, "basis":preview["basis"]})).unwrap(), Some(progress)).unwrap();
            assert_eq!(result["code"], 0);
            assert!(receiver.try_iter().any(|event| matches!(event, desktop_backend::hook_progress::CommitEvent::FetchOutput { stream: "stderr", .. })));
            assert_eq!(git(&target, &["rev-parse", "refs/heads/main"]), git(&source, &["rev-parse", "HEAD"]));
            assert_eq!(git(&target, &["rev-parse", "refs/tags/v1"]), git(&source, &["rev-parse", "HEAD"]));
            let previous = git(&target, &["rev-parse", "refs/heads/main"]);
            git(&source, &["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-qm", "next"]);
            let hook = source.join(".git/hooks/pre-push");
            fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
            let preview = backend.read(route(json!({"method":"renderer_push_preview", "request":request})).unwrap()).unwrap();
            let (sender, receiver) = std::sync::mpsc::channel();
            let progress = Arc::new(move |event| { let _ = sender.send(event); });
            let blocked = backend.read_with_progress(route(json!({"method":"renderer_push", "request":request, "basis":preview["basis"]})).unwrap(), Some(progress)).unwrap();
            assert_ne!(blocked["code"], 0);
            let statuses: Vec<_> = receiver.try_iter().filter_map(|event| match event { desktop_backend::hook_progress::CommitEvent::Hook(event) if event.hook_name == "pre-push" => Some(event.status), _ => None }).collect();
            assert_eq!(statuses, ["started", "failed"]);
            assert_eq!(git(&target, &["rev-parse", "refs/heads/main"]), previous);
            fs::remove_dir_all(root).unwrap();
        }).unwrap().join().unwrap();
    }

    #[test]
    fn wry_fast_forwards_reviewed_tracking_branches() {
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
            let root = std::env::temp_dir().join(format!("desktop-wry-ff-{}", uuid::Uuid::new_v4())); fs::create_dir(&root).unwrap();
            let git = |args: &[&str]| { let out = Command::new("git").arg("-C").arg(&root).args(args).output().unwrap(); assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr)); String::from_utf8(out.stdout).unwrap().trim().to_owned() };
            git(&["init", "-q", "-b", "main"]); git(&["config", "user.name", "Test"]); git(&["config", "user.email", "test@example.invalid"]); git(&["config", "commit.gpgsign", "false"]);
            fs::write(root.join("file"), "base\n").unwrap(); git(&["add", "file"]); git(&["commit", "-qm", "base"]); let old = git(&["rev-parse", "HEAD"]); git(&["branch", "tracking", &old]);
            fs::write(root.join("file"), "next\n").unwrap(); git(&["commit", "-qam", "next"]); let new = git(&["rev-parse", "HEAD"]);
            git(&["config", "remote.origin.url", "https://example.invalid/repo"]); git(&["config", "remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"]); git(&["config", "branch.tracking.remote", "origin"]); git(&["config", "branch.tracking.merge", "refs/heads/topic"]); git(&["update-ref", "refs/remotes/origin/topic", &new]);
            fs::write(root.join(".git/FETCH_HEAD"), "keep\n").unwrap();
            let request = json!({"branches":[{"ref":"refs/heads/tracking","sha":old,"upstream_ref":"refs/remotes/origin/topic","upstream_sha":new}],"recurse_submodules":false});
            let backend = Backend::new(16).unwrap();
            let route = |operation: Value| backend.admit_git_info(vec![json!({"operation":{"type":"fast_forward_branches","repository_path":root,"operation":operation}})]);
            assert!(route(json!({"method":"stage","paths":["file"]})).is_err());
            let preview = backend.read(route(json!({"method":"renderer_fast_forward_preview","request":request})).unwrap()).unwrap();
            assert!(backend.read(route(json!({"method":"renderer_fast_forward","request":request,"basis":"bad"})).unwrap()).is_err());
            let result = backend.read(route(json!({"method":"renderer_fast_forward","request":request,"basis":preview["basis"]})).unwrap()).unwrap();
            assert_eq!(result["code"], 0);
            assert_eq!(git(&["rev-parse", "tracking"]), new);
            assert_eq!(fs::read(root.join(".git/FETCH_HEAD")).unwrap(), b"keep\n");
            fs::remove_dir_all(root).unwrap();
        }).unwrap().join().unwrap();
    }

    #[test]
    fn wry_reads_last_fetch_for_selected_repository() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let root = std::env::temp_dir()
                    .join(format!("desktop-wry-last-fetch-{}", uuid::Uuid::new_v4()));
                fs::create_dir(&root).unwrap();
                assert!(
                    Command::new("git")
                        .args(["init", "-q"])
                        .arg(&root)
                        .status()
                        .unwrap()
                        .success()
                );
                let backend = Backend::new(4).unwrap();
                let read = || {
                    backend
                        .read_args(vec![
                            json!({"repository_path":root,"method":"renderer_last_fetch"}),
                        ])
                        .unwrap()
                };
                assert_eq!(read()["kind"], "unavailable");
                fs::write(root.join(".git/FETCH_HEAD"), "").unwrap();
                assert_eq!(read()["kind"], "empty");
                fs::write(root.join(".git/FETCH_HEAD"), "fetch\n").unwrap();
                assert_eq!(read()["kind"], "fetched");
                fs::remove_dir_all(root).unwrap();
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn wry_reads_apply_rebase_progress() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let root = std::env::temp_dir()
                    .join(format!("desktop-wry-apply-rebase-{}", uuid::Uuid::new_v4()));
                fs::create_dir(&root).unwrap();
                assert!(
                    Command::new("git")
                        .args(["init", "-q"])
                        .arg(&root)
                        .status()
                        .unwrap()
                        .success()
                );
                let apply = root.join(".git/rebase-apply");
                fs::create_dir(&apply).unwrap();
                fs::write(
                    root.join(".git/REBASE_HEAD"),
                    format!("{}\n", "a".repeat(40)),
                )
                .unwrap();
                for (name, content) in [
                    ("rebasing", ""),
                    ("orig-head", "original\n"),
                    ("head-name", "refs/heads/topic\n"),
                    ("onto", "base\n"),
                    ("next", "2\n"),
                    ("last", "3\n"),
                ] {
                    fs::write(apply.join(name), content).unwrap();
                }
                let backend = Backend::new(4).unwrap();
                let state = backend
                    .read_args(vec![
                        json!({"repository_path":root,"method":"renderer_rebase_state"}),
                    ])
                    .unwrap();
                assert_eq!(state["backend"], "apply");
                assert_eq!(state["progress"]["branch"], "refs/heads/topic\n");
                assert_eq!(state["progress"]["current"], "2\n");
                assert_eq!(state["progress"]["total"], "3\n");
                fs::remove_dir_all(root).unwrap();
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn wry_resets_a_reviewed_commit() {
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
            let root = std::env::temp_dir().join(format!("desktop-wry-reset-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let git = |args: &[&str]| { let out = Command::new("git").arg("-C").arg(&root).args(args).output().unwrap(); assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr)); String::from_utf8(out.stdout).unwrap().trim().to_owned() };
            git(&["init", "-q", "-b", "main"]); git(&["config", "user.name", "Test"]); git(&["config", "user.email", "test@example.invalid"]); git(&["config", "commit.gpgsign", "false"]);
            fs::write(root.join("file"), "base\n").unwrap(); git(&["add", "file"]); git(&["commit", "-qm", "base"]);
            let target = git(&["rev-parse", "HEAD"]);
            fs::write(root.join("file"), "new\n").unwrap(); git(&["commit", "-qam", "new"]); let head = git(&["rev-parse", "HEAD"]);
            fs::write(root.join("file"), "local\n").unwrap();
            let backend = Backend::new(16).unwrap();
            let request = json!({"target":target,"head":head,"branch":"refs/heads/main","undo":true});
            let route = |operation: Value| backend.admit_git_info(vec![json!({"operation":{"type":"mixed_reset","repository_path":root,"operation":operation}})]);
            assert!(route(json!({"method":"stage","paths":["file"]})).is_err());
            let preview = backend.read(route(json!({"method":"renderer_mixed_reset_preview","request":request})).unwrap()).unwrap();
            assert!(backend.read(route(json!({"method":"renderer_mixed_reset","request":request,"basis":"bad"})).unwrap()).is_err());
            let result = backend.read(route(json!({"method":"renderer_mixed_reset","request":request,"basis":preview["basis"]})).unwrap()).unwrap();
            assert_eq!(result["code"], 0);
            assert_eq!(git(&["rev-parse", "HEAD"]), target);
            assert_eq!(git(&["show", ":file"]), "base");
            assert_eq!(fs::read(root.join("file")).unwrap(), b"local\n");
            fs::remove_dir_all(root).unwrap();
        }).unwrap().join().unwrap();
    }

    #[test]
    fn wry_continues_reviewed_rebase() {
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
            let root = std::env::temp_dir().join(format!("desktop-wry-rebase-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let run = |args: &[&str]| Command::new("git").arg("-C").arg(&root).args(args).output().unwrap();
            let git = |args: &[&str]| { let out = run(args); assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr)); String::from_utf8(out.stdout).unwrap().trim().to_owned() };
            git(&["init", "-q", "-b", "main"]);
            git(&["config", "user.name", "Test"]); git(&["config", "user.email", "test@example.invalid"]);
            fs::write(root.join("file"), "base\n").unwrap(); git(&["add", "file"]); git(&["commit", "-qm", "base"]);
            git(&["switch", "-qc", "source"]);
            fs::write(root.join("file"), "source\n").unwrap(); git(&["commit", "-qam", "source"]);
            let target = git(&["rev-parse", "HEAD"]);
            git(&["switch", "-q", "main"]);
            fs::write(root.join("file"), "main\n").unwrap(); git(&["commit", "-qam", "main"]);
            let base = git(&["rev-parse", "HEAD"]);
            git(&["switch", "-q", "source"]);
            let backend = Backend::new(16).unwrap();
            let start = |operation: Value| backend.admit_git_info(vec![json!({"operation":{"type":"start_rebase","repository_path":root,"operation":operation}})]);
            let request = json!({"base_ref":"refs/heads/main","base_head":base,"target_ref":"refs/heads/source","target_head":target,"current_head":target,"current_branch":"refs/heads/source"});
            assert!(start(json!({"method":"stage","paths":["file"]})).is_err());
            let preview = backend.read(start(json!({"method":"renderer_rebase_start_preview","request":request})).unwrap()).unwrap();
            assert!(backend.read(start(json!({"method":"renderer_rebase_start","request":request,"basis":"bad"})).unwrap()).is_err());
            let started = backend.read(start(json!({"method":"renderer_rebase_start","request":request,"basis":preview["basis"]})).unwrap()).unwrap();
            assert_eq!(started["outcome"], "conflict");
            fs::write(root.join("file"), "resolved\n").unwrap(); git(&["add", "file"]);
            let route = |operation: Value| backend.admit_git_info(vec![json!({"operation":{"type":"continue_rebase","repository_path":root,"operation":operation}})]);
            assert!(route(json!({"method":"stage","paths":["file"]})).is_err());
            let preview = backend.read(route(json!({"method":"renderer_rebase_continue_preview","expected":target})).unwrap()).unwrap();
            assert!(backend.read(route(json!({"method":"renderer_rebase_continue","expected":target,"basis":"bad","empty":false,"no_verify":false})).unwrap()).is_err());
            let result = backend.read(route(json!({"method":"renderer_rebase_continue","expected":target,"basis":preview["basis"],"empty":false,"no_verify":false})).unwrap()).unwrap();
            assert_eq!(result["outcome"], "complete");
            assert_eq!(git(&["show", "HEAD:file"]), "resolved");
            fs::remove_dir_all(root).unwrap();
        }).unwrap().join().unwrap();
    }

    #[test]
    fn wry_continues_reviewed_cherry_pick() {
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
            use std::{fs, process::Command};
            let root = std::env::temp_dir().join(format!("desktop-wry-pick-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let run = |args: &[&str]| Command::new("git").arg("-C").arg(&root).args(args).output().unwrap();
            let git = |args: &[&str]| { let out = run(args); assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr)); String::from_utf8(out.stdout).unwrap().trim().to_owned() };
            git(&["init", "-q", "-b", "main"]);
            git(&["config", "user.name", "Test"]);
            git(&["config", "user.email", "test@example.invalid"]);
            fs::write(root.join("file"), "base\n").unwrap();
            git(&["add", "file"]); git(&["commit", "-qm", "base"]);
            git(&["switch", "-qc", "source"]);
            fs::write(root.join("file"), "source\n").unwrap(); git(&["commit", "-qam", "source"]);
            let target = git(&["rev-parse", "HEAD"]);
            git(&["switch", "-q", "main"]);
            fs::write(root.join("file"), "destination\n").unwrap(); git(&["commit", "-qam", "destination"]);
            let head = git(&["rev-parse", "HEAD"]);
            let backend = Backend::new(16).unwrap();
            let start = |operation: Value| backend.admit_git_info(vec![json!({"operation":{"type":"start_cherry_pick","repository_path":root,"operation":operation}})]);
            assert!(start(json!({"method":"stage","paths":["file"]})).is_err());
            let preview = backend.read(start(json!({"method":"renderer_cherry_pick_start_preview","commits":[target],"expected_head":head,"expected_branch":"refs/heads/main"})).unwrap()).unwrap();
            assert!(backend.read(start(json!({"method":"renderer_cherry_pick_start","commits":[target],"expected_head":head,"expected_branch":"refs/heads/main","basis":"bad"})).unwrap()).is_err());
            let pick = backend.read(start(json!({"method":"renderer_cherry_pick_start","commits":[target],"expected_head":head,"expected_branch":"refs/heads/main","basis":preview["basis"]})).unwrap()).unwrap();
            assert_eq!(pick["outcome"], "conflict");
            fs::write(root.join("file"), "resolution\n").unwrap(); git(&["add", "file"]);
            let route = |operation: Value| backend.admit_git_info(vec![json!({"operation":{"type":"continue_cherry_pick","repository_path":root,"operation":operation}})]);
            assert!(route(json!({"method":"stage","paths":["file"]})).is_err());
            let preview = backend.read(route(json!({"method":"renderer_cherry_pick_continue_preview","expected":target})).unwrap()).unwrap();
            let basis = preview["basis"].as_str().unwrap();
            assert!(backend.read(route(json!({"method":"renderer_cherry_pick_continue","expected":target,"basis":"bad","empty":false})).unwrap()).is_err());
            let result = backend.read(route(json!({"method":"renderer_cherry_pick_continue","expected":target,"basis":basis,"empty":false})).unwrap()).unwrap();
            assert_eq!(result["outcome"], "complete");
            assert_eq!(git(&["show", "HEAD:file"]), "resolution");
            fs::remove_dir_all(root).unwrap();
        }).unwrap().join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn wry_resolves_a_reviewed_conflict_and_retains_undo() {
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
            use std::{fs, process::Command};
            let root = std::env::temp_dir().join(format!("desktop-wry-conflict-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let git = |args: &[&str]| {
                let result = Command::new("git").arg("-C").arg(&root).args(args).output().unwrap();
                assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
                result.stdout
            };
            git(&["init", "-q"]);
            git(&["config", "user.name", "Test"]);
            git(&["config", "user.email", "test@example.invalid"]);
            fs::write(root.join("file"), "base\n").unwrap();
            git(&["add", "file"]); git(&["commit", "-qm", "base"]);
            git(&["checkout", "-qb", "side"]);
            fs::write(root.join("file"), "side\n").unwrap();
            git(&["commit", "-qam", "side"]);
            let source_head = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap().trim().to_owned();
            git(&["checkout", "-q", "-"]);
            fs::write(root.join("file"), "main\n").unwrap();
            git(&["commit", "-qam", "main"]);
            let backend = Backend::new(16).unwrap();
            let current_head = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap().trim().to_owned();
            let current_ref = String::from_utf8(git(&["symbolic-ref", "HEAD"])).unwrap().trim().to_owned();
            let request = json!({"source_ref":"refs/heads/side","source_head":source_head,"current_ref":current_ref,"current_head":current_head,"squash":false,"no_verify":false});
            let start = |operation: Value| backend.admit_git_info(vec![json!({"operation":{"type":"start_merge","repository_path":root,"operation":operation}})]);
            assert!(start(json!({"method":"stage","paths":["file"]})).is_err());
            let preview = backend.read(start(json!({"method":"renderer_merge_start_preview","request":request})).unwrap()).unwrap();
            assert!(backend.read(start(json!({"method":"renderer_merge_start","request":request,"basis":"bad"})).unwrap()).is_err());
            let result = backend.read(start(json!({"method":"renderer_merge_start","request":request,"basis":preview["basis"]})).unwrap()).unwrap();
            assert_eq!(result["outcome"], "conflict");
            let call = |operation: Value| backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":root,"operation":operation}})]).unwrap());
            let detail = call(json!({"method":"conflict_detail","path":"file"})).unwrap();
            let basis = detail["basis"].as_str().unwrap();
            let result = call(json!({"method":"resolve_conflict","path":"file","basis":basis,"choice":"ours"})).unwrap();
            assert_eq!(result["code"], 0);
            assert!(result["undo_id"].is_string());
            assert_eq!(fs::read(root.join("file")).unwrap(), b"main\n");
            assert!(git(&["ls-files", "--unmerged", "--", "file"]).is_empty());
            let abort = |kind: &str, action: &str| backend.admit_git_info(vec![json!({"operation":{"type":"abort_git_operation","repository_path":root,"operation":{"method":"recover","kind":kind,"action":action}}})]);
            assert!(abort("merge", "continue").is_err());
            assert!(abort("revert", "abort").is_err());
            assert!(backend.read(abort("rebase", "abort").unwrap()).is_err());
            assert_eq!(backend.read(abort("merge", "abort").unwrap()).unwrap()["code"], 0);
            assert!(!root.join(".git/MERGE_HEAD").exists());
            assert_eq!(fs::read(root.join("file")).unwrap(), b"main\n");
            assert!(!Command::new("git").arg("-C").arg(&root).args(["merge", "side"]).output().unwrap().status.success());
            fs::write(root.join("file"), "resolved\n").unwrap();
            git(&["add", "file"]);
            let finish = |operation: Value| backend.admit_git_info(vec![json!({"operation":{"type":"finish_merge_commit","repository_path":root,"operation":operation}})]);
            assert!(finish(json!({"method":"recover","kind":"merge","action":"abort"})).is_err());
            use std::os::unix::fs::PermissionsExt;
            let hook = root.join(".git/hooks/pre-commit");
            fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
            let rejected = backend.read(finish(json!({"method":"renderer_finish_merge_commit"})).unwrap()).unwrap();
            assert_ne!(rejected["code"], 0);
            assert!(root.join(".git/MERGE_HEAD").exists());
            fs::remove_file(hook).unwrap();
            let result = backend.read(finish(json!({"method":"renderer_finish_merge_commit"})).unwrap()).unwrap();
            assert_eq!(result["code"], 0, "{result}");
            let parents = String::from_utf8(git(&["rev-list", "--parents", "-n", "1", "HEAD"])).unwrap();
            assert_eq!(parents.split_whitespace().count(), 3);
            let head = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap().trim().to_owned();
            let branch = String::from_utf8(git(&["symbolic-ref", "HEAD"])).unwrap().trim().to_owned();
            fs::write(root.join(".git/SQUASH_MSG"), "squash\n").unwrap();
            fs::write(root.join("file"), "changed\n").unwrap();
            let squash = |expected_head: &str| backend.admit_git_info(vec![json!({"operation":{"type":"abort_git_operation","repository_path":root,"operation":{"method":"renderer_abort_squash","expected_head":expected_head,"expected_branch":branch}}})]).unwrap();
            assert!(backend.read(squash(&"0".repeat(head.len()))).is_err());
            assert_eq!(fs::read(root.join("file")).unwrap(), b"changed\n");
            assert_eq!(backend.read(squash(&head)).unwrap()["code"], 0);
            assert_eq!(fs::read(root.join("file")).unwrap(), b"resolved\n");
            assert!(!root.join(".git/SQUASH_MSG").exists());
            fs::remove_dir_all(root).unwrap();
        }).unwrap().join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn wry_cancels_a_running_commit_hook_with_checkpoint_retained() {
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
            use std::os::unix::fs::PermissionsExt;
            let root = std::env::temp_dir().join(format!("desktop-wry-commit-cancel-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            let git = |args: &[&str]| {
                let output = std::process::Command::new("git").arg("-C").arg(&root).args(args).output().unwrap();
                assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
                String::from_utf8(output.stdout).unwrap().trim().to_owned()
            };
            git(&["init", "-q"]);
            git(&["config", "user.name", "Test"]);
            git(&["config", "user.email", "test@example.invalid"]);
            git(&["config", "commit.gpgsign", "false"]);
            std::fs::write(root.join("file"), b"before\n").unwrap();
            git(&["add", "file"]);
            git(&["commit", "-qm", "initial"]);
            let head = git(&["rev-parse", "HEAD"]);
            let original_index = std::fs::read(root.join(".git/index")).unwrap();
            std::fs::write(root.join("file"), b"selected\n").unwrap();
            let hook = root.join(".git/hooks/pre-commit");
            std::fs::write(&hook, "#!/bin/sh\nprintf started > .git/commit-hook-started\nprintf live-output >&2\nsleep 60\nprintf finished > .git/commit-hook-finished\n").unwrap();
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
            let backend = Backend::new(16).unwrap();
            let call = |kind: &str, operation: Value| {
                backend.read(backend.admit_git_info(vec![json!({"operation":{"type":kind,"repository_path":root,"operation":operation}})]).unwrap())
            };
            let review = call("selected_commit_preview", json!({"method":"selected_commit_preview","paths":["file"],"remove_paths":[],"partial_selections":[],"allow_empty":false,"amend_head":null,"expected_head":null})).unwrap();
            let prepared = call("commit_checkpoint", json!({"method":"prepare_selected_commit","paths":["file"],"remove_paths":[],"partial_selections":[],"allow_empty":false,"amend_head":null,"expected_head":null,"basis":review["basis"]})).unwrap();
            let checkpoint = prepared["id"].as_str().unwrap();
            let request_id = uuid::Uuid::new_v4();
            let job = backend.admit_git_info(vec![json!({"request_id":request_id,"operation":{"type":"commit_checkpoint","repository_path":root,"operation":{"method":"publish_selected_commit","checkpoint_id":checkpoint,"message":"message\n","no_verify":false,"sign_off":false,"allow_large_files":false}}})]).unwrap();
            let (progress_tx, progress_rx) = std::sync::mpsc::channel();
            let progress = Arc::new(move |event: desktop_backend::hook_progress::CommitEvent| { let _ = progress_tx.send(event); });
            std::thread::scope(|scope| {
                let worker = std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn_scoped(scope, || backend.read_with_progress(job, Some(progress))).unwrap();
                let marker = root.join(".git/commit-hook-started");
                let deadline = std::time::Instant::now() + Duration::from_secs(15);
                while !marker.exists() && !worker.is_finished() && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                if !marker.exists() {
                    backend.cancel(&[json!({"request_id":request_id})]).unwrap();
                    let result = worker.join().unwrap();
                    if result.as_ref().err().is_some_and(|error| error.to_string().contains("does not support")) { return; }
                    panic!("commit hook did not start: {result:?}");
                }
                let mut saw_hook = false;
                let mut saw_output = false;
                while !saw_hook || !saw_output {
                    match progress_rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                        desktop_backend::hook_progress::CommitEvent::Hook(progress) => {
                            assert_eq!(progress.checkpoint_id, checkpoint);
                            if progress.hook_name == "pre-commit" && progress.status == "started" { saw_hook = true; }
                        }
                        desktop_backend::hook_progress::CommitEvent::Output { checkpoint_id, stream, data } => {
                            if checkpoint_id == checkpoint && stream == "stderr" && data == "bGl2ZS1vdXRwdXQ=" { saw_output = true; }
                        }
                        desktop_backend::hook_progress::CommitEvent::FetchOutput { .. } => {}
                        desktop_backend::hook_progress::CommitEvent::HookDecision { .. } => {}
                    }
                }
                backend.cancel(&[json!({"request_id":request_id})]).unwrap();
                let result = worker.join().unwrap();
                assert!(matches!(result, Err(AppError::Coded { code, message, .. }) if code == "cancelled" && message.contains("inspect commit checkpoints")));
                assert_eq!(git(&["rev-parse", "HEAD"]), head);
                assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), original_index);
                assert!(!root.join(".git/commit-hook-finished").exists());
                assert_eq!(call("commit_checkpoint", json!({"method":"inspect_commit_checkpoint","checkpoint_id":checkpoint})).unwrap()["state"], "publishing");
                assert_eq!(call("commit_checkpoint", json!({"method":"close_unchanged_commit_checkpoint","checkpoint_id":checkpoint})).unwrap()["state"], "closed");
            });
            std::fs::remove_dir_all(root).unwrap();
        }).unwrap().join().unwrap();
    }
    use std::{fs, process::Command};

    #[cfg(unix)]
    #[test]
    fn fake_wry_copilot_runtime() {
        use std::io::{BufRead, Read, Write};
        if std::env::var_os("DESKTOP_WRY_FAKE_RUNTIME").is_none() {
            return;
        }
        assert_eq!(
            std::env::var("COPILOT_SDK_AUTH_TOKEN").unwrap(),
            "wry-selected-token"
        );
        assert_eq!(std::env::var("GH_HOST").unwrap(), "github.com");
        assert!(std::env::var_os("GH_TOKEN").is_none());
        let mut input = std::io::BufReader::new(std::io::stdin());
        let mut output = fs::OpenOptions::new()
            .write(true)
            .open("/dev/fd/3")
            .unwrap();
        loop {
            let mut header = String::new();
            if input.read_line(&mut header).unwrap() == 0 {
                break;
            }
            let length: usize = header
                .trim()
                .strip_prefix("Content-Length: ")
                .unwrap()
                .parse()
                .unwrap();
            let mut blank = String::new();
            input.read_line(&mut blank).unwrap();
            assert_eq!(blank, "\r\n");
            let mut body = vec![0; length];
            input.read_exact(&mut body).unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap();
            let result = match request["method"].as_str().unwrap() {
                "connect" => json!({"protocolVersion":3}),
                "session.create" => {
                    assert_eq!(request["params"]["systemMessage"]["mode"], "append");
                    assert_eq!(request["params"]["availableTools"], json!([]));
                    json!({"sessionId":request["params"]["sessionId"]})
                }
                "session.options.update" => {
                    assert_eq!(request["params"]["coauthorEnabled"], false);
                    json!({})
                }
                "session.sendAndWait" => {
                    let prompt = request["params"]["prompt"].as_str().unwrap();
                    assert!(
                        prompt.contains("+selected content")
                            && !prompt.contains("unselected content")
                    );
                    json!({"aborted":false,"response":{"type":"assistant.message","data":{"content":"{\"title\":\"Fix selected\",\"description\":\"Details\"}"}}})
                }
                _ => panic!("unexpected fixture method"),
            };
            let reply = json!({"jsonrpc":"2.0","id":request["id"],"result":result}).to_string();
            write!(output, "Content-Length: {}\r\n\r\n{}", reply.len(), reply).unwrap();
            output.flush().unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn wry_copilot_generation_uses_host_runtime_and_verified_account() {
        use std::os::unix::fs::PermissionsExt;
        const CHILD: &str = "DESKTOP_WRY_COPILOT_TEST_CHILD";
        if let Some(root) = std::env::var_os(CHILD) {
            let root = PathBuf::from(root);
            let backend = Backend::new(4)
                .unwrap()
                .with_registry(desktop_backend::Registry::new(root.join("registry.json")))
                .with_copilot_runtime_root(Some(root.join("runtime")));
            let request = json!({"repository_path":root.join("repo"),"operation":{
                "method":"copilot_generate_commit_message", "source":"working_tree", "paths":["selected"],
                "account":{"host":"github.com","login":"alice","id":42}, "rule_descriptions":["Use an issue number"]
            }});
            let result = backend
                .read(backend.admit_copilot(vec![request]).unwrap())
                .unwrap();
            assert_eq!(result["summary"], "Fix selected");
            assert_eq!(result["description"], "Details");
            assert!(!root.join("repo/.git/index").exists());
            return;
        }
        let root =
            std::env::temp_dir().join(format!("desktop-wry-copilot-live-{}", uuid::Uuid::new_v4()));
        let repo = root.join("repo");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(&repo)
                .status()
                .unwrap()
                .success()
        );
        fs::write(repo.join("selected"), b"selected content\n").unwrap();
        fs::write(repo.join("unselected"), b"unselected content\n").unwrap();
        let gh = bin.join("gh");
        fs::write(&gh, r##"#!/bin/sh
case "$1 $2" in
  'auth status') [ -z "${GH_TOKEN+x}" ] || exit 90; printf '{"hosts":{"github.com":[{"login":"Alice","state":"success"}]}}\n';;
  'auth token') [ -z "${GH_TOKEN+x}" ] || exit 91; printf 'wry-selected-token\n';;
  'api --method') [ "$GH_TOKEN" = wry-selected-token ] || exit 92; printf '{"login":"Alice","id":42}\n';;
  *) exit 93;;
esac
"##).unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o700)).unwrap();
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            _ => return,
        };
        let platform = if std::env::consts::OS == "macos" {
            "darwin"
        } else {
            std::env::consts::OS
        };
        let runtime = root
            .join("runtime/prebuilds")
            .join(format!("{platform}-{arch}"));
        fs::create_dir_all(&runtime).unwrap();
        let exe = std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .replace('\'', "'\\''");
        let wrapper = runtime.join("copilot-runtime");
        fs::write(&wrapper, format!("#!/bin/sh\nexec 3>&1\nexec '{exe}' --exact backend::tests::fake_wry_copilot_runtime --nocapture >/dev/null\n")).unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        let path = std::env::join_paths(
            std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "backend::tests::wry_copilot_generation_uses_host_runtime_and_verified_account",
                "--nocapture",
            ])
            .env(CHILD, &root)
            .env("DESKTOP_WRY_FAKE_RUNTIME", "1")
            .env("HOME", &root)
            .env("PATH", path)
            .env("GH_TOKEN", "wrong-token")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", root.join("gitconfig"))
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "host service test failed: {}",
            String::from_utf8_lossy(&child.stderr)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scoped_copilot_admission_fences_account_and_cancellation() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(scoped_copilot_admission_fences_account_and_cancellation_inner)
            .unwrap()
            .join()
            .unwrap();
    }

    fn scoped_copilot_admission_fences_account_and_cancellation_inner() {
        use desktop_backend::{AccountAssignment, StoredAccount};
        let root =
            std::env::temp_dir().join(format!("desktop-wry-copilot-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.join("selected.txt"), b"selected line\n").unwrap();
        let registry = desktop_backend::Registry::new(root.join("state/registry.json"));
        let record = registry.add(root.clone(), true).unwrap().records.remove(0);
        registry
            .compare_and_replace_account(
                &record.id,
                0,
                StoredAccount {
                    revision: 1,
                    assignment: AccountAssignment::None {},
                    association: None,
                    source_remote: None,
                },
            )
            .unwrap();
        let backend = Backend::new(1).unwrap().with_registry(registry);
        let preview = || json!({"repository_path":root,"operation":{"method":"copilot_commit_diff_detail","path":"selected.txt"}});
        let value = backend
            .read(backend.admit_copilot(vec![preview()]).unwrap())
            .unwrap();
        assert_eq!(value["can_select"], true);
        assert_eq!(value["hunks"][0]["lines"][0]["text"], "selected line");
        for operation in [
            json!({"method":"stage","paths":["selected.txt"]}),
            json!({"method":"copilot_generate_commit_message"}),
            json!({"method":"copilot_generate_commit_message","account":{"host":"github.com","login":"alice","id":1,"token":"not-allowed"}}),
        ] {
            assert!(
                backend
                    .admit_copilot(vec![json!({"repository_path":root,"operation":operation})])
                    .is_err()
            );
        }
        let malformed = backend.admit_copilot(vec![json!({"repository_path":root,"operation":{
            "method":"copilot_generate_commit_message", "account":{"host":"github.com","login":"alice","id":1},
            "provider":{"type":"azure","baseUrl":"https://provider.example","azure":"synthetic-private-credential"}
        }})]);
        match malformed {
            Ok(_) => panic!("malformed provider was accepted"),
            Err(error) => assert!(!error.to_string().contains("synthetic-private-credential")),
        }
        let job = backend.admit_copilot(vec![json!({"repository_path":root,"operation":{"method":"copilot_generate_commit_message","account":{"host":"github.com","login":"alice","id":1},"source":"working_tree","paths":["selected.txt"]}})]).unwrap();
        assert!(
            backend
                .read(job)
                .unwrap_err()
                .to_string()
                .contains("No GitHub account")
        );
        assert!(backend.admit_copilot(vec![json!({"repository_path":"relative","operation":{"method":"copilot_commit_diff_detail","path":"x"}})]).is_err());
        assert!(backend.admit_copilot(vec![json!({"repository_path":root,"operation":{"method":"copilot_commit_diff_detail","path":"x"},"deadline_ms":330001})]).is_err());
        let id = uuid::Uuid::new_v4();
        let mut request = preview();
        request["request_id"] = json!(id);
        let job = backend.admit_copilot(vec![request.clone()]).unwrap();
        assert!(backend.admit_copilot(vec![request]).is_err());
        backend.cancel(&[json!({"request_id":id})]).unwrap();
        assert!(
            matches!(backend.read(job), Err(AppError::Coded { code, .. }) if code == "cancelled")
        );
        let git = |arguments: &[&str]| {
            let output = Command::new("git")
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
            output.stdout
        };
        git(&["add", "--", "selected.txt"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "initial",
        ]);
        git(&["mv", "--", "selected.txt", "renamed.txt"]);
        fs::write(root.join("renamed.txt"), b"changed line\n").unwrap();
        fs::write(root.join("selected.txt"), b"replacement\n").unwrap();
        git(&["reset", "--quiet", "HEAD", "--", "."]);
        let diff = backend.read(backend.admit_copilot(vec![json!({"repository_path":root,"operation":{"method":"copilot_commit_diff_detail","path":"renamed.txt","original_path":"selected.txt"}})]).unwrap()).unwrap();
        let lines: Vec<_> = diff["hunks"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|hunk| hunk["lines"].as_array().unwrap())
            .filter(|line| line["kind"] != "context")
            .map(|line| line["id"].clone())
            .collect();
        let stage = backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":root,"operation":{"method":"select_renamed_diff","path":"renamed.txt","original_path":"selected.txt","basis":diff["basis"],"lines":lines}}})]).unwrap();
        assert_eq!(backend.read(stage).unwrap()["code"], 0);
        assert_eq!(git(&["show", ":renamed.txt"]), b"changed line\n");
        assert!(
            !Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["cat-file", "-e", ":selected.txt"])
                .output()
                .unwrap()
                .status
                .success()
        );
        assert_eq!(
            fs::read(root.join("selected.txt")).unwrap(),
            b"replacement\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn trust_repository_uses_the_native_job() {
        const CHILD: &str = "DESKTOP_TEST_TRUST_CHILD";
        if let Some(root) = std::env::var_os(CHILD) {
            let root = std::path::PathBuf::from(root);
            let backend = Backend::new(4).unwrap();
            let query = || {
                backend.read_args(vec![
                    json!({"repository_path":root,"method":"trust_repository"}),
                ])
            };
            assert_eq!(query().unwrap(), json!({"trusted":true}));
            let config = fs::read(root.join("global")).unwrap();
            assert_eq!(query().unwrap(), json!({"trusted":true}));
            assert_eq!(fs::read(root.join("global")).unwrap(), config);
            return;
        }
        let root = std::env::temp_dir().join(format!("desktop-trust-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "backend::tests::trust_repository_uses_the_native_job",
                "--nocapture",
            ])
            .env(CHILD, &root)
            .env("GIT_CONFIG_GLOBAL", root.join("global"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn global_identity_uses_isolated_git_config_and_validates_requests() {
        const CHILD: &str = "DESKTOP_WRY_IDENTITY_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let backend = Backend::new(2).unwrap();
            let repository = PathBuf::from(std::env::var_os("HOME").unwrap()).join("repo");
            assert!(
                Command::new("git")
                    .args(["init", "-q"])
                    .arg(&repository)
                    .status()
                    .unwrap()
                    .success()
            );
            let read_identity = |only_local: bool| {
                backend.read_args(vec![json!({"repository_path":repository,"method":"git_identity_value","key":"user.name","only_local":only_local})]).unwrap()
            };
            assert_eq!(read_identity(true), Value::Null);
            assert_eq!(read_identity(false), "Before");
            let local_id = uuid::Uuid::new_v4();
            let cancelled_local = backend.admit_git_info(vec![json!({"operation":{"type":"set_repository","repository_path":repository,"key":"user.name","value":"Must not write"},"request_id":local_id})]).unwrap();
            backend.cancel(&[json!({"request_id":local_id})]).unwrap();
            assert!(backend.read(cancelled_local).is_err());
            assert_eq!(read_identity(true), Value::Null);
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&repository)
                    .args(["config", "--local", "user.name", "Local"])
                    .status()
                    .unwrap()
                    .success()
            );
            assert_eq!(read_identity(true), "Local");
            assert!(backend.read_args(vec![json!({"repository_path":repository,"method":"git_identity_value","key":"credential.helper"})]).is_err());
            assert!(
                backend
                    .read_args(vec![
                        json!({"repository_path":repository,"method":"status","key":"user.name"})
                    ])
                    .is_err()
            );
            let run = |operation: Value| {
                backend.read(
                    backend
                        .admit_git_info(vec![json!({"operation":operation})])
                        .unwrap(),
                )
            };
            let first = run(json!({"type":"read"})).unwrap();
            let config_path = PathBuf::from(std::env::var_os("GIT_CONFIG_GLOBAL").unwrap());
            let saved_config = fs::read(&config_path).unwrap();
            fs::remove_file(&config_path).unwrap();
            assert_eq!(
                run(json!({"type":"global_config_path"}))
                    .unwrap()
                    .as_str()
                    .unwrap(),
                config_path.to_str().unwrap()
            );
            assert!(config_path.is_file());
            fs::write(&config_path, saved_config).unwrap();
            let expected_version = Command::new("git").arg("--version").output().unwrap();
            assert!(expected_version.status.success());
            assert_eq!(
                run(json!({"type":"version"})).unwrap().as_str().unwrap(),
                String::from_utf8(expected_version.stdout)
                    .unwrap()
                    .trim()
                    .strip_prefix("git version ")
                    .unwrap()
            );
            assert_eq!(first["values"]["user.name"], "Before");
            assert_eq!(first["values"]["init.defaultbranch"], Value::Null);
            assert_eq!(
                run(json!({"type":"set","key":"init.defaultbranch","value":"trunk"})).unwrap()["values"]
                    ["init.defaultbranch"],
                "trunk"
            );
            assert!(
                run(json!({"type":"set","key":"init.defaultbranch","value":"bad..branch"}))
                    .is_err()
            );
            assert_eq!(
                run(json!({"type":"read"})).unwrap()["values"]["init.defaultbranch"],
                "trunk"
            );
            assert!(first["values"].get("credential.helper").is_none());
            run(json!({"type":"set_repository","repository_path":repository,"key":"user.name","value":"Updated"})).unwrap();
            assert_eq!(read_identity(true), "Updated");
            run(json!({"type":"unset_repository","repository_path":repository,"key":"user.name"}))
                .unwrap();
            run(json!({"type":"unset_repository","repository_path":repository,"key":"user.name"}))
                .unwrap();
            assert_eq!(read_identity(true), Value::Null);
            assert_eq!(read_identity(false), "Before");
            assert!(backend.admit_git_info(vec![json!({"operation":{"type":"set_repository","repository_path":"relative","key":"user.name","value":"bad"}})]).is_err());
            assert!(backend.admit_git_info(vec![json!({"operation":{"type":"lfs_install_repository","repository_path":"relative","force":true}})]).is_err());
            #[cfg(unix)]
            if run(json!({"type":"lfs_status"})).unwrap()["available"] == true {
                use std::os::unix::fs::PermissionsExt;
                let hook = repository.join(".git/hooks/pre-push");
                fs::create_dir_all(hook.parent().unwrap()).unwrap();
                fs::write(&hook, "#!/bin/sh\necho custom-test-hook\n").unwrap();
                fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
                let conflict = run(json!({"type":"lfs_install_repository","repository_path":repository,"force":false})).unwrap();
                assert_eq!(conflict["error_kind"], "configuration");
                assert!(
                    fs::read_to_string(&hook)
                        .unwrap()
                        .contains("custom-test-hook")
                );
                assert_eq!(run(json!({"type":"lfs_install_repository","repository_path":repository,"force":true})).unwrap()["code"], 0);
                assert!(
                    fs::read_to_string(&hook)
                        .unwrap()
                        .contains("git lfs pre-push")
                );
            }
            assert!(backend.admit_git_info(vec![json!({"operation":{"type":"unset_repository","repository_path":repository,"key":"credential.helper"}})]).is_err());
            assert!(backend.admit_git_info(vec![json!({"operation":{"type":"set_repository","repository_path":repository,"key":"user.name"}})]).is_err());
            let updated = run(json!({"type":"set","key":"user.name","value":""})).unwrap();
            assert_eq!(updated["values"]["user.name"], "");
            assert_eq!(updated["values"]["user.email"], "before@example.invalid");
            assert!(backend.admit_git_info(vec![json!({"operation":{"type":"set","key":"credential.helper","value":"unsafe"}})]).is_err());
            assert!(
                backend
                    .admit_git_info(vec![json!({"operation":{"type":"read","extra":true}})])
                    .is_err()
            );
            let id = uuid::Uuid::new_v4();
            let job = backend.admit_git_info(vec![json!({"operation":{"type":"set","key":"user.name","value":"Should not write"},"request_id":id})]).unwrap();
            assert!(
                backend
                    .admit_git_info(vec![json!({"operation":{"type":"read"},"request_id":id})])
                    .is_err()
            );
            backend.cancel(&[json!({"request_id":id})]).unwrap();
            assert!(backend.read(job).is_err());
            let released = backend
                .admit_git_info(vec![json!({"operation":{"type":"read"},"request_id":id})])
                .unwrap();
            assert!(backend.read(released).is_ok());
            assert_eq!(
                run(json!({"type":"read"})).unwrap()["values"]["user.name"],
                ""
            );
            return;
        }
        let root =
            std::env::temp_dir().join(format!("desktop-wry-identity-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("gitconfig"), "[user]\n name = Before\n email = before@example.invalid\n[credential]\n helper = secret-fixture\n").unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "backend::tests::global_identity_uses_isolated_git_config_and_validates_requests",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HOME", &root)
            .env("GIT_CONFIG_GLOBAL", root.join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_EDITOR", "false")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_COUNT")
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        let config = fs::read_to_string(root.join("gitconfig")).unwrap();
        assert!(config.contains("helper = secret-fixture"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repository_type_can_inspect_unselected_paths() {
        let root = std::env::temp_dir().join(format!("desktop-discovery-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("nested")).unwrap();
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success());
        let backend = Backend::new(4).unwrap();
        let query = |path: &Path| {
            backend.read_args(vec![
                json!({"repository_path":path,"method":"repository_type"}),
            ])
        };
        let directory = |path: &Path| {
            backend.read_args(vec![
                json!({"repository_path":path,"method":"directory_exists"}),
            ])
        };
        assert_eq!(directory(&root).unwrap(), true);
        assert_eq!(directory(&root.join("missing")).unwrap(), false);
        assert_eq!(directory(&root.join(".git/HEAD")).unwrap(), false);
        let exists = |path: &Path| {
            backend.read_args(vec![json!({"repository_path":path,"method":"path_exists"})])
        };
        assert_eq!(exists(&root).unwrap(), true);
        assert_eq!(exists(&root.join("missing")).unwrap(), false);
        assert_eq!(exists(&root.join(".git/HEAD")).unwrap(), true);
        let canonical = |path: &Path| {
            backend.read_args(vec![
                json!({"repository_path":path,"method":"canonical_mutation_path"}),
            ])
        };
        let canonical_value = canonical(&root.join("nested/new/deep")).unwrap();
        let canonical_path = Path::new(canonical_value.as_str().unwrap());
        assert_eq!(canonical_path.file_name().unwrap(), "deep");
        assert_eq!(canonical_path.parent().unwrap().file_name().unwrap(), "new");
        assert_eq!(
            fs::canonicalize(canonical_path.parent().unwrap().parent().unwrap()).unwrap(),
            fs::canonicalize(root.join("nested")).unwrap()
        );
        assert!(canonical(&root.join(".git/HEAD/child")).is_err());
        assert_eq!(
            query(&root.join("nested")).unwrap(),
            json!({"kind":"regular","topLevelWorkingDirectory":root,"gitDir":root.join(".git")})
        );
        assert_eq!(
            query(&root.join("missing")).unwrap(),
            json!({"kind":"missing"})
        );
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":root.join("missing"),"method":"trust_repository"})
                ])
                .is_err()
        );
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":root,"method":"repository_type","range":"HEAD"})
                ])
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn queued_reads_are_cancellable_and_registration_is_released() {
        let backend = Backend::new(1).unwrap();
        let id = uuid::Uuid::new_v4();
        let args = vec![json!({"repository_path":"/missing", "method":"status", "request_id":id})];
        let job = backend.admit(args.clone()).unwrap();
        assert!(backend.admit(args.clone()).is_err());
        backend.cancel(&[json!({"request_id":id})]).unwrap();
        assert!(
            matches!(backend.read(job), Err(AppError::Coded { code, .. }) if code == "cancelled")
        );
        let job = backend.admit(args.clone()).unwrap();
        backend.cancel_all();
        assert!(
            matches!(backend.read(job), Err(AppError::Coded { code, .. }) if code == "cancelled")
        );
        drop(backend.admit(args).unwrap());
        assert!(backend.jobs.lock().unwrap().is_empty());
    }

    #[test]
    fn reads_large_status_through_wry() {
        let root =
            std::env::temp_dir().join(format!("desktop-wry-large-status-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        let padding = "x".repeat(228);
        for index in 0..9_200 {
            fs::write(root.join(format!("{index:05}-{padding}")), b"").unwrap();
        }
        let backend = Backend::new(16).unwrap();
        let records = backend
            .read_args(vec![
                json!({"repository_path":root,"method":"status_detailed","include_untracked":true}),
            ])
            .unwrap();
        assert_eq!(
            records
                .as_array()
                .unwrap()
                .iter()
                .filter(|entry| entry["entry_kind"] == "?")
                .count(),
            9_200
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn enumerates_disambiguated_branches_and_respects_prefixes() {
        let root =
            std::env::temp_dir().join(format!("desktop-wry-branches-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&root)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", root.join("global"))
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "base",
        ]);
        let oid = git(&["rev-parse", "HEAD"]);
        git(&["branch", "topic"]);
        git(&["tag", "topic"]);
        git(&["remote", "add", "origin", "https://example.invalid/repo"]);
        git(&["update-ref", "refs/remotes/origin/topic", &oid]);
        git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/topic",
        ]);
        git(&["config", "branch.topic.remote", "origin"]);
        git(&["config", "branch.topic.merge", "refs/heads/topic"]);
        let backend = Backend::new(16).unwrap();
        let outside = root.with_extension("outside");
        fs::create_dir(&outside).unwrap();
        assert_eq!(
            backend
                .read_args(vec![
                    json!({"repository_path":outside,"method":"safe_fetch_remotes"})
                ])
                .unwrap(),
            json!([])
        );
        let bare = root.join("bare.git");
        assert!(
            Command::new("git")
                .args(["init", "--bare", "-q"])
                .arg(&bare)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&bare)
                .args([
                    "config",
                    "remote.desktop-bare-test.url",
                    "https://example.invalid/bare"
                ])
                .status()
                .unwrap()
                .success()
        );
        let bare_result = backend
            .read_args(vec![
                json!({"repository_path":bare,"method":"safe_fetch_remotes"}),
            ])
            .unwrap();
        if let Some(remotes) = bare_result.as_array() {
            assert!(
                remotes
                    .iter()
                    .any(|remote| remote["name"] == "desktop-bare-test"
                        && remote["url"] == "https://example.invalid/bare")
            );
        } else {
            assert!(bare_result.is_null());
        }
        let enumerate = |prefixes: Value| {
            backend.read_args(vec![json!({"repository_path":root, "method":"branch_enumeration", "prefixes":prefixes})]).unwrap()
        };
        let all = enumerate(json!([]));
        assert_eq!(all.as_array().unwrap().len(), 3);
        assert_eq!(all[1]["ref"], "refs/heads/topic");
        assert_eq!(all[1]["name"], "heads/topic");
        assert_eq!(all[1]["upstream"], "origin/topic");
        assert_eq!(all[1]["upstream_ref"], "refs/remotes/origin/topic");
        assert_eq!(all[0]["current"], true);
        assert_eq!(all[1]["current"], false);
        assert_eq!(all[2]["current"], false);
        assert_eq!(all[1]["oid"], oid);
        let remote = enumerate(json!(["refs/remotes/origin/"]));
        assert_eq!(remote.as_array().unwrap().len(), 1);
        assert_eq!(remote[0]["ref"], "refs/remotes/origin/topic");
        assert_eq!(enumerate(json!(["refs/remotes/missing"])), json!([]));
        assert_eq!(enumerate(json!(["--format=malicious"])), json!([]));
        git(&["switch", "-qc", "unmerged"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "unmerged",
        ]);
        git(&["switch", "-q", "main"]);
        let ahead_behind = |range: &str| {
            backend.read_args(vec![
                json!({"repository_path":root, "method":"ahead_behind", "range":range}),
            ])
        };
        assert_eq!(
            ahead_behind("main...unmerged").unwrap(),
            json!({"ahead":0, "behind":1})
        );
        assert_eq!(
            ahead_behind("unmerged...main").unwrap(),
            json!({"ahead":1, "behind":0})
        );
        assert_eq!(
            ahead_behind("main...main").unwrap(),
            json!({"ahead":0, "behind":0})
        );
        assert_eq!(ahead_behind("missing...main").unwrap(), Value::Null);
        assert_eq!(ahead_behind("--all").unwrap(), Value::Null);
        assert!(backend.read_args(vec![json!({"repository_path":root, "method":"ahead_behind", "range":"main...main", "branch":"main"})]).is_err());
        let merged = backend
            .read_args(vec![
                json!({"repository_path":root, "method":"branch_merged", "branch":"main"}),
            ])
            .unwrap();
        assert_eq!(
            merged,
            json!([{ "ref":"refs/heads/main", "oid":oid }, { "ref":"refs/heads/topic", "oid":oid }])
        );
        let pointing = backend
            .read_args(vec![
                json!({"repository_path":root, "method":"branch_points_at", "commitish":"HEAD"}),
            ])
            .unwrap();
        assert_eq!(pointing, json!(["main", "heads/topic"]));
        assert_eq!(backend.read_args(vec![json!({"repository_path":root, "method":"branch_points_at", "commitish":"missing-ref"})]).unwrap(), Value::Null);
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":root, "method":"branch_merged", "branch":"-D"})
                ])
                .is_err()
        );
        assert!(backend.read_args(vec![json!({"repository_path":root, "method":"branch_merged", "branch":"main", "commitish":"HEAD"})]).is_err());
        assert_eq!(git(&["rev-parse", "refs/heads/main"]), oid);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "diverged",
        ]);
        assert_eq!(
            ahead_behind("main...unmerged").unwrap(),
            json!({"ahead":1, "behind":1})
        );
        let revision_read = |method: &str, range: &str| {
            backend.read_args(vec![
                json!({"repository_path":root,"method":method,"range":range}),
            ])
        };
        let expected = git(&[
            "rev-list",
            "--reverse",
            "--oneline",
            "--no-abbrev-commit",
            "--end-of-options",
            "main..unmerged",
            "--",
        ]);
        assert_eq!(
            revision_read("revision_list", "main..unmerged")
                .unwrap()
                .as_str()
                .unwrap()
                .trim(),
            expected
        );
        assert_eq!(revision_read("revision_list", "main..main").unwrap(), "");
        assert_eq!(
            revision_read("revision_list", "missing..main").unwrap(),
            Value::Null
        );
        assert_eq!(
            revision_read("revision_list", "--all").unwrap(),
            Value::Null
        );
        assert_eq!(revision_read("has_merge_commits", "main").unwrap(), false);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "merge",
            "--no-ff",
            "-qm",
            "merge",
            "unmerged",
        ]);
        assert_eq!(revision_read("has_merge_commits", "HEAD").unwrap(), true);
        assert_eq!(
            revision_read("has_merge_commits", "HEAD..HEAD").unwrap(),
            false
        );
        assert!(backend.read_args(vec![json!({"repository_path":root, "method":"branch_enumeration", "include_untracked":true})]).is_err());
        fs::remove_dir_all(outside).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_worktrees_from_explicit_paths() {
        let root =
            std::env::temp_dir().join(format!("desktop-wry-worktrees-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let bare = root.join("bare.git");
        assert!(
            Command::new("git")
                .args(["init", "--bare", "-q"])
                .arg(&bare)
                .status()
                .unwrap()
                .success()
        );
        let backend = Backend::new(16).unwrap();
        let result = backend
            .read_args(vec![json!({"repository_path":bare,"method":"worktrees"})])
            .unwrap();
        assert_eq!(result[0]["bare"], true);
        assert_eq!(result[0]["main"], true);
        let from_git_dir = backend
            .read_args(vec![
                json!({"repository_path":bare,"method":"worktrees_from_git_dir"}),
            ])
            .unwrap();
        assert_eq!(from_git_dir, result);
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":"relative","method":"worktrees_from_git_dir"})
                ])
                .is_err()
        );
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":bare,"method":"worktrees","include_untracked":true})
                ])
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_blob_prefix_through_wry() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(reads_blob_prefix_through_wry_inner)
            .unwrap()
            .join()
            .unwrap();
    }

    fn reads_blob_prefix_through_wry_inner() {
        let root = std::env::temp_dir().join(format!("desktop-wry-blob-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.join("file"), b"hello\0world").unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["add", "--", "file"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "commit",
                    "-qm",
                    "initial"
                ])
                .status()
                .unwrap()
                .success()
        );
        let backend = Backend::new(16).unwrap();
        for (key, value) in [
            ("user.name", "Stash Test"),
            ("user.email", "stash@example.invalid"),
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(["config", key, value])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let stash = |operation: Value| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"stash_workflow","repository_path":root,"operation":operation}})]).unwrap())
        };
        fs::write(root.join("file"), b"stashed changes").unwrap();
        assert_eq!(
            stash(json!({"method":"renderer_stash_save","branch":"target"})).unwrap()["created"],
            true
        );
        let saved = stash(json!({"method":"stash_list"})).unwrap();
        assert_eq!(saved["entries"][0]["branch"], "target");
        let changes =
            stash(json!({"method":"renderer_stash_changes","oid":saved["entries"][0]["oid"]}))
                .unwrap();
        assert!(changes.as_str().unwrap().contains("file"));
        let popped = stash(json!({"method":"renderer_stash_pop","oid":saved["entries"][0]["oid"],"index":0,"basis":saved["basis"]})).unwrap();
        assert_eq!(popped["code"], 0);
        assert_eq!(popped["retained"], false);
        assert_eq!(fs::read(root.join("file")).unwrap(), b"stashed changes");
        fs::write(root.join("file"), b"ordinary stash").unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["stash", "push", "-qm", "ordinary"])
                .status()
                .unwrap()
                .success()
        );
        let ordinary = stash(json!({"method":"stash_list"})).unwrap();
        assert!(stash(json!({"method":"renderer_stash_drop","oid":ordinary["entries"][0]["oid"],"index":0,"basis":ordinary["basis"]})).is_err());
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["stash", "drop", "-q"])
                .status()
                .unwrap()
                .success()
        );
        let branch = |operation: Value| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"branch_creation","repository_path":root,"operation":operation}})]).unwrap())
        };
        let plan = branch(json!({"method":"create_branch_preview","name":"native-created","start_point":"HEAD","checkout":false})).unwrap();
        let created = branch(json!({"method":"create_branch","name":"native-created","start_point":"HEAD","checkout":false,"basis":plan["basis"]})).unwrap();
        assert_eq!(created["created"], true);
        assert_eq!(created["checked_out"], false);
        let current = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["symbolic-ref", "HEAD"])
            .output()
            .unwrap();
        let current = std::str::from_utf8(&current.stdout).unwrap().trim();
        let checkout = |operation: Value| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"branch_checkout","repository_path":root,"operation":operation}})]).unwrap())
        };
        let switch =
            checkout(json!({"method":"checkout_preview","branch":"refs/heads/native-created"}))
                .unwrap();
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"branch_checkout","repository_path":root,"operation":{"method":"checkout","branch":"refs/heads/native-created"}}})]).is_err());
        assert_eq!(checkout(json!({"method":"checkout","branch":"refs/heads/native-created","basis":switch["basis"]})).unwrap()["checked_out"], true);
        let back = checkout(json!({"method":"checkout_preview","branch":current})).unwrap();
        assert_eq!(
            checkout(json!({"method":"renderer_checkout_direct","branch":current,"basis":back["basis"],"allow_file_protocol":false})).unwrap()
                ["checked_out"],
            true
        );
        let head = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let head = std::str::from_utf8(&head.stdout).unwrap().trim();
        let detach = checkout(json!({"method":"renderer_detach_preview","commit":head})).unwrap();
        assert_eq!(detach["target"]["detached"], true);
        assert_eq!(
            checkout(json!({"method":"renderer_detach","commit":head,"basis":detach["basis"]}))
                .unwrap()["checked_out"],
            true
        );
        assert!(
            !Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["symbolic-ref", "--quiet", "HEAD"])
                .status()
                .unwrap()
                .success()
        );
        let back = checkout(json!({"method":"checkout_preview","branch":current})).unwrap();
        assert_eq!(
            checkout(
                json!({"method":"renderer_checkout_direct","branch":current,"basis":back["basis"]})
            )
            .unwrap()["checked_out"],
            true
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args([
                    "show-ref",
                    "--verify",
                    "--quiet",
                    "refs/heads/native-created"
                ])
                .status()
                .unwrap()
                .success()
        );
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"branch_creation","repository_path":root,"operation":{"method":"create_branch","name":"unsafe","checkout":true,"basis":plan["basis"]}}})]).is_err());
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"branch_creation","repository_path":root,"operation":{"method":"create_branch","name":"unsafe","checkout":false}}})]).is_err());
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["branch", "rename-source"])
                .status()
                .unwrap()
                .success()
        );
        let rename = |operation: Value| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"branch_rename","repository_path":root,"operation":operation}})]).unwrap())
        };
        let rename_plan = rename(json!({"method":"rename_branch_preview","name":"refs/heads/rename-source","new_name":"renamed"})).unwrap();
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"branch_rename","repository_path":root,"operation":{"method":"rename_branch","name":"rename-source","new_name":"renamed"}}})]).is_err());
        let renamed = rename(json!({"method":"rename_branch","name":"refs/heads/rename-source","new_name":"renamed","basis":rename_plan["basis"]})).unwrap();
        assert_eq!(renamed["receipt"]["phase"], "complete");
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["show-ref", "--verify", "--quiet", "refs/heads/renamed"])
                .status()
                .unwrap()
                .success()
        );
        let delete = |operation: Value| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"branch_deletion","repository_path":root,"operation":operation}})]).unwrap())
        };
        let deletion = delete(json!({"method":"delete_branch_preview","name":"refs/heads/native-created","to_checkout":null,"upstream":null})).unwrap();
        assert_eq!(deletion["current"], false);
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"branch_deletion","repository_path":root,"operation":{"method":"delete_branch","name":"native-created","to_checkout":null,"upstream":null,"basis":deletion["basis"]}}})]).is_err());
        let deleted = delete(json!({"method":"delete_branch","name":"refs/heads/native-created","to_checkout":null,"upstream":null,"ref_only":true,"force":true,"basis":deletion["basis"]})).unwrap();
        assert_eq!(deleted["code"], 0);
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args([
                    "show-ref",
                    "--verify",
                    "--quiet",
                    deleted["backup_ref"].as_str().unwrap()
                ])
                .status()
                .unwrap()
                .success()
        );
        let stale =
            branch(json!({"method":"create_branch_preview","name":"stale","checkout":false}))
                .unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["branch", "other"])
                .status()
                .unwrap()
                .success()
        );
        assert!(branch(json!({"method":"create_branch","name":"stale","checkout":false,"basis":stale["basis"]})).is_err());
        assert!(
            !Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["show-ref", "--verify", "--quiet", "refs/heads/stale"])
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.join("file"), b"changed").unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["add", "--", "file"])
                .status()
                .unwrap()
                .success()
        );
        let reset = backend.admit_git_info(vec![json!({"operation":{"type":"reset_index","repository_path":root,"operation":{"method":"renderer_reset_index","paths":["file"]}}})]).unwrap();
        assert_eq!(backend.read(reset).unwrap()["code"], 0);
        assert_eq!(fs::read(root.join("file")).unwrap(), b"changed");
        let update_index = backend.admit_git_info(vec![json!({"operation":{"type":"update_index","repository_path":root,"operation":{"method":"renderer_update_index","paths":["file"],"force_remove":false}}})]).unwrap();
        assert_eq!(backend.read(update_index).unwrap()["code"], 0);
        assert_eq!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["show", ":file"])
                .output()
                .unwrap()
                .stdout,
            b"changed"
        );
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"update_index","repository_path":root,"operation":{"method":"stage","paths":["file"]}}})]).is_err());
        let unstage = backend.admit_git_info(vec![json!({"operation":{"type":"reset_index","repository_path":root,"operation":{"method":"renderer_unstage_all"}}})]).unwrap();
        assert_eq!(backend.read(unstage).unwrap()["code"], 0);
        let checkout = backend.admit_git_info(vec![json!({"operation":{"type":"checkout_index","repository_path":root,"operation":{"method":"renderer_checkout_index","paths":["file"]}}})]).unwrap();
        assert_eq!(backend.read(checkout).unwrap()["code"], 0);
        assert_eq!(fs::read(root.join("file")).unwrap(), b"hello\0world");
        fs::remove_file(root.join("file")).unwrap();
        let checkout_head = backend.admit_git_info(vec![json!({"operation":{"type":"checkout_index","repository_path":root,"operation":{"method":"renderer_checkout_head_paths","paths":["file"]}}})]).unwrap();
        assert_eq!(backend.read(checkout_head).unwrap()["code"], 0);
        assert_eq!(fs::read(root.join("file")).unwrap(), b"hello\0world");
        let remove = backend.admit_git_info(vec![json!({"operation":{"type":"reset_index","repository_path":root,"operation":{"method":"renderer_remove_index_all"}}})]).unwrap();
        assert_eq!(backend.read(remove).unwrap()["code"], 0);
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["ls-files"])
                .output()
                .unwrap()
                .stdout
                .is_empty()
        );
        assert_eq!(fs::read(root.join("file")).unwrap(), b"hello\0world");
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["add", "--", "file"])
                .status()
                .unwrap()
                .success()
        );
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"checkout_index","repository_path":root,"operation":{"method":"stage","paths":["file"]}}})]).is_err());
        let head = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let head = String::from_utf8(head.stdout).unwrap().trim().to_owned();
        assert_eq!(backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"submodule_status","repository_path":root,"operation":{"method":"renderer_submodule_status"}}})]).unwrap()).unwrap(), "");
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"submodule_status","repository_path":root,"operation":{"method":"stage","paths":["file"]}}})]).is_err());
        let reset = backend.admit_git_info(vec![json!({"operation":{"type":"reset_submodules","repository_path":root,"operation":{"method":"renderer_reset_submodules","paths":["../outside"]}}})]).unwrap();
        assert!(backend.read(reset).is_err());
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"reset_submodules","repository_path":root,"operation":{"method":"stage","paths":["file"]}}})]).is_err());
        let update = backend.admit_git_info(vec![json!({"operation":{"type":"update_submodules","repository_path":root,"operation":{"method":"renderer_update_submodules","allow_file_protocol":false}}})]).unwrap();
        assert_eq!(backend.read(update).unwrap()["code"], 0);
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"update_submodules","repository_path":root,"operation":{"method":"stage","paths":["file"]}}})]).is_err());
        for setting in [
            "user.name=Wry Test",
            "user.email=wry@example.invalid",
            "tag.gpgSign=false",
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args([
                        "config",
                        setting.split_once('=').unwrap().0,
                        setting.split_once('=').unwrap().1
                    ])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let tag = |operation: Value| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"tag_mutation","repository_path":root,"operation":operation}})]).unwrap())
        };
        assert_eq!(
            tag(json!({"method":"tag_create_annotated","name":"typed-tag","target":"HEAD"}))
                .unwrap()["code"],
            0
        );
        assert_eq!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["cat-file", "-t", "refs/tags/typed-tag"])
                .output()
                .unwrap()
                .stdout,
            b"tag\n"
        );
        assert_eq!(
            tag(json!({"method":"tag_delete","name":"typed-tag"})).unwrap()["code"],
            0
        );
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"tag_mutation","repository_path":root,"operation":{"method":"stage","paths":["file"]}}})]).is_err());
        let base = |first: &str, second: &str| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"merge_base","repository_path":root,"operation":{"method":"merge_base","first":first,"second":second}}})]).unwrap())
        };
        assert_eq!(base("HEAD", "HEAD").unwrap(), json!(head));
        assert_eq!(base("missing", "HEAD").unwrap(), Value::Null);
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"merge_base","repository_path":root,"operation":{"method":"stage","paths":["file"]}}})]).is_err());
        let preview = backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"merge_tree_preview","repository_path":root,"operation":{"method":"merge_tree_preview","ours":head,"theirs":head}}})]).unwrap()).unwrap();
        assert_eq!(preview, json!({"kind":"clean"}));
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"merge_tree_preview","repository_path":root,"operation":{"method":"stage","paths":["file"]}}})]).is_err());
        let blob = |revision: &str, path: &str| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":root,"operation":{"method":"renderer_blob_prefix","revision":revision,"path":path,"length":5}}})]).unwrap())
        };
        assert_eq!(blob("HEAD", "file").unwrap(), json!("aGVsbG8="));
        assert_eq!(blob("HEAD", "missing").unwrap(), Value::Null);
        assert!(blob("missing", "file").is_err());
        let snapshot = backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":root,"operation":{"method":"renderer_blob_snapshot","revision":"HEAD","path":"file"}}})]).unwrap()).unwrap();
        assert_eq!(snapshot["kind"], "snapshot");
        assert_eq!(snapshot["size"], 11);
        let chunk = backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":root,"operation":{"method":"renderer_compare_diff_chunk","snapshot_id":snapshot["id"],"offset":0,"length":11}}})]).unwrap()).unwrap();
        assert_eq!(chunk["data"], "aGVsbG8Ad29ybGQ=");
        let release = backend.admit_git_info(vec![json!({"operation":{"type":"release_comparison_snapshot","snapshot_id":snapshot["id"]}})]).unwrap();
        assert_eq!(backend.read(release).unwrap()["released"], true);
        let chunk = |offset: u64, expected: Option<&str>| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":root,"operation":{"method":"renderer_blob_chunk","revision":"HEAD","path":"file","offset":offset,"length":5,"expected_oid":expected}}})]).unwrap())
        };
        let first = chunk(0, None).unwrap();
        assert_eq!(first["data"], "aGVsbG8=");
        assert_eq!(first["size"], 11);
        let oid = first["oid"].as_str().unwrap();
        assert_eq!(chunk(5, Some(oid)).unwrap()["data"], "AHdvcmw=");
        assert!(chunk(5, Some(&"0".repeat(40))).is_err());
        let recent = backend
            .read_args(vec![
                json!({"repository_path":root,"method":"renderer_recent_branches"}),
            ])
            .unwrap();
        let trailer = |operation: serde_json::Value| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"interpret_trailers","repository_path":root,"operation":operation}})]).unwrap())
        };
        assert_eq!(trailer(json!({"method":"interpret_trailers","message":"Summary\n","parse":false,"unfold":false,"trailers":[{"token":"Reviewed-by","value":"Test"}]})).unwrap(), "Summary\n\nReviewed-by: Test\n");
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"interpret_trailers","repository_path":root,"operation":{"method":"status"}}})]).is_err());
        let resources = backend
            .read_args(vec![
                json!({"repository_path":root,"method":"protected_resources"}),
            ])
            .unwrap();
        assert!(resources["paths"].as_array().is_some_and(|paths| {
            paths
                .iter()
                .any(|path| path.as_str() == root.join(".git").to_str())
        }));
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":"relative","method":"protected_resources"})
                ])
                .is_err()
        );
        assert!(
            recent
                .as_str()
                .is_some_and(|value| value.contains("initial"))
        );
        assert!(
            std::process::Command::new("git")
                .current_dir(&root)
                .args(["checkout", "-qb", "recent-checkout"])
                .status()
                .unwrap()
                .success()
        );
        let checkouts = backend.read_args(vec![json!({"repository_path":root,"method":"renderer_branch_checkouts","after_date":"2000-01-01T00:00:00.000Z"})]).unwrap();
        assert!(
            checkouts
                .as_str()
                .is_some_and(|value| value.contains("to recent-checkout"))
        );
        assert!(backend.read_args(vec![json!({"repository_path":root,"method":"renderer_branch_checkouts","after_date":"--all"})]).is_err());
        assert!(backend.read_args(vec![json!({"repository_path":root,"method":"renderer_recent_branches","after_date":"2000-01-01T00:00:00.000Z"})]).is_err());
        let history = backend.read_args(vec![json!({"repository_path":root,"method":"renderer_history","commitish":"HEAD","limit":1,"skip":0,"first_parent":true,"exclude_remotes":false})]).unwrap();
        assert!(history.as_str().is_some_and(|value| !value.is_empty()));
        assert_eq!(
            backend
                .read_args(vec![
                    json!({"repository_path":root,"method":"renderer_tag_refs"})
                ])
                .unwrap(),
            ""
        );
        let changes = backend.read_args(vec![json!({"repository_path":root,"method":"renderer_commit_changes","commitish":"HEAD"})]).unwrap();
        assert!(changes.as_str().is_some_and(|value| value.contains("file")));
        let diff = backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":root,"operation":{"method":"renderer_commit_diff","revision":"HEAD","path":"file","old_path":null,"hide_whitespace":false}}})]).unwrap()).unwrap();
        assert!(diff.as_str().is_some_and(|value| !value.is_empty()));
        fs::File::create(root.join("large"))
            .unwrap()
            .set_len(100 * 1024 * 1024 + 1)
            .unwrap();
        assert_eq!(backend.read_args(vec![json!({"repository_path":root,"method":"renderer_large_files","paths":["file","large","missing"]})]).unwrap(), json!([false,true,false]));
        assert!(backend.read_args(vec![json!({"repository_path":root,"method":"renderer_large_files","paths":["../outside"]})]).is_err());
        assert!(backend.read_args(vec![json!({"repository_path":root,"method":"renderer_commit_changes","commitish":"missing"})]).is_err());
        assert_eq!(backend.read_args(vec![json!({"repository_path":root,"method":"renderer_history","commitish":"missing"})]).unwrap(), Value::Null);
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":root,"method":"renderer_recent_branches","limit":1})
                ])
                .is_err()
        );
        assert_eq!(
            backend
                .read_args(vec![
                    json!({"repository_path":root,"method":"renderer_stash_log"})
                ])
                .unwrap(),
            Value::Null
        );
        fs::write(root.join("selected"), b"selected content\n").unwrap();
        let original_index = fs::read(root.join(".git/index")).unwrap();
        let operation = json!({"method":"selected_commit_preview","paths":["selected"],"remove_paths":[],"partial_selections":[],"amend_head":null,"expected_head":null});
        let preview = || {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"selected_commit_preview","repository_path":root,"operation":operation}})]).unwrap())
        };
        let first = preview().unwrap();
        let empty_operation = json!({"method":"selected_commit_preview","paths":[],"remove_paths":[],"partial_selections":[],"allow_empty":true,"amend_head":null,"expected_head":null});
        let empty_review = backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"selected_commit_preview","repository_path":root,"operation":empty_operation}})]).unwrap()).unwrap();
        assert_eq!(empty_review["review"]["staged_count"], 0);
        let verify = backend.admit_git_info(vec![json!({"operation":{"type":"selected_commit_preview","repository_path":root,"operation":{"method":"verify_selected_index","tree":empty_review["tree"],"head":empty_review["head"],"branch":empty_review["branch"]}}})]).unwrap();
        assert_eq!(backend.read(verify).unwrap()["matches"], true);
        assert_eq!(first, preview().unwrap());
        assert_eq!(first["review"]["staged_count"], 1);
        assert_eq!(
            first["review"]["files"],
            json!([{ "bytes": b"selected".to_vec() }])
        );
        assert!(
            first["basis"]
                .as_str()
                .is_some_and(|basis| basis.len() == 64)
        );
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), original_index);
        let prepare = json!({"method":"prepare_selected_commit","paths":["selected"],"remove_paths":[],"partial_selections":[],"allow_empty":false,"amend_head":null,"expected_head":null,"basis":first["basis"]});
        let checkpoint = |operation: Value| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"commit_checkpoint","repository_path":root,"operation":operation}})]).unwrap())
        };
        let prepared = checkpoint(prepare).unwrap();
        let id = prepared["id"].as_str().unwrap();
        assert_eq!(
            checkpoint(json!({"method":"list_commit_checkpoints"})).unwrap()["checkpoints"][0]["id"],
            id
        );
        let inspection =
            checkpoint(json!({"method":"inspect_commit_checkpoint","checkpoint_id":id})).unwrap();
        assert_eq!(inspection["recorded_files_verified"], true);
        assert_eq!(inspection["index_matches"], true);
        assert!(
            checkpoint(json!({"method":"recover_published_commit_checkpoint","checkpoint_id":id}))
                .is_err()
        );
        assert!(
            checkpoint(json!({"method":"close_unchanged_commit_checkpoint","checkpoint_id":id}))
                .is_err()
        );
        assert_eq!(
            checkpoint(json!({"method":"revalidate_commit_checkpoint","checkpoint_id":id}))
                .unwrap()["matches"],
            true
        );
        fs::write(root.join("selected"), b"changed after checkpoint\n").unwrap();
        assert_eq!(
            checkpoint(json!({"method":"revalidate_commit_checkpoint","checkpoint_id":id}))
                .unwrap()["matches"],
            false
        );
        assert_eq!(
            checkpoint(json!({"method":"forget_commit_checkpoint","checkpoint_id":id})).unwrap()["forgotten"],
            true
        );
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), original_index);
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"commit_checkpoint","repository_path":root,"operation":{"method":"stage","paths":["selected"]}}})]).is_err());
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"selected_commit_preview","repository_path":root,"operation":{"method":"stage","paths":["selected"]}}})]).is_err());
        let delete = backend.admit_git_info(vec![json!({"operation":{"type":"delete_initial_head","repository_path":root,"operation":{"method":"renderer_delete_initial_head","expected_oid":head}}})]).unwrap();
        assert_eq!(backend.read(delete).unwrap()["code"], 0);
        assert_eq!(fs::read(root.join("file")).unwrap(), b"hello\0world");
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"delete_initial_head","repository_path":root,"operation":{"method":"stage","paths":["file"]}}})]).is_err());
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.invalid"),
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(["config", key, value])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let selected = checkpoint(json!({"method":"prepare_selected_commit","paths":["selected"],"remove_paths":[],"partial_selections":[],"allow_empty":false,"amend_head":null,"expected_head":null,"basis":preview().unwrap()["basis"]})).unwrap();
        let result = checkpoint(
            json!({"method":"publish_selected_commit","checkpoint_id":selected["id"],"message":"reviewed\n","no_verify":false,"sign_off":false,"allow_large_files":false}),
        );
        match result {
            Ok(result) => assert_eq!(result["state"], "complete"),
            Err(error) if error.to_string().contains("does not support") => {}
            Err(error) => panic!("{error:?}"),
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_the_requested_repository_and_rejects_mutations() {
        // Exercise the same executor used by Wry repository requests, including
        // selected-line discard and undo, rather than a custom test thread.
        let executor = crate::bridge::repository_executor(1, 1, 1024).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let task = desktop_core::io::executor::Task::new(
            0,
            std::time::Instant::now() + std::time::Duration::from_secs(120),
            move |outcome| {
                assert_eq!(outcome, desktop_core::io::executor::Outcome::Run);
                reads_the_requested_repository_and_rejects_mutations_inner();
                sender.send(()).unwrap();
            },
        );
        assert!(executor.submit(task).is_ok());
        receiver
            .recv_timeout(std::time::Duration::from_secs(120))
            .unwrap();
        executor.close();
    }

    fn reads_the_requested_repository_and_rejects_mutations_inner() {
        let root =
            std::env::temp_dir().join(format!("desktop-wry-backend-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let first = root.join("first");
        let second = root.join("second");
        for path in [&first, &second] {
            assert!(
                Command::new("git")
                    .args(["init", "-q"])
                    .arg(path)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        fs::write(first.join("first-only"), "test").unwrap();
        let backend = Backend::new(16).unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&second)
                .args([
                    "-c",
                    "user.name=Author",
                    "-c",
                    "user.email=author@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "author"
                ])
                .status()
                .unwrap()
                .success()
        );
        let output = Command::new("git")
            .arg("-C")
            .arg(&second)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let oid = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        let authors = backend
            .read_args(vec![
                json!({"repository_path":second,"method":"renderer_authors","revisions":[oid]}),
            ])
            .unwrap();
        assert_eq!(authors["authors"][0]["oid"], oid);
        assert!(
            authors["authors"][0]["identity"]
                .as_str()
                .unwrap()
                .starts_with("Author <author@example.invalid> ")
        );
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":second,"method":"index_changes","revisions":[oid]})
                ])
                .is_err()
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&first)
                .args(["add", "--", "first-only"])
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            backend
                .read_args(vec![
                    json!({"repository_path":first,"method":"index_changes"})
                ])
                .unwrap(),
            json!([{"status":"A","path":{"bytes":b"first-only"}}])
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&first)
                .args(["rm", "--cached", "-q", "--", "first-only"])
                .status()
                .unwrap()
                .success()
        );
        assert!(backend.admit(vec![json!({"repository_path":first,"method":"index_changes","include_untracked":true})]).is_err());
        let reviewed = |operation: Value| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":first,"operation":operation}})]).unwrap())
        };
        let renderer =
            reviewed(json!({"method":"renderer_working_diff","path":"first-only","base":"empty"}))
                .unwrap();
        assert!(
            renderer["stdout"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
        assert!(
            reviewed(json!({"method":"renderer_working_diff","path":"../outside","base":"empty"}))
                .is_err()
        );
        let diff =
            reviewed(json!({"method":"diff_detail","path":"first-only","staged":false})).unwrap();
        let stage = reviewed(json!({"method":"select_diff","path":"first-only","staged":false,"basis":diff["basis"],"lines":[diff["hunks"][0]["lines"][0]["id"]]})).unwrap();
        assert_eq!(stage["code"], 0);
        assert_eq!(
            Command::new("git")
                .arg("-C")
                .arg(&first)
                .args(["show", ":first-only"])
                .output()
                .unwrap()
                .stdout,
            b"test"
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&first)
                .args(["rm", "--cached", "-q", "--", "first-only"])
                .status()
                .unwrap()
                .success()
        );
        let diff =
            reviewed(json!({"method":"diff_detail","path":"first-only","staged":false})).unwrap();
        let selection = json!({"basis":diff["basis"],"lines":[diff["hunks"][0]["lines"][0]["id"]]});
        let preview = reviewed(json!({"method":"discard_preview","path":"first-only","scope":"working_tree","selection":selection})).unwrap();
        let result = reviewed(json!({"method":"discard","path":"first-only","scope":"working_tree","selection":selection,"basis":preview["basis"]})).unwrap();
        let records = reviewed(json!({"method":"list_discards"})).unwrap();
        assert!(
            records
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["id"] == result["undo_id"] && entry["state"] == "applied")
        );
        assert!(!first.join("first-only").exists());
        let manifest_path = first
            .join(".git/desktop-undo")
            .join(result["undo_id"].as_str().unwrap())
            .join("manifest.json");
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["state"] = json!("prepared");
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(reviewed(json!({"method":"forget_discard","undo_id":result["undo_id"]})).is_err());
        assert!(manifest_path.exists());
        manifest["state"] = json!("applied");
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        reviewed(json!({"method":"undo_discard","undo_id":result["undo_id"]})).unwrap();
        assert_eq!(
            fs::read_to_string(first.join("first-only")).unwrap(),
            "test"
        );
        fs::write(first.join("batch-extra"), "extra").unwrap();
        let batch = json!(["first-only", "batch-extra"]);
        let batch_preview =
            reviewed(json!({"method":"discard_batch_preview","paths":batch})).unwrap();
        assert_eq!(batch_preview["files"].as_array().unwrap().len(), 2);
        let batch_result = reviewed(
            json!({"method":"discard_batch","paths":batch,"basis":batch_preview["basis"]}),
        )
        .unwrap();
        assert!(!first.join("first-only").exists());
        assert!(!first.join("batch-extra").exists());
        reviewed(json!({"method":"undo_discard","undo_id":batch_result["undo_id"]})).unwrap();
        assert_eq!(
            fs::read_to_string(first.join("first-only")).unwrap(),
            "test"
        );
        assert_eq!(
            fs::read_to_string(first.join("batch-extra")).unwrap(),
            "extra"
        );
        fs::remove_file(first.join("batch-extra")).unwrap();
        reviewed(json!({"method":"forget_discard","undo_id":batch_result["undo_id"]})).unwrap();
        reviewed(json!({"method":"forget_discard","undo_id":result["undo_id"]})).unwrap();
        assert!(
            reviewed(json!({"method":"list_discards"}))
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            fs::read_to_string(first.join("first-only")).unwrap(),
            "test"
        );
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":first,"operation":{"method":"stage","paths":["first-only"]}}})]).is_err());
        let destination = root.join("existing folder");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("keep.txt"), "keep").unwrap();
        let init = |branch: &str| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"initialize_repository","repository_path":destination,"default_branch":branch}})]).unwrap())
        };
        assert_eq!(init("trunk").unwrap(), Value::Null);
        assert_eq!(
            fs::read_to_string(destination.join("keep.txt")).unwrap(),
            "keep"
        );
        assert_eq!(
            fs::read_to_string(destination.join(".git/HEAD")).unwrap(),
            "ref: refs/heads/trunk\n"
        );
        assert!(init("other").is_err());
        let symbolic = |reference: &str| {
            backend.read_args(vec![json!({"repository_path":destination,"method":"symbolic_ref","reference":reference})])
        };
        assert_eq!(symbolic("HEAD").unwrap(), "refs/heads/trunk");
        assert_eq!(symbolic("refs/remotes/origin/HEAD").unwrap(), Value::Null);
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&destination)
                .args([
                    "symbolic-ref",
                    "refs/remotes/origin/HEAD",
                    "refs/remotes/origin/main"
                ])
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            symbolic("refs/remotes/origin/HEAD").unwrap(),
            "refs/remotes/origin/main"
        );
        assert!(symbolic("--invalid").is_err());
        assert!(
            backend
                .admit(vec![
                    json!({"repository_path":destination,"method":"symbolic_ref"})
                ])
                .is_err()
        );
        assert!(
            backend
                .admit(vec![
                    json!({"repository_path":destination,"method":"status","reference":"HEAD"})
                ])
                .is_err()
        );
        let has_remotes = || {
            backend.read_args(vec![
                json!({"repository_path":destination,"method":"safe_fetch_remotes"}),
            ])
        };
        let before_remotes = has_remotes().unwrap();
        assert!(before_remotes.is_array() || before_remotes.is_null());
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&destination)
                .args([
                    "config",
                    "--add",
                    "remote.desktop-rust-test.url",
                    "https://example.invalid/repo.git"
                ])
                .status()
                .unwrap()
                .success()
        );
        let remote_url = |name: &str| {
            backend.read_args(vec![json!({"repository_path":destination,"method":"safe_remote_url","remote_name":name})])
        };
        assert_eq!(
            remote_url("desktop-rust-test").unwrap(),
            json!({"kind":"resolved","url":"https://example.invalid/repo.git"})
        );
        assert_eq!(
            remote_url("missing-remote").unwrap(),
            json!({"kind":"missing"})
        );
        assert!(
            backend
                .admit(vec![
                    json!({"repository_path":destination,"method":"safe_remote_url"})
                ])
                .is_err()
        );
        assert!(
            backend
                .admit(vec![
                    json!({"repository_path":destination,"method":"status","remote_name":"origin"})
                ])
                .is_err()
        );
        if before_remotes.is_array() {
            assert!(
                has_remotes()
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|remote| remote["name"] == "desktop-rust-test"
                        && remote["url"] == "https://example.invalid/repo.git")
            );
        }
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&destination)
                .args(["config", "--unset-all", "remote.desktop-rust-test.url"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&destination)
                .args([
                    "config",
                    "--add",
                    "remote.desktop-rust-test.url",
                    "https://user:synthetic-secret@example.invalid/repo"
                ])
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(has_remotes().unwrap(), Value::Null);
        assert_eq!(
            remote_url("desktop-rust-test").unwrap(),
            json!({"kind":"unsupported"})
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&destination)
                .args(["config", "--unset-all", "remote.desktop-rust-test.url"])
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(has_remotes().unwrap(), before_remotes);
        let manage = |action: Value| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"manage_remote","repository_path":destination,"action":action}})]).unwrap())
        };
        assert_eq!(manage(json!({"type":"add","name":"test-rust-mutation","url":"https://example.invalid/one"})).unwrap(), json!({"kind":"updated"}));
        assert!(manage(json!({"type":"add","name":"test-rust-mutation","url":"https://example.invalid/duplicate"})).is_err());
        assert_eq!(manage(json!({"type":"set_url","name":"test-rust-mutation","url":"https://example.invalid/two"})).unwrap(), json!({"kind":"updated"}));
        assert_eq!(manage(json!({"type":"set_url","name":"test-rust-mutation","url":"https://user:synthetic-secret@example.invalid/repo"})).unwrap(), json!({"kind":"unsupported"}));
        let configured = Command::new("git")
            .arg("-C")
            .arg(&destination)
            .args([
                "config",
                "--local",
                "--get",
                "remote.test-rust-mutation.url",
            ])
            .output()
            .unwrap();
        assert_eq!(configured.stdout, b"https://example.invalid/two\n");
        assert_eq!(
            manage(json!({"type":"remove","name":"test-rust-mutation"})).unwrap(),
            json!({"kind":"updated"})
        );
        assert_eq!(
            manage(json!({"type":"remove","name":"test-rust-mutation"})).unwrap(),
            json!({"kind":"updated"})
        );
        let creation = |path: &Path, kind: &str, contents: &str| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"write_creation_file","repository_path":path,"kind":kind,"contents":contents}})]).unwrap())
        };
        let gitignore = |action: Value| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"git_ignore","repository_path":destination,"action":action}})]).unwrap())
        };
        for (key, value) in [("core.autocrlf", "true"), ("core.safecrlf", "false")] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&destination)
                    .args(["config", "--local", key, value])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        assert_eq!(gitignore(json!({"type":"read"})).unwrap(), Value::Null);
        gitignore(json!({"type":"save","text":"first\n"})).unwrap();
        gitignore(json!({"type":"append","text":"second\n"})).unwrap();
        assert_eq!(
            gitignore(json!({"type":"read"})).unwrap(),
            json!("first\nsecond\n")
        );
        gitignore(json!({"type":"save","text":""})).unwrap();
        assert_eq!(gitignore(json!({"type":"read"})).unwrap(), Value::Null);
        assert_eq!(
            creation(&destination, "readme", "hello 🦀\n").unwrap(),
            Value::Null
        );
        assert_eq!(
            fs::read_to_string(destination.join("README.md")).unwrap(),
            "hello 🦀\n"
        );
        let created_status = backend.read_args(vec![json!({"repository_path":destination,"method":"status_detailed","include_untracked":true})]).unwrap();
        assert!(
            created_status
                .as_array()
                .unwrap()
                .iter()
                .any(|record| record["path"]["bytes"] == json!(b"README.md"))
        );
        assert_eq!(
            backend
                .read_args(vec![
                    json!({"repository_path":destination,"method":"status_context"})
                ])
                .unwrap()["mergeHeadFound"],
            false
        );
        assert_eq!(
            creation(&destination, "git_attributes", "original").unwrap(),
            Value::Null
        );
        assert_eq!(
            creation(&destination, "git_attributes", "replacement").unwrap(),
            Value::Null
        );
        assert_eq!(
            fs::read_to_string(destination.join(".gitattributes")).unwrap(),
            "original"
        );
        fs::create_dir(destination.join("nested")).unwrap();
        assert!(creation(&destination.join("nested"), "readme", "bad").is_err());
        for (key, value) in [
            ("user.name", "Creation Test"),
            ("user.email", "creation@example.invalid"),
            ("author.name", "Creation Test"),
            ("author.email", "creation@example.invalid"),
            ("committer.name", "Creation Test"),
            ("committer.email", "creation@example.invalid"),
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&destination)
                    .args(["config", key, value])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let author = backend
            .read_args(vec![
                json!({"repository_path":destination,"method":"git_author_identity"}),
            ])
            .unwrap();
        let committer = backend
            .read_args(vec![
                json!({"repository_path":destination,"method":"git_committer_identity"}),
            ])
            .unwrap();
        assert!(
            author
                .as_str()
                .unwrap()
                .starts_with("Creation Test <creation@example.invalid> ")
        );
        assert!(
            committer
                .as_str()
                .unwrap()
                .starts_with("Creation Test <creation@example.invalid> ")
        );
        assert!(backend.admit(vec![json!({"repository_path":destination,"method":"git_author_identity","key":"credential.helper"})]).is_err());
        let initial_commit = |path: &Path, paths: Value| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"initial_commit","repository_path":path,"paths":paths}})]).unwrap())
        };
        assert!(initial_commit(&destination.join("nested"), json!(["README.md"])).is_err());
        let result = initial_commit(&destination, json!(["README.md", ".gitattributes"])).unwrap();
        assert_eq!(result["code"], 0);
        let commit = backend.read_args(vec![json!({"repository_path":destination,"method":"renderer_commit","commitish":"HEAD"})]).unwrap();
        assert!(commit.as_str().is_some_and(|value| !value.is_empty()));
        assert_eq!(backend.read_args(vec![json!({"repository_path":destination,"method":"renderer_commit","commitish":"missing"})]).unwrap(), Value::Null);
        assert!(initial_commit(&destination, json!(["keep.txt"])).is_err());
        let tree = Command::new("git")
            .arg("-C")
            .arg(&destination)
            .args(["ls-tree", "--name-only", "HEAD"])
            .output()
            .unwrap();
        assert_eq!(tree.stdout, b".gitattributes\nREADME.md\n");
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"write_creation_file","repository_path":destination,"kind":"other","contents":"bad"}})]).is_err());
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"initialize_repository","repository_path":"relative","default_branch":"trunk"}})]).is_err());
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&first)
                .args([
                    "config",
                    "--local",
                    "submodule.libs/nested.url",
                    "https://user:synthetic-secret@github.com/Owner/Repo.git?token=synthetic-secret"
                ])
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(backend.read_args(vec![json!({"repository_path":first,"method":"submodule_display_url","submodule_path":"libs/nested"})]).unwrap(), "https://github.com/owner/repo");
        assert!(backend.read_args(vec![json!({"repository_path":first,"method":"submodule_display_url","submodule_path":"../outside"})]).is_err());
        assert!(backend.admit(vec![json!({"repository_path":first,"method":"status","submodule_path":"libs/nested"})]).is_err());
        fs::write(first.join(".git/description"), "  description\n").unwrap();
        assert_eq!(
            backend
                .read_args(vec![
                    json!({"repository_path":first,"method":"git_description"})
                ])
                .unwrap(),
            "  description\n"
        );
        let write = |description: &str| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"set_description","repository_path":first,"description":description}})]).unwrap())
        };
        assert_eq!(write("changed 🦀\n").unwrap(), Value::Null);
        assert_eq!(
            fs::read_to_string(first.join(".git/description")).unwrap(),
            "changed 🦀\n"
        );
        assert!(write(&"x".repeat(65537)).is_err());
        assert_eq!(
            fs::read_to_string(first.join(".git/description")).unwrap(),
            "changed 🦀\n"
        );
        write("  description\n").unwrap();
        assert!(backend.admit(vec![json!({"repository_path":first,"method":"git_description","key":"credential.helper"})]).is_err());
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&first)
                .args(["config", "--local", "commit.gpgsign", "yes"])
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(backend.read_args(vec![json!({"repository_path":first,"method":"git_read_setting","key":"commit.gpgsign","canonical_bool":true})]).unwrap(), "true");
        assert!(backend.read_args(vec![json!({"repository_path":first,"method":"git_read_setting","key":"credential.helper"})]).is_err());
        assert!(backend.read_args(vec![json!({"repository_path":first,"method":"git_read_setting","key":"core.safecrlf","canonical_bool":true})]).is_err());
        fs::write(first.join(".gitattributes"), "*.bin filter=lfs\n").unwrap();
        let tracking = backend.read_args(vec![json!({"repository_path":first,"method":"lfs_file_tracking","paths":["a.bin", "-odd.txt", "a.bin"]})]).unwrap();
        assert_eq!(tracking, json!([true, false, true]));
        assert!(backend.read_args(vec![json!({"repository_path":first,"method":"lfs_file_tracking","paths":["../outside"]})]).is_err());
        assert!(
            backend
                .admit(vec![
                    json!({"repository_path":first,"method":"status","paths":[]})
                ])
                .is_err()
        );
        assert!(
            backend
                .admit(vec![
                    json!({"repository_path":first,"method":"lfs_file_tracking"})
                ])
                .is_err()
        );
        fs::remove_file(first.join(".gitattributes")).unwrap();
        assert_eq!(
            backend
                .read_args(vec![
                    json!({"repository_path":first,"method":"renderer_cherry_pick_state"})
                ])
                .unwrap(),
            Value::Null
        );
        assert_eq!(
            backend
                .read_args(vec![
                    json!({"repository_path":first,"method":"renderer_rebase_state"})
                ])
                .unwrap(),
            Value::Null
        );
        let rebase_head = "b".repeat(40);
        fs::write(first.join(".git/REBASE_HEAD"), format!("{rebase_head}\n")).unwrap();
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":first,"method":"renderer_rebase_state"})
                ])
                .unwrap()
                .is_null()
        );
        fs::create_dir(first.join(".git/rebase-apply")).unwrap();
        assert_eq!(
            backend
                .read_args(vec![
                    json!({"repository_path":first,"method":"renderer_rebase_state"})
                ])
                .unwrap()["head"],
            rebase_head
        );
        assert!(backend.admit(vec![json!({"repository_path":first,"method":"renderer_rebase_state","kind":"working"})]).is_err());
        fs::remove_dir(first.join(".git/rebase-apply")).unwrap();
        fs::remove_file(first.join(".git/REBASE_HEAD")).unwrap();
        let pick_head = "a".repeat(40);
        fs::write(
            first.join(".git/CHERRY_PICK_HEAD"),
            format!("{pick_head}\n"),
        )
        .unwrap();
        assert_eq!(
            backend
                .read_args(vec![
                    json!({"repository_path":first,"method":"renderer_cherry_pick_state"})
                ])
                .unwrap()["head"],
            pick_head
        );
        assert!(backend.admit(vec![json!({"repository_path":first,"method":"renderer_cherry_pick_state","kind":"working"})]).is_err());
        fs::remove_file(first.join(".git/CHERRY_PICK_HEAD")).unwrap();
        fs::write(first.join(".git/SQUASH_MSG"), "squash").unwrap();
        let context = backend
            .read_args(vec![
                json!({"repository_path":first,"method":"status_context"}),
            ])
            .unwrap();
        assert_eq!(context["squashMsgFound"], true);
        let context = backend
            .read_args(vec![
                json!({"repository_path":second,"method":"status_context"}),
            ])
            .unwrap();
        assert_eq!(context["squashMsgFound"], false);
        let conflicts = backend.read_args(vec![json!({"repository_path":first,"method":"status_conflict_details","kind":"working"})]).unwrap();
        assert_eq!(conflicts["check"], "");
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":first,"method":"status_conflict_details"})
                ])
                .is_err()
        );
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":first,"method":"status_context","kind":"working"})
                ])
                .is_err()
        );
        assert_eq!(
            backend
                .read_args(vec![
                    json!({"repository_path":second,"method":"has_merge_commits","range":"HEAD"})
                ])
                .unwrap(),
            false
        );
        let read = |path: &Path| {
            backend
                .read_args(vec![json!({"repository_path":path,"method":"snapshot"})])
                .unwrap()
        };
        let one = read(&first).to_string();
        let two = read(&second).to_string();
        assert!(one.contains("first-only"));
        assert!(!two.contains("first-only"));
        let detailed = backend.read_args(vec![json!({"repository_path":first,"method":"status_detailed","include_untracked":true})]).unwrap();
        assert!(
            detailed
                .as_array()
                .unwrap()
                .iter()
                .any(|record| record["path"]["bytes"] == json!(b"first-only"))
        );
        assert!(
            backend
                .read_args(vec![json!({"repository_path":first,"method":"commit"})])
                .is_err()
        );
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":first,"method":"status","extra":true})
                ])
                .is_err()
        );
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":first,"method":"status","include_untracked":true})
                ])
                .is_err()
        );
        assert!(
            backend
                .read_args(vec![json!({"repository_path":"","method":"status"})])
                .is_err()
        );
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":"relative","method":"status"})
                ])
                .is_err()
        );
        assert!(
            Command::new("git")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", root.join("empty-gitconfig"))
                .arg("-C")
                .arg(&first)
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "initial"
                ])
                .status()
                .unwrap()
                .success()
        );
        let linked = root.join("linked");
        assert!(
            Command::new("git")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", root.join("empty-gitconfig"))
                .arg("-C")
                .arg(&first)
                .args(["worktree", "add", "-q", "-b", "linked-branch"])
                .arg(&linked)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            backend
                .read_args(vec![
                    json!({"repository_path":linked,"method":"git_description"})
                ])
                .unwrap(),
            "  description\n"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
