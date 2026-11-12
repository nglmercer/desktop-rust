//! Typed, repository-scoped operations exposed by the Wry host.
//!
//! Cancellable reads and explicit repository-trust requests.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use desktop_backend::{Operation, Repository, cancellation::Cancellation};
use desktop_core::error::{AppError, Result};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadRequest {
    repository_path: String,
    method: ReadMethod,
    #[serde(default)]
    include_untracked: Option<bool>,
    #[serde(default)]
    prefixes: Option<Vec<String>>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    commitish: Option<String>,
    #[serde(default)]
    range: Option<String>,
    #[serde(default)]
    kind: Option<desktop_backend::StatusConflictKind>,
    #[serde(default)]
    request_id: Option<uuid::Uuid>,
    #[serde(default)]
    deadline_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CopilotRequest {
    repository_path: String,
    operation: Operation,
    request_id: Option<uuid::Uuid>,
    deadline_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReadMethod {
    RepositoryType,
    DirectoryExists,
    PathExists,
    CanonicalMutationPath,
    TrustRepository,
    Snapshot,
    Status,
    StatusDetailed,
    StatusContext,
    StatusConflictDetails,
    BranchEnumeration,
    BranchMerged,
    BranchPointsAt,
    AheadBehind,
    RevisionList,
    HasMergeCommits,
}

pub struct Backend {
    runtime: tokio::runtime::Runtime,
    jobs: Arc<Mutex<HashMap<uuid::Uuid, Cancellation>>>,
    capacity: usize,
    registry: Option<desktop_backend::Registry>,
    copilot_runtime_root: Option<PathBuf>,
}

pub struct ReadJob {
    path: String,
    operation: BackendOperation,
    token: Cancellation,
    id: Option<uuid::Uuid>,
    jobs: Arc<Mutex<HashMap<uuid::Uuid, Cancellation>>>,
    pub deadline: Instant,
}

#[derive(Clone)]
enum BackendOperation {
    RepositoryType,
    DirectoryExists,
    PathExists,
    CanonicalMutationPath,
    TrustRepository,
    Repository(Operation),
    Copilot(Operation),
}

impl Drop for ReadJob {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            self.jobs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
        }
    }
}

impl Backend {
    #[cfg(test)]
    fn read_args(&self, args: Vec<Value>) -> Result<Value> {
        self.read(self.admit(args)?)
    }
    pub fn new(capacity: usize) -> std::io::Result<Self> {
        Ok(Self {
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?,
            jobs: Arc::new(Mutex::new(HashMap::new())),
            capacity,
            registry: None,
            copilot_runtime_root: None,
        })
    }

    pub fn with_registry(mut self, registry: desktop_backend::Registry) -> Self {
        self.registry = Some(registry);
        self
    }

    pub fn with_copilot_runtime_root(mut self, root: Option<PathBuf>) -> Self {
        self.copilot_runtime_root = root;
        self
    }

    pub fn admit_copilot(&self, args: Vec<Value>) -> Result<ReadJob> {
        let [value]: [Value; 1] = args
            .try_into()
            .map_err(|_| AppError::BadArgs("Copilot requires exactly one request".into()))?;
        // Serde diagnostics may contain malformed credential values.
        let request: CopilotRequest = serde_json::from_value(value)
            .map_err(|_| AppError::BadArgs("invalid Copilot request".into()))?;
        if !Path::new(&request.repository_path).is_absolute() {
            return Err(AppError::BadArgs("repository path must be absolute".into()));
        }
        if self.registry.is_none() {
            return Err(AppError::BadArgs(
                "Copilot account registry is unavailable".into(),
            ));
        }
        match &request.operation {
            Operation::CopilotCommitDiffDetail { .. } => {}
            Operation::CopilotGenerateCommitMessage {
                account: Some(_), ..
            } => {}
            Operation::CopilotGenerateCommitMessage { account: None, .. } => {
                return Err(AppError::BadArgs(
                    "choose a GitHub account for Copilot".into(),
                ));
            }
            _ => return Err(AppError::BadArgs("unsupported Copilot operation".into())),
        }
        let duration = request.deadline_ms.unwrap_or(170_000);
        if !(1..=330_000).contains(&duration) {
            return Err(AppError::BadArgs(
                "deadline_ms must be between 1 and 330000".into(),
            ));
        }
        let token = Cancellation::default();
        if let Some(id) = request.request_id {
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            if jobs.len() >= self.capacity || jobs.contains_key(&id) {
                return Err(AppError::Coded {
                    code: "EBUSY".into(),
                    message: "backend queue is full or request ID is already active".into(),
                });
            }
            jobs.insert(id, token.clone());
        }
        Ok(ReadJob {
            path: request.repository_path,
            operation: BackendOperation::Copilot(request.operation),
            token,
            id: request.request_id,
            jobs: self.jobs.clone(),
            deadline: Instant::now() + Duration::from_millis(duration),
        })
    }

    pub fn admit(&self, args: Vec<Value>) -> Result<ReadJob> {
        let [value]: [Value; 1] = args
            .try_into()
            .map_err(|_| AppError::BadArgs("backend read requires exactly one request".into()))?;
        let request: ReadRequest = serde_json::from_value(value)?;
        if !Path::new(&request.repository_path).is_absolute() {
            return Err(AppError::BadArgs("repository path must be absolute".into()));
        }
        if !matches!(
            &request.method,
            ReadMethod::AheadBehind | ReadMethod::RevisionList | ReadMethod::HasMergeCommits
        ) && request.range.is_some()
        {
            return Err(AppError::BadArgs(
                "range only applies to revision reads".into(),
            ));
        }
        if !matches!(&request.method, ReadMethod::StatusConflictDetails) && request.kind.is_some() {
            return Err(AppError::BadArgs(
                "kind only applies to conflict detail reads".into(),
            ));
        }
        let operation = if matches!(
            &request.method,
            ReadMethod::RepositoryType
                | ReadMethod::DirectoryExists
                | ReadMethod::PathExists
                | ReadMethod::CanonicalMutationPath
                | ReadMethod::TrustRepository
        ) && request.include_untracked.is_none()
            && request.prefixes.is_none()
            && request.branch.is_none()
            && request.commitish.is_none()
        {
            match request.method {
                ReadMethod::RepositoryType => BackendOperation::RepositoryType,
                ReadMethod::DirectoryExists => BackendOperation::DirectoryExists,
                ReadMethod::PathExists => BackendOperation::PathExists,
                ReadMethod::CanonicalMutationPath => BackendOperation::CanonicalMutationPath,
                ReadMethod::TrustRepository => BackendOperation::TrustRepository,
                _ => unreachable!(),
            }
        } else {
            BackendOperation::Repository(match request.method {
                ReadMethod::Snapshot
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::Snapshot
                }
                ReadMethod::Status
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::Status
                }
                ReadMethod::StatusDetailed
                    if request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::StatusDetailed {
                        include_untracked: request.include_untracked.unwrap_or(true),
                    }
                }
                ReadMethod::StatusContext
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::StatusContext
                }
                ReadMethod::StatusConflictDetails
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::StatusConflictDetails {
                        kind: request
                            .kind
                            .ok_or_else(|| AppError::BadArgs("kind is required".into()))?,
                    }
                }
                ReadMethod::BranchEnumeration
                    if request.include_untracked.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::BranchEnumeration {
                        prefixes: request.prefixes.unwrap_or_default(),
                    }
                }
                ReadMethod::BranchMerged
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::BranchMerged {
                        branch: request
                            .branch
                            .ok_or_else(|| AppError::BadArgs("branch is required".into()))?,
                    }
                }
                ReadMethod::BranchPointsAt
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none() =>
                {
                    Operation::BranchPointsAt {
                        commitish: request
                            .commitish
                            .ok_or_else(|| AppError::BadArgs("commitish is required".into()))?,
                    }
                }
                ReadMethod::AheadBehind
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::AheadBehind {
                        range: request
                            .range
                            .ok_or_else(|| AppError::BadArgs("range is required".into()))?,
                    }
                }
                ReadMethod::RevisionList
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::RevisionList {
                        range: request
                            .range
                            .ok_or_else(|| AppError::BadArgs("range is required".into()))?,
                    }
                }
                ReadMethod::HasMergeCommits
                    if request.include_untracked.is_none()
                        && request.prefixes.is_none()
                        && request.branch.is_none()
                        && request.commitish.is_none() =>
                {
                    Operation::HasMergeCommits {
                        range: request
                            .range
                            .ok_or_else(|| AppError::BadArgs("range is required".into()))?,
                    }
                }
                _ => {
                    return Err(AppError::BadArgs(
                        "invalid options for repository read".into(),
                    ));
                }
            })
        };
        let duration = request.deadline_ms.unwrap_or(60_000);
        if !(1..=60_000).contains(&duration) {
            return Err(AppError::BadArgs(
                "deadline_ms must be between 1 and 60000".into(),
            ));
        }
        let token = Cancellation::default();
        if let Some(id) = request.request_id {
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            if jobs.len() >= self.capacity || jobs.contains_key(&id) {
                return Err(AppError::Coded {
                    code: "EBUSY".into(),
                    message: "backend read queue is full or request ID is already active".into(),
                });
            }
            jobs.insert(id, token.clone());
        }
        Ok(ReadJob {
            path: request.repository_path,
            operation,
            token,
            id: request.request_id,
            jobs: self.jobs.clone(),
            deadline: Instant::now() + Duration::from_millis(duration),
        })
    }

    pub fn cancel(&self, args: &[Value]) -> Result<()> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Cancel {
            request_id: uuid::Uuid,
        }
        let [value] = args else {
            return Err(AppError::BadArgs(
                "backend cancel requires one request".into(),
            ));
        };
        let request: Cancel = serde_json::from_value(value.clone())?;
        if let Some(token) = self
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&request.request_id)
        {
            token.cancel();
        }
        Ok(())
    }

    pub fn cancel_all(&self) {
        for token in self.jobs.lock().unwrap_or_else(|e| e.into_inner()).values() {
            token.cancel();
        }
    }

    pub fn read(&self, job: ReadJob) -> Result<Value> {
        let token = job.token.clone();
        self.runtime.block_on(token.clone().scope(async {
            if token.is_cancelled() { return Err(AppError::Coded { code: "cancelled".into(), message: "repository read cancelled".into() }); }
            let remaining = job.deadline.saturating_duration_since(Instant::now());
            let work = async {
                let (operation, scoped) = match job.operation.clone() {
                    BackendOperation::RepositoryType => return desktop_backend::repository_type(Path::new(&job.path)).await.map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::DirectoryExists => return desktop_backend::directory_exists(Path::new(&job.path)).await.map(serde_json::Value::Bool).map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::PathExists => return desktop_backend::path_exists(Path::new(&job.path)).await.map(serde_json::Value::Bool).map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::CanonicalMutationPath => return desktop_backend::canonical_mutation_path(Path::new(&job.path)).await.map(serde_json::Value::String).map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::TrustRepository => return desktop_backend::trust_repository(Path::new(&job.path)).await.map_err(|error| AppError::msg(error.to_string())),
                    BackendOperation::Repository(operation) => (operation, false),
                    BackendOperation::Copilot(operation) => (operation, true),
                };
                let mut repo = Repository::open(Path::new(&job.path))
                    .await
                    .map_err(|error| AppError::msg(error.to_string()))?;
                if scoped {
                    repo = repo.with_registry(self.registry.clone().ok_or_else(|| AppError::BadArgs("Copilot account registry is unavailable".into()))?);
                    if let Some(root) = &self.copilot_runtime_root {
                        repo = repo.with_copilot_runtime_root(root.clone());
                    }
                }
                repo.execute(operation)
                    .await
                    .map_err(|error| AppError::msg(error.to_string()))
            };
            tokio::pin!(work);
            tokio::select! {
                result = &mut work => if token.is_cancelled() { Err(AppError::Coded { code: "cancelled".into(), message: "repository read cancelled".into() }) } else { result },
                _ = tokio::time::sleep(remaining) => {
                    token.cancel();
                    let _ = tokio::time::timeout(Duration::from_secs(3), &mut work).await;
                    Err(AppError::Coded { code: "ETIMEDOUT".into(), message: "repository read timed out".into() })
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{fs, process::Command};

    #[cfg(unix)]
    #[test]
    fn fake_wry_copilot_runtime() {
        use std::io::{BufRead, Read, Write};
        if std::env::var_os("DESKTOP_WRY_FAKE_RUNTIME").is_none() {
            return;
        }
        assert_eq!(
            std::env::var("COPILOT_SDK_AUTH_TOKEN").unwrap(),
            "wry-selected-token"
        );
        assert_eq!(std::env::var("GH_HOST").unwrap(), "github.com");
        assert!(std::env::var_os("GH_TOKEN").is_none());
        let mut input = std::io::BufReader::new(std::io::stdin());
        let mut output = fs::OpenOptions::new()
            .write(true)
            .open("/dev/fd/3")
            .unwrap();
        loop {
            let mut header = String::new();
            if input.read_line(&mut header).unwrap() == 0 {
                break;
            }
            let length: usize = header
                .trim()
                .strip_prefix("Content-Length: ")
                .unwrap()
                .parse()
                .unwrap();
            let mut blank = String::new();
            input.read_line(&mut blank).unwrap();
            assert_eq!(blank, "\r\n");
            let mut body = vec![0; length];
            input.read_exact(&mut body).unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap();
            let result = match request["method"].as_str().unwrap() {
                "connect" => json!({"protocolVersion":3}),
                "session.create" => {
                    assert_eq!(request["params"]["systemMessage"]["mode"], "append");
                    assert_eq!(request["params"]["availableTools"], json!([]));
                    json!({"sessionId":request["params"]["sessionId"]})
                }
                "session.options.update" => {
                    assert_eq!(request["params"]["coauthorEnabled"], false);
                    json!({})
                }
                "session.sendAndWait" => {
                    let prompt = request["params"]["prompt"].as_str().unwrap();
                    assert!(
                        prompt.contains("+selected content")
                            && !prompt.contains("unselected content")
                    );
                    json!({"aborted":false,"response":{"type":"assistant.message","data":{"content":"{\"title\":\"Fix selected\",\"description\":\"Details\"}"}}})
                }
                _ => panic!("unexpected fixture method"),
            };
            let reply = json!({"jsonrpc":"2.0","id":request["id"],"result":result}).to_string();
            write!(output, "Content-Length: {}\r\n\r\n{}", reply.len(), reply).unwrap();
            output.flush().unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn wry_copilot_generation_uses_host_runtime_and_verified_account() {
        use std::os::unix::fs::PermissionsExt;
        const CHILD: &str = "DESKTOP_WRY_COPILOT_TEST_CHILD";
        if let Some(root) = std::env::var_os(CHILD) {
            let root = PathBuf::from(root);
            let backend = Backend::new(4)
                .unwrap()
                .with_registry(desktop_backend::Registry::new(root.join("registry.json")))
                .with_copilot_runtime_root(Some(root.join("runtime")));
            let request = json!({"repository_path":root.join("repo"),"operation":{
                "method":"copilot_generate_commit_message", "source":"working_tree", "paths":["selected"],
                "account":{"host":"github.com","login":"alice","id":42}, "rule_descriptions":["Use an issue number"]
            }});
            let result = backend
                .read(backend.admit_copilot(vec![request]).unwrap())
                .unwrap();
            assert_eq!(result["summary"], "Fix selected");
            assert_eq!(result["description"], "Details");
            assert!(!root.join("repo/.git/index").exists());
            return;
        }
        let root =
            std::env::temp_dir().join(format!("desktop-wry-copilot-live-{}", uuid::Uuid::new_v4()));
        let repo = root.join("repo");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(&repo)
                .status()
                .unwrap()
                .success()
        );
        fs::write(repo.join("selected"), b"selected content\n").unwrap();
        fs::write(repo.join("unselected"), b"unselected content\n").unwrap();
        let gh = bin.join("gh");
        fs::write(&gh, r##"#!/bin/sh
case "$1 $2" in
  'auth status') [ -z "${GH_TOKEN+x}" ] || exit 90; printf '{"hosts":{"github.com":[{"login":"Alice","state":"success"}]}}\n';;
  'auth token') [ -z "${GH_TOKEN+x}" ] || exit 91; printf 'wry-selected-token\n';;
  'api --method') [ "$GH_TOKEN" = wry-selected-token ] || exit 92; printf '{"login":"Alice","id":42}\n';;
  *) exit 93;;
esac
"##).unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o700)).unwrap();
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            _ => return,
        };
        let platform = if std::env::consts::OS == "macos" {
            "darwin"
        } else {
            std::env::consts::OS
        };
        let runtime = root
            .join("runtime/prebuilds")
            .join(format!("{platform}-{arch}"));
        fs::create_dir_all(&runtime).unwrap();
        let exe = std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .replace('\'', "'\\''");
        let wrapper = runtime.join("copilot-runtime");
        fs::write(&wrapper, format!("#!/bin/sh\nexec 3>&1\nexec '{exe}' --exact backend::tests::fake_wry_copilot_runtime --nocapture >/dev/null\n")).unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        let path = std::env::join_paths(
            std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "backend::tests::wry_copilot_generation_uses_host_runtime_and_verified_account",
                "--nocapture",
            ])
            .env(CHILD, &root)
            .env("DESKTOP_WRY_FAKE_RUNTIME", "1")
            .env("HOME", &root)
            .env("PATH", path)
            .env("GH_TOKEN", "wrong-token")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", root.join("gitconfig"))
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "host service test failed: {}",
            String::from_utf8_lossy(&child.stderr)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scoped_copilot_admission_fences_account_and_cancellation() {
        use desktop_backend::{AccountAssignment, StoredAccount};
        let root =
            std::env::temp_dir().join(format!("desktop-wry-copilot-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.join("selected.txt"), b"selected line\n").unwrap();
        let registry = desktop_backend::Registry::new(root.join("state/registry.json"));
        let record = registry.add(root.clone(), true).unwrap().records.remove(0);
        registry
            .compare_and_replace_account(
                &record.id,
                0,
                StoredAccount {
                    revision: 1,
                    assignment: AccountAssignment::None {},
                    association: None,
                    source_remote: None,
                },
            )
            .unwrap();
        let backend = Backend::new(1).unwrap().with_registry(registry);
        let preview = || json!({"repository_path":root,"operation":{"method":"copilot_commit_diff_detail","path":"selected.txt"}});
        let value = backend
            .read(backend.admit_copilot(vec![preview()]).unwrap())
            .unwrap();
        assert_eq!(value["can_select"], true);
        assert_eq!(value["hunks"][0]["lines"][0]["text"], "selected line");
        for operation in [
            json!({"method":"stage","paths":["selected.txt"]}),
            json!({"method":"copilot_generate_commit_message"}),
            json!({"method":"copilot_generate_commit_message","account":{"host":"github.com","login":"alice","id":1,"token":"not-allowed"}}),
        ] {
            assert!(
                backend
                    .admit_copilot(vec![json!({"repository_path":root,"operation":operation})])
                    .is_err()
            );
        }
        let malformed = backend.admit_copilot(vec![json!({"repository_path":root,"operation":{
            "method":"copilot_generate_commit_message", "account":{"host":"github.com","login":"alice","id":1},
            "provider":{"type":"azure","baseUrl":"https://provider.example","azure":"synthetic-private-credential"}
        }})]);
        match malformed {
            Ok(_) => panic!("malformed provider was accepted"),
            Err(error) => assert!(!error.to_string().contains("synthetic-private-credential")),
        }
        let job = backend.admit_copilot(vec![json!({"repository_path":root,"operation":{"method":"copilot_generate_commit_message","account":{"host":"github.com","login":"alice","id":1},"source":"working_tree","paths":["selected.txt"]}})]).unwrap();
        assert!(
            backend
                .read(job)
                .unwrap_err()
                .to_string()
                .contains("No GitHub account")
        );
        assert!(backend.admit_copilot(vec![json!({"repository_path":"relative","operation":{"method":"copilot_commit_diff_detail","path":"x"}})]).is_err());
        assert!(backend.admit_copilot(vec![json!({"repository_path":root,"operation":{"method":"copilot_commit_diff_detail","path":"x"},"deadline_ms":330001})]).is_err());
        let id = uuid::Uuid::new_v4();
        let mut request = preview();
        request["request_id"] = json!(id);
        let job = backend.admit_copilot(vec![request.clone()]).unwrap();
        assert!(backend.admit_copilot(vec![request]).is_err());
        backend.cancel(&[json!({"request_id":id})]).unwrap();
        assert!(
            matches!(backend.read(job), Err(AppError::Coded { code, .. }) if code == "cancelled")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn trust_repository_uses_the_native_job() {
        const CHILD: &str = "DESKTOP_TEST_TRUST_CHILD";
        if let Some(root) = std::env::var_os(CHILD) {
            let root = std::path::PathBuf::from(root);
            let backend = Backend::new(4).unwrap();
            let query = || {
                backend.read_args(vec![
                    json!({"repository_path":root,"method":"trust_repository"}),
                ])
            };
            assert_eq!(query().unwrap(), json!({"trusted":true}));
            let config = fs::read(root.join("global")).unwrap();
            assert_eq!(query().unwrap(), json!({"trusted":true}));
            assert_eq!(fs::read(root.join("global")).unwrap(), config);
            return;
        }
        let root = std::env::temp_dir().join(format!("desktop-trust-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "backend::tests::trust_repository_uses_the_native_job",
                "--nocapture",
            ])
            .env(CHILD, &root)
            .env("GIT_CONFIG_GLOBAL", root.join("global"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repository_type_can_inspect_unselected_paths() {
        let root = std::env::temp_dir().join(format!("desktop-discovery-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("nested")).unwrap();
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success());
        let backend = Backend::new(4).unwrap();
        let query = |path: &Path| {
            backend.read_args(vec![
                json!({"repository_path":path,"method":"repository_type"}),
            ])
        };
        let directory = |path: &Path| {
            backend.read_args(vec![
                json!({"repository_path":path,"method":"directory_exists"}),
            ])
        };
        assert_eq!(directory(&root).unwrap(), true);
        assert_eq!(directory(&root.join("missing")).unwrap(), false);
        assert_eq!(directory(&root.join(".git/HEAD")).unwrap(), false);
        let exists = |path: &Path| {
            backend.read_args(vec![json!({"repository_path":path,"method":"path_exists"})])
        };
        assert_eq!(exists(&root).unwrap(), true);
        assert_eq!(exists(&root.join("missing")).unwrap(), false);
        assert_eq!(exists(&root.join(".git/HEAD")).unwrap(), true);
        let canonical = |path: &Path| {
            backend.read_args(vec![
                json!({"repository_path":path,"method":"canonical_mutation_path"}),
            ])
        };
        let canonical_value = canonical(&root.join("nested/new/deep")).unwrap();
        let canonical_path = Path::new(canonical_value.as_str().unwrap());
        assert_eq!(canonical_path.file_name().unwrap(), "deep");
        assert_eq!(canonical_path.parent().unwrap().file_name().unwrap(), "new");
        assert_eq!(
            fs::canonicalize(canonical_path.parent().unwrap().parent().unwrap()).unwrap(),
            fs::canonicalize(root.join("nested")).unwrap()
        );
        assert!(canonical(&root.join(".git/HEAD/child")).is_err());
        assert_eq!(
            query(&root.join("nested")).unwrap(),
            json!({"kind":"regular","topLevelWorkingDirectory":root,"gitDir":root.join(".git")})
        );
        assert_eq!(
            query(&root.join("missing")).unwrap(),
            json!({"kind":"missing"})
        );
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":root.join("missing"),"method":"trust_repository"})
                ])
                .is_err()
        );
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":root,"method":"repository_type","range":"HEAD"})
                ])
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn queued_reads_are_cancellable_and_registration_is_released() {
        let backend = Backend::new(1).unwrap();
        let id = uuid::Uuid::new_v4();
        let args = vec![json!({"repository_path":"/missing", "method":"status", "request_id":id})];
        let job = backend.admit(args.clone()).unwrap();
        assert!(backend.admit(args.clone()).is_err());
        backend.cancel(&[json!({"request_id":id})]).unwrap();
        assert!(
            matches!(backend.read(job), Err(AppError::Coded { code, .. }) if code == "cancelled")
        );
        let job = backend.admit(args.clone()).unwrap();
        backend.cancel_all();
        assert!(
            matches!(backend.read(job), Err(AppError::Coded { code, .. }) if code == "cancelled")
        );
        drop(backend.admit(args).unwrap());
        assert!(backend.jobs.lock().unwrap().is_empty());
    }

    #[test]
    fn enumerates_disambiguated_branches_and_respects_prefixes() {
        let root =
            std::env::temp_dir().join(format!("desktop-wry-branches-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&root)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", root.join("global"))
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "base",
        ]);
        let oid = git(&["rev-parse", "HEAD"]);
        git(&["branch", "topic"]);
        git(&["tag", "topic"]);
        git(&["remote", "add", "origin", "https://example.invalid/repo"]);
        git(&["update-ref", "refs/remotes/origin/topic", &oid]);
        git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/topic",
        ]);
        git(&["config", "branch.topic.remote", "origin"]);
        git(&["config", "branch.topic.merge", "refs/heads/topic"]);
        let backend = Backend::new(16).unwrap();
        let enumerate = |prefixes: Value| {
            backend.read_args(vec![json!({"repository_path":root, "method":"branch_enumeration", "prefixes":prefixes})]).unwrap()
        };
        let all = enumerate(json!([]));
        assert_eq!(all.as_array().unwrap().len(), 3);
        assert_eq!(all[1]["ref"], "refs/heads/topic");
        assert_eq!(all[1]["name"], "heads/topic");
        assert_eq!(all[1]["upstream"], "origin/topic");
        assert_eq!(all[1]["upstream_ref"], "refs/remotes/origin/topic");
        assert_eq!(all[0]["current"], true);
        assert_eq!(all[1]["current"], false);
        assert_eq!(all[2]["current"], false);
        assert_eq!(all[1]["oid"], oid);
        let remote = enumerate(json!(["refs/remotes/origin/"]));
        assert_eq!(remote.as_array().unwrap().len(), 1);
        assert_eq!(remote[0]["ref"], "refs/remotes/origin/topic");
        assert_eq!(enumerate(json!(["refs/remotes/missing"])), json!([]));
        assert_eq!(enumerate(json!(["--format=malicious"])), json!([]));
        git(&["switch", "-qc", "unmerged"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "unmerged",
        ]);
        git(&["switch", "-q", "main"]);
        let ahead_behind = |range: &str| {
            backend.read_args(vec![
                json!({"repository_path":root, "method":"ahead_behind", "range":range}),
            ])
        };
        assert_eq!(
            ahead_behind("main...unmerged").unwrap(),
            json!({"ahead":0, "behind":1})
        );
        assert_eq!(
            ahead_behind("unmerged...main").unwrap(),
            json!({"ahead":1, "behind":0})
        );
        assert_eq!(
            ahead_behind("main...main").unwrap(),
            json!({"ahead":0, "behind":0})
        );
        assert_eq!(ahead_behind("missing...main").unwrap(), Value::Null);
        assert_eq!(ahead_behind("--all").unwrap(), Value::Null);
        assert!(backend.read_args(vec![json!({"repository_path":root, "method":"ahead_behind", "range":"main...main", "branch":"main"})]).is_err());
        let merged = backend
            .read_args(vec![
                json!({"repository_path":root, "method":"branch_merged", "branch":"main"}),
            ])
            .unwrap();
        assert_eq!(
            merged,
            json!([{ "ref":"refs/heads/main", "oid":oid }, { "ref":"refs/heads/topic", "oid":oid }])
        );
        let pointing = backend
            .read_args(vec![
                json!({"repository_path":root, "method":"branch_points_at", "commitish":"HEAD"}),
            ])
            .unwrap();
        assert_eq!(pointing, json!(["main", "heads/topic"]));
        assert_eq!(backend.read_args(vec![json!({"repository_path":root, "method":"branch_points_at", "commitish":"missing-ref"})]).unwrap(), Value::Null);
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":root, "method":"branch_merged", "branch":"-D"})
                ])
                .is_err()
        );
        assert!(backend.read_args(vec![json!({"repository_path":root, "method":"branch_merged", "branch":"main", "commitish":"HEAD"})]).is_err());
        assert_eq!(git(&["rev-parse", "refs/heads/main"]), oid);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "diverged",
        ]);
        assert_eq!(
            ahead_behind("main...unmerged").unwrap(),
            json!({"ahead":1, "behind":1})
        );
        let revision_read = |method: &str, range: &str| {
            backend.read_args(vec![
                json!({"repository_path":root,"method":method,"range":range}),
            ])
        };
        let expected = git(&[
            "rev-list",
            "--reverse",
            "--oneline",
            "--no-abbrev-commit",
            "--end-of-options",
            "main..unmerged",
            "--",
        ]);
        assert_eq!(
            revision_read("revision_list", "main..unmerged")
                .unwrap()
                .as_str()
                .unwrap()
                .trim(),
            expected
        );
        assert_eq!(revision_read("revision_list", "main..main").unwrap(), "");
        assert_eq!(
            revision_read("revision_list", "missing..main").unwrap(),
            Value::Null
        );
        assert_eq!(
            revision_read("revision_list", "--all").unwrap(),
            Value::Null
        );
        assert_eq!(revision_read("has_merge_commits", "main").unwrap(), false);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "merge",
            "--no-ff",
            "-qm",
            "merge",
            "unmerged",
        ]);
        assert_eq!(revision_read("has_merge_commits", "HEAD").unwrap(), true);
        assert_eq!(
            revision_read("has_merge_commits", "HEAD..HEAD").unwrap(),
            false
        );
        assert!(backend.read_args(vec![json!({"repository_path":root, "method":"branch_enumeration", "include_untracked":true})]).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_the_requested_repository_and_rejects_mutations() {
        let root =
            std::env::temp_dir().join(format!("desktop-wry-backend-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let first = root.join("first");
        let second = root.join("second");
        for path in [&first, &second] {
            assert!(
                Command::new("git")
                    .args(["init", "-q"])
                    .arg(path)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        fs::write(first.join("first-only"), "test").unwrap();
        let backend = Backend::new(16).unwrap();
        fs::write(first.join(".git/SQUASH_MSG"), "squash").unwrap();
        let context = backend
            .read_args(vec![
                json!({"repository_path":first,"method":"status_context"}),
            ])
            .unwrap();
        assert_eq!(context["squashMsgFound"], true);
        let context = backend
            .read_args(vec![
                json!({"repository_path":second,"method":"status_context"}),
            ])
            .unwrap();
        assert_eq!(context["squashMsgFound"], false);
        let conflicts = backend.read_args(vec![json!({"repository_path":first,"method":"status_conflict_details","kind":"working"})]).unwrap();
        assert_eq!(conflicts["check"], "");
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":first,"method":"status_conflict_details"})
                ])
                .is_err()
        );
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":first,"method":"status_context","kind":"working"})
                ])
                .is_err()
        );
        assert_eq!(
            backend
                .read_args(vec![
                    json!({"repository_path":second,"method":"has_merge_commits","range":"HEAD"})
                ])
                .unwrap(),
            false
        );
        let read = |path: &Path| {
            backend
                .read_args(vec![json!({"repository_path":path,"method":"snapshot"})])
                .unwrap()
        };
        let one = read(&first).to_string();
        let two = read(&second).to_string();
        assert!(one.contains("first-only"));
        assert!(!two.contains("first-only"));
        let detailed = backend.read_args(vec![json!({"repository_path":first,"method":"status_detailed","include_untracked":true})]).unwrap();
        assert!(
            detailed
                .as_array()
                .unwrap()
                .iter()
                .any(|record| record["path"]["bytes"] == json!(b"first-only"))
        );
        assert!(
            backend
                .read_args(vec![json!({"repository_path":first,"method":"commit"})])
                .is_err()
        );
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":first,"method":"status","extra":true})
                ])
                .is_err()
        );
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":first,"method":"status","include_untracked":true})
                ])
                .is_err()
        );
        assert!(
            backend
                .read_args(vec![json!({"repository_path":"","method":"status"})])
                .is_err()
        );
        assert!(
            backend
                .read_args(vec![
                    json!({"repository_path":"relative","method":"status"})
                ])
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
