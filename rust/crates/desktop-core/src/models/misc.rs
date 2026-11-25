//! Shared enums and small value objects from `app/src/models/*` and
//! `app/src/lib/*`. JSON naming follows the TypeScript camelCase/kebab-case
//! wire format so the web layer can consume the same shapes.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowState {
    #[default]
    Normal,
    Minimized,
    Maximized,
    FullScreen,
    Hidden,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeSource {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Architecture {
    #[serde(rename = "x64")]
    X64,
    #[serde(rename = "arm64")]
    Arm64,
    #[serde(rename = "x86")]
    X86,
}

impl Architecture {
    pub fn current() -> Self {
        match std::env::consts::ARCH {
            "x86_64" => Self::X64,
            "aarch64" => Self::Arm64,
            _ => Self::X86,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PathType {
    Home,
    AppData,
    UserData,
    Cache,
    Temp,
    Documents,
    Downloads,
    Desktop,
    Exe,
    Module,
    Logs,
    CrashDumps,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppleActionOnDoubleClick {
    Maximize,
    Minimize,
    None,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DesktopNotificationPermission {
    #[default]
    Default,
    Granted,
    Denied,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectionType {
    #[default]
    Repository,
    CloningRepository,
    MissingRepository,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RepositorySectionTab {
    #[default]
    Changes,
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FoldoutType {
    Repository,
    Branch,
    AppMenu,
    AddMenu,
    PullRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PopupType {
    RenameBranch,
    DeleteBranch,
    ConfirmDiscardChanges,
    Preferences,
    RepositorySettings,
    AddRepository,
    CreateRepository,
    CloneRepository,
    CreateBranch,
    SignIn,
    About,
    CreatePullRequest,
    MergeConflicts,
    AbortMerge,
    StashAndSwitchBranch,
    ConfirmForcePush,
    ConfirmRemoveRepository,
    ReleaseNotes,
    MoveToApplicationsFolder,
    MissingRepository,
    CreateTag,
    DeleteTag,
    DiscardChanges,
    DeletePullRequest,
    UnsupportedWideWarning,
    CreateIssue,
    TestNotifications,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Popup {
    #[serde(rename = "type")]
    pub kind: PopupType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<crate::models::repository::Repository>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BannerType {
    SuccessfulMerge,
    SuccessfulRebase,
    SuccessfulCherryPick,
    MergeConflictsFound,
    RebaseConflictsFound,
    AlreadyOnBranch,
    RevertUndone,
    BranchUpToDate,
    GitLFSNotFound,
    UpdateAvailable,
    CouldNotResolveRef,
    ConflictsFound,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Banner {
    #[serde(rename = "type")]
    pub kind: BannerType,
    pub message: String,
}

/// Serialized application menu (`IMenu` and its host menu item payload).
/// GitHub Desktop renders its own title-bar menu from this payload, which is
/// why the Rust shell can push it to the webview instead of building a native
/// menu on Windows. A native `muda` menu is built from the same model.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppMenu {
    #[serde(default)]
    pub items: Vec<crate::ipc::protocol::SerializableMenuItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub kind: String,
    pub title: String,
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchStats {
    pub main_ready_time: f64,
    pub load_time: f64,
    pub renderer_ready_time: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashDetails {
    #[serde(rename = "type")]
    pub kind: String,
    pub error: CrashError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashError {
    pub message: String,
    pub name: String,
    #[serde(default)]
    pub stack: Option<String>,
}
