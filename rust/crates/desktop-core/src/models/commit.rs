//! Port of `app/src/models/commit.ts`, `commit-identity.ts`, `git-author.ts`
//! and `app/src/lib/git/log.ts` output shapes.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitIdentity {
    pub name: String,
    pub email: String,
    /// ISO-8601. The legacy shell used `Date`; the wire format is a string.
    pub date: String,
    #[serde(default)]
    pub tz_offset: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitAuthor {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trailer {
    pub token: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub sha: String,
    pub short_sha: String,
    pub summary: String,
    #[serde(default)]
    pub body: String,
    pub author: CommitIdentity,
    #[serde(default)]
    pub committer: Option<CommitIdentity>,
    #[serde(default)]
    pub parent_shas: Vec<String>,
    #[serde(default)]
    pub trailers: Vec<Trailer>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub co_authors: Vec<GitAuthor>,
}

impl Commit {
    pub fn short(sha: &str) -> String {
        sha.chars().take(7).collect()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitMessage {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub status: String,
    #[serde(default)]
    pub old_path: Option<String>,
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
}
