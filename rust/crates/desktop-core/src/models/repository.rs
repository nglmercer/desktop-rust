//! Port of `app/src/models/repository.ts`, `github-repository.ts`, `owner.ts`,
//! `remote.ts`, `worktree.ts` and `workflow-preferences.ts`.
//!
//! Electron persisted these through Dexie `IDatabase*` DTOs. Rust persists
//! `Repository` directly (see `storage::json_store`), keeping the same JSON
//! field names so existing user data can be imported.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Owner {
    pub login: String,
    pub endpoint: String,
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default = "default_owner_type")]
    pub kind: String,
}

fn default_owner_type() -> String {
    "user".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryPermissions {
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub push: bool,
    #[serde(default)]
    pub pull: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubRepository {
    pub name: String,
    pub owner: Owner,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub db_id: Option<i64>,
    #[serde(default)]
    pub is_private: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clone_url: Option<String>,
    #[serde(default)]
    pub issues_enabled: bool,
    #[serde(default)]
    pub is_archived: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<RepositoryPermissions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Box<GitHubRepository>>,
}

impl GitHubRepository {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner.login, self.name)
    }
    pub fn endpoint(&self) -> &str {
        &self.owner.endpoint
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ForkContributionTarget {
    #[default]
    Parent,
    SelfTarget,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPreferences {
    #[serde(default)]
    pub fork_contribution_target: ForkContributionTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub path: String,
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_hub_repository: Option<GitHubRepository>,
    #[serde(default)]
    pub missing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default)]
    pub workflow_preferences: WorkflowPreferences,
    #[serde(default)]
    pub is_tutorial_repository: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main_worktree_path: Option<String>,
    #[serde(default)]
    pub last_stash_check_date: Option<i64>,
}

impl Repository {
    pub fn local(path: impl Into<String>, id: i64) -> Self {
        Self {
            path: path.into(),
            id,
            git_hub_repository: None,
            missing: false,
            alias: None,
            workflow_preferences: WorkflowPreferences::default(),
            is_tutorial_repository: false,
            git_dir: None,
            main_worktree_path: None,
            last_stash_check_date: None,
        }
    }

    /// Structural-equality hash, equivalent to `createEqualityHash` in the TS
    /// model. Used by the renderer to key caches.
    pub fn hash(&self) -> String {
        let mut input = format!("{}|{}|{}", self.id, self.path, self.missing);
        if let Some(gh) = &self.git_hub_repository {
            input.push('|');
            input.push_str(&gh.full_name());
            input.push('|');
            input.push_str(gh.endpoint());
        }
        if let Some(alias) = &self.alias {
            input.push('|');
            input.push_str(alias);
        }
        format!("{:016x}", simple_hash(&input))
    }

    pub fn name(&self) -> String {
        std::path::Path::new(&self.path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.clone())
    }
}

/// Stable, non-cryptographic FNV-1a. Only used for cache keys / equality checks.
pub fn simple_hash(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeEntry {
    pub path: String,
    pub head: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default)]
    pub is_bare: bool,
    #[serde(default)]
    pub is_locked: bool,
    #[serde(default)]
    pub is_prunable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloningRepository {
    pub path: String,
    pub url: String,
    #[serde(default)]
    pub progress: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AheadBehind {
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRepositoryState {
    #[serde(default)]
    pub ahead_behind: AheadBehind,
    #[serde(default)]
    pub changed_files_count: u32,
    #[serde(default)]
    pub branches_state: serde_json::Value,
}
