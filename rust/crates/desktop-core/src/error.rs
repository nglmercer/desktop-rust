//! Error model.
//!
//! Electron's `getIpcFriendlyError` flattens `Error` into `{ message, name, stack }`
//! because `Error` does not survive structured clone. Our bridge does the same with
//! [`RpcError`] so the web layer keeps the exact same shape it already understands.

use serde::{Deserialize, Serialize};

pub type Result<T, E = AppError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("git error: {0}")]
    Git(#[from] GitError),
    #[error("unknown ipc channel: {0}")]
    UnknownChannel(String),
    #[error("invalid ipc arguments: {0}")]
    BadArgs(String),
    #[error("not authenticated")]
    NotAuthenticated,
    #[error("not implemented in this migration stage: {0}")]
    NotImplemented(String),
    #[error("{0}")]
    Message(String),
    /// A failure carrying the `code` Node would attach (`ENOENT`, ...), for
    /// renderer code that branches on it.
    #[error("{message}")]
    Coded { code: String, message: String },
    /// A coded failure that also carries typed data for the caller, such as
    /// the output a git command produced before it was stopped.
    #[error("{message}")]
    Detailed {
        code: String,
        message: String,
        details: serde_json::Value,
    },
}

impl AppError {
    pub fn msg(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    /// Flatten into the shape consumed by the renderer, mirroring
    /// `getIpcFriendlyError` in `app/src/ui/main-process-proxy.ts`.
    pub fn to_rpc_error(&self) -> RpcError {
        let name = match self {
            Self::Git(_) => "GitError",
            Self::Http(_) => "APIError",
            Self::Io(_) => "IOError",
            Self::Json(_) | Self::BadArgs(_) => "TypeError",
            Self::UnknownChannel(_) => "UnknownChannelError",
            Self::NotAuthenticated => "AuthenticationError",
            Self::NotImplemented(_) => "NotImplementedError",
            Self::Message(_) | Self::Coded { .. } | Self::Detailed { .. } => "Error",
        };
        RpcError {
            message: self.to_string(),
            name: name.to_string(),
            stack: None,
            code: match self {
                Self::Git(e) => Some(format!("{:?}", e.kind)),
                Self::Coded { code, .. } | Self::Detailed { code, .. } => Some(code.clone()),
                _ => None,
            },
            details: match self {
                Self::Detailed { details, .. } => Some(details.clone()),
                _ => None,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcError {
    pub message: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Typed data for the renderer, such as partial command output. Absent for
    /// ordinary errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// The Rust equivalent of dugite's `GitError` enum plus Desktop's
/// `GitError` wrapper (`app/src/lib/git/core.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum GitErrorKind {
    None,
    SshAuthenticationFailed,
    SshPermissionDenied,
    HttpsAuthenticationFailed,
    RemoteDisconnection,
    HostDown,
    RebaseConflicts,
    MergeConflicts,
    MergeWithLocalChanges,
    RebaseWithLocalChanges,
    ConflictModifyDeletedInBranch,
    HttpsRepositoryNotFound,
    SshRepositoryNotFound,
    BranchDeletionFailed,
    PushWithFileSizeExceeding2GB,
    HexBranchNameRejected,
    ForcePushRejected,
    InvalidConfigValue,
    ConfigLockFileAlreadyExists,
    RemoteAlreadyExists,
    TagAlreadyExists,
    BranchAlreadyExists,
    NoMatchingRemoteBranch,
    NoExistingRemoteBranch,
    NothingToCommit,
    NoSubmoduleMappingFoundInGitmodules,
    SubmoduleRepositoryDoesNotExist,
    InvalidSubmoduleSHA,
    LocalChangesOverwritten,
    UnresolvedConflicts,
    OutOfMemory,
    InvalidObjectName,
    NotAGitRepository,
    CannotMergeUnrelatedHistories,
    LfsAttributeDoesNotMatch,
    PushNotFastForward,
    UnsupportedProtocol,
    UnsafeDirectory,
    BadConfigValue,
    Unknown,
}

impl GitErrorKind {
    pub fn description(self) -> &'static str {
        match self {
            Self::None => "No error",
            Self::SshAuthenticationFailed | Self::SshPermissionDenied => {
                "SSH authentication failed. Please check your SSH keys."
            }
            Self::HttpsAuthenticationFailed => {
                "Authentication failed. You may need to sign in again."
            }
            Self::RemoteDisconnection | Self::HostDown => {
                "The remote disconnected. Check your network connection and try again."
            }
            Self::MergeConflicts | Self::RebaseConflicts => {
                "There are conflicts that need to be resolved before continuing."
            }
            Self::MergeWithLocalChanges | Self::RebaseWithLocalChanges => {
                "You have uncommitted changes. Commit or stash them first."
            }
            Self::HttpsRepositoryNotFound | Self::SshRepositoryNotFound => {
                "The repository does not exist or you do not have access to it."
            }
            Self::PushNotFastForward | Self::ForcePushRejected => {
                "The push was rejected. Fetch and integrate the remote changes first."
            }
            Self::NothingToCommit => "There are no changes to commit.",
            Self::NotAGitRepository => "This directory is not a Git repository.",
            Self::ConfigLockFileAlreadyExists => {
                "Another Git process holds the config lock. Close it and try again."
            }
            Self::UnsafeDirectory => {
                "Git refused the operation because the repository is owned by another user."
            }
            Self::OutOfMemory => "Git ran out of memory.",
            _ => "Git returned an error.",
        }
    }
}

/// Classify git's stderr the way `dugite.parseError` does. Kept as a single
/// ordered match because git messages are not versioned APIs.
pub fn parse_git_error(stdout: &str, stderr: &str) -> GitErrorKind {
    let haystack = format!("{stdout}\n{stderr}").to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| haystack.contains(n));
    if has(&["permission denied (publickey", "ssh authentication failed"]) {
        GitErrorKind::SshAuthenticationFailed
    } else if has(&["authentication failed", "invalid username or password"]) {
        GitErrorKind::HttpsAuthenticationFailed
    } else if has(&[
        "could not resolve host",
        "network is unreachable",
        "timed out",
    ]) {
        GitErrorKind::HostDown
    } else if has(&["the remote end hung up", "early eof", "rpc failed"]) {
        GitErrorKind::RemoteDisconnection
    } else if has(&["conflict", "automatic merge failed", "fix conflicts"]) {
        GitErrorKind::MergeConflicts
    } else if has(&["not a git repository", "not a git repo"]) {
        GitErrorKind::NotAGitRepository
    } else if has(&["nothing to commit", "no changes added to commit"]) {
        GitErrorKind::NothingToCommit
    } else if has(&["non-fast-forward", "fetch first", "rejected"]) {
        GitErrorKind::PushNotFastForward
    } else if has(&[
        "repository not found",
        "could not read from remote repository",
    ]) {
        GitErrorKind::HttpsRepositoryNotFound
    } else if has(&["could not lock config file", "index.lock"]) {
        GitErrorKind::ConfigLockFileAlreadyExists
    } else if has(&["dubious ownership", "unsafe repository"]) {
        GitErrorKind::UnsafeDirectory
    } else if has(&["unrelated histories"]) {
        GitErrorKind::CannotMergeUnrelatedHistories
    } else if has(&["local changes", "would be overwritten"]) {
        GitErrorKind::LocalChangesOverwritten
    } else if has(&["already exists", "a branch named", "a tag named"]) {
        GitErrorKind::BranchAlreadyExists
    } else if has(&["out of memory", "oom"]) {
        GitErrorKind::OutOfMemory
    } else if has(&["protocol", "not supported"]) {
        GitErrorKind::UnsupportedProtocol
    } else {
        GitErrorKind::Unknown
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitError {
    pub kind: GitErrorKind,
    pub message: String,
    pub args: Vec<String>,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

impl GitError {
    pub fn new(
        kind: GitErrorKind,
        args: Vec<String>,
        stdout: String,
        stderr: String,
        exit_code: Option<i32>,
    ) -> Self {
        let message = if stderr.trim().is_empty() {
            kind.description().to_string()
        } else {
            stderr.trim().lines().last().unwrap_or_default().to_string()
        };
        Self {
            kind,
            message,
            args,
            stdout,
            stderr,
            exit_code,
        }
    }

    pub fn is_auth_failure(&self) -> bool {
        matches!(
            self.kind,
            GitErrorKind::SshAuthenticationFailed
                | GitErrorKind::SshPermissionDenied
                | GitErrorKind::HttpsAuthenticationFailed
        )
    }
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (git {:?})", self.message, self.kind)
    }
}

impl std::error::Error for GitError {}
