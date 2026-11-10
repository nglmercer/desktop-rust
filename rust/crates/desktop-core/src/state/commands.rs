//! Domain commands introduced by the Rust port.
//!
//! In Electron the renderer imported `fs`, `path`, `dugite` and `api.ts`
//! directly thanks to `nodeIntegration: true`. wry has no Node runtime, so
//! every one of those capabilities must become an explicit Rust command.
//! These are namespaced `desktop.*` to distinguish them from the 79 legacy
//! Electron channels handled in [`super::dispatcher`].
//!
//! Convention: the first argument is an options object. `repositoryPath` may
//! be omitted, in which case the selected repository is used.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::app::DesktopApp;
use crate::error::{AppError, Result};
use crate::git::{diff as git_diff, log as git_log, operations as ops, refs as git_refs, status as git_status};
use crate::ipc::protocol::IpcMessage;
use crate::models::account::Account;
use crate::models::app_state::{AppState, RepositoryState};
use crate::models::misc::PopupType;
use crate::models::repository::Repository;

use super::dispatcher::Outcome;

fn options(msg: &IpcMessage) -> Value {
    msg.arg(0).cloned().unwrap_or(json!({}))
}

fn get_str(o: &Value, key: &str) -> Option<String> {
    o.get(key).and_then(Value::as_str).map(str::to_string)
}

fn get_bool(o: &Value, key: &str, default: bool) -> bool {
    o.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn get_u32(o: &Value, key: &str, default: u32) -> u32 {
    o.get(key).and_then(Value::as_u64).map(|v| v as u32).unwrap_or(default)
}

fn get_strings(o: &Value, key: &str) -> Vec<String> {
    o.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn required(o: &Value, key: &str) -> Result<String> {
    get_str(o, key).ok_or_else(|| AppError::BadArgs(format!("missing `{key}`")))
}

fn repo_path(state: &AppState, o: &Value) -> Result<PathBuf> {
    if let Some(path) = get_str(o, "repositoryPath") {
        return Ok(PathBuf::from(path));
    }
    state
        .selected_repository()
        .map(|r| PathBuf::from(&r.path))
        .ok_or_else(|| AppError::msg("no repository selected"))
}

fn git_value(out: &crate::git::core::GitOutput) -> Value {
    json!({
        "stdout": out.stdout,
        "stderr": out.stderr,
        "exitCode": out.exit_code,
    })
}

/// Refresh and cache the repository state exactly as `AppStore._refreshRepository`
/// would, then return the sanitized state to the caller.
fn refresh(app: &DesktopApp, state: &mut AppState, path: &Path) -> Result<Value> {
    let status = git_status::get_status(&app.git, path)?;
    let branches = git_refs::get_branches(&app.git, path).unwrap_or_default();
    let remotes = git_refs::get_remotes(&app.git, path).unwrap_or_default();
    let commits = git_log::get_commits(&app.git, path, None, 100, 0).unwrap_or_default();

    let repository_state = state
        .repository_states
        .entry(path.to_string_lossy().into_owned())
        .or_insert_with(RepositoryState::default);
    repository_state.changes_state.working_directory = status.clone();
    repository_state.branches_state.all_branches = branches.clone();
    repository_state.history_state.commits = commits.clone();
    repository_state.is_loading_status = false;

    let key = path.to_string_lossy().into_owned();
    state.local_repository_state_lookup.insert(
        key,
        crate::models::repository::LocalRepositoryState {
            ahead_behind: status.branch_ahead_behind.clone().unwrap_or_default(),
            changed_files_count: status.changed_files_count(),
            ..Default::default()
        },
    );

    Ok(json!({
        "status": status,
        "branches": branches,
        "remotes": remotes,
        "commits": commits,
    }))
}

pub fn invoke(app: &DesktopApp, state: &mut AppState, msg: &IpcMessage) -> Result<Outcome> {
    let o = options(msg);
    let channel = msg.channel.as_str();

    let value = match channel {
        // -- state ---------------------------------------------------------
        "desktop.get-state" => serde_json::to_value(state.sanitized())?,
        "desktop.get-app-info" => json!({
            "name": "GitHub Desktop (Rust)",
            "version": env!("CARGO_PKG_VERSION"),
            "runtime": "tao + wry",
            "git": app.git.version().unwrap_or_else(|_| "unavailable".into()),
            "userData": app.paths.user_data.to_string_lossy(),
            "pendingPromises": app.resources.limits.pending_promises,
            "profile": app.resources.limits.profile.name(),
        }),

        // -- repositories --------------------------------------------------
        "desktop.add-repository" => {
            let path = required(&o, "path")?;
            if !app.git.is_repository(Path::new(&path)) {
                return Err(AppError::msg(format!("{path} is not a Git repository")));
            }
            if state.repositories.iter().any(|r| r.path == path) {
                return Ok((json!({ "added": false }), false));
            }
            let id = state.next_repository_id();
            let mut repository = Repository::local(path.clone(), id);
            if let Ok(Some(url)) = app.git.config("remote.origin.url", Path::new(&path)) {
                let (owner, name) = parse_remote_repo(&url);
                repository.git_hub_repository = Some(crate::models::repository::GitHubRepository {
                    name,
                    owner: crate::models::repository::Owner {
                        login: owner,
                        endpoint: crate::api::endpoints::get_endpoint_for_repository(&url),
                        id: None,
                        kind: "user".into(),
                    },
                    db_id: None,
                    is_private: false,
                    html_url: Some(url.trim_end_matches(".git").to_string()),
                    clone_url: Some(url),
                    issues_enabled: true,
                    is_archived: false,
                    permissions: None,
                    parent: None,
                });
            }
            state.repositories.push(repository.clone());
            state.selected_state.repository = Some(repository);
            state.preferences.last_selected_repository_id = Some(id);
            json!({ "added": true, "id": id })
        }
        "desktop.add-repositories" => {
            let mut added = Vec::new();
            for path in get_strings(&o, "paths") {
                if app.git.is_repository(Path::new(&path))
                    && !state.repositories.iter().any(|r| r.path == path)
                {
                    let id = state.next_repository_id() + added.len() as i64;
                    state.repositories.push(Repository::local(path.clone(), id));
                    added.push(id);
                }
            }
            json!({ "added": added })
        }
        "desktop.remove-repository" => {
            let id = o.get("id").and_then(Value::as_i64).unwrap_or(-1);
            let before = state.repositories.len();
            state.repositories.retain(|r| r.id != id);
            if state.selected_state.repository.as_ref().map(|r| r.id) == Some(id) {
                state.selected_state.repository = state.repositories.first().cloned();
            }
            json!({ "removed": before - state.repositories.len() })
        }
        "desktop.select-repository" => {
            let id = o.get("id").and_then(Value::as_i64).unwrap_or(-1);
            let found = state.repository_by_id(id).cloned();
            if let Some(repository) = found {
                state.selected_state.repository = Some(repository);
                state.preferences.last_selected_repository_id = Some(id);
                if !state.recent_repositories.contains(&id) {
                    state.recent_repositories.insert(0, id);
                    state.recent_repositories.truncate(10);
                }
                json!({ "selected": id })
            } else {
                json!({ "selected": null })
            }
        }
        "desktop.refresh-repository" => {
            let path = repo_path(state, &o)?;
            let data = refresh(app, state, &path)?;
            return Ok((data, true));
        }
        "desktop.get-status" => {
            let path = repo_path(state, &o)?;
            serde_json::to_value(git_status::get_status(&app.git, &path)?)?
        }

        // -- read-only git --------------------------------------------------
        "desktop.get-commits" => {
            let path = repo_path(state, &o)?;
            let revision = get_str(&o, "revision");
            let limit = get_u32(&o, "limit", 100);
            let skip = get_u32(&o, "skip", 0);
            serde_json::to_value(git_log::get_commits(&app.git, &path, revision.as_deref(), limit, skip)?)?
        }
        "desktop.get-commit" => {
            let path = repo_path(state, &o)?;
            let sha = required(&o, "sha")?;
            serde_json::to_value(git_log::get_commit(&app.git, &path, &sha)?)?
        }
        "desktop.get-changed-files" => {
            let path = repo_path(state, &o)?;
            let sha = required(&o, "sha")?;
            serde_json::to_value(git_log::get_changed_files(&app.git, &path, &sha)?)?
        }
        "desktop.get-authors" => {
            let path = repo_path(state, &o)?;
            serde_json::to_value(git_log::get_authors(&app.git, &path)?)?
        }
        "desktop.get-branches" => {
            let path = repo_path(state, &o)?;
            serde_json::to_value(git_refs::get_branches(&app.git, &path)?)?
        }
        "desktop.get-remotes" => {
            let path = repo_path(state, &o)?;
            serde_json::to_value(git_refs::get_remotes(&app.git, &path)?)?
        }
        "desktop.get-tags" => {
            let path = repo_path(state, &o)?;
            serde_json::to_value(git_refs::get_tags(&app.git, &path)?)?
        }
        "desktop.get-worktrees" => {
            let path = repo_path(state, &o)?;
            serde_json::to_value(git_refs::get_worktrees(&app.git, &path)?)?
        }
        "desktop.get-stashes" => {
            let path = repo_path(state, &o)?;
            serde_json::to_value(ops::get_stashes(&app.git, &path)?)?
        }
        "desktop.get-ahead-behind" => {
            let path = repo_path(state, &o)?;
            let range = required(&o, "range")?;
            serde_json::to_value(git_status::get_ahead_behind(&app.git, &path, &range))?
        }
        "desktop.get-merge-base" => {
            let path = repo_path(state, &o)?;
            serde_json::to_value(git_refs::get_merge_base(&app.git, &path, &required(&o, "a")?, &required(&o, "b")?)?)?
        }
        "desktop.get-config" => {
            let path = repo_path(state, &o)?;
            serde_json::to_value(app.git.config(&required(&o, "key")?, &path)?)?
        }
        "desktop.set-config" => {
            let path = repo_path(state, &o)?;
            app.git.set_config(&required(&o, "key")?, &required(&o, "value")?, &path)?;
            Value::Null
        }
        "desktop.get-diff" => {
            let path = repo_path(state, &o)?;
            let file = get_str(&o, "path").unwrap_or_default();
            serde_json::to_value(git_diff::get_working_directory_diff(&app.git, &path, &file)?)?
        }
        "desktop.get-commit-diff" => {
            let path = repo_path(state, &o)?;
            let sha = required(&o, "sha")?;
            let file = get_str(&o, "path").unwrap_or_default();
            serde_json::to_value(git_diff::get_commit_diff(&app.git, &path, &sha, &file)?)?
        }

        // -- mutating git ---------------------------------------------------
        "desktop.checkout-branch" => {
            let path = repo_path(state, &o)?;
            let out = ops::checkout_branch(&app.git, &path, &required(&o, "name")?)?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.create-branch" => {
            let path = repo_path(state, &o)?;
            let name = required(&o, "name")?;
            let start = get_str(&o, "startPoint").unwrap_or_else(|| "HEAD".into());
            let out = if get_bool(&o, "checkout", true) {
                ops::create_and_checkout_branch(&app.git, &path, &name, Some(&start))?
            } else {
                ops::create_branch(&app.git, &path, &name, &start)?
            };
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.rename-branch" => {
            let path = repo_path(state, &o)?;
            let out = ops::rename_branch(&app.git, &path, &required(&o, "oldName")?, &required(&o, "newName")?)?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.delete-branch" => {
            let path = repo_path(state, &o)?;
            let name = required(&o, "name")?;
            let out = if get_bool(&o, "remote", false) {
                ops::delete_remote_branch(&app.git, &path, &get_str(&o, "remote").unwrap_or_else(|| "origin".into()), &name)?
            } else {
                ops::delete_local_branch(&app.git, &path, &name)?
            };
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.checkout-commit" => {
            let path = repo_path(state, &o)?;
            let out = ops::checkout_commit(&app.git, &path, &required(&o, "sha")?)?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.stage" => {
            let path = repo_path(state, &o)?;
            let out = ops::stage_paths(&app.git, &path, &get_strings(&o, "paths"))?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.unstage-all" => {
            let path = repo_path(state, &o)?;
            let out = ops::unstage_all(&app.git, &path)?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.discard-changes" => {
            let path = repo_path(state, &o)?;
            let out = ops::discard_changes(&app.git, &path, &get_strings(&o, "paths"))?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.commit" => {
            let path = repo_path(state, &o)?;
            let out = ops::create_commit(&app.git, &path, &required(&o, "summary")?, &get_str(&o, "body").unwrap_or_default())?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.reset" => {
            let path = repo_path(state, &o)?;
            let mode = get_str(&o, "mode").unwrap_or_else(|| "--mixed".into());
            let out = ops::reset_to_commit(&app.git, &path, &required(&o, "sha")?, &mode)?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.fetch" => {
            let path = repo_path(state, &o)?;
            let remote = get_str(&o, "remote").unwrap_or_else(|| "origin".into());
            let out = ops::fetch(&app.git, &path, &remote, get_bool(&o, "prune", true))?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.pull" => {
            let path = repo_path(state, &o)?;
            let remote = get_str(&o, "remote").unwrap_or_else(|| "origin".into());
            let out = ops::pull(&app.git, &path, &remote, get_str(&o, "branch").as_deref(), get_bool(&o, "rebase", false))?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.push" => {
            let path = repo_path(state, &o)?;
            let remote = get_str(&o, "remote").unwrap_or_else(|| "origin".into());
            let branch = get_str(&o, "branch")
                .or_else(|| git_status::get_status(&app.git, &path).ok().and_then(|s| s.current_branch))
                .ok_or_else(|| AppError::msg("unable to determine the branch to push"))?;
            let out = ops::push(&app.git, &path, &remote, &branch, get_bool(&o, "setUpstream", false), get_bool(&o, "forceWithLease", false))?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.clone" => {
            let url = required(&o, "url")?;
            let target = required(&o, "path")?;
            let parent = Path::new(&target).parent().unwrap_or(Path::new("."));
            let name = Path::new(&target).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| target.clone());
            let out = ops::clone(&app.git, parent, &url, &name, get_str(&o, "branch").as_deref())?;
            if app.git.is_repository(Path::new(&target)) {
                let id = state.next_repository_id();
                state.repositories.push(Repository::local(target, id));
            }
            git_value(&out)
        }
        "desktop.merge" => {
            let path = repo_path(state, &o)?;
            let out = ops::merge(&app.git, &path, &required(&o, "branch")?, get_bool(&o, "squash", false))?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.rebase" => {
            let path = repo_path(state, &o)?;
            let out = ops::rebase(&app.git, &path, &required(&o, "branch")?, get_bool(&o, "interactive", false))?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.cherry-pick" => {
            let path = repo_path(state, &o)?;
            let out = ops::cherry_pick(&app.git, &path, &required(&o, "sha")?)?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.revert" => {
            let path = repo_path(state, &o)?;
            let out = ops::revert_commit(&app.git, &path, &required(&o, "sha")?)?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.create-stash" => {
            let path = repo_path(state, &o)?;
            let branch = get_str(&o, "branch")
                .or_else(|| git_refs::get_current_branch(&app.git, &path))
                .unwrap_or_else(|| "HEAD".into());
            let out = ops::create_desktop_stash(&app.git, &path, &branch)?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.pop-stash" => {
            let path = repo_path(state, &o)?;
            let out = ops::pop_stash(&app.git, &path, &required(&o, "name")?)?;
            refresh(app, state, &path)?;
            git_value(&out)
        }
        "desktop.drop-stash" => {
            let path = repo_path(state, &o)?;
            git_value(&ops::drop_stash(&app.git, &path, &required(&o, "name")?)?)
        }
        "desktop.create-tag" => {
            let path = repo_path(state, &o)?;
            git_value(&ops::create_tag(&app.git, &path, &required(&o, "name")?, &get_str(&o, "target").unwrap_or_else(|| "HEAD".into()))?)
        }
        "desktop.delete-tag" => {
            let path = repo_path(state, &o)?;
            git_value(&ops::delete_tag(&app.git, &path, &required(&o, "name")?)?)
        }
        "desktop.add-remote" => {
            let path = repo_path(state, &o)?;
            git_value(&ops::add_remote(&app.git, &path, &get_str(&o, "name").unwrap_or_else(|| "origin".into()), &required(&o, "url")?)?)
        }
        "desktop.set-remote-url" => {
            let path = repo_path(state, &o)?;
            git_value(&ops::set_remote_url(&app.git, &path, &get_str(&o, "name").unwrap_or_else(|| "origin".into()), &required(&o, "url")?)?)
        }

        // -- UI state -------------------------------------------------------
        // A web-rendered menu/panel still needs a native confirmation dialog.
        // Routing it through `WindowAction` keeps rfd on the tao UI thread.
        "desktop.show-message-box" => {
            let options: crate::ipc::protocol::MessageOptions = serde_json::from_value(o.clone())
                .map_err(|e| AppError::BadArgs(format!("invalid message box options: {e}")))?;
            app.window(crate::ipc::protocol::WindowAction::ShowMessageBox(options))?
        }
        "desktop.show-popup" => {
            let kind: PopupType = serde_json::from_value(o.get("type").cloned().unwrap_or(json!("unknown")))
                .unwrap_or(PopupType::Unknown);
            state.current_popup = Some(crate::models::misc::Popup {
                kind,
                repository: state.selected_repository().cloned(),
                data: o.get("data").cloned(),
            });
            return Ok((Value::Null, true));
        }
        "desktop.close-popup" => {
            state.current_popup = None;
            return Ok((Value::Null, true));
        }
        "desktop.set-preference" => {
            let key = required(&o, "key")?;
            let value = o.get("value").cloned().unwrap_or(Value::Null);
            app.storage.set_item(&format!("pref:{key}"), value.clone())?;
            apply_preference(state, &key, &value);
            return Ok((value, true));
        }
        "desktop.get-preference" => {
            let key = required(&o, "key")?;
            app.storage.get_item(&format!("pref:{key}")).unwrap_or(Value::Null)
        }

        // -- accounts / API -------------------------------------------------
        "desktop.sign-in" => {
            let endpoint = get_str(&o, "endpoint").unwrap_or_else(crate::api::ApiClient::default_endpoint);
            let token = required(&o, "token")?;
            let user = app.api.fetch_user(&endpoint, &token)?;
            let login = get_str(&o, "login").unwrap_or_else(|| user.login.clone());
            app.secure.set_item(&format!("{login}@{endpoint}"), &login, &token)?;
            let account = Account {
                login: login.clone(),
                endpoint: endpoint.clone(),
                token: String::new(),
                emails: Vec::new(),
                avatar_url: user.avatar_url.clone(),
                id: user.id,
                name: None,
                plan: None,
                copilot_endpoint: None,
            };
            state.accounts.retain(|a| !(a.login == login && a.endpoint == endpoint));
            state.accounts.push(account.clone());
            return Ok((serde_json::to_value(account)?, true));
        }
        "desktop.sign-out" => {
            let login = required(&o, "login")?;
            for account in state.accounts.iter().filter(|a| a.login == login) {
                app.secure.delete_item(&account.key(), &login)?;
            }
            state.accounts.retain(|a| a.login != login);
            return Ok((Value::Null, true));
        }
        "desktop.fetch-user" => {
            let (endpoint, token, _) = account_context(app, state, &o)?;
            serde_json::to_value(app.api.fetch_user(&endpoint, &token)?)?
        }
        "desktop.fetch-repositories" => {
            let (endpoint, token, _) = account_context(app, state, &o)?;
            serde_json::to_value(app.api.fetch_user_repositories(&endpoint, &token)?)?
        }
        "desktop.fetch-pull-requests" => {
            let (endpoint, token, _) = account_context(app, state, &o)?;
            serde_json::to_value(app.api.fetch_pull_requests(&endpoint, &token, &required(&o, "owner")?, &required(&o, "name")?)?)?
        }
        "desktop.create-pull-request" => {
            let (endpoint, token, _) = account_context(app, state, &o)?;
            serde_json::to_value(app.api.create_pull_request(
                &endpoint, &token,
                &required(&o, "owner")?, &required(&o, "name")?,
                &required(&o, "title")?, &required(&o, "head")?,
                &required(&o, "base")?, &get_str(&o, "body").unwrap_or_default(),
            )?)?
        }
        "desktop.fetch-issues" => {
            let (endpoint, token, _) = account_context(app, state, &o)?;
            serde_json::to_value(app.api.fetch_issues(&endpoint, &token, &required(&o, "owner")?, &required(&o, "name")?)?)?
        }

        // -- misc -----------------------------------------------------------
        "desktop.open-in-shell" => {
            let path = repo_path(state, &o)?;
            let _ = app.window(crate::ipc::protocol::WindowAction::OpenExternal(path.to_string_lossy().into_owned()));
            Value::Null
        }
        other => return Err(AppError::UnknownChannel(other.to_string())),
    };

    // Mutations that refresh() already applied need a state broadcast.
    let changed = channel.starts_with("desktop.")
        && !matches!(
            channel,
            "desktop.get-state" | "desktop.get-app-info" | "desktop.get-status"
                | "desktop.get-commits" | "desktop.get-commit" | "desktop.get-changed-files"
                | "desktop.get-authors" | "desktop.get-branches" | "desktop.get-remotes"
                | "desktop.get-tags" | "desktop.get-worktrees" | "desktop.get-stashes"
                | "desktop.get-ahead-behind" | "desktop.get-merge-base" | "desktop.get-config"
                | "desktop.get-diff" | "desktop.get-commit-diff" | "desktop.get-preference"
                | "desktop.fetch-user" | "desktop.fetch-repositories"
                | "desktop.fetch-pull-requests" | "desktop.create-pull-request"
                | "desktop.fetch-issues" | "desktop.open-in-shell"
                | "desktop.show-message-box"
        );
    Ok((value, changed))
}

fn account_context(app: &DesktopApp, state: &AppState, o: &Value) -> Result<(String, String, String)> {
    let login = get_str(o, "login");
    let account = state
        .accounts
        .iter()
        .find(|a| login.as_ref().map(|l| &a.login == l).unwrap_or(true))
        .ok_or_else(|| AppError::NotAuthenticated)?;
    let token = app
        .api
        .token_for(account)
        .ok_or_else(|| AppError::NotAuthenticated)?;
    Ok((account.endpoint.clone(), token, account.login.clone()))
}

fn apply_preference(state: &mut AppState, key: &str, value: &Value) {
    let p = &mut state.preferences;
    match key {
        "confirmRepoRemoval" => p.confirm_repo_removal = value.as_bool().unwrap_or(false),
        "confirmDiscardChanges" => p.confirm_discard_changes = value.as_bool().unwrap_or(true),
        "confirmForcePush" => p.confirm_force_push = value.as_bool().unwrap_or(true),
        "sidebarWidth" => p.sidebar_width = value.as_u64().unwrap_or(250) as u32,
        "commitListWidth" => p.commit_list_width = value.as_u64().unwrap_or(300) as u32,
        "tabSize" => p.tab_size = value.as_u64().unwrap_or(4) as u32,
        "externalEditor" => p.external_editor = value.as_str().map(str::to_string),
        "shell" => p.shell = value.as_str().map(str::to_string),
        "statsOptOut" => p.stats_opt_out = value.as_bool().unwrap_or(false),
        "zoomFactor" => p.zoom_factor = value.as_f64().unwrap_or(1.0),
        "showStashedChanges" => p.show_stashed_changes = value.as_bool().unwrap_or(false),
        "repositoryIndicatorsEnabled" => p.repository_indicators_enabled = value.as_bool().unwrap_or(true),
        _ => {}
    }
}

/// Extract `owner/name` from an https or SSH remote URL.
fn parse_remote_repo(url: &str) -> (String, String) {
    let clean = url.trim_end_matches(".git");
    let tail = if clean.contains("://") {
        clean.split("://").nth(1).unwrap_or(clean).split('/').skip(1).collect::<Vec<_>>().join("/")
    } else if let Some((_, path)) = clean.split_once(':') {
        path.to_string()
    } else {
        clean.to_string()
    };
    match tail.rsplit_once('/') {
        Some((owner, name)) => (owner.to_string(), name.to_string()),
        None => (tail, String::new()),
    }
}
