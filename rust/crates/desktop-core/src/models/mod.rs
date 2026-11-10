//! Rust ports of `app/src/models/*.ts`.
//!
//! All types derive `Serialize`/`Deserialize` with camelCase field names so
//! the JSON that crosses the wry bridge is drop-in compatible with the shapes
//! the existing React renderer already expects.

pub mod account;
pub mod app_state;
pub mod branch;
pub mod commit;
pub mod diff;
pub mod misc;
pub mod repository;
pub mod status;

pub use account::{Account, ApiRepository, Email, EndpointToken, GitAccount, OAuthToken};
pub use app_state::{
    AppState, BranchesState, ChangesState, CommitSelection, HistoryState, Preferences,
    RepositoryState, STATE_VERSION, SelectedState, SignInState,
};
pub use branch::{Branch, BranchType, PullRequest, StashEntry, Tip};
pub use commit::{Commit, CommitIdentity, CommitMessage, GitAuthor, Trailer};
pub use diff::{Diff, DiffHunk, DiffLine, DiffSelection, DiffType, ImageDiffType};
pub use misc::{
    AppMenu, AppleActionOnDoubleClick, Architecture, Banner, CrashDetails,
    DesktopNotificationPermission, FoldoutType, LaunchStats, PathType, Popup, PopupType, Progress,
    RepositorySectionTab, SelectionType, ThemeSource, WindowState,
};
pub use repository::{
    AheadBehind, CloneOptions, CloningRepository, GitHubRepository, LocalRepositoryState, Owner,
    Remote, Repository, WorkflowPreferences, WorktreeEntry,
};
pub use status::{AppFileStatusKind, GitStatusEntry, StatusResult, WorkingDirectoryFileChange};
