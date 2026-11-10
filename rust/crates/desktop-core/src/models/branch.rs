//! Port of `app/src/models/branch.ts`, `tip.ts`, `pull-request.ts`,
//! `stash-entry.ts` and `app/src/models/retry-actions.ts`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BranchType {
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchTip {
    pub sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    pub tip: BranchTip,
    #[serde(rename = "type")]
    pub kind: BranchType,
    #[serde(default)]
    pub ref_name: String,
    #[serde(default)]
    pub is_head: bool,
}

impl Branch {
    pub fn name_without_remote(&self) -> &str {
        self.name
            .split_once('/')
            .map(|(_, rest)| rest)
            .unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Tip {
    Unknown,
    Unborn,
    DetachedHead { sha: String },
    Valid { branch: Box<Branch> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestRef {
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub sha: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_full_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub number: i64,
    pub title: String,
    pub created_at: String,
    pub head: PullRequestRef,
    pub base: PullRequestRef,
    pub author: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StashEntry {
    pub name: String,
    pub branch_name: String,
    pub sha: String,
    #[serde(default)]
    pub files: Vec<StashedFileChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StashedFileChange {
    pub path: String,
    pub status: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RetryActionType {
    Push,
    Pull,
    Fetch,
    Clone,
    Checkout,
    Merge,
    Rebase,
    CherryPick,
    CreateBranch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryAction {
    #[serde(rename = "type")]
    pub kind: RetryActionType,
    pub repository_path: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}
