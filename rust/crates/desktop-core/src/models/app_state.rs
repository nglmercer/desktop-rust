//! Port of `IAppState` (`app/src/lib/app-state.ts`) and the ~40 localStorage
//! preference keys in `app/src/lib/stores/app-store.ts`.
//!
//! In Electron this state lived in the renderer and was rebuilt on every
//! launch from localStorage + Dexie. Here it is owned by Rust and pushed to
//! the webview as an immutable snapshot via the `state-changed` event, which
//! mirrors `AppStore.emitUpdate(state)`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::account::Account;
use super::branch::{Branch, PullRequest};
use super::commit::Commit;
use super::diff::{Diff, ImageDiffType};
use super::misc::{
    AppMenu, Banner, FoldoutType, Popup, RepositorySectionTab, SelectionType, ThemeSource,
    WindowState,
};
use super::repository::{AheadBehind, LocalRepositoryState, Repository};
use super::status::{StatusResult, WorkingDirectoryFileChange};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitSelection {
    #[serde(default)]
    pub sha: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub changeset_data: HashMap<String, String>,
    #[serde(default)]
    pub diff: Option<Diff>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesState {
    #[serde(default)]
    pub working_directory: StatusResult,
    #[serde(default)]
    pub commit_message: Option<super::commit::CommitMessage>,
    #[serde(default)]
    pub show_co_authored_by: bool,
    #[serde(default)]
    pub conflict_state: Option<Value>,
    #[serde(default)]
    pub selection: DiffSelectionKind,
    #[serde(default)]
    pub staged_files: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffSelectionKind {
    #[default]
    None,
    All,
    Partial,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchesState {
    #[serde(default)]
    pub tip: TipKind,
    #[serde(default)]
    pub default_branch: Option<Branch>,
    #[serde(default)]
    pub all_branches: Vec<Branch>,
    #[serde(default)]
    pub recent_branches: Vec<Branch>,
    #[serde(default)]
    pub pull_requests: Vec<PullRequest>,
    #[serde(default)]
    pub ahead_behind: AheadBehind,
    #[serde(default)]
    pub is_loading: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TipKind {
    #[default]
    Unknown,
    Unborn,
    Detached,
    Valid(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryState {
    #[serde(default)]
    pub commits: Vec<Commit>,
    #[serde(default)]
    pub selection: CommitSelection,
    #[serde(default)]
    pub changed_files: Vec<WorkingDirectoryFileChange>,
    #[serde(default)]
    pub has_more: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryState {
    #[serde(default)]
    pub branches_state: BranchesState,
    #[serde(default)]
    pub changes_state: ChangesState,
    #[serde(default)]
    pub history_state: HistoryState,
    #[serde(default)]
    pub selected_section: RepositorySectionTab,
    #[serde(default)]
    pub commit_selection: CommitSelection,
    #[serde(default)]
    pub local_repository_state: LocalRepositoryState,
    #[serde(default)]
    pub is_loading_status: bool,
}

/// `IRepositoryState` keyed by repository id, equivalent to
/// `repositoryStateManager` / `stateLookup` in `AppStore`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<Repository>,
    #[serde(default)]
    pub state: Option<RepositoryState>,
    #[serde(rename = "type", default)]
    pub kind: SelectionTypeKind,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectionTypeKind {
    #[default]
    Repository,
    CloningRepository,
    MissingRepository,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInState {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub is_loading: bool,
}

/// Persistable user preferences. Field names match the localStorage keys used
/// by `app-store.ts` (camelCased) so the importer can read them verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: u32,
    #[serde(default = "default_commit_list_width")]
    pub commit_list_width: u32,
    #[serde(default)]
    pub external_editor: Option<String>,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default = "default_image_diff_type")]
    pub image_diff_type: ImageDiffType,
    #[serde(default = "default_tab_size")]
    pub tab_size: u32,
    #[serde(default)]
    pub confirm_repo_removal: bool,
    #[serde(default = "default_true")]
    pub confirm_discard_changes: bool,
    #[serde(default = "default_true")]
    pub confirm_force_push: bool,
    #[serde(default = "default_true")]
    pub confirm_committing_conflicted_files: bool,
    #[serde(default)]
    pub uncommitted_changes_strategy_kind: String,
    #[serde(default)]
    pub theme_source: ThemeSource,
    #[serde(default = "default_zoom")]
    pub zoom_factor: f64,
    #[serde(default)]
    pub stats_opt_out: bool,
    #[serde(default)]
    pub ask_to_move_to_applications_folder: bool,
    #[serde(default)]
    pub repository_indicators_enabled: bool,
    #[serde(default)]
    pub show_stashed_changes: bool,
    #[serde(default)]
    pub use_new_close_ui: bool,
    #[serde(default)]
    pub last_selected_repository_id: Option<i64>,
    #[serde(default)]
    pub recently_selected_repositories: Vec<i64>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            sidebar_width: 250,
            commit_list_width: 300,
            external_editor: None,
            shell: None,
            image_diff_type: ImageDiffType::TwoUp,
            tab_size: 4,
            confirm_repo_removal: false,
            confirm_discard_changes: true,
            confirm_force_push: true,
            confirm_committing_conflicted_files: true,
            uncommitted_changes_strategy_kind: "askForConfirmation".to_string(),
            theme_source: ThemeSource::System,
            zoom_factor: 1.0,
            stats_opt_out: false,
            ask_to_move_to_applications_folder: true,
            repository_indicators_enabled: true,
            show_stashed_changes: false,
            use_new_close_ui: false,
            last_selected_repository_id: None,
            recently_selected_repositories: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_sidebar_width() -> u32 {
    250
}
fn default_commit_list_width() -> u32 {
    300
}
fn default_tab_size() -> u32 {
    4
}
fn default_zoom() -> f64 {
    1.0
}
fn default_image_diff_type() -> ImageDiffType {
    ImageDiffType::TwoUp
}

/// The full snapshot sent to the webview. Tokens are deliberately absent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub version: u32,
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub repositories: Vec<Repository>,
    #[serde(default)]
    pub recent_repositories: Vec<i64>,
    #[serde(default)]
    pub selected_state: SelectedState,
    #[serde(default)]
    pub repository_states: HashMap<String, RepositoryState>,
    #[serde(default)]
    pub local_repository_state_lookup: HashMap<String, LocalRepositoryState>,
    #[serde(default)]
    pub sign_in_state: SignInState,
    #[serde(default)]
    pub preferences: Preferences,
    #[serde(default)]
    pub window_state: WindowState,
    #[serde(default)]
    pub window_zoom_factor: f64,
    #[serde(default)]
    pub app_menu: AppMenu,
    #[serde(default)]
    pub current_popup: Option<Popup>,
    #[serde(default)]
    pub current_foldout: Option<FoldoutType>,
    #[serde(default)]
    pub current_banner: Option<Banner>,
    #[serde(default)]
    pub errors: Vec<Value>,
    #[serde(default)]
    pub emoji: HashMap<String, String>,
    #[serde(default)]
    pub selected_section: RepositorySectionTab,
    #[serde(default)]
    pub selection_type: SelectionType,
    #[serde(default)]
    pub is_update_available: bool,
    #[serde(default)]
    pub is_update_downloading: bool,
    #[serde(default)]
    pub quit_flags: QuitFlags,
    #[serde(default)]
    pub launch_stats: Option<super::misc::LaunchStats>,
    #[serde(default)]
    pub endpoint_tokens: Vec<super::account::EndpointToken>,
    #[serde(default)]
    pub image_diff_type: ImageDiffType,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuitFlags {
    #[serde(default)]
    pub quitting: bool,
    #[serde(default)]
    pub quitting_even_if_updating: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            accounts: Vec::new(),
            repositories: Vec::new(),
            recent_repositories: Vec::new(),
            selected_state: SelectedState::default(),
            repository_states: HashMap::new(),
            local_repository_state_lookup: HashMap::new(),
            sign_in_state: SignInState::default(),
            preferences: Preferences::default(),
            window_state: WindowState::Normal,
            window_zoom_factor: 1.0,
            app_menu: AppMenu::default(),
            current_popup: None,
            current_foldout: None,
            current_banner: None,
            errors: Vec::new(),
            emoji: HashMap::new(),
            selected_section: RepositorySectionTab::Changes,
            selection_type: SelectionType::Repository,
            is_update_available: false,
            is_update_downloading: false,
            quit_flags: QuitFlags::default(),
            launch_stats: None,
            endpoint_tokens: Vec::new(),
            image_diff_type: ImageDiffType::TwoUp,
        }
    }
}

pub const STATE_VERSION: u32 = 1;

impl AppState {
    /// Never leak credentials into a state snapshot (mirrors the token
    /// stripping done by `AccountsStore` before writing localStorage).
    pub fn sanitized(&self) -> Self {
        let mut clone = self.clone();
        clone.accounts = clone.accounts.iter().map(Account::without_token).collect();
        clone.endpoint_tokens.clear();
        clone
    }

    pub fn selected_repository(&self) -> Option<&Repository> {
        self.selected_state.repository.as_ref()
    }

    pub fn repository_by_id(&self, id: i64) -> Option<&Repository> {
        self.repositories.iter().find(|r| r.id == id)
    }

    pub fn next_repository_id(&self) -> i64 {
        self.repositories.iter().map(|r| r.id).max().unwrap_or(0) + 1
    }
}
