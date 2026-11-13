//! Loopback HTTP/WebSocket transport for the browser and native shells.
#[cfg(feature = "embedded-ui")]
mod frontend;
use axum::{
    Router,
    extract::{
        Request, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use desktop_core::{
    Operation, Registry, Repository,
    application::{ApplicationService, BACKEND_VERSION},
    cancellation::{self, Cancellation},
};
use serde::Deserialize;
use serde_json::json;
use std::{collections::VecDeque, future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::{
    net::TcpListener,
    sync::{Mutex, Semaphore, mpsc, watch},
};

struct Bridge {
    repo: Mutex<Option<Repository>>,
    registry: Registry,
    token: String,
    authority: String,
    changes: watch::Sender<serde_json::Value>,
    preferences: watch::Sender<desktop_core::Preferences>,
    shutdown: watch::Receiver<bool>,
    sessions: Arc<Semaphore>,
}

/// Owns a loopback server. Dropping the handle requests shutdown, including open sockets.
/// The session URL is a credential; do not log it in native hosts or diagnostics.
pub struct ServerHandle {
    session_url: String,
    shutdown: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
}

impl ServerHandle {
    pub fn session_url(&self) -> &str {
        &self.session_url
    }

    pub fn is_finished(&self) -> bool {
        self.task.as_ref().is_none_or(|task| task.is_finished())
    }

    /// Wait for a server failure or shutdown. Safe to cancel while waiting.
    pub async fn stopped(&mut self) -> anyhow::Result<()> {
        if let Some(task) = self.task.as_mut() {
            let result = task.await;
            self.task.take();
            result??;
        }
        Ok(())
    }

    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        self.shutdown.send_replace(true);
        self.stopped().await
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
    }
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Start the shared transport without installing process-wide signal handlers.
pub async fn start(
    repo: Option<Repository>,
    registry: Registry,
    port: u16,
) -> anyhow::Result<ServerHandle> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let authority = listener.local_addr()?.to_string();
    let repo = if let Some(repo) = repo {
        let initial_registry = registry.clone();
        let root = repo.path().to_owned();
        let fixed = repo
            .fixed_saved_record()
            .map(|(namespace, id)| (namespace, id.to_owned()));
        if let Some((namespace, _)) = &fixed {
            anyhow::ensure!(
                namespace == &registry.namespace_id(),
                "Repository registry changed before startup"
            );
        }
        let record = cancellation::blocking(move || {
            if let Some((_, id)) = fixed {
                initial_registry.select_record(&id, &root)
            } else {
                initial_registry
                    .add(root.clone(), true)?
                    .records
                    .into_iter()
                    .find(|record| record.path == root)
                    .ok_or_else(|| anyhow::anyhow!("Initial repository record was not saved"))
            }
        })
        .await?;
        Some(repo.with_saved_record(registry.clone(), record.id))
    } else {
        None
    };
    let initial = if let Some(repo) = &repo {
        repo.execute(Operation::Snapshot).await?
    } else {
        serde_json::Value::Null
    };
    let (changes, _) = watch::channel(initial);
    let store = registry.preferences();
    let loaded = cancellation::blocking(move || store.read()).await?;
    let (preferences, _) = watch::channel(loaded);
    let (shutdown, stopping) = watch::channel(false);
    let state = Arc::new(Bridge {
        repo: Mutex::new(repo),
        registry,
        token: uuid::Uuid::new_v4().to_string(),
        authority,
        changes,
        preferences,
        shutdown: stopping.clone(),
        sessions: Arc::new(Semaphore::new(16)),
    });
    let publisher_state = state.clone();
    let publisher = tokio::spawn(async move {
        let mut preferences = publisher_state.preferences.subscribe();
        loop {
            let delay = Duration::from_millis(preferences.borrow().poll_interval_ms);
            tokio::select! {_=tokio::time::sleep(delay)=>{},changed=preferences.changed()=>{if changed.is_err(){break;}continue;}}
            let store = publisher_state.registry.preferences();
            if let Ok(value) = cancellation::blocking(move || store.read()).await {
                publisher_state.preferences.send_if_modified(|previous| {
                    if *previous == value {
                        false
                    } else {
                        *previous = value;
                        true
                    }
                });
            }

            let current = publisher_state.repo.lock().await;
            if let Some(repo) = current.as_ref()
                && let Ok(snapshot) = repo.execute(Operation::Snapshot).await
            {
                publisher_state.changes.send_if_modified(|previous| {
                    if *previous == snapshot {
                        false
                    } else {
                        *previous = snapshot;
                        true
                    }
                });
            }
        }
    });
    let app = Router::new().route("/ws", get(upgrade));
    #[cfg(not(feature = "embedded-ui"))]
    let app = app.route(
            "/",
            get(|| async {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "The browser frontend is not available in this build. The authenticated WebSocket API remains available at /ws.\n",
                )
            }),
        );
    #[cfg(feature = "embedded-ui")]
    let app = app
        .route("/", get(frontend::index))
        .route("/__bundle", get(frontend::entry))
        .route("/{*path}", get(frontend::asset));
    let app = app
        .layer(middleware::from_fn_with_state(state.clone(), check_host))
        .with_state(state.clone());
    let session_url = format!("http://{}/#token={}", state.authority, state.token);
    let mut stopping = stopping;
    let session_slots = state.sessions.clone();
    let task = tokio::spawn(async move {
        let mut publisher = AbortOnDrop(publisher);
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = stopping.wait_for(|stopping| *stopping).await;
            })
            .await;
        publisher.0.abort();
        let _ = (&mut publisher.0).await;
        // Upgraded WebSockets are outside axum's connection join; drain them too.
        let _ = tokio::time::timeout(Duration::from_secs(5), session_slots.acquire_many_owned(16))
            .await;
        result
    });
    Ok(ServerHandle {
        session_url,
        shutdown,
        task: Some(task),
    })
}

/// Run the headless CLI server until Ctrl-C or a listener failure.
pub async fn serve(repo: Option<Repository>, registry: Registry, port: u16) -> anyhow::Result<()> {
    let mut signal = cancellation::ShutdownSignal::new()?;
    let mut server = start(repo, registry, port).await?;
    eprintln!("Open {}", server.session_url());
    eprintln!("Repository browser bridge; Ctrl-C to stop. Keep this session URL private.");
    let result = tokio::select! {
        result = signal.received() => result.map(|_| ()).map_err(anyhow::Error::from),
        result = server.stopped() => result,
    };
    let shutdown = server.shutdown().await;
    result?;
    shutdown
}

async fn check_host(State(state): State<Arc<Bridge>>, req: Request, next: Next) -> Response {
    if req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        != Some(&state.authority)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = next.run(req).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        format!("default-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self' ws://{}; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'", state.authority)
            .parse()
            .unwrap(),
    );
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    response
}

async fn upgrade(
    State(state): State<Arc<Bridge>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let expected = format!("http://{}", state.authority);
    if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) != Some(&expected) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(permit) = state.sessions.clone().try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    ws.max_message_size(65536)
        .max_frame_size(65536)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            session(socket, state).await;
        })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Auth {
    token: String,
}

#[derive(Deserialize)]
struct Rpc {
    id: u64,
    repository_id: Option<String>,
    #[serde(flatten)]
    operation: ApplicationOperation,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ApplicationOperation {
    Control(ControlOperation),
    Management(ManagementOperation),
    Repository(Operation),
}

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
enum ControlOperation {
    Cancel { request_id: u64 },
}

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
enum ManagementOperation {
    AvailableEditors,
    GitVersion,
    GitGlobalConfigPath,
    OpenEditor {
        path: Option<desktop_core::GitPath>,
    },
    GetPreferences,
    UpdatePreferences {
        preferences: desktop_core::PreferencesPatch,
    },
    ListRepositories,
    ListCloneRecoveries,
    RecoverClone { recovery_id: String },
    RepositoryBinding {
        path: desktop_core::GitPath,
    },
    ReleaseComparisonSnapshot {
        snapshot_id: String,
    },
    LfsRepository {
        path: desktop_core::GitPath,
        action: LfsRepositoryAction,
    },
    GitReadRepositorySetting {
        path: desktop_core::GitPath,
        key: desktop_core::GitReadSettingKey,
        #[serde(default)]
        only_local: bool,
        #[serde(default)]
        canonical_bool: bool,
    },
    GitRepositoryEffectiveIdentity {
        path: desktop_core::GitPath,
        role: desktop_core::GitIdentityRole,
    },
    RepositorySafeFetchRemotes {
        path: desktop_core::GitPath,
    },
    RepositoryWorktrees {
        path: desktop_core::GitPath,
    },
    RepositoryWorktreesFromGitDir {
        path: desktop_core::GitPath,
    },
    RepositoryProtectedResources {
        path: desktop_core::GitPath,
    },
    RepositorySafeRemoteUrl {
        path: desktop_core::GitPath,
        name: String,
    },
    RepositorySymbolicRef {
        path: desktop_core::GitPath,
        reference: String,
    },
    ManageRepositoryRemote {
        path: desktop_core::GitPath,
        action: desktop_core::ManagedRemoteAction,
    },
    SetRepositoryDescription {
        path: desktop_core::GitPath,
        description: String,
    },
    WriteRepositoryCreationFile {
        path: desktop_core::GitPath,
        kind: desktop_core::CreationFile,
        contents: String,
    },
    RepositoryGitIgnore {
        path: desktop_core::GitPath,
        action: desktop_core::GitIgnoreAction,
    },
    ReadRepositoryCreationStatus {
        path: desktop_core::GitPath,
        action: CreationStatusAction,
    },
    CreateInitialCommit {
        path: desktop_core::GitPath,
        paths: Vec<desktop_core::GitPath>,
    },
    RepositoryType {
        path: desktop_core::GitPath,
    },
    DirectoryExists {
        path: desktop_core::GitPath,
    },
    PathExists {
        path: desktop_core::GitPath,
    },
    CanonicalMutationPath {
        path: desktop_core::GitPath,
    },
    TrustRepository {
        path: desktop_core::GitPath,
    },
    OpenRepository {
        path: desktop_core::GitPath,
    },
    SelectRepository {
        path: desktop_core::GitPath,
    },
    ClearRepository,
    OpenWorktree {
        path: desktop_core::GitPath,
    },
    ForgetRepository {
        path: desktop_core::GitPath,
    },
    InitRepository {
        path: desktop_core::GitPath,
    },
    InitializeDesktopRepository {
        path: desktop_core::GitPath,
        default_branch: String,
    },
    CloneRepository {
        url: String,
        path: desktop_core::GitPath,
        #[serde(default)]
        branch: Option<String>,
        #[serde(default)]
        default_branch: Option<String>,
        #[serde(default)]
        recursive: bool,
        #[serde(default)]
        report_progress: bool,
        #[serde(default)]
        account: Option<desktop_core::CloneAccount>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum LfsRepositoryAction {
    Status {},
    FileTracking { paths: Vec<desktop_core::GitPath> },
    InstallHooks { force: bool },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum CreationStatusAction {
    Detailed {
        include_untracked: bool,
    },
    Context {},
    ConflictDetails {
        kind: desktop_core::StatusConflictKind,
    },
}

async fn execute(
    state: &Bridge,
    operation: ApplicationOperation,
    expected_repository: Option<String>,
) -> anyhow::Result<serde_json::Value> {
    use ManagementOperation::*;
    let registry = state.registry.clone();
    match operation {
        ApplicationOperation::Control(_) => {
            anyhow::bail!("control requests must be handled by the session")
        }
        ApplicationOperation::Repository(Operation::Github { request })
            if request.is_auth_status() =>
        {
            desktop_core::execute_github(&std::env::current_dir()?, request).await
        }
        ApplicationOperation::Repository(Operation::LfsToolStatus) => {
            desktop_core::lfs_tool_status(&std::env::current_dir()?).await
        }
        ApplicationOperation::Repository(Operation::LfsInstall {
            scope: desktop_core::LfsInstallScope::Global,
            force,
            skip_repo: true,
            skip_smudge,
        }) => {
            desktop_core::lfs_install(
                &std::env::current_dir()?,
                desktop_core::LfsInstallScope::Global,
                force,
                true,
                skip_smudge,
            )
            .await
        }
        ApplicationOperation::Repository(Operation::GitSettings {
            scope: desktop_core::ConfigScope::Global,
        }) => {
            desktop_core::git_settings(&std::env::current_dir()?, desktop_core::ConfigScope::Global)
                .await
        }
        ApplicationOperation::Repository(Operation::SetGitSetting {
            scope: desktop_core::ConfigScope::Global,
            key,
            value,
        }) => {
            desktop_core::set_git_setting(
                &std::env::current_dir()?,
                desktop_core::ConfigScope::Global,
                &key,
                value,
            )
            .await
        }
        ApplicationOperation::Repository(operation) => {
            let current = cancellation::wait(state.repo.lock()).await?;
            match current.as_ref() {
                Some(repo) => {
                    if !matches!(operation, Operation::Snapshot)
                        && let Some(expected) = expected_repository
                    {
                        anyhow::ensure!(
                            repo.id() == expected,
                            "Repository selection changed; refresh before retrying"
                        );
                    }
                    repo.execute(operation).await
                }
                None if matches!(operation, Operation::Snapshot) => Ok(serde_json::Value::Null),
                None if matches!(&operation,Operation::LfsStatus {paths} if paths.is_empty()) => {
                    desktop_core::lfs_tool_status(&std::env::current_dir()?).await
                }
                None => anyhow::bail!("open, initialize, or clone a repository first"),
            }
        }
        ApplicationOperation::Management(ReleaseComparisonSnapshot { snapshot_id }) => {
            desktop_core::comparison_snapshots::release_owned(&snapshot_id)
        }
        ApplicationOperation::Management(AvailableEditors) => {
            let preferences = cancellation::blocking(move || registry.preferences().read()).await?;
            desktop_core::available_editors(preferences.external_editor).await
        }
        ApplicationOperation::Management(GitVersion) => {
            Ok(json!(desktop_core::git_version().await?))
        }
        ApplicationOperation::Management(GitGlobalConfigPath) => {
            Ok(json!(desktop_core::git_global_config_path().await?))
        }
        ApplicationOperation::Management(InitializeDesktopRepository {
            path,
            default_branch,
        }) => {
            anyhow::ensure!(
                expected_repository.is_none(),
                "Repository initialization does not use a selected-repository binding"
            );
            let path = desktop_core::directory_path(path)?;
            let _current = cancellation::wait(state.repo.lock()).await?;
            desktop_core::initialize_desktop_repository(&path, &default_branch).await?;
            Ok(serde_json::Value::Null)
        }
        ApplicationOperation::Management(RepositoryType { path }) => {
            desktop_core::repository_type(&desktop_core::directory_path(path)?).await
        }
        ApplicationOperation::Management(DirectoryExists { path }) => Ok(serde_json::json!(
            desktop_core::directory_exists(&desktop_core::directory_path(path)?).await?
        )),
        ApplicationOperation::Management(PathExists { path }) => Ok(serde_json::json!(
            desktop_core::path_exists(&desktop_core::directory_path(path)?).await?
        )),
        ApplicationOperation::Management(CanonicalMutationPath { path }) => Ok(serde_json::json!(
            desktop_core::canonical_mutation_path(&desktop_core::directory_path(path)?).await?
        )),
        ApplicationOperation::Management(TrustRepository { path }) => {
            desktop_core::trust_repository(&desktop_core::directory_path(path)?).await
        }
        ApplicationOperation::Management(OpenEditor { path }) => {
            let current = cancellation::wait(state.repo.lock()).await?;
            let repository = current
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("open a repository before launching an editor"))?;
            if let Some(expected) = expected_repository {
                anyhow::ensure!(
                    repository.id() == expected,
                    "Repository selection changed; refresh before opening an editor"
                );
            }
            let preferences = cancellation::blocking(move || registry.preferences().read()).await?;
            repository
                .open_editor(path, preferences.external_editor)
                .await
        }
        ApplicationOperation::Management(GetPreferences) => {
            let store = registry.preferences();
            Ok(serde_json::to_value(
                cancellation::blocking(move || store.read()).await?,
            )?)
        }
        ApplicationOperation::Management(UpdatePreferences { preferences }) => {
            let store = registry.preferences();
            let updated = cancellation::blocking(move || store.update(preferences)).await?;
            state.preferences.send_replace(updated.clone());
            Ok(serde_json::to_value(updated)?)
        }
        ApplicationOperation::Management(ListRepositories) => {
            cancellation::blocking(move || registry.read())
                .await?
                .public_view()
        }
        ApplicationOperation::Management(ListCloneRecoveries) => {
            anyhow::ensure!(expected_repository.is_none(), "Clone recovery cannot use a selected-repository binding");
            cancellation::blocking(move || registry.clone_recovery_records()).await
        }
        ApplicationOperation::Management(RecoverClone { recovery_id }) => {
            anyhow::ensure!(expected_repository.is_none(), "Clone recovery cannot use a selected-repository binding");
            anyhow::ensure!(uuid::Uuid::parse_str(&recovery_id).is_ok_and(|value| value.to_string() == recovery_id), "Invalid clone recovery ID");
            anyhow::bail!("Selected-account clone recovery is temporarily unavailable; inspect the staging directory before retrying")
        }
        ApplicationOperation::Management(RepositorySafeFetchRemotes { path }) => {
            anyhow::ensure!(
                expected_repository.is_none(),
                "An explicit repository cannot use a selected-repository binding"
            );
            let requested = desktop_core::directory_path(path)?;
            anyhow::ensure!(requested.is_absolute(), "repository path must be absolute");
            let _current = cancellation::wait(state.repo.lock()).await?;
            desktop_core::safe_fetch_remotes(&requested).await
        }
        ApplicationOperation::Management(RepositoryWorktrees { path }) => {
            anyhow::ensure!(
                expected_repository.is_none(),
                "An explicit repository cannot use a selected-repository binding"
            );
            let requested = desktop_core::directory_path(path)?;
            anyhow::ensure!(requested.is_absolute(), "repository path must be absolute");
            let _current = cancellation::wait(state.repo.lock()).await?;
            desktop_core::worktrees_at(&requested).await
        }
        ApplicationOperation::Management(RepositoryWorktreesFromGitDir { path }) => {
            anyhow::ensure!(
                expected_repository.is_none(),
                "An explicit repository cannot use a selected-repository binding"
            );
            let requested = desktop_core::directory_path(path)?;
            anyhow::ensure!(requested.is_absolute(), "Git directory must be absolute");
            let _current = cancellation::wait(state.repo.lock()).await?;
            desktop_core::worktrees_from_git_dir(&requested).await
        }
        ApplicationOperation::Management(RepositoryProtectedResources { path }) => {
            anyhow::ensure!(
                expected_repository.is_none(),
                "An explicit repository cannot use a selected-repository binding"
            );
            let requested = desktop_core::directory_path(path)?;
            anyhow::ensure!(requested.is_absolute(), "repository path must be absolute");
            let _current = cancellation::wait(state.repo.lock()).await?;
            Ok(serde_json::to_value(
                desktop_core::protected_resources(&requested).await?,
            )?)
        }
        ApplicationOperation::Management(RepositorySafeRemoteUrl { path, name }) => {
            anyhow::ensure!(
                expected_repository.is_none(),
                "An explicit repository cannot use a selected-repository binding"
            );
            let requested = desktop_core::directory_path(path)?;
            anyhow::ensure!(requested.is_absolute(), "repository path must be absolute");
            let _current = cancellation::wait(state.repo.lock()).await?;
            desktop_core::safe_remote_url(&requested, &name).await
        }
        ApplicationOperation::Management(RepositorySymbolicRef { path, reference }) => {
            anyhow::ensure!(
                expected_repository.is_none(),
                "An explicit repository cannot use a selected-repository binding"
            );
            let requested = desktop_core::directory_path(path)?;
            anyhow::ensure!(requested.is_absolute(), "repository path must be absolute");
            let _current = cancellation::wait(state.repo.lock()).await?;
            desktop_core::symbolic_ref(&requested, &reference).await
        }
        ApplicationOperation::Management(
            operation @ (LfsRepository { .. }
            | GitReadRepositorySetting { .. }
            | GitRepositoryEffectiveIdentity { .. }
            | ManageRepositoryRemote { .. }
            | SetRepositoryDescription { .. }
            | WriteRepositoryCreationFile { .. }
            | RepositoryGitIgnore { .. }
            | ReadRepositoryCreationStatus { .. }
            | CreateInitialCommit { .. }),
        ) => {
            let registry_bound_mutation = matches!(
                operation,
                CreateInitialCommit { .. } | ManageRepositoryRemote { .. }
            );
            let (path, operation) = match operation {
                LfsRepository { path, action } => (
                    path,
                    match action {
                        LfsRepositoryAction::Status {} => Operation::LfsStatus { paths: vec![] },
                        LfsRepositoryAction::FileTracking { paths } => {
                            Operation::LfsFileTracking { paths }
                        }
                        LfsRepositoryAction::InstallHooks { force } => Operation::LfsInstall {
                            scope: desktop_core::LfsInstallScope::Global,
                            force,
                            skip_repo: false,
                            skip_smudge: false,
                        },
                    },
                ),
                GitReadRepositorySetting {
                    path,
                    key,
                    only_local,
                    canonical_bool,
                } => (
                    path,
                    Operation::GitReadSetting {
                        key,
                        only_local,
                        canonical_bool,
                    },
                ),
                GitRepositoryEffectiveIdentity { path, role } => {
                    (path, Operation::GitEffectiveIdentity { role })
                }
                ManageRepositoryRemote { path, action } => {
                    (path, Operation::ManageRemote { action })
                }
                SetRepositoryDescription { path, description } => {
                    (path, Operation::SetGitDescription { description })
                }
                WriteRepositoryCreationFile {
                    path,
                    kind,
                    contents,
                } => (path, Operation::WriteCreationFile { kind, contents }),
                RepositoryGitIgnore { path, action } => (path, Operation::GitIgnore { action }),
                ReadRepositoryCreationStatus { path, action } => (
                    path,
                    match action {
                        CreationStatusAction::Detailed { include_untracked } => {
                            Operation::StatusDetailed { include_untracked }
                        }
                        CreationStatusAction::Context {} => Operation::StatusContext,
                        CreationStatusAction::ConflictDetails { kind } => {
                            Operation::StatusConflictDetails { kind }
                        }
                    },
                ),
                CreateInitialCommit { path, paths } => (path, Operation::InitialCommit { paths }),
                _ => unreachable!(),
            };
            anyhow::ensure!(
                expected_repository.is_none(),
                "An explicit repository cannot use a selected-repository binding"
            );
            let requested = desktop_core::directory_path(path)?;
            anyhow::ensure!(requested.is_absolute(), "repository path must be absolute");
            let canonical = cancellation::blocking(move || Ok(requested.canonicalize()?)).await?;
            // Hold the selection lock through the operation, sharing the selected
            // repository's gate when possible and preventing selection changes.
            let current = cancellation::wait(state.repo.lock()).await?;
            let repo = if let Some(repo) = current.as_ref().filter(|repo| repo.path() == canonical)
            {
                repo.clone()
            } else {
                let mut repo = Repository::open(&canonical).await?;
                anyhow::ensure!(
                    repo.path() == canonical,
                    "select a repository root, not a nested path"
                );
                if registry_bound_mutation {
                    repo = repo.with_registry(registry.clone());
                }
                repo
            };
            repo.execute(operation).await
        }
        ApplicationOperation::Management(RepositoryBinding { path }) => {
            let Ok(requested) = desktop_core::directory_path(path) else {
                return Ok(serde_json::Value::Null);
            };
            if !requested.is_absolute() {
                return Ok(serde_json::Value::Null);
            }
            let canonical =
                cancellation::blocking(move || Ok(requested.canonicalize().ok())).await?;
            let Some(canonical) = canonical else {
                return Ok(serde_json::Value::Null);
            };
            let current = cancellation::wait(state.repo.lock()).await?;
            Ok(match current.as_ref() {
                Some(repo) if canonical == repo.path() => serde_json::json!({"root_id":repo.id()}),
                _ => serde_json::Value::Null,
            })
        }
        ApplicationOperation::Management(ClearRepository) => {
            anyhow::ensure!(
                expected_repository.is_none(),
                "Cannot clear a repository-bound request"
            );
            let mut current = cancellation::wait(state.repo.lock()).await?;
            let registry = registry.clone();
            cancellation::blocking(move || registry.clear_selection()).await?;
            *current = None;
            state.changes.send_replace(serde_json::Value::Null);
            Ok(serde_json::Value::Null)
        }
        ApplicationOperation::Management(ForgetRepository { path }) => {
            cancellation::blocking(move || {
                let path = desktop_core::directory_path(path)?;
                registry.forget(&path.canonicalize().unwrap_or(path))
            })
            .await?
            .public_view()
        }
        ApplicationOperation::Management(OpenWorktree { path }) => {
            let mut current = cancellation::wait(state.repo.lock()).await?;
            let source = current
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Open a repository before switching worktrees"))?;
            if let Some(expected) = expected_repository {
                anyhow::ensure!(
                    source.id() == expected,
                    "Repository selection changed; review the worktree again"
                );
            }
            let repo = source.open_worktree(path).await?;
            let snapshot = repo.execute(Operation::Snapshot).await?;
            *current = Some(repo);
            state.changes.send_replace(snapshot.clone());
            Ok(snapshot)
        }
        ApplicationOperation::Management(CloneRepository { account: Some(_), .. }) => {
            anyhow::ensure!(expected_repository.is_none(), "Clone cannot use a selected-repository binding");
            anyhow::bail!("Selected-account cloning is temporarily unavailable")
        }
        ApplicationOperation::Management(operation) => {
            let selecting = matches!(operation, SelectRepository { .. });
            let repo = match operation {
                OpenRepository { path } | SelectRepository { path } => {
                    Repository::open(&desktop_core::directory_path(path)?).await?
                }
                InitRepository { path } => {
                    desktop_core::initialize(&desktop_core::directory_path(path)?).await?
                }
                CloneRepository {
                    url,
                    path,
                    branch,
                    default_branch,
                    recursive,
                    report_progress,
                    account,
                } => {
                    let destination = desktop_core::directory_path(path)?;
                    let options = desktop_core::CloneOptions {
                            branch,
                            default_branch,
                            recursive,
                            report_progress,
                        };
                    if account.is_some() {
                        anyhow::bail!("Selected-account clone is not available yet")
                    } else {
                        desktop_core::clone_repository_with_options(&url, &destination, options).await?
                    }
                }
                _ => unreachable!("handled above"),
            };
            let root = repo.path().to_owned();
            let mut current = cancellation::wait(state.repo.lock()).await?;
            let update_registry = registry.clone();
            let saved = cancellation::blocking(move || {
                if selecting {
                    update_registry.select(root)
                } else {
                    update_registry.add(root, true)
                }
            })
            .await?;
            let id = saved
                .records
                .iter()
                .find(|record| record.path == repo.path())
                .ok_or_else(|| anyhow::anyhow!("Selected repository record was not saved"))?
                .id
                .clone();
            let repo = repo.with_saved_record(registry, id);
            let snapshot = repo.execute(Operation::Snapshot).await?;
            *current = Some(repo);
            state.changes.send_replace(snapshot.clone());
            Ok(snapshot)
        }
    }
}

async fn session(mut socket: WebSocket, state: Arc<Bridge>) {
    let mut stopping = state.shutdown.clone();
    let first = tokio::select! {
        _ = async { let _ = stopping.wait_for(|stopping| *stopping).await; } => return,
        first = tokio::time::timeout(Duration::from_secs(5), socket.recv()) => first,
    };
    let authenticated = match first {
        Ok(Some(Ok(Message::Text(text)))) => {
            serde_json::from_str::<Auth>(&text).is_ok_and(|auth| auth.token == state.token)
        }
        _ => false,
    };
    if !authenticated {
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code: 1008,
                reason: "invalid session".into(),
            })))
            .await;
        return;
    }
    // Capabilities come from the common application service so the bridge
    // and GUI never drift (still version 3, still `{cancel,max_pending}`).
    let svc = ApplicationService::new();
    let caps = svc.capabilities();
    if socket
        .send(Message::Text(
            json!({"ready":true,"version":BACKEND_VERSION,"capabilities":{"cancel":caps.cancel,"max_pending":caps.max_pending}})
                .to_string()
                .into(),
        ))
        .await
        .is_err()
    {
        return;
    }
    let mut changes = state.changes.subscribe();
    let mut preferences = state.preferences.subscribe();
    let mut active: Option<Active> = None;
    // RAII also covers early returns while writing replies or closing sockets.
    struct ComparisonSession(Arc<desktop_core::comparison_snapshots::Owner>);
    impl Drop for ComparisonSession {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let comparison_owner = ComparisonSession(desktop_core::comparison_snapshots::Owner::new());
    let (hook_events, mut hook_events_rx) =
        mpsc::channel::<(u64, desktop_core::hook_progress::CommitEvent)>(64);
    let mut queue = VecDeque::<Rpc>::new();
    loop {
        if active.is_none()
            && let Some(rpc) = queue.pop_front()
        {
            active = Some(Active::new(
                state.clone(),
                rpc,
                hook_events.clone(),
                comparison_owner.0.clone(),
            ));
        }
        let message = tokio::select! {
            biased;
            _ = async { let _ = stopping.wait_for(|stopping| *stopping).await; } => break,
            result = async {
                match active.as_mut() {
                    Some(job) => job.future.as_mut().await,
                    None => std::future::pending().await,
                }
            } => {
                active.take();
                while let Ok((request_id, event)) = hook_events_rx.try_recv() {
                    if !send(&mut socket, commit_event_value(request_id, event), &state).await { return; }
                }
                if !send(&mut socket, result, &state).await {break;}
                continue;
            },
            Some((request_id, event)) = hook_events_rx.recv() => {
                if !send(&mut socket, commit_event_value(request_id, event), &state).await { break; }
                continue;
            },
            message = socket.recv() => match message { Some(Ok(message)) => message, _ => break },
            changed=preferences.changed()=>{
                if changed.is_err(){break;}
                let value=preferences.borrow_and_update().clone();
                if !send(&mut socket, json!({"event":"preferences_changed","preferences":value}), &state).await {break;}
                continue;
            },
            changed = changes.changed() => {
                if changed.is_err() { break; }
                let snapshot = changes.borrow_and_update().clone();
                if !send(&mut socket, json!({"event":"repository_changed","snapshot":snapshot}), &state).await {break;}
                continue;
            }
        };
        let Message::Text(text) = message else {
            if matches!(message, Message::Close(_)) {
                break;
            }
            continue;
        };
        let parsed = serde_json::from_str::<serde_json::Value>(&text).ok();
        let id = parsed
            .as_ref()
            .and_then(|value| value.get("id"))
            .and_then(serde_json::Value::as_u64);
        if id.is_some_and(|id| {
            active.as_ref().is_some_and(|job| job.id == id) || queue.iter().any(|rpc| rpc.id == id)
        }) {
            let _ = send(
                &mut socket,
                json!({"id":null,"error":"duplicate in-flight request id"}),
                &state,
            )
            .await;
            break;
        }
        let request = parsed.and_then(|value| serde_json::from_value::<Rpc>(value).ok());
        let response = match request {
            Some(Rpc {
                id,
                operation: ApplicationOperation::Control(ControlOperation::Cancel { request_id }),
                ..
            }) => {
                let status = if let Some(job) = active.as_ref().filter(|job| job.id == request_id) {
                    job.cancellation.cancel();
                    "requested"
                } else if let Some(index) = queue.iter().position(|rpc| rpc.id == request_id) {
                    queue.remove(index);
                    if !send(&mut socket, cancelled(request_id), &state).await {
                        break;
                    }
                    "queued"
                } else {
                    "not_found"
                };
                json!({"id":id,"result":{"request_id":request_id,"status":status}})
            }
            Some(rpc) => {
                if queue.len() + usize::from(active.is_some()) < 32 {
                    queue.push_back(rpc);
                    continue;
                }
                json!({"id":rpc.id,"error":"too many pending requests; wait for current work to finish","error_code":"queue_full"})
            }
            None => json!({"id":id,"error":"invalid typed repository request"}),
        };
        if !send(&mut socket, response, &state).await {
            break;
        }
    }
    if let Some(mut job) = active.take() {
        job.cancellation.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(3), job.future.as_mut()).await;
    }
}

async fn send(socket: &mut WebSocket, value: serde_json::Value, state: &Bridge) -> bool {
    let mut stopping = state.shutdown.clone();
    tokio::select! {
        _ = async { let _ = stopping.wait_for(|stopping| *stopping).await; } => false,
        result = tokio::time::timeout(Duration::from_secs(5), socket.send(Message::Text(value.to_string().into()))) => matches!(result, Ok(Ok(()))),
    }
}

fn cancelled(id: u64) -> serde_json::Value {
    cancelled_message(id, cancellation::CANCELLED)
}
fn cancelled_message(id: u64, message: &str) -> serde_json::Value {
    json!({"id":id,"error":message,"error_code":"cancelled"})
}

fn commit_event_value(
    request_id: u64,
    event: desktop_core::hook_progress::CommitEvent,
) -> serde_json::Value {
    use desktop_core::hook_progress::CommitEvent;
    match event {
        CommitEvent::Hook(event) => {
            json!({"event":"commit_hook_progress","request_id":request_id,"checkpoint_id":event.checkpoint_id,"hook_name":event.hook_name,"status":event.status})
        }
        CommitEvent::Output {
            checkpoint_id,
            stream,
            data,
        } => {
            json!({"event":"commit_output","request_id":request_id,"checkpoint_id":checkpoint_id,"stream":stream,"data":data})
        }
        CommitEvent::FetchOutput { stream, data } => {
            json!({"event":"fetch_output","request_id":request_id,"stream":stream,"data":data})
        }
    }
}

struct Active {
    id: u64,
    cancellation: Cancellation,
    future: Pin<Box<dyn Future<Output = serde_json::Value> + Send>>,
}
impl Active {
    fn new(
        state: Arc<Bridge>,
        rpc: Rpc,
        events: mpsc::Sender<(u64, desktop_core::hook_progress::CommitEvent)>,
        comparison_owner: Arc<desktop_core::comparison_snapshots::Owner>,
    ) -> Self {
        let cancellation = Cancellation::default();
        let request_cancellation = cancellation.clone();
        let request_id = rpc.id;
        let future = Box::pin(desktop_core::comparison_snapshots::scope(
            comparison_owner,
            cancellation
                .clone()
                .scope(desktop_core::hook_progress::scope(
                    events,
                    request_id,
                    async move {
                        let result = match request_cancellation.check() {
                            Ok(()) => execute(&state, rpc.operation, rpc.repository_id).await,
                            Err(error) => Err(error),
                        };
                        if request_cancellation.is_cancelled() {
                            return cancelled_message(rpc.id, request_cancellation.message());
                        }
                        match result {
                            Ok(result) => json!({"id":rpc.id,"result":result}),
                            Err(error) => json!({"id":rpc.id,"error":error.to_string()}),
                        }
                    },
                )),
        ));
        Self {
            id: rpc.id,
            cancellation,
            future,
        }
    }
}
impl Drop for Active {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}
