//! Provisional selected-account credentials for a network-only clone phase.
//!
//! Checkout, filters, hooks, LFS and recursive submodules must run only after
//! this scope ends and the destination has its own saved account binding.
use super::*;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

tokio::task_local! { static CURRENT: Arc<CloneLease>; }
tokio::task_local! { static PINNED_CHECKOUT: bool; }

pub(crate) async fn pinned_checkout<T>(work: impl std::future::Future<Output = T>) -> T {
    PINNED_CHECKOUT.scope(true, work).await
}

pub(super) fn pinned_checkout_active() -> bool {
    PINNED_CHECKOUT.try_with(|value| *value).unwrap_or(false)
}

struct CloneLease {
    endpoint: Endpoint,
    login: String,
    id: u64,
    account_path: PathBuf,
    session: Arc<Session>,
    config: Vec<(String, String)>,
    secrets: Vec<Vec<u8>>,
    address: String,
    capability: String,
    helper: PathBuf,
    directory: PathBuf,
    cancellation: Cancellation,
}

struct Guard {
    lease: Arc<CloneLease>,
    broker: Option<tokio::task::JoinHandle<()>>,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn create_broker_home_before_create(parent: &std::fs::File, expected: &Path, before_create: impl FnOnce() -> Result<()>) -> Result<std::fs::File> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let pinned_parent = crate::registry::pinned_clone_directory(parent, Path::new("/proc"))?;
    before_create()?;
    ensure!(std::fs::canonicalize(&pinned_parent)? == expected, "Clone staging container changed before broker initialization");
    let anchored = pinned_parent.join("broker-home");
    std::fs::DirBuilder::new().mode(0o700).create(&anchored).context("Cannot create private clone directory")?;
    let home = std::fs::OpenOptions::new().read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&anchored).context("Cannot pin private clone HOME")?;
    let pinned_home = crate::registry::pinned_clone_directory(&home, Path::new("/proc"))?;
    ensure!(std::fs::canonicalize(pinned_home)? == expected.join("broker-home"), "Clone temporary HOME changed during creation");
    Ok(home)
}

/// Verify the chosen actor before creating a persistent recovery record.
pub(crate) async fn verify_clone_account(
    path: &Path,
    url: &str,
    account: ChosenAccount<'_>,
) -> Result<()> {
    ensure!(
        account
            .id
            .is_some_and(|id| id > 0 && id <= 9_007_199_254_740_991),
        "A verified account ID is required for clone"
    );
    let version = crate::process::run(path, &crate::args(&["--version"])).await?;
    ensure!(
        version.code == 0 && supported_git(&version.stdout),
        "Assigned HTTPS credentials require Git 2.46 or newer"
    );
    ensure!(
        std::env::var_os("GIT_SSL_NO_VERIFY").is_none(),
        "Clone cannot use GIT_SSL_NO_VERIFY with an assigned account"
    );
    let endpoint = Endpoint::from_operation_url(url)?;
    ensure!(
        endpoint.kind == endpoint::EndpointKind::Git
            && endpoint
                .origin
                .authority()
                .eq_ignore_ascii_case(account.host),
        "Clone account belongs to a different host"
    );
    let identity = endpoint.identity();
    auth_sources::check(path, std::slice::from_ref(&endpoint)).await?;
    let session = Session::open_for_account(path, &identity, Some(account))
        .await?
        .map_err(|_| anyhow::anyhow!(REFUSED))?;
    ensure!(
        session.repository_access(&identity).await?,
        "Assigned account cannot access the requested repository"
    );
    Ok(())
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.lease.cancellation.cancel();
        if let Some(broker) = self.broker.take() {
            broker.abort();
        }
    }
}

/// Run only the network portion of a clone. No clone result is selected or
/// persisted here. The caller owns binding the account, checkout and recovery.
#[cfg(unix)]
#[expect(
    clippy::too_many_arguments,
    reason = "Pinned paths, handles, account, and options are checked separately"
)]
pub(crate) async fn clone_no_checkout(
    path: &Path,
    url: &str,
    destination: &Path,
    destination_handle: &std::fs::File,
    container_handle: &std::fs::File,
    container: &Path,
    branch: Option<&str>,
    default_branch: Option<&str>,
    account: ChosenAccount<'_>,
    progress: bool,
) -> Result<()> {
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    use std::os::unix::fs::DirBuilderExt;
    let id = account
        .id
        .filter(|id| *id > 0 && *id <= 9_007_199_254_740_991)
        .context("A verified account ID is required for clone")?;
    ensure!(
        destination.is_absolute(),
        "Clone destination must be absolute"
    );
    for value in [branch, default_branch].into_iter().flatten() {
        ensure!(
            !value.is_empty()
                && value.len() <= 4096
                && !value.starts_with('-')
                && !value.chars().any(char::is_control),
            "Invalid clone branch"
        );
    }
    let endpoint = Endpoint::from_operation_url(url)?;
    ensure!(
        endpoint.kind == endpoint::EndpointKind::Git,
        "Clone URL must name a repository"
    );
    ensure!(
        endpoint
            .origin
            .authority()
            .eq_ignore_ascii_case(account.host),
        "Clone account belongs to a different host"
    );
    let version = crate::process::run(path, &crate::args(&["--version"])).await?;
    ensure!(
        version.code == 0 && supported_git(&version.stdout),
        "Assigned HTTPS credentials require Git 2.46 or newer"
    );
    let identity = endpoint.identity();
    let session = Session::open_for_account(path, &identity, Some(account))
        .await?
        .map_err(|_| anyhow::anyhow!(REFUSED))?;
    ensure!(
        session.repository_access(&identity).await?,
        "Assigned account cannot access the requested repository"
    );
    let mut config = vec![("credential.helper".into(), String::new())];
    config.extend(configuration(account.host, &[])?);
    // Preserve only bounded transport settings, with all inherited Git config
    // and environment discarded for the clone itself. Never log these values.
    for key in [
        "http.proxy",
        "http.proxyAuthMethod",
        "http.sslCAInfo",
        "http.sslCAPath",
        "http.sslCert",
        "http.sslKey",
        "http.sslVersion",
        "http.sslVerify",
    ] {
        let output =
            crate::process::run(path, &crate::args(&["config", "--get-urlmatch", key, url]))
                .await?;
        ensure!(
            output.code == 0 || (output.code == 1 && output.stdout.is_empty()),
            "Cannot inspect clone TLS or proxy configuration"
        );
        if output.code == 0 {
            let value = std::str::from_utf8(&output.stdout)
                .context("Invalid clone TLS or proxy configuration")?
                .strip_suffix('\n')
                .context("Invalid clone TLS or proxy configuration")?;
            ensure!(
                value.len() <= 16384 && !value.contains(['\n', '\r', '\0']),
                "Unsupported clone TLS or proxy configuration"
            );
            if key == "http.sslVerify" {
                ensure!(
                    ["true", "yes", "on", "1"]
                        .iter()
                        .any(|allowed| value.eq_ignore_ascii_case(allowed)),
                    "Selected-account clone requires HTTPS certificate verification"
                );
            }
            config.push((key.into(), value.into()));
        }
    }
    if default_branch.is_none() {
        let output = crate::process::run(
            path,
            &crate::args(&["config", "--get", "init.defaultBranch"]),
        )
        .await?;
        ensure!(
            output.code == 0 || (output.code == 1 && output.stdout.is_empty()),
            "Cannot inspect default clone branch"
        );
        if output.code == 0 {
            let value = std::str::from_utf8(&output.stdout)
                .context("Invalid default clone branch")?
                .strip_suffix('\n')
                .context("Invalid default clone branch")?;
            ensure!(
                !value.is_empty()
                    && value.len() <= 4096
                    && !value.starts_with('-')
                    && !value.chars().any(char::is_control),
                "Invalid default clone branch"
            );
            config.push(("init.defaultBranch".into(), value.into()));
        }
    }
    for (variable, key) in [
        ("GIT_SSL_CAINFO", "http.sslCAInfo"),
        ("GIT_SSL_CAPATH", "http.sslCAPath"),
        ("GIT_SSL_CERT", "http.sslCert"),
        ("GIT_SSL_KEY", "http.sslKey"),
        ("GIT_SSL_VERSION", "http.sslVersion"),
    ] {
        if let Some(value) = std::env::var_os(variable) {
            let value = value
                .into_string()
                .map_err(|_| anyhow::anyhow!("Unsupported clone TLS environment"))?;
            ensure!(
                value.len() <= 16384 && !value.contains(['\n', '\r', '\0']),
                "Unsupported clone TLS environment"
            );
            config.push((key.into(), value));
        }
    }
    ensure!(
        std::env::var_os("GIT_SSL_NO_VERIFY").is_none(),
        "Clone cannot use GIT_SSL_NO_VERIFY with an assigned account"
    );
    config.extend([
        ("core.hooksPath".into(), "/dev/null".into()),
        ("init.templateDir".into(), String::new()),
        ("http.followRedirects".into(), "false".into()),
    ]);
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .context("Cannot start clone credential broker")?;
    let address = listener.local_addr()?.to_string();
    #[cfg(not(test))]
    let helper = std::env::current_exe()?;
    #[cfg(test)]
    let helper = match std::env::var_os("DESKTOP_TEST_CLONE_HELPER") {
        Some(path) => PathBuf::from(path),
        None => std::env::current_exe()?,
    };
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let temporary_handle = create_broker_home_before_create(container_handle, container, || Ok(()))?;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let command_directory = crate::registry::pinned_clone_directory(&temporary_handle, Path::new("/proc"))?;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let child_home = {
        use std::os::fd::AsRawFd;
        PathBuf::from(format!("/proc/self/fd/{}", temporary_handle.as_raw_fd()))
    };
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let command_directory = {
        let _ = container_handle;
        let temporary = container.join("broker-home");
        std::fs::DirBuilder::new().mode(0o700).create(&temporary).context("Cannot create private clone directory")?;
        temporary
    };
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let child_home = command_directory.clone();
    let secrets = [
        "https_proxy",
        "HTTPS_PROXY",
        "http_proxy",
        "HTTP_PROXY",
        "all_proxy",
        "ALL_PROXY",
    ]
    .iter()
    .filter_map(|key| std::env::var(key).ok())
    .filter(|value| !value.is_empty())
    .map(String::into_bytes)
    .collect();
    let lease = Arc::new(CloneLease {
        endpoint,
        login: account.login.to_owned(),
        id,
        account_path: path.to_owned(),
        session: Arc::new(session),
        config,
        secrets,
        address,
        capability: uuid::Uuid::new_v4().to_string(),
        helper,
        directory: child_home,
        cancellation: Cancellation::default(),
    });
    let broker_lease = lease.clone();
    let parent = Cancellation::current();
    let broker = tokio::spawn(async move {
        serve(listener, broker_lease, parent).await;
    });
    let guard = Guard {
        lease: lease.clone(),
        broker: Some(broker),
    };
    let mut args = crate::args(&["clone", "--no-checkout", "--no-recurse-submodules"]);
    if progress {
        args.push("--progress".into());
    }
    if let Some(branch) = branch {
        args.push(format!("--branch={branch}").into());
    }
    if let Some(default_branch) = default_branch {
        args.splice(
            0..0,
            crate::args(&["-c", &format!("init.defaultBranch={default_branch}")]),
        );
    }
    args.extend(crate::args(&["--", url]));
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::fd::AsRawFd;
        args.push(format!("/proc/self/fd/{}", destination_handle.as_raw_fd()).into());
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    args.push(destination.as_os_str().to_owned());
    let output = CURRENT
        .scope(
            lease.clone(),
            async {
                #[cfg(any(target_os = "linux", target_os = "android"))]
                { crate::process::run_clone_with_inherited_fd(&command_directory, &args, progress, destination_handle, Some(&temporary_handle)).await }
                #[cfg(not(any(target_os = "linux", target_os = "android")))]
                { let _ = destination_handle; crate::process::run_clone(&command_directory, &args, progress).await }
            },
        )
        .await;
    lease.cancellation.cancel();
    drop(guard);
    let output = output?;
    if output.code != 0 {
        anyhow::bail!("{}", crate::registry::clone_failure_message(&output));
    }
    let current = Session::open_for_account(path, &identity, Some(account))
        .await?
        .map_err(|_| anyhow::anyhow!(REFUSED))?;
    let original = lease.session.redaction_secrets();
    let refreshed = current.redaction_secrets();
    ensure!(
        original
            .first()
            .zip(refreshed.first())
            .is_some_and(|(a, b)| constant_time_equal(a, b)),
        "Selected account changed during clone; inspect the destination before retrying"
    );
    Cancellation::current().check()?;
    Ok(())
}

async fn serve(listener: TcpListener, lease: Arc<CloneLease>, parent: Cancellation) {
    let mut requests = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = parent.cancelled() => break,
            _ = lease.cancellation.cancelled() => break,
            Some(_) = requests.join_next(), if !requests.is_empty() => {},
            accepted = listener.accept(), if requests.len() < 16 => {
                let Ok((mut stream, peer)) = accepted else { break; };
                if !peer.ip().is_loopback() { continue; }
                let lease = lease.clone();
                requests.spawn(async move {
                    let _ = tokio::time::timeout(Duration::from_secs(30), async {
                        let bytes = read_frame(&mut stream).await?;
                        let request: Request = serde_json::from_slice(&bytes)?;
                        ensure!(constant_time_equal(request.capability.as_bytes(), lease.capability.as_bytes()), "Invalid clone capability");
                        let reply = match lease.respond(request).await { Ok(reply) => reply, Err(_) => Reply::refused(REFUSED) };
                        write_frame(&mut stream, &serde_json::to_vec(&reply)?).await
                    }).await;
                });
            }
        }
    }
    requests.abort_all();
    while requests.join_next().await.is_some() {}
}

impl CloneLease {
    async fn respond(&self, request: Request) -> Result<Reply> {
        let operation = protocol::Operation::parse(&request.operation)?;
        let input = STANDARD.decode(request.input)?;
        let context = protocol::CredentialContext::parse(&input)?;
        if operation != protocol::Operation::Get {
            return Ok(Reply::output(&[]));
        }
        self.cancellation.check()?;
        let endpoint = Endpoint::from_credential(&context)?;
        ensure!(
            endpoint.kind == endpoint::EndpointKind::Git
                && self.endpoint.same_repository(&endpoint),
            "Unintended clone credential destination"
        );
        let host = self.endpoint.origin.authority();
        let current = Session::open_for_account(
            &self.account_path,
            &endpoint.identity(),
            Some(ChosenAccount {
                host: &host,
                login: &self.login,
                id: Some(self.id),
            }),
        )
        .await?
        .map_err(|_| anyhow::anyhow!(REFUSED))?;
        let original = self.session.redaction_secrets();
        let refreshed = current.redaction_secrets();
        ensure!(
            original
                .first()
                .zip(refreshed.first())
                .is_some_and(|(a, b)| constant_time_equal(a, b)),
            "Selected account changed during clone"
        );
        self.cancellation.check()?;
        Ok(Reply::output(
            &self.session.git_credential_response(&self.login)?,
        ))
    }
}

pub(super) fn apply(command: &mut Command) -> bool {
    CURRENT
        .try_with(|lease| {
            for (key, _) in std::env::vars_os() {
                if key.to_string_lossy().starts_with("GIT_") {
                    command.env_remove(key);
                }
            }
            for key in [
                "GH_TOKEN",
                "GITHUB_TOKEN",
                "GH_ENTERPRISE_TOKEN",
                "GITHUB_ENTERPRISE_TOKEN",
                "GH_DEBUG",
            ] {
                command.env_remove(key);
            }
            command
                .env("HOME", &lease.directory)
                .env("XDG_CONFIG_HOME", &lease.directory)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CEILING_DIRECTORIES", &lease.directory)
                .env("GIT_ATTR_NOSYSTEM", "1")
                .env("GIT_ALLOW_PROTOCOL", "https")
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GIT_CONFIG_COUNT", lease.config.len().to_string())
                .env(ADDRESS, &lease.address)
                .env(CAPABILITY, &lease.capability)
                .env(HELPER_EXE, &lease.helper);
            for (index, (key, value)) in lease.config.iter().enumerate() {
                command
                    .env(format!("GIT_CONFIG_KEY_{index}"), key)
                    .env(format!("GIT_CONFIG_VALUE_{index}"), value);
            }
        })
        .is_ok()
}
pub(crate) fn active() -> bool {
    CURRENT.try_with(|_| ()).is_ok()
}
pub(super) fn parameters() -> Option<OsString> {
    CURRENT.try_with(|_| OsString::new()).ok()
}
pub(super) fn secrets() -> Option<Vec<Vec<u8>>> {
    CURRENT
        .try_with(|lease| {
            let mut values = lease.session.redaction_secrets();
            values.push(lease.capability.as_bytes().to_vec());
            values.extend(lease.secrets.iter().cloned());
            values.extend(
                lease
                    .config
                    .iter()
                    .filter(|(_, value)| !value.is_empty())
                    .map(|(_, value)| value.as_bytes().to_vec()),
            );
            values
        })
        .ok()
}

#[cfg(all(test, any(target_os = "linux", target_os = "android")))]
mod tests {
    use super::*;

    /// Run only from the offline HTTPS fixture; public clone remains disabled.
    #[tokio::test]
    #[ignore = "requires the offline selected-account HTTPS fixture"]
    async fn child_fd_authenticated_clone_fixture() {
        if let Some(path) = std::env::var_os("DESKTOP_TEST_CLONE_PID_FILE") {
            std::fs::write(path, std::process::id().to_string()).unwrap();
        }
        let stage = PathBuf::from(std::env::var_os("DESKTOP_TEST_CLONE_STAGE").expect("fixture stage"));
        let url = std::env::var("DESKTOP_TEST_CLONE_URL").expect("fixture URL");
        let host = std::env::var("DESKTOP_TEST_CLONE_HOST").expect("fixture host");
        let handle = std::fs::File::open(&stage).unwrap();
        let account = ChosenAccount { host: &host, login: "AliceCase", id: Some(101) };
        let container = stage.parent().unwrap();
        let container_handle = std::fs::File::open(container).unwrap();
        clone_no_checkout(container, &url, &stage, &handle, &container_handle, container, None, Some("main"), account, false).await.unwrap();
        assert!(stage.join(".git").is_dir());
    }
}
