//! Port of `app/src/lib/api.ts` (`API` class) and `app/src/lib/http.ts`.
//!
//! Desktop's renderer owned the token (via `AccountsStore` + keytar) and
//! injected it with `API.setTokenProvider`. In Rust the token never leaves the
//! process: [`ApiClient`] reads it from the OS credential store on each
//! request and only returns safe response data to the webview.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::{AppError, Result};
use crate::models::account::{Account, ApiIdentity, ApiRepository};
use crate::storage::SecureStore;

use super::endpoints::{USER_AGENT, get_absolute_url, get_dot_com_api_endpoint};

#[derive(Debug, Clone)]
pub struct ApiClient {
    client: reqwest::blocking::Client,
    secure: SecureStore,
}

impl ApiClient {
    pub fn new(secure: SecureStore) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { client, secure })
    }

    pub fn token_for(&self, account: &Account) -> Option<String> {
        self.secure
            .get_item(&account.key(), &account.login)
            .ok()
            .flatten()
    }

    /// `request()` in `http.ts`: bearer auth, JSON accept headers, error text.
    fn request_raw(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> Result<reqwest::blocking::Response> {
        let url = get_absolute_url(endpoint, path);
        let mut builder = self
            .client
            .request(method, &url)
            .header("Accept", "application/vnd.github.v3+json, application/json")
            .header("Content-Type", "application/json")
            .header("X-GitHub-Client", USER_AGENT);
        if let Some(token) = token {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        if let Some(body) = body {
            builder = builder.json(body);
        }
        let response = builder.send()?;
        if !response.status().is_success() {
            return Err(AppError::msg(format!(
                "GitHub API {url} returned {} {}",
                response.status().as_u16(),
                response.status().canonical_reason().unwrap_or("error")
            )));
        }
        Ok(response)
    }

    pub fn request<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> Result<T> {
        let response = self.request_raw(method, endpoint, path, token, body)?;
        Ok(response.json::<T>()?)
    }

    pub fn request_value(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> Result<Value> {
        self.request(method, endpoint, path, token, body)
    }

    /// `/user` — `fetchUser()`.
    pub fn fetch_user(&self, endpoint: &str, token: &str) -> Result<ApiIdentity> {
        self.request(reqwest::Method::GET, endpoint, "/user", Some(token), None)
    }

    /// `/user/emails`.
    pub fn fetch_emails(&self, endpoint: &str, token: &str) -> Result<Value> {
        self.request_value(
            reqwest::Method::GET,
            endpoint,
            "/user/emails",
            Some(token),
            None,
        )
    }

    /// `/user/repos?affiliation=owner,collaborator` (Desktop's
    /// `streamUserRepositories`), with `Link:` pagination.
    pub fn fetch_user_repositories(
        &self,
        endpoint: &str,
        token: &str,
    ) -> Result<Vec<ApiRepository>> {
        let mut all = Vec::new();
        let mut path = "/user/repos?per_page=100&affiliation=owner,collaborator".to_string();
        for _ in 0..20 {
            let response =
                self.request_raw(reqwest::Method::GET, endpoint, &path, Some(token), None)?;
            let next = response
                .headers()
                .get("link")
                .and_then(|v| v.to_str().ok())
                .and_then(super::endpoints::get_next_page_path_from_link);
            let mut page: Vec<ApiRepository> = response.json()?;
            all.append(&mut page);
            match next {
                Some(url) => path = url,
                None => break,
            }
        }
        Ok(all)
    }

    pub fn fetch_repository(
        &self,
        endpoint: &str,
        token: &str,
        owner: &str,
        name: &str,
    ) -> Result<ApiRepository> {
        self.request(
            reqwest::Method::GET,
            endpoint,
            &format!("/repos/{owner}/{name}"),
            Some(token),
            None,
        )
    }

    pub fn fetch_pull_requests(
        &self,
        endpoint: &str,
        token: &str,
        owner: &str,
        name: &str,
    ) -> Result<Value> {
        self.request_value(
            reqwest::Method::GET,
            endpoint,
            &format!("/repos/{owner}/{name}/pulls?state=open&per_page=100"),
            Some(token),
            None,
        )
    }

    pub fn create_pull_request(
        &self,
        endpoint: &str,
        token: &str,
        owner: &str,
        name: &str,
        title: &str,
        head: &str,
        base: &str,
        body: &str,
    ) -> Result<Value> {
        self.request_value(
            reqwest::Method::POST,
            endpoint,
            &format!("/repos/{owner}/{name}/pulls"),
            Some(token),
            Some(&json!({ "title": title, "head": head, "base": base, "body": body })),
        )
    }

    pub fn fetch_issues(
        &self,
        endpoint: &str,
        token: &str,
        owner: &str,
        name: &str,
    ) -> Result<Value> {
        self.request_value(
            reqwest::Method::GET,
            endpoint,
            &format!("/repos/{owner}/{name}/issues?state=open&per_page=100"),
            Some(token),
            None,
        )
    }

    pub fn fetch_check_runs(
        &self,
        endpoint: &str,
        token: &str,
        owner: &str,
        name: &str,
        sha: &str,
    ) -> Result<Value> {
        self.request_value(
            reqwest::Method::GET,
            endpoint,
            &format!("/repos/{owner}/{name}/commits/{sha}/check-runs"),
            Some(token),
            None,
        )
    }

    pub fn graphql(
        &self,
        endpoint: &str,
        token: &str,
        query: &str,
        variables: Value,
    ) -> Result<Value> {
        self.request_value(
            reqwest::Method::POST,
            endpoint,
            "/graphql",
            Some(token),
            Some(&json!({ "query": query, "variables": variables })),
        )
    }

    /// `/desktop/avatar-token` — used by the authenticated-image filter.
    pub fn fetch_avatar_token(&self, endpoint: &str, token: &str) -> Result<Value> {
        self.request_value(
            reqwest::Method::GET,
            endpoint,
            "/desktop/avatar-token",
            Some(token),
            None,
        )
    }

    pub fn delete_token(&self, endpoint: &str, token: &str) -> Result<()> {
        self.request_value(
            reqwest::Method::DELETE,
            endpoint,
            "/desktop/token",
            Some(token),
            None,
        )?;
        Ok(())
    }

    pub fn default_endpoint() -> String {
        get_dot_com_api_endpoint()
    }
}
