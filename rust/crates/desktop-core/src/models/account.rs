//! Port of `app/src/models/account.ts`, `git-account.ts`, `oauth-token.ts`,
//! `app/src/lib/endpoint-token.ts` and `account-credential.ts`.
//!
//! Tokens are NEVER serialized into the app-state snapshot that goes to the
//! webview. They live in the OS credential store (see `storage::secure_store`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub login: String,
    pub endpoint: String,
    /// Always empty in any state sent to the webview. The legacy shell stripped tokens
    /// before writing `localStorage.setItem('users', …)`; we do the same.
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub emails: Vec<Email>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub copilot_endpoint: Option<String>,
}

impl Account {
    /// Stable key used by the token store, matching `getKeyForAccount`.
    pub fn key(&self) -> String {
        format!("{}@{}", self.login, self.endpoint)
    }
    pub fn without_token(&self) -> Self {
        let mut clone = self.clone();
        clone.token.clear();
        clone
    }
    pub fn is_dot_com(&self) -> bool {
        self.endpoint == crate::api::endpoints::DOTCOM_API_ENDPOINT
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Email {
    pub email: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub visibility: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitAccount {
    pub login: String,
    pub endpoint: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointToken {
    pub endpoint: String,
    pub token: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthToken {
    pub access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token_expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token_expires_at: Option<String>,
}

impl OAuthToken {
    /// `serializeAccountCredential` / `deserializeAccountCredential`.
    pub fn serialize(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
    pub fn deserialize(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }
}

/// `IAPIRepositoryPermissions` from `app/src/lib/api.ts`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiRepositoryPermissions {
    #[serde(default)]
    pub admin: bool,
    #[serde(default, rename = "push")]
    pub push: bool,
    #[serde(default, rename = "pull")]
    pub pull: bool,
}

/// Subset of `IAPIRepository` / `IAPIFullRepository` used for repository
/// matching, publishing and clone URL discovery.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiRepository {
    #[serde(default, rename = "name")]
    pub name: String,
    #[serde(default, rename = "full_name")]
    pub full_name: String,
    #[serde(default, rename = "clone_url")]
    pub clone_url: String,
    #[serde(default, rename = "html_url")]
    pub html_url: String,
    #[serde(default, rename = "default_branch")]
    pub default_branch: String,
    #[serde(default, rename = "private")]
    pub is_private: bool,
    #[serde(default, rename = "archived")]
    pub is_archived: bool,
    #[serde(default, rename = "id")]
    pub id: i64,
    #[serde(default)]
    pub owner: Option<ApiIdentity>,
    #[serde(default)]
    pub parent: Option<Box<ApiRepository>>,
    #[serde(default)]
    pub permissions: Option<ApiRepositoryPermissions>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiIdentity {
    #[serde(default, rename = "login")]
    pub login: String,
    #[serde(default, rename = "id")]
    pub id: i64,
    #[serde(default, rename = "avatar_url")]
    pub avatar_url: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default, rename = "html_url")]
    pub html_url: Option<String>,
}
