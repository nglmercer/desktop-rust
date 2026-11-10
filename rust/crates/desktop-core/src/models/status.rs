//! Port of `app/src/models/status.ts` and the output of
//! `app/src/lib/status-parser.ts` (`IStatusResult`).

use serde::{Deserialize, Serialize};

/// `AppFileStatusKind` in the TS model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppFileStatusKind {
    #[default]
    New,
    Modified,
    Deleted,
    Renamed,
    Conflicted,
    Copied,
}

/// Raw two-letter porcelain codes (`GitStatusEntry`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GitStatusEntry {
    #[default]
    Unmodified,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
    Untracked,
    Ignored,
    TypeChanged,
    UpdatedButUnmerged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub status: AppFileStatusKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingDirectoryFileChange {
    #[serde(flatten)]
    pub change: FileChange,
    pub index_status: GitStatusEntry,
    pub working_tree_status: GitStatusEntry,
    #[serde(default)]
    pub selected: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictDetails {
    #[serde(default)]
    pub conflicted_files: Vec<String>,
    #[serde(default)]
    pub rebase_internal_state: Option<serde_json::Value>,
    #[serde(default)]
    pub merge_internal_state: Option<serde_json::Value>,
}

/// `IStatusResult` — the single most important git payload in Desktop.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResult {
    pub exists: bool,
    #[serde(default)]
    pub current_branch: Option<String>,
    #[serde(default)]
    pub current_upstream_branch: Option<String>,
    #[serde(default)]
    pub current_tip: Option<String>,
    #[serde(default)]
    pub branch_ahead_behind: Option<crate::models::repository::AheadBehind>,
    #[serde(default)]
    pub working_directory_files: Vec<WorkingDirectoryFileChange>,
    #[serde(default)]
    pub do_conflicted_files_exist: bool,
    #[serde(default)]
    pub merge_head_found: bool,
    #[serde(default)]
    pub squash_msg_found: bool,
    #[serde(default)]
    pub rebase_internal_state: Option<serde_json::Value>,
    #[serde(default)]
    pub is_detached_head: bool,
}

impl StatusResult {
    pub fn changed_files_count(&self) -> u32 {
        self.working_directory_files.len() as u32
    }
}

/// Map a porcelain `XY` status pair onto the app's coarse status kind.
pub fn status_kind(index: char, worktree: char) -> AppFileStatusKind {
    if index == 'U'
        || worktree == 'U'
        || (index == 'A' && worktree == 'A')
        || (index == 'D' && worktree == 'D')
    {
        return AppFileStatusKind::Conflicted;
    }
    match (index, worktree) {
        ('?', _) => AppFileStatusKind::New,
        ('A', _) | (_, 'A') => AppFileStatusKind::New,
        ('R', _) | (_, 'R') | ('C', _) => AppFileStatusKind::Renamed,
        ('D', _) | (_, 'D') => AppFileStatusKind::Deleted,
        _ => AppFileStatusKind::Modified,
    }
}

pub fn git_status_entry(code: char) -> GitStatusEntry {
    match code {
        ' ' => GitStatusEntry::Unmodified,
        'M' => GitStatusEntry::Modified,
        'A' => GitStatusEntry::Added,
        'D' => GitStatusEntry::Deleted,
        'R' => GitStatusEntry::Renamed,
        'C' => GitStatusEntry::Copied,
        'U' => GitStatusEntry::Unmerged,
        '?' => GitStatusEntry::Untracked,
        '!' => GitStatusEntry::Ignored,
        'T' => GitStatusEntry::TypeChanged,
        _ => GitStatusEntry::UpdatedButUnmerged,
    }
}
