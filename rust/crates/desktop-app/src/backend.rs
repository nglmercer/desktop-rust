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
    SelectedCommitPreview {
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
    RendererHistory,
    RendererRecentBranches,
    RendererStashLog,
    RendererTagRefs,
    RendererCommitChanges,
    RendererLargeFiles,
    Worktrees,
    WorktreesFromGitDir,
    StatusContext,
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

impl Backend {
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
        | GitInfoOperation::SelectedCommitPreview {
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
                    | Operation::RendererWorkingDiff { .. }
                    | Operation::RendererBlobPrefix { .. }
                    | Operation::RendererCommitDiff { .. }
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
            )
        {
            return Err(AppError::BadArgs(
                "unsupported reviewed discard operation".into(),
            ));
        }
        if let GitInfoOperation::SelectedCommitPreview { operation, .. } = &request.operation
            && !matches!(operation, Operation::SelectedCommitPreview { .. })
        {
            return Err(AppError::BadArgs(
                "unsupported selected commit preview operation".into(),
            ));
        }
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

    pub fn read(&self, job: ReadJob) -> Result<Value> {
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
                    | Operation::ForgetDiscard { .. },
                ..
            })
        );
        let initialization = matches!(
            job.operation,
            BackendOperation::GitInfo(GitInfoOperation::InitializeRepository { .. })
        );
        let lfs_write = matches!(
            job.operation,
            BackendOperation::GitInfo(
                GitInfoOperation::LfsInstall { .. } | GitInfoOperation::LfsInstallRepository { .. }
            )
        );
        self.runtime.block_on(token.clone().scope(async {
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
                    BackendOperation::SafeRemoteUrl(name) => return desktop_backend::safe_remote_url(Path::new(&job.path), &name).await.map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::SymbolicRef(reference) => return desktop_backend::symbolic_ref(Path::new(&job.path), &reference).await.map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::GitInfo(operation) => {
                        return match operation {
                            GitInfoOperation::Version {} => desktop_backend::git_version().await.map(serde_json::Value::String),
                            GitInfoOperation::GlobalConfigPath {} => desktop_backend::git_global_config_path().await.map(serde_json::Value::String),
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
                            GitInfoOperation::ReviewedDiscard { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
                                repo.execute(operation).await
                            }
                            GitInfoOperation::SelectedCommitPreview { repository_path, operation } => {
                                let canonical = Path::new(&repository_path).canonicalize().map_err(|error| AppError::msg(error.to_string()))?;
                                let repo = Repository::open(&canonical).await.map_err(|error| AppError::msg(error.to_string()))?;
                                if repo.path() != canonical { return Err(AppError::msg("select a repository root, not a nested path")); }
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
                result = &mut work => if token.is_cancelled() { Err(AppError::Coded { code: "cancelled".into(), message: if identity_write { "Git identity update cancelled; inspect Git settings before retrying" } else if settings_write { "Git setting update cancelled; inspect Git settings before retrying" } else if config_path_write { "Git config preparation cancelled; check the configuration file before retrying" } else if description_write { "Description update cancelled; inspect the repository description before retrying" } else if creation_write { "Repository creation operation cancelled; inspect the repository and index before retrying" } else if discard_write { "Discard operation cancelled; inspect the file and discard recovery history before retrying" } else if gitignore_write { "Ignore file update cancelled; inspect .gitignore before retrying" } else if remote_write { "Remote update cancelled; inspect Git configuration before retrying" } else if initialization { "Repository initialization cancelled; inspect the destination before retrying" } else if lfs_write { "Git LFS setup cancelled; inspect Git settings and hooks before retrying" } else { "backend operation cancelled" }.into() }) } else { result },
                _ = tokio::time::sleep(remaining) => {
                    token.cancel();
                    let _ = tokio::time::timeout(Duration::from_secs(3), &mut work).await;
                    Err(AppError::Coded { code: "ETIMEDOUT".into(), message: if identity_write { "Git identity update timed out; inspect Git settings before retrying" } else if settings_write { "Git setting update timed out; inspect Git settings before retrying" } else if config_path_write { "Git config preparation timed out; check the configuration file before retrying" } else if description_write { "Description update timed out; inspect the repository description before retrying" } else if creation_write { "Repository creation operation timed out; inspect the repository and index before retrying" } else if discard_write { "Discard operation timed out; inspect the file and discard recovery history before retrying" } else if gitignore_write { "Ignore file update timed out; inspect .gitignore before retrying" } else if remote_write { "Remote update timed out; inspect Git configuration before retrying" } else if initialization { "Repository initialization timed out; inspect the destination before retrying" } else if lfs_write { "Git LFS setup timed out; inspect Git settings and hooks before retrying" } else { "backend operation timed out" }.into() })
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
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
        let outside = root.join("outside");
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
        let blob = |revision: &str, path: &str| {
            backend.read(backend.admit_git_info(vec![json!({"operation":{"type":"reviewed_discard","repository_path":root,"operation":{"method":"renderer_blob_prefix","revision":revision,"path":path,"length":5}}})]).unwrap())
        };
        assert_eq!(blob("HEAD", "file").unwrap(), json!("aGVsbG8="));
        assert_eq!(blob("HEAD", "missing").unwrap(), Value::Null);
        assert!(blob("missing", "file").is_err());
        let recent = backend
            .read_args(vec![
                json!({"repository_path":root,"method":"renderer_recent_branches"}),
            ])
            .unwrap();
        assert!(
            recent
                .as_str()
                .is_some_and(|value| value.contains("initial"))
        );
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
        assert!(backend.admit_git_info(vec![json!({"operation":{"type":"selected_commit_preview","repository_path":root,"operation":{"method":"stage","paths":["selected"]}}})]).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_the_requested_repository_and_rejects_mutations() {
        // This integration fixture exercises many request shapes in one test;
        // its debug-build stack frame exceeds Rust's default test-thread stack.
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(reads_the_requested_repository_and_rejects_mutations_inner)
            .unwrap()
            .join()
            .unwrap();
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
