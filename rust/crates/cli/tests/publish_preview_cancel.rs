//! Cancellation during the final publication availability check.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    registry: PathBuf,
    bin: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("desktop-publish-cancel-{}", uuid::Uuid::new_v4()));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let repo = root.join("preview cancel");
        let registry = root.join("accounts/repositories.json");
        let f = Self {
            root,
            repo,
            registry,
            bin,
        };
        f.run(
            "git",
            &["init", "-q", "-b", "main", f.repo.to_str().unwrap()],
        );
        f.git(&["config", "user.name", "Preview Cancel"]);
        f.git(&["config", "user.email", "preview@example.invalid"]);
        f.git(&["config", "commit.gpgsign", "false"]);
        fs::write(f.repo.join("file"), "base\n").unwrap();
        f.git(&["add", "--all"]);
        f.git(&["commit", "-qm", "base"]);
        f.run(
            env!("CARGO_BIN_EXE_desktop-rs"),
            &[
                "--config",
                f.registry.to_str().unwrap(),
                "repositories",
                "add",
                f.repo.to_str().unwrap(),
            ],
        );
        f
    }
    fn command(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        let mut paths = vec![self.bin.clone()];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        c.env("HOME", &self.root)
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("global"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GH_CONFIG_DIR", self.root.join("gh-config"))
            .env("GH_TOKEN", "synthetic-secret")
            .env("GITHUB_TOKEN", "synthetic-secret")
            .env("GH_ENTERPRISE_TOKEN", "synthetic-secret")
            .env("GITHUB_ENTERPRISE_TOKEN", "synthetic-secret")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GH_HOST")
            .env_remove("GH_REPO");
        c
    }
    fn run(&self, program: &str, args: &[&str]) -> Vec<u8> {
        let o = self.command(program).args(args).output().unwrap();
        assert!(
            o.status.success(),
            "{program} {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        o.stdout
    }
    fn git(&self, args: &[&str]) -> Vec<u8> {
        let mut full = vec!["-C", self.repo.to_str().unwrap()];
        full.extend(args);
        self.run("git", &full)
    }
    fn install_stub(&self) {
        fs::write(self.root.join("orgs.json"), "[]").unwrap();
        fs::write(self.root.join("orgs-page2.json"), "[]").unwrap();
        fs::write(self.root.join("catalog.json"), r#"{"hosts":{"github.com":[{"host":"github.com","login":"AliceCase","active":true,"state":"success","gitProtocol":"https"}]}}"#).unwrap();
        fs::write(
            self.root.join("user.json"),
            r#"{"id":303,"login":"environment-user"}"#,
        )
        .unwrap();
        fs::write(
            self.root.join("assigned-user.json"),
            r#"{"id":101,"login":"alicecase"}"#,
        )
        .unwrap();
        executable(
            &self.bin.join("gh"),
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$GH_LOG\"\ncase \"$*\" in\n *'--method POST'*) [ \"$GH_TOKEN\" = \"$GITHUB_TOKEN\" ] && [ \"$GH_TOKEN\" = \"$GH_ENTERPRISE_TOKEN\" ] && [ \"$GH_TOKEN\" = \"$GITHUB_ENTERPRISE_TOKEN\" ] || exit 93; cat > \"$GH_BODY\"; printf 'POST\\n' >> \"$GH_POST_LOG\"; printf started > \"$GH_POST_STARTED\"; if [ -e \"$GH_POST_GATE\" ]; then while [ ! -e \"$GH_POST_RELEASE\" ]; do sleep 0.02; done; fi; if [ -e \"$GH_POST_FAIL\" ]; then printf '%s' \"$GH_TOKEN\" >&2; exit 1; fi; cat \"$GH_REPOSITORY\";;\n *'auth status'*) cat \"$GH_CATALOG\";;\n *'auth token'*'--user'*) printf 'assigned-secret\\n';;\n *'auth token'*) printf 'synthetic-secret\\n';;\n *'/user/orgs?'*) if [ -e \"$GH_FINAL_GATE\" ] && [ \"$(grep -c '/user/orgs?' \"$GH_LOG\")\" = 3 ]; then printf started > \"$GH_FINAL_STARTED\"; while [ ! -e \"$GH_POST_RELEASE\" ]; do sleep 0.02; done; fi; [ \"$GH_TOKEN\" = \"$GITHUB_TOKEN\" ] && [ \"$GH_TOKEN\" = \"$GH_ENTERPRISE_TOKEN\" ] && [ \"$GH_TOKEN\" = \"$GITHUB_ENTERPRISE_TOKEN\" ] || exit 93; if [ -e \"$GH_FAIL\" ]; then printf '%s' \"$GH_TOKEN\" >&2; exit 1; fi; if [ -e \"$GH_DELAY\" ]; then printf started > \"$GH_STARTED\"; while [ ! -e \"$GH_RELEASE\" ]; do sleep 0.02; done; fi; case \"$*\" in *'&page=1') cat \"$GH_ORGS\";; *) for arg do endpoint=$arg; done; page=${endpoint##*page=}; if [ -e \"$GH_PAGES/page-$page.json\" ]; then cat \"$GH_PAGES/page-$page.json\"; elif [ -e \"$GH_REPEAT\" ]; then cat \"$GH_ORGS\"; else cat \"$GH_ORGS2\"; fi;; esac;;\n *'/repos/'*'/example-repo'*) if [ -e \"$GH_LOCK_GATE\" ] && [ -e \"$GH_CONFIG_LOCK\" ]; then printf started > \"$GH_STARTED\"; while [ ! -e \"$GH_RELEASE\" ]; do sleep 0.02; done; fi; if [ -e \"$GH_REPO_DELAY\" ]; then printf started > \"$GH_STARTED\"; while [ ! -e \"$GH_RELEASE\" ]; do sleep 0.02; done; fi; cat \"$GH_REPOSITORY\";;\n *'/repos/fork/repo'*) printf '{\"id\":2001,\"name\":\"repo\",\"full_name\":\"fork/repo\",\"html_url\":\"https://github.com/fork/repo\",\"owner\":{\"login\":\"fork\"},\"private\":true,\"fork\":false,\"parent\":null}\\n';;\n *'/user'*) if [ \"$GH_TOKEN\" = assigned-secret ]; then cat \"$GH_ASSIGNED_USER\"; else cat \"$GH_USER\"; fi;;\n *) printf 'unexpected gh command\\n' >&2; exit 42;;\nesac\n",
        );
    }
    fn request(&self) -> Value {
        json!({"method":"publish_preview","request":{"host":"github.com","target":{"mode":"effective"},"owner":"environment-user","name":"example-repo","description":" exact description\n","private":true}})
    }
    fn preview_command(&self) -> Command {
        let mut c = self.command(env!("CARGO_BIN_EXE_desktop-rs"));
        c.args([
            "--config",
            self.registry.to_str().unwrap(),
            "-C",
            self.repo.to_str().unwrap(),
            "repo",
            "-",
        ])
        .env("GH_LOG", self.root.join("gh-log"))
        .env("GH_ORGS", self.root.join("orgs.json"))
        .env("GH_ORGS2", self.root.join("orgs-page2.json"))
        .env("GH_REPEAT", self.root.join("gh-repeat"))
        .env("GH_PAGES", self.root.join("pages"))
        .env("GH_CATALOG", self.root.join("catalog.json"))
        .env("GH_USER", self.root.join("user.json"))
        .env("GH_ASSIGNED_USER", self.root.join("assigned-user.json"))
        .env("GH_DELAY", self.root.join("gh-delay"))
        .env("GH_STARTED", self.root.join("gh-started"))
        .env("GH_RELEASE", self.root.join("gh-release"))
        .env("GH_FAIL", self.root.join("gh-fail"))
        .env("GH_POST_LOG", self.root.join("post-log"))
        .env("GH_BODY", self.root.join("post-body"))
        .env("GH_POST_STARTED", self.root.join("post-started"))
        .env("GH_POST_GATE", self.root.join("post-gate"))
        .env("GH_FINAL_GATE", self.root.join("final-gate"))
        .env("GH_FINAL_STARTED", self.root.join("final-started"))
        .env("GH_POST_RELEASE", self.root.join("post-release"))
        .env("GH_POST_FAIL", self.root.join("post-fail"))
        .env("GH_REPOSITORY", self.root.join("repository-response.json"))
        .env("GH_REPO_DELAY", self.root.join("repo-delay"))
        .env("GH_LOCK_GATE", self.root.join("lock-gate"))
        .env("GH_CONFIG_LOCK", self.repo.join(".git/config.lock"));
        c
    }
    fn snapshot(&self) -> [Vec<u8>; 5] {
        [
            fs::read(self.repo.join(".git/config")).unwrap(),
            fs::read(self.repo.join(".git/HEAD")).unwrap(),
            self.git(&["for-each-ref", "--format=%(refname) %(objectname)"]),
            fs::read(self.repo.join(".git/index")).unwrap_or_default(),
            fs::read(&self.registry).unwrap(),
        ]
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn executable(path: &Path, source: &str) {
    fs::write(path, source).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn wait_for(path: &Path, child: &mut Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !path.exists() && Instant::now() < deadline {
        assert!(
            child.try_wait().unwrap().is_none(),
            "child exited before {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert!(path.exists(), "timed out waiting for {}", path.display());
}

#[test]
fn cancellation_while_waiting_for_registry_lock_is_read_only() {
    let f = Fixture::new();
    f.install_stub();
    let before = f.snapshot();
    // Prove the stub exercises a successful review before introducing contention.
    let mut baseline = f
        .preview_command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    baseline
        .stdin
        .take()
        .unwrap()
        .write_all(f.request().to_string().as_bytes())
        .unwrap();
    let baseline = baseline.wait_with_output().unwrap();
    assert!(
        baseline.status.success(),
        "{}",
        String::from_utf8_lossy(&baseline.stderr)
    );
    assert!(serde_json::from_slice::<Value>(&baseline.stdout).unwrap()["basis"].is_string());
    assert_eq!(f.snapshot(), before);
    let real_git = String::from_utf8(f.run("sh", &["-c", "command -v git"]))
        .unwrap()
        .trim()
        .to_owned();
    let marker = f.root.join("availability");
    let count = f.root.join("count");
    executable(
        &f.bin.join("git"),
        "#!/bin/sh\n\"$REAL_GIT\" \"$@\"\nstatus=$?\nfor arg do\n if [ \"$arg\" = --absolute-git-dir ]; then\n  n=0; [ ! -f \"$COUNT\" ] || n=$(cat \"$COUNT\")\n  n=$((n+1)); printf '%s' \"$n\" > \"$COUNT\"\n  [ \"$n\" != 4 ] || printf ready > \"$MARKER\"\n fi\ndone\nexit \"$status\"\n",
    );
    let locked = f.root.join("lock-ready");
    let mut guard = f
        .command("flock")
        .args([
            "-x",
            f.registry.with_extension("lock").to_str().unwrap(),
            "sh",
            "-c",
            "printf ready > \"$LOCK_READY\"; exec cat",
        ])
        .env("LOCK_READY", &locked)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for(&locked, &mut guard, Duration::from_secs(5));
    let mut child = f
        .preview_command()
        .env("REAL_GIT", real_git)
        .env("COUNT", count)
        .env("MARKER", &marker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(f.request().to_string().as_bytes())
        .unwrap();
    wait_for(&marker, &mut child, Duration::from_secs(45));
    thread::sleep(Duration::from_millis(300));
    assert!(child.try_wait().unwrap().is_none());
    f.run("kill", &["-INT", &child.id().to_string()]);
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    if child.try_wait().unwrap().is_none() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("preview did not cancel");
    }
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(!child.wait().unwrap().success());
    assert!(stdout.trim().is_empty());
    assert!(stderr.to_ascii_lowercase().contains("cancel"), "{stderr}");
    for secret in ["synthetic-secret", "c3ludGhldGljLXNlY3JldA=="] {
        assert!(!stdout.contains(secret));
        assert!(!stderr.contains(secret));
    }
    drop(guard.stdin.take());
    assert!(guard.wait().unwrap().success());
    fs::remove_file(f.bin.join("git")).unwrap();
    assert_eq!(f.snapshot(), before);
    assert!(
        !fs::read_to_string(f.root.join("gh-log"))
            .unwrap()
            .contains("POST")
    );
}

fn preview(f: &Fixture, request: Value, fail: bool) -> Value {
    let mut child = f
        .preview_command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.success(),
        !fail,
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for secret in [
        "synthetic-secret",
        "assigned-secret",
        "YXNzaWduZWQtc2VjcmV0",
    ] {
        assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
        assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    }
    if fail {
        assert!(output.stdout.is_empty());
        Value::Null
    } else {
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

#[test]
fn preview_binds_owner_request_git_configuration_and_organization_identity() {
    let f = Fixture::new();
    f.install_stub();
    fs::write(
        f.root.join("orgs.json"),
        r#"[{"login":"ExampleOrg","id":987}]"#,
    )
    .unwrap();
    let before = f.snapshot();
    let request = f.request();
    let personal = preview(&f, request.clone(), false);
    assert_eq!(
        personal["actor"],
        json!({"login":"environment-user","id":303})
    );
    assert_eq!(
        personal["owner"],
        json!({"login":"environment-user","id":303,"kind":"personal"})
    );
    assert_eq!(personal["push_status"], "planned");
    assert_eq!(personal["pushes"].as_array().unwrap().len(), 1);
    assert_eq!(personal["pushes"][0]["branch"], "main");
    assert_eq!(
        personal["pushes"][0]["oid"],
        String::from_utf8(f.git(&["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
    );
    assert_eq!(f.snapshot(), before);
    let mut org_request = request.clone();
    org_request["request"]["owner"] = json!("ExampleOrg");
    let organization = preview(&f, org_request.clone(), false);
    assert_eq!(
        organization["owner"],
        json!({"login":"ExampleOrg","id":987,"kind":"organization"})
    );
    assert_ne!(organization["basis"], personal["basis"]);
    assert_eq!(
        preview(&f, org_request.clone(), false)["basis"],
        organization["basis"]
    );
    for (key, value) in [
        ("name", json!("other-repo")),
        ("private", json!(false)),
        ("description", json!("exact description")),
    ] {
        let mut changed = org_request.clone();
        changed["request"][key] = value;
        assert_ne!(preview(&f, changed, false)["basis"], organization["basis"]);
    }
    let mut missing = org_request.clone();
    missing["request"]["owner"] = json!("MissingOrg");
    preview(&f, missing, true);
    assert_eq!(f.snapshot(), before);
    let first = organization["basis"].clone();
    f.git(&["config", "branch.main.merge", "refs/heads/alternate"]);
    let after_config = f.snapshot();
    let second = preview(&f, org_request.clone(), false)["basis"].clone();
    assert_ne!(first, second);
    assert_eq!(f.snapshot(), after_config);
    f.git(&[
        "remote",
        "add",
        "upstream",
        "https://github.com/fork/repo.git",
    ]);
    let after_remote = f.snapshot();
    let third = preview(&f, org_request.clone(), false)["basis"].clone();
    assert_ne!(second, third);
    assert_eq!(f.snapshot(), after_remote);
    fs::write(f.repo.join("file"), "second commit\n").unwrap();
    f.git(&["add", "--all"]);
    f.git(&["commit", "-qm", "second"]);
    let after_head = f.snapshot();
    let fourth = preview(&f, org_request.clone(), false)["basis"].clone();
    assert_ne!(third, fourth);
    assert_eq!(f.snapshot(), after_head);
    fs::write(
        f.root.join("orgs.json"),
        r#"[{"login":"ExampleOrg","id":988}]"#,
    )
    .unwrap();
    assert_ne!(preview(&f, org_request, false)["basis"], fourth);
    assert_eq!(f.snapshot(), after_head);
    let log = fs::read_to_string(f.root.join("gh-log")).unwrap();
    for method in ["POST", "PATCH", "PUT", "DELETE", "--input"] {
        assert!(!log.contains(method));
    }
}

#[test]
fn preview_rejects_invalid_local_state_and_provider_identifiers() {
    let f = Fixture::new();
    f.install_stub();
    let request = f.request();
    let before = f.snapshot();
    let mut no_account = request.clone();
    no_account["request"]["target"] = json!({"mode":"none"});
    preview(&f, no_account, true);
    assert_eq!(f.snapshot(), before);
    fs::write(
        f.root.join("user.json"),
        r#"{"id":9007199254740992,"login":"environment-user"}"#,
    )
    .unwrap();
    preview(&f, request.clone(), true);
    assert_eq!(f.snapshot(), before);
    fs::write(
        f.root.join("user.json"),
        r#"{"id":303,"login":"environment-user"}"#,
    )
    .unwrap();
    fs::write(
        f.root.join("orgs.json"),
        r#"[{"login":"ExampleOrg","id":9007199254740992}]"#,
    )
    .unwrap();
    let mut org = request.clone();
    org["request"]["owner"] = json!("ExampleOrg");
    preview(&f, org, true);
    assert_eq!(f.snapshot(), before);
    f.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/fork/repo.git",
    ]);
    let with_origin = f.snapshot();
    preview(&f, request.clone(), true);
    assert_eq!(f.snapshot(), with_origin);
    f.run(
        env!("CARGO_BIN_EXE_desktop-rs"),
        &[
            "--config",
            f.registry.to_str().unwrap(),
            "repositories",
            "forget",
            f.repo.to_str().unwrap(),
        ],
    );
    f.git(&["remote", "remove", "origin"]);
    let unsaved = f.snapshot();
    preview(&f, request, true);
    assert_eq!(f.snapshot(), unsaved);
}

#[test]
fn preview_refuses_concurrent_repository_and_registry_changes() {
    for kind in ["remote", "config", "head", "saved-record", "account"] {
        let f = Fixture::new();
        f.install_stub();
        fs::write(
            f.root.join("orgs.json"),
            r#"[{"login":"ExampleOrg","id":987}]"#,
        )
        .unwrap();
        fs::write(f.root.join("gh-delay"), "on").unwrap();
        let mut request = f.request();
        request["request"]["owner"] = json!("ExampleOrg");
        let mut child = f
            .preview_command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.to_string().as_bytes())
            .unwrap();
        wait_for(
            &f.root.join("gh-started"),
            &mut child,
            Duration::from_secs(15),
        );
        match kind {
            "remote" => {
                f.git(&[
                    "remote",
                    "add",
                    "upstream",
                    "https://github.com/fork/repo.git",
                ]);
            }
            "config" => {
                f.git(&["config", "branch.main.merge", "refs/heads/changed"]);
            }
            "head" => {
                f.git(&["switch", "-qc", "other"]);
            }
            "account" => {
                let target = json!({"mode":"none"});
                let plan = preview(
                    &f,
                    json!({"method":"repository_account_preview","target":target}),
                    false,
                );
                assert_eq!(plan["can_apply"], true);
                preview(
                    &f,
                    json!({"method":"set_repository_account","target":target,"basis":plan["basis"]}),
                    false,
                );
            }
            _ => {
                let mut registry: Value =
                    serde_json::from_slice(&fs::read(&f.registry).unwrap()).unwrap();
                registry["records"] = json!([]);
                fs::write(&f.registry, registry.to_string()).unwrap();
            }
        }
        let after = f.snapshot();
        fs::write(f.root.join("gh-release"), "release").unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        if child.try_wait().unwrap().is_none() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("preview race did not finish");
        }
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success(), "{kind}");
        assert!(output.stdout.is_empty(), "{kind}");
        for secret in [
            "synthetic-secret",
            "assigned-secret",
            "YXNzaWduZWQtc2VjcmV0",
        ] {
            assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
            assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
        }
        assert_eq!(f.snapshot(), after, "{kind}");
    }
}

#[test]
fn preview_plans_unborn_detached_and_tracking_branches_read_only() {
    let unborn = Fixture::new();
    unborn.install_stub();
    unborn.git(&["update-ref", "-d", "refs/heads/main"]);
    let before = unborn.snapshot();
    let value = preview(&unborn, unborn.request(), false);
    assert_eq!(value["push_status"], "unborn");
    assert_eq!(value["pushes"], json!([]));
    assert_eq!(unborn.snapshot(), before);
    let detached = Fixture::new();
    detached.install_stub();
    detached.git(&["switch", "--detach", "-q"]);
    let before = detached.snapshot();
    let value = preview(&detached, detached.request(), false);
    assert_eq!(value["push_status"], "detached");
    assert_eq!(value["pushes"], json!([]));
    assert_eq!(detached.snapshot(), before);
    let current = Fixture::new();
    current.install_stub();
    current.git(&["switch", "-qc", "feature"]);
    fs::write(current.repo.join("file"), "feature commit\n").unwrap();
    current.git(&["add", "--all"]);
    current.git(&["commit", "-qm", "feature"]);
    let before = current.snapshot();
    let value = preview(&current, current.request(), false);
    let pushes = value["pushes"].as_array().unwrap();
    assert_eq!(pushes.last().unwrap()["branch"], "feature");
    assert_eq!(
        pushes.last().unwrap()["oid"],
        String::from_utf8(current.git(&["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
    );
    assert_eq!(current.snapshot(), before);
    let tracked = Fixture::new();
    tracked.install_stub();
    tracked.git(&["branch", "tracked"]);
    tracked.git(&[
        "remote",
        "add",
        "upstream",
        "https://github.com/fork/repo.git",
    ]);
    let tip = String::from_utf8(tracked.git(&["rev-parse", "tracked"])).unwrap();
    tracked.git(&["update-ref", "refs/remotes/upstream/stable", tip.trim()]);
    tracked.git(&[
        "symbolic-ref",
        "refs/remotes/upstream/HEAD",
        "refs/remotes/upstream/stable",
    ]);
    tracked.git(&["config", "branch.tracked.remote", "upstream"]);
    tracked.git(&["config", "branch.tracked.merge", "refs/heads/stable"]);
    let before = tracked.snapshot();
    let value = preview(&tracked, tracked.request(), false);
    assert_eq!(value["push_status"], "planned");
    assert_eq!(
        value["pushes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| (
                item["branch"].as_str().unwrap(),
                item["role"].as_str().unwrap()
            ))
            .collect::<Vec<_>>(),
        vec![("tracked", "default"), ("main", "current")]
    );
    assert_eq!(value["pushes"][0]["oid"], tip.trim());
    assert_eq!(tracked.snapshot(), before);
}

fn publish_cli(f: &Fixture, options: &[&str]) -> Value {
    publish_operation_cli(f, "preview", options)
}
fn publish_command(f: &Fixture, operation: &str, options: &[&str]) -> Command {
    let mut command = f.command(env!("CARGO_BIN_EXE_desktop-rs"));
    command
        .args([
            "--config",
            f.registry.to_str().unwrap(),
            "-C",
            f.repo.to_str().unwrap(),
            "publish",
            operation,
        ])
        .args(options)
        .env("GH_LOG", f.root.join("gh-log"))
        .env("GH_ORGS", f.root.join("orgs.json"))
        .env("GH_ORGS2", f.root.join("orgs-page2.json"))
        .env("GH_REPEAT", f.root.join("gh-repeat"))
        .env("GH_PAGES", f.root.join("pages"))
        .env("GH_CATALOG", f.root.join("catalog.json"))
        .env("GH_USER", f.root.join("user.json"))
        .env("GH_ASSIGNED_USER", f.root.join("assigned-user.json"))
        .env("GH_DELAY", f.root.join("gh-delay"))
        .env("GH_STARTED", f.root.join("gh-started"))
        .env("GH_RELEASE", f.root.join("gh-release"))
        .env("GH_FAIL", f.root.join("gh-fail"))
        .env("GH_POST_LOG", f.root.join("post-log"))
        .env("GH_BODY", f.root.join("post-body"))
        .env("GH_POST_STARTED", f.root.join("post-started"))
        .env("GH_POST_GATE", f.root.join("post-gate"))
        .env("GH_FINAL_GATE", f.root.join("final-gate"))
        .env("GH_FINAL_STARTED", f.root.join("final-started"))
        .env("GH_POST_RELEASE", f.root.join("post-release"))
        .env("GH_POST_FAIL", f.root.join("post-fail"))
        .env("GH_REPOSITORY", f.root.join("repository-response.json"))
        .env("GH_REPO_DELAY", f.root.join("repo-delay"))
        .env("GH_LOCK_GATE", f.root.join("lock-gate"))
        .env("GH_CONFIG_LOCK", f.repo.join(".git/config.lock"));
    command
}
fn publish_operation_cli(f: &Fixture, operation: &str, options: &[&str]) -> Value {
    let out = publish_command(f, operation, options).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn assigned_and_effective_accounts_have_cli_parity_and_are_pinned() {
    let f = Fixture::new();
    f.install_stub();
    fs::write(
        f.root.join("orgs.json"),
        r#"[{"login":"ExampleOrg","id":987}]"#,
    )
    .unwrap();
    let before = f.snapshot();
    let mut request = f.request();
    request["request"]["target"] =
        json!({"mode":"assigned","host":"github.com","login":"AliceCase"});
    request["request"]["owner"] = json!("alicecase");
    let personal = preview(&f, request.clone(), false);
    assert_eq!(personal["actor"], json!({"login":"alicecase","id":101}));
    assert_eq!(
        personal["owner"],
        json!({"login":"alicecase","id":101,"kind":"personal"})
    );
    assert_eq!(
        publish_cli(
            &f,
            &[
                "--host",
                "github.com",
                "--login",
                "AliceCase",
                "--owner",
                "alicecase",
                "--name",
                "example-repo",
                "--description",
                " exact description\n"
            ]
        ),
        personal
    );
    request["request"]["owner"] = json!("ExampleOrg");
    let org = preview(&f, request.clone(), false);
    assert_eq!(
        org["owner"],
        json!({"login":"ExampleOrg","id":987,"kind":"organization"})
    );
    let mut effective = f.request();
    effective["request"]["owner"] = json!("ExampleOrg");
    let effective_value = preview(&f, effective.clone(), false);
    assert_eq!(
        effective_value["actor"],
        json!({"login":"environment-user","id":303})
    );
    assert_eq!(
        publish_cli(
            &f,
            &[
                "--host",
                "github.com",
                "--effective",
                "--owner",
                "ExampleOrg",
                "--name",
                "example-repo",
                "--description",
                " exact description\n"
            ]
        ),
        effective_value
    );
    effective["request"]["private"] = json!(false);
    assert_eq!(
        publish_cli(
            &f,
            &[
                "--host",
                "github.com",
                "--effective",
                "--owner",
                "ExampleOrg",
                "--name",
                "example-repo",
                "--description",
                " exact description\n",
                "--public"
            ]
        ),
        preview(&f, effective, false)
    );
    assert_eq!(f.snapshot(), before);
    let log = fs::read_to_string(f.root.join("gh-log")).unwrap();
    assert!(log.contains("auth token --hostname github.com --user AliceCase --no-refresh"));
    for method in ["POST", "PATCH", "PUT", "DELETE", "--input"] {
        assert!(!log.contains(method));
    }
}

#[test]
fn reflected_credentials_and_provider_errors_are_not_disclosed() {
    let f = Fixture::new();
    f.install_stub();
    fs::write(
        f.root.join("orgs.json"),
        r#"[{"login":"assigned-secret","id":987}]"#,
    )
    .unwrap();
    let before = f.snapshot();
    let mut request = f.request();
    request["request"]["target"] =
        json!({"mode":"assigned","host":"github.com","login":"AliceCase"});
    request["request"]["owner"] = json!("assigned-secret");
    preview(&f, request.clone(), true);
    assert_eq!(f.snapshot(), before);
    fs::write(f.root.join("gh-fail"), "on").unwrap();
    preview(&f, request, true);
    assert_eq!(f.snapshot(), before);
}

#[test]
fn publish_context_reads_description_and_catalog_safely() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.install_stub();
    let before = f.snapshot();
    let context = preview(
        &f,
        json!({"method":"publish_context","host":"github.com"}),
        false,
    );
    assert_eq!(context["saved"], true);
    assert_eq!(context["origin_exists"], false);
    assert_eq!(context["private"], true);
    assert_eq!(context["name"], "preview cancel");
    assert_eq!(context["description"], "");
    assert_eq!(context["accounts"][0]["login"], "AliceCase");
    assert_eq!(
        publish_operation_cli(&f, "context", &["--host", "github.com"]),
        context
    );
    assert_eq!(f.snapshot(), before);
    let description = f.repo.join(".git/description");
    let content = "  literal leading and trailing  \nsecond line\n";
    fs::write(&description, content).unwrap();
    let before = f.snapshot();
    assert_eq!(
        preview(
            &f,
            json!({"method":"publish_context","host":"github.com"}),
            false
        )["description"],
        content
    );
    assert_eq!(fs::read_to_string(&description).unwrap(), content);
    assert_eq!(f.snapshot(), before);
    let outside = f.root.join("outside");
    fs::write(&outside, "private outside sentinel").unwrap();
    fs::remove_file(&description).unwrap();
    symlink(&outside, &description).unwrap();
    assert_eq!(
        preview(
            &f,
            json!({"method":"publish_context","host":"github.com"}),
            false
        )["description"],
        ""
    );
    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "private outside sentinel"
    );
    assert!(fs::symlink_metadata(&description).unwrap().is_symlink());
    fs::remove_file(&description).unwrap();
    f.run("mkfifo", &[description.to_str().unwrap()]);
    let request = json!({"method":"publish_context","host":"github.com"}).to_string();
    let mut command = f.preview_command();
    // A bounded poll makes a regression in no-follow/nonblocking handling visible.
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.as_bytes())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    if child.try_wait().unwrap().is_none() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("FIFO description blocked context");
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["description"],
        ""
    );
    fs::remove_file(&description).unwrap();
    f.run(
        env!("CARGO_BIN_EXE_desktop-rs"),
        &[
            "--config",
            f.registry.to_str().unwrap(),
            "repositories",
            "forget",
            f.repo.to_str().unwrap(),
        ],
    );
    assert_eq!(
        preview(
            &f,
            json!({"method":"publish_context","host":"github.com"}),
            false
        )["saved"],
        false
    );
}

#[test]
fn organization_selection_is_paginated_sorted_and_pinned() {
    let f = Fixture::new();
    f.install_stub();
    let mut page: Vec<_> = (0..100)
        .map(|i| json!({"login":format!("org{i:03}"),"id":1000+i}))
        .collect();
    page[0] = json!({"login":"Zulu","id":900});
    page[1] = json!({"login":"alpha","id":901});
    fs::write(f.root.join("orgs.json"), json!(page).to_string()).unwrap();
    fs::write(
        f.root.join("orgs-page2.json"),
        json!([{"login":"last","id":5000}]).to_string(),
    )
    .unwrap();
    let before = f.snapshot();
    let assigned = json!({"mode":"assigned","host":"github.com","login":"AliceCase"});
    let result = preview(
        &f,
        json!({"method":"publish_organizations","host":"github.com","target":assigned}),
        false,
    );
    assert_eq!(result["status"], "available");
    let rows = result["organizations"].as_array().unwrap();
    assert_eq!(rows.len(), 101);
    let names: Vec<_> = rows
        .iter()
        .map(|v| v["login"].as_str().unwrap().to_ascii_lowercase())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    assert_eq!(
        publish_operation_cli(
            &f,
            "organizations",
            &["--host", "github.com", "--login", "AliceCase"]
        ),
        result
    );
    let log = fs::read_to_string(f.root.join("gh-log")).unwrap();
    assert!(log.contains("&page=1"));
    assert!(log.contains("&page=2"));
    let effective = preview(
        &f,
        json!({"method":"publish_organizations","host":"github.com","target":{"mode":"effective"}}),
        false,
    );
    assert_eq!(
        effective["actor"],
        json!({"login":"environment-user","id":303})
    );
    assert_eq!(
        publish_operation_cli(
            &f,
            "organizations",
            &["--host", "github.com", "--effective"]
        ),
        effective
    );
    preview(
        &f,
        json!({"method":"publish_organizations","host":"github.com","target":{"mode":"none"}}),
        true,
    );
    assert_eq!(f.snapshot(), before);
    fs::write(
        f.root.join("catalog.json"),
        r#"{"hosts":{"github.com":[]}}"#,
    )
    .unwrap();
    assert_eq!(
        preview(
            &f,
            json!({"method":"publish_context","host":"github.com"}),
            false
        )["accounts"],
        json!([])
    );
    assert_eq!(
        preview(
            &f,
            json!({"method":"publish_organizations","host":"github.com","target":{"mode":"effective"}}),
            false
        )["actor"]["id"],
        303
    );
}

#[test]
fn organizations_reject_duplicate_ids_and_page_limit_without_mutation() {
    let f = Fixture::new();
    f.install_stub();
    let before = f.snapshot();
    let target = json!({"mode":"assigned","host":"github.com","login":"AliceCase"});
    let request = json!({"method":"publish_organizations","host":"github.com","target":target});
    fs::write(
        f.root.join("orgs.json"),
        r#"[{"login":"First","id":9},{"login":"Second","id":9}]"#,
    )
    .unwrap();
    preview(&f, request.clone(), true);
    assert_eq!(f.snapshot(), before);
    fs::create_dir(f.root.join("pages")).unwrap();
    for page in 1..=16 {
        let values: Vec<_> = (0..100)
            .map(|i| {
                let n = (page - 1) * 100 + i;
                json!({"login":format!("org{n:04}"),"id":n+1})
            })
            .collect();
        let path = if page == 1 {
            f.root.join("orgs.json")
        } else {
            f.root.join("pages").join(format!("page-{page}.json"))
        };
        fs::write(path, json!(values).to_string()).unwrap();
    }
    fs::remove_file(f.root.join("gh-log")).unwrap();
    preview(&f, request, true);
    let log = fs::read_to_string(f.root.join("gh-log")).unwrap();
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("/user/orgs?"))
            .count(),
        16
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn organization_reads_expire_if_actor_remote_or_assignment_changes() {
    for kind in ["actor", "remote", "url", "account"] {
        let f = Fixture::new();
        f.install_stub();
        fs::write(
            f.root.join("orgs.json"),
            r#"[{"login":"TrustedOrg","id":42}]"#,
        )
        .unwrap();
        if kind == "url" {
            f.git(&[
                "remote",
                "add",
                "upstream",
                "https://github.com/fork/repo.git?initial-marker=1",
            ]);
        }
        fs::write(f.root.join("gh-delay"), "on").unwrap();
        let target = if kind == "actor" {
            json!({"mode":"effective"})
        } else {
            json!({"mode":"assigned","host":"github.com","login":"AliceCase"})
        };
        let request = json!({"method":"publish_organizations","host":"github.com","target":target})
            .to_string();
        let mut child = f
            .preview_command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.as_bytes())
            .unwrap();
        wait_for(
            &f.root.join("gh-started"),
            &mut child,
            Duration::from_secs(15),
        );
        match kind {
            "actor" => fs::write(
                f.root.join("user.json"),
                r#"{"id":999,"login":"environment-user"}"#,
            )
            .unwrap(),
            "remote" => {
                f.git(&[
                    "remote",
                    "add",
                    "origin",
                    "https://github.com/other/repo.git",
                ]);
            }
            "url" => {
                f.git(&[
                    "remote",
                    "set-url",
                    "upstream",
                    "https://github.com/fork/repo?changed-marker=2",
                ]);
            }
            _ => {
                let target = json!({"mode":"none"});
                let plan = preview(
                    &f,
                    json!({"method":"repository_account_preview","target":target}),
                    false,
                );
                preview(
                    &f,
                    json!({"method":"set_repository_account","target":target,"basis":plan["basis"]}),
                    false,
                );
            }
        }
        let retained = f.snapshot();
        fs::write(f.root.join("gh-release"), "release").unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        if child.try_wait().unwrap().is_none() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("organization read did not finish");
        }
        let out = child.wait_with_output().unwrap();
        assert!(!out.status.success(), "{kind}");
        assert!(out.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&out.stderr);
        for secret in [
            "synthetic-secret",
            "assigned-secret",
            "initial-marker",
            "changed-marker",
        ] {
            assert!(!stderr.contains(secret));
        }
        assert_eq!(f.snapshot(), retained);
    }
}

#[test]
fn retained_assigned_account_rejects_changed_numeric_actor() {
    let f = Fixture::new();
    f.install_stub();
    f.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/fork/repo.git",
    ]);
    let target = json!({"mode":"assigned","host":"github.com","login":"AliceCase"});
    let plan = preview(
        &f,
        json!({"method":"repository_account_preview","target":target}),
        false,
    );
    assert_eq!(plan["can_apply"], true, "{plan}");
    preview(
        &f,
        json!({"method":"set_repository_account","target":target,"basis":plan["basis"]}),
        false,
    );
    f.git(&["remote", "remove", "origin"]);
    assert_eq!(
        preview(&f, json!({"method":"repository_account"}), false)["assignment"]["id"],
        101
    );
    let before = f.snapshot();
    fs::write(
        f.root.join("assigned-user.json"),
        r#"{"id":999,"login":"alicecase"}"#,
    )
    .unwrap();
    fs::write(
        f.root.join("orgs.json"),
        r#"[{"login":"WrongActor","id":444}]"#,
    )
    .unwrap();
    fs::write(f.root.join("gh-log"), "").unwrap();
    preview(
        &f,
        json!({"method":"publish_organizations","host":"github.com","target":target}),
        true,
    );
    assert!(
        !fs::read_to_string(f.root.join("gh-log"))
            .unwrap()
            .contains("/user/orgs?")
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn organization_reflections_and_unrelated_corrupt_cache_are_fenced() {
    let f = Fixture::new();
    f.install_stub();
    let target = json!({"mode":"assigned","host":"github.com","login":"AliceCase"});
    let request = json!({"method":"publish_organizations","host":"github.com","target":target});
    let before = f.snapshot();
    for secret in ["assigned-secret", "YXNzaWduZWQtc2VjcmV0"] {
        fs::write(
            f.root.join("orgs.json"),
            json!([{"login":secret,"id":400}]).to_string(),
        )
        .unwrap();
        preview(&f, request.clone(), true);
        assert_eq!(f.snapshot(), before);
    }
    let cache = f.repo.join(".git/desktop-repository.json");
    fs::write(&cache, b"{broken unrelated association cache").unwrap();
    fs::write(
        f.root.join("orgs.json"),
        r#"[{"login":"VerifiedOrg","id":88}]"#,
    )
    .unwrap();
    assert_eq!(
        preview(
            &f,
            json!({"method":"publish_context","host":"github.com"}),
            false
        )["status"],
        "available"
    );
    assert_eq!(
        preview(&f, request, false)["organizations"],
        json!([{"login":"VerifiedOrg","id":88}])
    );
    assert_eq!(
        fs::read(cache).unwrap(),
        b"{broken unrelated association cache"
    );
    assert_eq!(f.snapshot(), before);
}

fn repository_response(owner: &str, id: u64) -> Value {
    json!({"id":8001,"name":"example-repo","full_name":format!("{owner}/example-repo"),"owner":{"login":owner,"id":id},"html_url":format!("https://github.com/{owner}/example-repo"),"clone_url":format!("https://github.com/{owner}/example-repo.git"),"private":true})
}
fn create_publication(f: &Fixture, owner: &str, owner_id: u64, effective: bool) -> String {
    fs::write(
        f.root.join("orgs.json"),
        r#"[{"login":"ExampleOrg","id":987}]"#,
    )
    .unwrap();
    fs::write(
        f.root.join("repository-response.json"),
        repository_response(owner, owner_id).to_string(),
    )
    .unwrap();
    let mut request = f.request()["request"].clone();
    request["owner"] = json!(owner);
    if !effective {
        request["target"] = json!({"mode":"assigned","host":"github.com","login":"AliceCase"});
    }
    let plan = preview(
        f,
        json!({"method":"publish_preview","request":request}),
        false,
    );
    let result = preview(
        f,
        json!({"method":"publish_create","request":request,"basis":plan["basis"],"acknowledge_partial":true}),
        false,
    );
    assert_eq!(result["phase"], "created");
    result["receipt_id"].as_str().unwrap().to_owned()
}

#[test]
fn continuation_verifies_repository_identity_and_never_replays_creation() {
    let f = Fixture::new();
    f.install_stub();
    let receipt = create_publication(&f, "alicecase", 101, false);
    let before = f.snapshot();
    fs::write(f.root.join("gh-log"), "").unwrap();
    let result = preview(
        &f,
        json!({"method":"publish_continue_preview","receipt_id":receipt}),
        false,
    );
    assert_eq!(result["phase"], "created");
    assert!(result["basis"].is_string());
    assert_eq!(result["can_continue"], false);
    assert_eq!(result["url"], "https://github.com/alicecase/example-repo");
    assert_eq!(result["actor"], json!({"login":"alicecase","id":101}));
    assert_eq!(result["owner"], json!({"login":"alicecase","id":101}));
    assert_eq!(result["private"], true);
    assert_eq!(
        publish_operation_cli(&f, "continue-preview", &[&receipt]),
        result
    );
    assert_eq!(
        preview(&f, json!({"method":"publish_history"}), false)["receipts"][0]["url"],
        Value::Null
    );
    assert_eq!(f.snapshot(), before);
    let original = repository_response("alicecase", 101);
    for (key, val) in [
        ("id", json!(8002)),
        ("full_name", json!("other/example-repo")),
        (
            "html_url",
            json!("https://attacker.invalid/alicecase/example-repo"),
        ),
        (
            "clone_url",
            json!("https://github.com/alicecase/example-repo.git?token=assigned-secret"),
        ),
    ] {
        let mut changed = original.clone();
        changed[key] = val;
        fs::write(f.root.join("repository-response.json"), changed.to_string()).unwrap();
        preview(
            &f,
            json!({"method":"publish_continue_preview","receipt_id":receipt}),
            true,
        );
        assert_eq!(f.snapshot(), before);
    }
    let mut wrong_owner = original.clone();
    wrong_owner["owner"]["id"] = json!(777);
    fs::write(
        f.root.join("repository-response.json"),
        wrong_owner.to_string(),
    )
    .unwrap();
    preview(
        &f,
        json!({"method":"publish_continue_preview","receipt_id":receipt}),
        true,
    );
    let mut public = original;
    public["private"] = json!(false);
    fs::write(f.root.join("repository-response.json"), public.to_string()).unwrap();
    let changed = preview(
        &f,
        json!({"method":"publish_continue_preview","receipt_id":receipt}),
        false,
    );
    assert_eq!(changed["private"], false);
    assert_ne!(changed["basis"], result["basis"]);
    assert_eq!(
        fs::read_to_string(f.root.join("post-log")).unwrap(),
        "POST\n"
    );
    assert_eq!(f.snapshot(), before);
    let log = fs::read_to_string(f.root.join("gh-log")).unwrap();
    assert!(log.contains("/repos/alicecase/example-repo"));
    assert!(!log.contains("POST"));
}

#[test]
fn continuation_fences_effective_actor_and_organization_identity() {
    for kind in ["effective", "organization"] {
        let f = Fixture::new();
        f.install_stub();
        let (owner, id, effective) = if kind == "effective" {
            ("environment-user", 303, true)
        } else {
            ("ExampleOrg", 987, false)
        };
        let receipt = create_publication(&f, owner, id, effective);
        let before = f.snapshot();
        fs::write(f.root.join("gh-log"), "").unwrap();
        let result = preview(
            &f,
            json!({"method":"publish_continue_preview","receipt_id":receipt}),
            false,
        );
        assert_eq!(result["owner"], json!({"login":owner,"id":id}));
        assert_eq!(
            result["url"],
            format!("https://github.com/{owner}/example-repo")
        );
        let user_file = if effective {
            "user.json"
        } else {
            "assigned-user.json"
        };
        let login = if effective {
            "environment-user"
        } else {
            "alicecase"
        };
        fs::write(
            f.root.join(user_file),
            json!({"id":999,"login":login}).to_string(),
        )
        .unwrap();
        fs::write(f.root.join("gh-log"), "").unwrap();
        preview(
            &f,
            json!({"method":"publish_continue_preview","receipt_id":receipt}),
            true,
        );
        assert!(
            !fs::read_to_string(f.root.join("gh-log"))
                .unwrap()
                .contains("/repos/")
        );
        if !effective {
            fs::write(f.root.join(user_file), r#"{"id":101,"login":"alicecase"}"#).unwrap();
            fs::write(
                f.root.join("orgs.json"),
                r#"[{"login":"ExampleOrg","id":999}]"#,
            )
            .unwrap();
            fs::write(f.root.join("gh-log"), "").unwrap();
            preview(
                &f,
                json!({"method":"publish_continue_preview","receipt_id":receipt}),
                true,
            );
            assert!(
                !fs::read_to_string(f.root.join("gh-log"))
                    .unwrap()
                    .contains("/repos/")
            );
        }
        assert_eq!(f.snapshot(), before);
        assert_eq!(
            fs::read_to_string(f.root.join("post-log")).unwrap(),
            "POST\n"
        );
    }
}

#[test]
fn continuation_rejects_changed_repository_state_during_remote_lookup() {
    for kind in ["origin", "config", "head", "record"] {
        let f = Fixture::new();
        f.install_stub();
        let receipt = create_publication(&f, "alicecase", 101, false);
        fs::write(f.root.join("repo-delay"), "on").unwrap();
        let mut child = f
            .preview_command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                json!({"method":"publish_continue_preview","receipt_id":receipt})
                    .to_string()
                    .as_bytes(),
            )
            .unwrap();
        wait_for(
            &f.root.join("gh-started"),
            &mut child,
            Duration::from_secs(15),
        );
        match kind {
            "origin" => {
                f.git(&[
                    "remote",
                    "add",
                    "origin",
                    "https://github.com/other/repo.git",
                ]);
            }
            "config" => {
                f.git(&["config", "branch.main.merge", "refs/heads/other"]);
            }
            "head" => {
                f.git(&["switch", "-qc", "another"]);
            }
            _ => {
                let mut registry: Value =
                    serde_json::from_slice(&fs::read(&f.registry).unwrap()).unwrap();
                let row = registry["records"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|r| r["path"] == f.repo.to_str().unwrap())
                    .unwrap();
                row["id"] = json!(uuid::Uuid::new_v4().to_string());
                fs::write(&f.registry, registry.to_string()).unwrap();
                fs::set_permissions(&f.registry, fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
        let retained = f.snapshot();
        fs::write(f.root.join("gh-release"), "release").unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        if child.try_wait().unwrap().is_none() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("continuation race timed out");
        }
        let out = child.wait_with_output().unwrap();
        assert!(!out.status.success(), "{kind}");
        assert!(out.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("assigned-secret"));
        assert_eq!(f.snapshot(), retained);
        assert_eq!(
            fs::read_to_string(f.root.join("post-log")).unwrap(),
            "POST\n"
        );
    }
}

#[test]
fn continuation_rejects_unknown_legacy_or_disabled_account_receipts() {
    use sha2::{Digest, Sha256};
    let unknown = Fixture::new();
    unknown.install_stub();
    fs::write(unknown.root.join("post-fail"), "yes").unwrap();
    fs::write(
        unknown.root.join("repository-response.json"),
        repository_response("alicecase", 101).to_string(),
    )
    .unwrap();
    let mut request = unknown.request()["request"].clone();
    request["owner"] = json!("alicecase");
    request["target"] = json!({"mode":"assigned","host":"github.com","login":"AliceCase"});
    let plan = preview(
        &unknown,
        json!({"method":"publish_preview","request":request}),
        false,
    );
    preview(
        &unknown,
        json!({"method":"publish_create","request":request,"basis":plan["basis"],"acknowledge_partial":true}),
        false,
    );
    let history = preview(&unknown, json!({"method":"publish_history"}), false);
    assert_eq!(history["receipts"][0]["phase"], "creation_unknown");
    let before = unknown.snapshot();
    fs::write(unknown.root.join("gh-log"), "").unwrap();
    preview(
        &unknown,
        json!({"method":"publish_continue_preview","receipt_id":history["receipts"][0]["id"]}),
        true,
    );
    assert!(
        !fs::read_to_string(unknown.root.join("gh-log"))
            .unwrap()
            .contains("api")
    );
    assert_eq!(unknown.snapshot(), before);
    assert_eq!(
        fs::read_to_string(unknown.root.join("post-log")).unwrap(),
        "POST\n"
    );

    let legacy = Fixture::new();
    legacy.install_stub();
    let hash = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    use std::os::unix::ffi::OsStrExt;
    let registry: Value = serde_json::from_slice(&fs::read(&legacy.registry).unwrap()).unwrap();
    let record = &registry["records"][0];
    let namespace = hash(legacy.registry.as_os_str().as_bytes());
    let directory = hash(json!([namespace, record["id"]]).to_string().as_bytes());
    let base = legacy.repo.join(".git/desktop-publish-receipts");
    let dir = base.join(directory);
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let legacy_receipt = json!({"version":1,"id":id,"repository_id":hash(legacy.repo.as_os_str().as_bytes()),"namespace":namespace,"record_id":record["id"],"phase":"created","host":"github.com","owner":"ExampleOrg","name":"example-repo","error":null,"acknowledged":false});
    let path = dir.join(format!("{id}.json"));
    fs::write(&path, legacy_receipt.to_string()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    let before = legacy.snapshot();
    fs::write(legacy.root.join("gh-log"), "").unwrap();
    preview(
        &legacy,
        json!({"method":"publish_continue_preview","receipt_id":id}),
        true,
    );
    assert!(
        !fs::read_to_string(legacy.root.join("gh-log"))
            .unwrap()
            .contains("api")
    );
    assert_eq!(legacy.snapshot(), before);

    let disabled = Fixture::new();
    disabled.install_stub();
    let id = create_publication(&disabled, "alicecase", 101, false);
    let target = json!({"mode":"none"});
    let plan = preview(
        &disabled,
        json!({"method":"repository_account_preview","target":target}),
        false,
    );
    preview(
        &disabled,
        json!({"method":"set_repository_account","target":target,"basis":plan["basis"]}),
        false,
    );
    let before = disabled.snapshot();
    fs::write(disabled.root.join("gh-log"), "").unwrap();
    preview(
        &disabled,
        json!({"method":"publish_continue_preview","receipt_id":id}),
        true,
    );
    assert!(
        !fs::read_to_string(disabled.root.join("gh-log"))
            .unwrap()
            .contains("api")
    );
    assert_eq!(disabled.snapshot(), before);
}

#[test]
fn token_like_publication_names_never_reach_creation() {
    let f = Fixture::new();
    f.install_stub();
    let before = f.snapshot();
    for token in ["assigned-secret", "YXNzaWduZWQtc2VjcmV0"] {
        let mut request = f.request()["request"].clone();
        request["target"] = json!({"mode":"assigned","host":"github.com","login":"AliceCase"});
        request["owner"] = json!("alicecase");
        request["name"] = json!(token);
        let review = preview(
            &f,
            json!({"method":"publish_preview","request":request}),
            false,
        );
        preview(
            &f,
            json!({"method":"publish_create","request":request,"basis":review["basis"],"acknowledge_partial":true}),
            true,
        );
        assert!(!f.root.join("post-log").exists());
        assert_eq!(f.snapshot(), before);
        assert!(
            !String::from_utf8_lossy(&fs::read(&f.registry).unwrap()).contains("assigned-secret")
        );
    }
}

fn prepare_origin(f: &Fixture, owner: &str, owner_id: u64, effective: bool) -> (String, Value) {
    let id = create_publication(f, owner, owner_id, effective);
    let mut repository = repository_response(owner, owner_id);
    repository["fork"] = json!(false);
    repository["parent"] = Value::Null;
    fs::write(
        f.root.join("repository-response.json"),
        repository.to_string(),
    )
    .unwrap();
    fs::write(f.root.join("gh-log"), "").unwrap();
    let plan = preview(
        f,
        json!({"method":"publish_continue_preview","receipt_id":id}),
        false,
    );
    assert_eq!(plan["can_continue"], true);
    (id, plan)
}

#[test]
fn reviewed_origin_continuation_assigns_account_once_without_pushing() {
    for (owner, owner_id, effective) in [
        ("alicecase", 101, false),
        ("ExampleOrg", 987, false),
        ("environment-user", 303, true),
    ] {
        let f = Fixture::new();
        f.install_stub();
        let (receipt, plan) = prepare_origin(&f, owner, owner_id, effective);
        let before = f.snapshot();
        assert_eq!(
            publish_operation_cli(&f, "continue-preview", &[&receipt]),
            plan
        );
        preview(
            &f,
            json!({"method":"publish_continue","receipt_id":receipt,"basis":plan["basis"],"acknowledge_partial":false}),
            true,
        );
        let no_consent = bounded_command(
            publish_command(
                &f,
                "continue",
                &[&receipt, "--basis", plan["basis"].as_str().unwrap()],
            ),
            None,
        );
        assert!(!no_consent.status.success());
        assert_eq!(f.snapshot(), before);
        let result = preview(
            &f,
            json!({"method":"publish_continue","receipt_id":receipt,"basis":plan["basis"],"acknowledge_partial":true}),
            false,
        );
        assert_eq!(result["phase"], "account_assigned");
        assert_eq!(result["receipt_id"], receipt);
        let history = preview(&f, json!({"method":"publish_history"}), false);
        assert_eq!(history["receipts"][0]["phase"], "account_assigned");
        assert_eq!(
            String::from_utf8(f.git(&["config", "--get", "remote.origin.url"]))
                .unwrap()
                .trim(),
            format!("https://github.com/{owner}/example-repo.git")
        );
        assert_eq!(
            String::from_utf8(f.git(&["config", "--get-all", "remote.origin.fetch"]))
                .unwrap()
                .trim(),
            "+refs/heads/*:refs/remotes/origin/*"
        );
        let account = preview(&f, json!({"method":"repository_account"}), false);
        assert_eq!(
            account["assignment"]["mode"],
            if effective { "effective" } else { "assigned" }
        );
        if !effective {
            assert_eq!(account["assignment"]["id"], 101);
            assert_eq!(account["assignment"]["host"], "github.com");
            assert_eq!(account["scoped_metadata"], true);
        }
        let after = f.snapshot();
        assert_eq!(&after[1..4], &before[1..4]);
        preview(
            &f,
            json!({"method":"publish_continue","receipt_id":receipt,"basis":plan["basis"],"acknowledge_partial":true}),
            true,
        );
        assert_eq!(f.snapshot(), after);
        assert_eq!(
            fs::read_to_string(f.root.join("post-log")).unwrap(),
            "POST\n"
        );
        assert!(
            !fs::read_to_string(f.root.join("gh-log"))
                .unwrap()
                .contains("POST")
        );
    }
}

#[test]
fn occupied_config_lock_records_unknown_without_replay() {
    let f = Fixture::new();
    f.install_stub();
    let (receipt, plan) = prepare_origin(&f, "alicecase", 101, false);
    let before = f.snapshot();
    let lock = f.repo.join(".git/config.lock");
    fs::write(&lock, b"other-operation-sentinel").unwrap();
    let result = preview(
        &f,
        json!({"method":"publish_continue","receipt_id":receipt,"basis":plan["basis"],"acknowledge_partial":true}),
        false,
    );
    assert_eq!(result["phase"], "origin_unknown");
    assert_eq!(fs::read(&lock).unwrap(), b"other-operation-sentinel");
    let after = f.snapshot();
    assert_eq!(&after[..4], &before[..4]);
    fs::remove_file(lock).unwrap();
    let retained = f.snapshot();
    preview(
        &f,
        json!({"method":"publish_continue","receipt_id":receipt,"basis":plan["basis"],"acknowledge_partial":true}),
        true,
    );
    assert_eq!(f.snapshot(), retained);
    assert_eq!(
        preview(&f, json!({"method":"publish_history"}), false)["receipts"][0]["phase"],
        "origin_unknown"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("post-log")).unwrap(),
        "POST\n"
    );
}

#[test]
fn origin_preview_rejects_rewrites_occupied_config_and_fork_metadata() {
    for kind in [
        "insteadOf",
        "pushInsteadOf",
        "pushurl",
        "fetch",
        "fork",
        "global",
        "worktree",
        "include",
        "trailing",
    ] {
        let f = Fixture::new();
        f.install_stub();
        let (receipt, _) = prepare_origin(&f, "alicecase", 101, false);
        match kind {
            "insteadOf" | "pushInsteadOf" => {
                f.git(&[
                    "config",
                    &format!("url.ssh://untrusted.invalid/.{kind}"),
                    "https://github.com/",
                ]);
            }
            "pushurl" => {
                f.git(&[
                    "config",
                    "--add",
                    "remote.origin.pushurl",
                    "https://assigned-secret@untrusted.invalid/foreign.git",
                ]);
            }
            "fetch" => {
                f.git(&[
                    "config",
                    "--add",
                    "remote.origin.fetch",
                    "+refs/heads/*:refs/remotes/origin/*",
                ]);
            }
            "fork" => {
                let mut response = repository_response("alicecase", 101);
                response["fork"] = json!(true);
                response["parent"] =
                    json!({"full_name":"other/base","html_url":"https://github.com/other/base"});
                fs::write(
                    f.root.join("repository-response.json"),
                    response.to_string(),
                )
                .unwrap();
            }
            "global" => {
                fs::write(
                    f.root.join("global"),
                    "[url \"ssh://untrusted.invalid/\"]\n insteadOf = https://github.com/\n",
                )
                .unwrap();
            }
            "worktree" => {
                f.git(&["config", "extensions.worktreeConfig", "true"]);
                f.git(&[
                    "config",
                    "--worktree",
                    "remote.origin.fetch",
                    "+refs/heads/*:refs/remotes/origin/*",
                ]);
            }
            "include" => {
                let include = f.root.join("included-config");
                fs::write(
                    &include,
                    "[remote \"origin\"]\n fetch = +refs/heads/*:refs/remotes/origin/*\n",
                )
                .unwrap();
                use std::io::Write as _;
                let mut config = fs::OpenOptions::new()
                    .append(true)
                    .open(f.repo.join(".git/config"))
                    .unwrap();
                writeln!(
                    config,
                    "[includeIf \"gitdir:{}\"]\n path = {}",
                    f.repo.join(".git").display(),
                    include.display()
                )
                .unwrap();
            }
            _ => {
                use std::io::Write as _;
                let mut config = fs::OpenOptions::new()
                    .append(true)
                    .open(f.repo.join(".git/config"))
                    .unwrap();
                write!(config, "[user]\n name = dangling\\").unwrap();
            }
        }
        let before = if kind == "trailing" {
            None
        } else {
            Some(f.snapshot())
        };
        let request = json!({"method":"publish_continue_preview","receipt_id":receipt});
        let mut child = f
            .preview_command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        if output.status.success() {
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["can_continue"], false, "{kind}");
        } else {
            assert!(output.stdout.is_empty(), "{kind}");
        }
        assert!(!String::from_utf8_lossy(&output.stderr).contains("assigned-secret"));
        if let Some(before) = before {
            assert_eq!(f.snapshot(), before, "{kind}");
        }
        assert_eq!(
            fs::read_to_string(f.root.join("post-log")).unwrap(),
            "POST\n"
        );
    }
}

#[test]
fn unsafe_git_config_paths_are_rejected_before_mutation() {
    use std::os::unix::fs::symlink;
    for kind in ["symlink", "hardlink", "fifo"] {
        let f = Fixture::new();
        f.install_stub();
        let (receipt, plan) = prepare_origin(&f, "alicecase", 101, false);
        let config = f.repo.join(".git/config");
        let original = fs::read(&config).unwrap();
        let outside = f.root.join("outside-config");
        match kind {
            "symlink" => {
                fs::write(&outside, &original).unwrap();
                fs::remove_file(&config).unwrap();
                symlink(&outside, &config).unwrap();
            }
            "hardlink" => {
                fs::hard_link(&config, &outside).unwrap();
            }
            _ => {
                fs::remove_file(&config).unwrap();
                f.run("mkfifo", &[config.to_str().unwrap()]);
            }
        }
        let request = json!({"method":"publish_continue","receipt_id":receipt,"basis":plan["basis"],"acknowledge_partial":true}).to_string();
        let mut child = f
            .preview_command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.as_bytes())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        if child.try_wait().unwrap().is_none() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{kind} config blocked continuation");
        }
        let output = child.wait_with_output().unwrap();
        if output.status.success() {
            let result: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(result["phase"] == "created" || result["phase"] == "origin_unknown");
        } else {
            assert!(output.stdout.is_empty());
        }
        if kind == "fifo" {
            let cli = bounded_command(
                publish_command(
                    &f,
                    "continue",
                    &[
                        &receipt,
                        "--basis",
                        plan["basis"].as_str().unwrap(),
                        "--acknowledge-partial",
                    ],
                ),
                None,
            );
            assert!(!cli.status.success());
            assert!(cli.stdout.is_empty());
        } else {
            assert_eq!(fs::read(&outside).unwrap(), original);
        }
        if kind == "symlink" {
            assert!(fs::symlink_metadata(&config).unwrap().is_symlink());
        }
        if kind == "hardlink" {
            assert_eq!(fs::read(&config).unwrap(), original);
        }
        assert_eq!(
            fs::read_to_string(f.root.join("post-log")).unwrap(),
            "POST\n"
        );
    }
}

fn bounded_command(mut command: Command, payload: Option<&Value>) -> std::process::Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(payload) = payload {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
    } else {
        drop(child.stdin.take());
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    if child.try_wait().unwrap().is_none() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("unsafe Git configuration blocked continuation");
    }
    child.wait_with_output().unwrap()
}

#[test]
fn fifo_includes_and_unsafe_system_configuration_fail_promptly() {
    use std::os::unix::fs::symlink;
    for kind in [
        "local-include",
        "global-include",
        "system-fifo",
        "system-symlink",
        "system-writable",
        "system-default",
    ] {
        let f = Fixture::new();
        f.install_stub();
        let (receipt, plan) = prepare_origin(&f, "alicecase", 101, false);
        let config = f.repo.join(".git/config");
        let original = fs::read(&config).unwrap();
        let path = f.root.join("unsafe-config");
        match kind {
            "local-include" | "global-include" => {
                f.run("mkfifo", &[path.to_str().unwrap()]);
                let include = format!("[include]\n path = {}\n", path.display());
                if kind == "local-include" {
                    use std::io::Write as _;
                    let mut file = fs::OpenOptions::new().append(true).open(&config).unwrap();
                    write!(file, "\n{include}").unwrap();
                } else {
                    fs::write(f.root.join("global"), include).unwrap();
                    fs::set_permissions(f.root.join("global"), fs::Permissions::from_mode(0o600))
                        .unwrap();
                }
            }
            "system-fifo" => {
                f.run("mkfifo", &[path.to_str().unwrap()]);
            }
            "system-symlink" => {
                let target = f.root.join("system-target");
                fs::write(&target, "private system sentinel").unwrap();
                symlink(&target, &path).unwrap();
            }
            "system-writable" => {
                fs::write(&path, "[core]\n autocrlf = false\n").unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
            }
            _ => {}
        }
        let payload = json!({"method":"publish_continue","receipt_id":receipt,"basis":plan["basis"],"acknowledge_partial":true});
        let mut command = f.preview_command();
        if kind.starts_with("system-") {
            command.env_remove("GIT_CONFIG_NOSYSTEM");
            if kind != "system-default" {
                command.env("GIT_CONFIG_SYSTEM", &path);
            } else {
                command.env_remove("GIT_CONFIG_SYSTEM");
                let shadow = f.root.join("no-system-git");
                fs::create_dir(&shadow).unwrap();
                executable(
                    &shadow.join("git"),
                    "#!/bin/sh\nprintf blocked > \"$GIT_MARKER\"\nexit 97\n",
                );
                let mut paths = vec![shadow, f.bin.clone()];
                paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
                command
                    .env("PATH", std::env::join_paths(paths).unwrap())
                    .env("GIT_MARKER", f.root.join("git-invoked"));
            }
        }
        let output = bounded_command(command, Some(&payload));
        if kind == "local-include" || kind == "global-include" {
            let output = bounded_command(
                publish_command(
                    &f,
                    "continue",
                    &[
                        &receipt,
                        "--basis",
                        plan["basis"].as_str().unwrap(),
                        "--acknowledge-partial",
                    ],
                ),
                None,
            );
            assert!(!output.status.success(), "CLI {kind}");
            assert!(output.stdout.is_empty());
        }
        assert!(!output.status.success(), "{kind}");
        assert!(output.stdout.is_empty());
        if kind == "system-default" {
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .to_ascii_lowercase()
                    .contains("system")
            );
            assert!(!f.root.join("git-invoked").exists());
        }
        if kind == "system-symlink" {
            assert_eq!(
                fs::read_to_string(f.root.join("system-target")).unwrap(),
                "private system sentinel"
            );
            assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
        }
        if kind == "local-include" {
            fs::write(&config, &original).unwrap();
        }
        if kind == "global-include" {
            fs::remove_file(f.root.join("global")).unwrap();
        }
        assert_eq!(fs::read(&config).unwrap(), original);
        assert_eq!(
            preview(&f, json!({"method":"publish_history"}), false)["receipts"][0]["phase"],
            "created"
        );
        assert_eq!(
            fs::read_to_string(f.root.join("post-log")).unwrap(),
            "POST\n"
        );
    }
}

#[test]
fn explicit_private_system_config_is_preserved_during_continuation() {
    let f = Fixture::new();
    f.install_stub();
    let (receipt, _) = prepare_origin(&f, "alicecase", 101, false);
    let config = f.root.join("system-config");
    fs::write(&config, "[core]\n autocrlf = false\n").unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    let original = fs::read(&config).unwrap();
    let mut command = f.preview_command();
    command
        .env_remove("GIT_CONFIG_NOSYSTEM")
        .env("GIT_CONFIG_SYSTEM", &config);
    let output = bounded_command(
        command,
        Some(&json!({"method":"publish_continue_preview","receipt_id":receipt})),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["can_continue"], true);
    let mut command = f.preview_command();
    command
        .env_remove("GIT_CONFIG_NOSYSTEM")
        .env("GIT_CONFIG_SYSTEM", &config);
    let output = bounded_command(
        command,
        Some(
            &json!({"method":"publish_continue","receipt_id":receipt,"basis":plan["basis"],"acknowledge_partial":true}),
        ),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["phase"],
        "account_assigned"
    );
    assert_eq!(fs::read(config).unwrap(), original);
    assert_eq!(
        fs::read_to_string(f.root.join("post-log")).unwrap(),
        "POST\n"
    );
}

#[test]
fn origin_continuation_rejects_concurrent_changes_and_cancellation() {
    for kind in ["origin", "config", "record", "account", "cancel"] {
        let f = Fixture::new();
        f.install_stub();
        let (receipt, plan) = prepare_origin(&f, "alicecase", 101, false);
        fs::write(f.root.join("repo-delay"), "on").unwrap();
        let payload = json!({"method":"publish_continue","receipt_id":receipt,"basis":plan["basis"],"acknowledge_partial":true});
        let mut child = f
            .preview_command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        wait_for(
            &f.root.join("gh-started"),
            &mut child,
            Duration::from_secs(15),
        );
        match kind {
            "origin" => {
                f.git(&[
                    "remote",
                    "add",
                    "origin",
                    "https://github.com/other/repo.git",
                ]);
            }
            "config" => {
                f.git(&["config", "branch.main.merge", "refs/heads/other"]);
            }
            "record" | "account" => {
                let mut data: Value =
                    serde_json::from_slice(&fs::read(&f.registry).unwrap()).unwrap();
                let row = data["records"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|r| r["path"] == f.repo.to_str().unwrap())
                    .unwrap();
                if kind == "record" {
                    row["id"] = json!(uuid::Uuid::new_v4().to_string());
                } else {
                    let account = &mut row["account"];
                    account["assignment"] = json!({"mode":"none"});
                    account["association"] = Value::Null;
                    account["source_remote"] = Value::Null;
                    account["revision"] = json!(account["revision"].as_u64().unwrap() + 1);
                }
                fs::write(&f.registry, data.to_string()).unwrap();
                fs::set_permissions(&f.registry, fs::Permissions::from_mode(0o600)).unwrap();
            }
            _ => {
                f.run("kill", &["-INT", &child.id().to_string()]);
            }
        }
        let retained = f.snapshot();
        fs::write(f.root.join("gh-release"), "release").unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        if child.try_wait().unwrap().is_none() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("origin {kind} did not finish");
        }
        let output = child.wait_with_output().unwrap();
        if output.status.success() {
            let result: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_ne!(result["phase"], "account_assigned");
        } else {
            assert!(output.stdout.is_empty());
        }
        assert!(!String::from_utf8_lossy(&output.stderr).contains("assigned-secret"));
        assert_eq!(f.snapshot(), retained);
        assert_eq!(
            fs::read_to_string(f.root.join("post-log")).unwrap(),
            "POST\n"
        );
    }
}

#[test]
fn replaced_config_lock_is_not_removed_or_committed() {
    let f = Fixture::new();
    f.install_stub();
    let (receipt, plan) = prepare_origin(&f, "alicecase", 101, false);
    let before = f.snapshot();
    let lock = f.repo.join(".git/config.lock");
    let parked = f.root.join("original-config-lock");
    fs::write(f.root.join("lock-gate"), "on").unwrap();
    let mut child = f
        .preview_command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(json!({"method":"publish_continue","receipt_id":receipt,"basis":plan["basis"],"acknowledge_partial":true}).to_string().as_bytes()).unwrap();
    wait_for(
        &f.root.join("gh-started"),
        &mut child,
        Duration::from_secs(15),
    );
    assert!(lock.is_file());
    assert_eq!(
        preview(&f, json!({"method":"publish_history"}), false)["receipts"][0]["phase"],
        "origin_unknown"
    );
    fs::rename(&lock, &parked).unwrap();
    fs::write(&lock, b"foreign-lock-sentinel").unwrap();
    fs::write(f.root.join("gh-release"), "release").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    if child.try_wait().unwrap().is_none() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("config lock replacement did not finish");
    }
    let output = child.wait_with_output().unwrap();
    if output.status.success() {
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap()["phase"],
            "origin_unknown"
        );
    } else {
        assert!(output.stdout.is_empty());
    }
    assert_eq!(fs::read(&lock).unwrap(), b"foreign-lock-sentinel");
    let after = f.snapshot();
    assert_eq!(&after[..4], &before[..4]);
    assert_eq!(
        preview(&f, json!({"method":"publish_history"}), false)["receipts"][0]["phase"],
        "origin_unknown"
    );
    assert!(f.git(&["remote"]).is_empty());
    assert_eq!(
        fs::read_to_string(f.root.join("post-log")).unwrap(),
        "POST\n"
    );
}

#[test]
fn creation_requires_consent_and_performs_exactly_one_pinned_post() {
    for (owner, owner_id, effective) in [
        ("alicecase", 101, false),
        ("ExampleOrg", 987, false),
        ("environment-user", 303, true),
    ] {
        let f = Fixture::new();
        f.install_stub();
        fs::write(
            f.root.join("orgs.json"),
            r#"[{"login":"ExampleOrg","id":987}]"#,
        )
        .unwrap();
        fs::write(
            f.root.join("repository-response.json"),
            repository_response(owner, owner_id).to_string(),
        )
        .unwrap();
        let mut request = f.request()["request"].clone();
        request["owner"] = json!(owner);
        if !effective {
            request["target"] = json!({"mode":"assigned","host":"github.com","login":"AliceCase"});
        }
        let plan = preview(
            &f,
            json!({"method":"publish_preview","request":request}),
            false,
        );
        let before = f.snapshot();
        preview(
            &f,
            json!({"method":"publish_create","request":request,"basis":plan["basis"],"acknowledge_partial":false}),
            true,
        );
        let basis = plan["basis"].as_str().unwrap();
        let mut cli_args = if effective {
            vec!["--effective"]
        } else {
            vec!["--login", "AliceCase"]
        };
        cli_args.extend([
            "--owner",
            owner,
            "--name",
            "example-repo",
            "--description",
            " exact description\n",
            "--basis",
            basis,
        ]);
        let denied = bounded_command(publish_command(&f, "create", &cli_args), None);
        assert!(!denied.status.success());
        assert!(!f.root.join("post-log").exists());
        let result = if owner == "ExampleOrg" {
            preview(
                &f,
                json!({"method":"publish_create","request":request,"basis":plan["basis"],"acknowledge_partial":true}),
                false,
            )
        } else {
            cli_args.push("--acknowledge-partial");
            publish_operation_cli(&f, "create", &cli_args)
        };
        assert_eq!(result["phase"], "created");
        assert!(result["receipt_id"].is_string());
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(f.root.join("post-body")).unwrap()).unwrap(),
            json!({"name":"example-repo","description":" exact description\n","private":true})
        );
        let log = fs::read_to_string(f.root.join("gh-log")).unwrap();
        let expected = if owner == "ExampleOrg" {
            "/orgs/ExampleOrg/repos"
        } else {
            "/user/repos"
        };
        assert!(
            log.lines()
                .any(|line| line.contains("--method POST") && line.contains(expected))
        );
        assert_eq!(
            fs::read_to_string(f.root.join("post-log")).unwrap(),
            "POST\n"
        );
        assert_eq!(&f.snapshot()[..4], &before[..4]);
        let history = preview(&f, json!({"method":"publish_history"}), false);
        assert_eq!(history["receipts"][0]["phase"], "created");
        preview(
            &f,
            json!({"method":"publish_create","request":request,"basis":plan["basis"],"acknowledge_partial":true}),
            true,
        );
        preview(
            &f,
            json!({"method":"acknowledge_publish","receipt_id":result["receipt_id"]}),
            false,
        );
        assert_eq!(
            preview(
                &f,
                json!({"method":"publish_preview","request":request}),
                false
            )["can_create"],
            false
        );
        f.run(
            env!("CARGO_BIN_EXE_desktop-rs"),
            &[
                "--config",
                f.registry.to_str().unwrap(),
                "repositories",
                "forget",
                f.repo.to_str().unwrap(),
            ],
        );
        f.run(
            env!("CARGO_BIN_EXE_desktop-rs"),
            &[
                "--config",
                f.registry.to_str().unwrap(),
                "repositories",
                "add",
                f.repo.to_str().unwrap(),
            ],
        );
        assert_eq!(
            preview(&f, json!({"method":"publish_history"}), false)["receipts"][0]["phase"],
            "created"
        );
        assert_eq!(
            preview(
                &f,
                json!({"method":"publish_preview","request":request}),
                false
            )["can_create"],
            false
        );
        assert_eq!(
            fs::read_to_string(f.root.join("post-log")).unwrap(),
            "POST\n"
        );
    }
}

fn start_creation(f: &Fixture, request: &Value, plan: &Value) -> Child {
    let mut child = f
        .preview_command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(json!({"method":"publish_create","request":request,"basis":plan["basis"],"acknowledge_partial":true}).to_string().as_bytes()).unwrap();
    child
}
fn finish_child(mut child: Child) -> std::process::Output {
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    if child.try_wait().unwrap().is_none() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("publication child timed out");
    }
    child.wait_with_output().unwrap()
}

#[test]
fn cancelled_or_concurrent_creation_never_repeats_post() {
    for kind in ["cancel", "concurrent"] {
        let f = Fixture::new();
        f.install_stub();
        fs::write(
            f.root.join("repository-response.json"),
            repository_response("alicecase", 101).to_string(),
        )
        .unwrap();
        let mut request = f.request()["request"].clone();
        request["owner"] = json!("alicecase");
        request["target"] = json!({"mode":"assigned","host":"github.com","login":"AliceCase"});
        let plan = preview(
            &f,
            json!({"method":"publish_preview","request":request}),
            false,
        );
        let before = f.snapshot();
        fs::write(f.root.join("post-gate"), "on").unwrap();
        let mut first = start_creation(&f, &request, &plan);
        wait_for(
            &f.root.join("post-started"),
            &mut first,
            Duration::from_secs(15),
        );
        let registry: Value = serde_json::from_slice(&fs::read(&f.registry).unwrap()).unwrap();
        assert_eq!(registry["publication_intents"].as_array().unwrap().len(), 1);
        assert_eq!(
            registry["publication_intents"][0]["phase"],
            "creation_unknown"
        );
        assert_eq!(
            preview(&f, json!({"method":"publish_history"}), false)["receipts"][0]["phase"],
            "creation_unknown"
        );
        if kind == "cancel" {
            f.run("kill", &["-INT", &first.id().to_string()]);
            let result = finish_child(first);
            assert!(!result.status.success());
            assert!(result.stdout.is_empty());
            fs::write(f.root.join("post-release"), "release").unwrap();
            assert_eq!(
                preview(&f, json!({"method":"publish_history"}), false)["receipts"][0]["phase"],
                "creation_unknown"
            );
            preview(
                &f,
                json!({"method":"publish_create","request":request,"basis":plan["basis"],"acknowledge_partial":true}),
                true,
            );
        } else {
            let second = start_creation(&f, &request, &plan);
            fs::write(f.root.join("post-release"), "release").unwrap();
            let first = finish_child(first);
            assert!(
                first.status.success(),
                "{}",
                String::from_utf8_lossy(&first.stderr)
            );
            assert_eq!(
                serde_json::from_slice::<Value>(&first.stdout).unwrap()["phase"],
                "created"
            );
            let second = finish_child(second);
            assert!(!second.status.success());
            assert!(second.stdout.is_empty());
        }
        assert_eq!(
            fs::read_to_string(f.root.join("post-log")).unwrap(),
            "POST\n"
        );
        assert_eq!(&f.snapshot()[..4], &before[..4]);
        assert_eq!(
            fs::metadata(&f.registry).unwrap().permissions().mode() & 0o077,
            0
        );
    }
}

#[test]
fn failed_or_untrusted_provider_responses_leave_unknown_receipt() {
    for kind in ["failure", "id", "owner-id", "name", "host", "token"] {
        let f = Fixture::new();
        f.install_stub();
        let mut response = repository_response("alicecase", 101);
        match kind {
            "failure" => fs::write(f.root.join("post-fail"), "on").unwrap(),
            "id" => response["id"] = json!(9007199254740992u64),
            "owner-id" => response["owner"]["id"] = json!(999),
            "name" => response["full_name"] = json!("other/example-repo"),
            "host" => {
                response["html_url"] = json!("https://private.invalid/alicecase/example-repo")
            }
            _ => {
                response["clone_url"] =
                    json!("https://github.com/alicecase/example-repo.git?token=assigned-secret")
            }
        }
        fs::write(
            f.root.join("repository-response.json"),
            response.to_string(),
        )
        .unwrap();
        let mut request = f.request()["request"].clone();
        request["owner"] = json!("alicecase");
        request["target"] = json!({"mode":"assigned","host":"github.com","login":"AliceCase"});
        let plan = preview(
            &f,
            json!({"method":"publish_preview","request":request}),
            false,
        );
        let before = f.snapshot();
        let mut child = start_creation(&f, &request, &plan);
        let deadline = Instant::now() + Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        if child.try_wait().unwrap().is_none() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("provider case timed out");
        }
        let output = child.wait_with_output().unwrap();
        if output.status.success() {
            assert_eq!(
                serde_json::from_slice::<Value>(&output.stdout).unwrap()["phase"],
                "creation_unknown"
            );
        }
        assert!(!String::from_utf8_lossy(&output.stderr).contains("assigned-secret"));
        assert_eq!(
            preview(&f, json!({"method":"publish_history"}), false)["receipts"][0]["phase"],
            "creation_unknown"
        );
        preview(
            &f,
            json!({"method":"publish_create","request":request,"basis":plan["basis"],"acknowledge_partial":true}),
            true,
        );
        assert_eq!(
            fs::read_to_string(f.root.join("post-log")).unwrap(),
            "POST\n"
        );
        assert_eq!(&f.snapshot()[..4], &before[..4]);
    }
}

#[test]
fn publication_creation_rejects_preflight_and_physical_repository_races() {
    for kind in ["remote", "actor", "account", "gitdir", "worktree"] {
        let f = Fixture::new();
        f.install_stub();
        fs::write(
            f.root.join("repository-response.json"),
            repository_response("alicecase", 101).to_string(),
        )
        .unwrap();
        let mut request = f.request()["request"].clone();
        request["owner"] = json!("alicecase");
        request["target"] = json!({"mode":"assigned","host":"github.com","login":"AliceCase"});
        let plan = preview(
            &f,
            json!({"method":"publish_preview","request":request}),
            false,
        );
        let before = f.snapshot();
        let replacement = f.root.join("replacement");
        let parked = f.root.join("parked");
        if kind == "gitdir" || kind == "worktree" {
            f.run(
                "cp",
                &[
                    "-a",
                    f.repo.to_str().unwrap(),
                    replacement.to_str().unwrap(),
                ],
            );
        }
        fs::write(f.root.join("gh-delay"), "on").unwrap();
        let mut child = start_creation(&f, &request, &plan);
        wait_for(
            &f.root.join("gh-started"),
            &mut child,
            Duration::from_secs(15),
        );
        match kind {
            "remote" => {
                f.git(&[
                    "remote",
                    "add",
                    "upstream",
                    "https://github.com/fork/repo.git",
                ]);
            }
            "actor" => fs::write(
                f.root.join("assigned-user.json"),
                r#"{"id":999,"login":"alicecase"}"#,
            )
            .unwrap(),
            "account" => {
                let mut registry: Value =
                    serde_json::from_slice(&fs::read(&f.registry).unwrap()).unwrap();
                let row = registry["records"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|r| r["path"] == f.repo.to_str().unwrap())
                    .unwrap();
                let account = &mut row["account"];
                account["assignment"] = json!({"mode":"none"});
                account["association"] = Value::Null;
                account["source_remote"] = Value::Null;
                account["revision"] = json!(account["revision"].as_u64().unwrap() + 1);
                fs::write(&f.registry, registry.to_string()).unwrap();
                fs::set_permissions(&f.registry, fs::Permissions::from_mode(0o600)).unwrap();
            }
            "gitdir" => {
                fs::rename(f.repo.join(".git"), &parked).unwrap();
                fs::rename(replacement.join(".git"), f.repo.join(".git")).unwrap();
            }
            _ => {
                fs::rename(&f.repo, &parked).unwrap();
                fs::rename(&replacement, &f.repo).unwrap();
            }
        }
        let after = f.snapshot();
        if kind == "gitdir" || kind == "worktree" {
            assert_eq!(after, before);
        }
        fs::write(f.root.join("gh-release"), "release").unwrap();
        let output = finish_child(child);
        assert!(!output.status.success(), "{kind}");
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("assigned-secret"));
        assert_eq!(f.snapshot(), after);
        assert!(!f.root.join("post-log").exists());
        let registry: Value = serde_json::from_slice(&fs::read(&f.registry).unwrap()).unwrap();
        assert!(
            registry["publication_intents"]
                .as_array()
                .is_none_or(|v| v.is_empty())
        );
    }
}

#[test]
fn changed_organization_after_intent_blocks_post() {
    let f = Fixture::new();
    f.install_stub();
    fs::write(
        f.root.join("orgs.json"),
        r#"[{"login":"ExampleOrg","id":987}]"#,
    )
    .unwrap();
    fs::write(
        f.root.join("repository-response.json"),
        repository_response("ExampleOrg", 987).to_string(),
    )
    .unwrap();
    let mut request = f.request()["request"].clone();
    request["owner"] = json!("ExampleOrg");
    request["target"] = json!({"mode":"assigned","host":"github.com","login":"AliceCase"});
    let plan = preview(
        &f,
        json!({"method":"publish_preview","request":request}),
        false,
    );
    fs::write(f.root.join("gh-log"), "").unwrap();
    fs::write(f.root.join("final-gate"), "on").unwrap();
    let mut child = start_creation(&f, &request, &plan);
    wait_for(
        &f.root.join("final-started"),
        &mut child,
        Duration::from_secs(15),
    );
    let data: Value = serde_json::from_slice(&fs::read(&f.registry).unwrap()).unwrap();
    assert_eq!(data["publication_intents"][0]["phase"], "creation_unknown");
    fs::write(
        f.root.join("orgs.json"),
        r#"[{"login":"ExampleOrg","id":999}]"#,
    )
    .unwrap();
    fs::write(f.root.join("post-release"), "release").unwrap();
    let output = finish_child(child);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["phase"],
        "creation_unknown"
    );
    assert!(!f.root.join("post-log").exists());
    assert_eq!(
        preview(&f, json!({"method":"publish_history"}), false)["receipts"][0]["phase"],
        "creation_unknown"
    );
}

fn create_unknown(f: &Fixture) -> Value {
    f.install_stub();
    fs::write(f.root.join("post-fail"), "on").unwrap();
    fs::write(
        f.root.join("repository-response.json"),
        repository_response("alicecase", 101).to_string(),
    )
    .unwrap();
    let mut request = f.request()["request"].clone();
    request["owner"] = json!("alicecase");
    request["target"] = json!({"mode":"assigned","host":"github.com","login":"AliceCase"});
    let plan = preview(
        f,
        json!({"method":"publish_preview","request":request}),
        false,
    );
    let result = preview(
        f,
        json!({"method":"publish_create","request":request,"basis":plan["basis"],"acknowledge_partial":true}),
        false,
    );
    assert_eq!(result["phase"], "creation_unknown");
    request
}

#[test]
fn unknown_publication_blocks_related_worktrees_and_relocated_repository() {
    for kind in ["linked", "moved"] {
        let mut f = Fixture::new();
        let request = create_unknown(&f);
        let old = f.repo.clone();
        if kind == "linked" {
            let linked = f.root.join("linked publish worktree");
            f.git(&["worktree", "add", "-qb", "linked", linked.to_str().unwrap()]);
            f.run(
                env!("CARGO_BIN_EXE_desktop-rs"),
                &[
                    "--config",
                    f.registry.to_str().unwrap(),
                    "repositories",
                    "add",
                    linked.to_str().unwrap(),
                ],
            );
            f.repo = linked;
        } else {
            let moved = f.root.join("moved repository");
            fs::rename(&old, &moved).unwrap();
            let mut data: Value = serde_json::from_slice(&fs::read(&f.registry).unwrap()).unwrap();
            for item in data["repositories"].as_array_mut().unwrap() {
                if item == old.to_str().unwrap() {
                    *item = json!(moved);
                }
            }
            if data["selected"] == old.to_str().unwrap() {
                data["selected"] = json!(moved);
            }
            for record in data["records"].as_array_mut().unwrap() {
                if record["path"] == old.to_str().unwrap() {
                    record["path"] = json!(moved);
                }
            }
            fs::write(&f.registry, data.to_string()).unwrap();
            fs::set_permissions(&f.registry, fs::Permissions::from_mode(0o600)).unwrap();
            f.repo = moved;
        }
        assert_eq!(
            preview(
                &f,
                json!({"method":"publish_preview","request":request}),
                false
            )["can_create"],
            false,
            "{kind}"
        );
        assert_eq!(
            preview(&f, json!({"method":"publish_history"}), false)["receipts"][0]["phase"],
            "creation_unknown"
        );
        assert_eq!(
            fs::read_to_string(f.root.join("post-log")).unwrap(),
            "POST\n"
        );
    }
}

#[test]
fn unsafe_registry_locations_cannot_authorize_publication() {
    use std::os::unix::fs::symlink;
    for kind in ["worktree", "gitdir", "symlink", "linked-common"] {
        let mut f = Fixture::new();
        f.install_stub();
        let original = f.repo.clone();
        let config = match kind {
            "worktree" => original.join("metadata/registry.json"),
            "gitdir" => original.join(".git/unsafe/registry.json"),
            "symlink" => {
                let outside = f.root.join("outside-registry");
                fs::create_dir(&outside).unwrap();
                fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).unwrap();
                let alias = f.root.join("registry-alias");
                symlink(outside, &alias).unwrap();
                alias.join("registry.json")
            }
            _ => {
                let linked = f.root.join("unsafe linked worktree");
                f.git(&[
                    "worktree",
                    "add",
                    "-qb",
                    "unsafe-linked",
                    linked.to_str().unwrap(),
                ]);
                f.repo = linked;
                original.join(".git/common-registry/registry.json")
            }
        };
        f.run(
            env!("CARGO_BIN_EXE_desktop-rs"),
            &[
                "--config",
                config.to_str().unwrap(),
                "repositories",
                "add",
                f.repo.to_str().unwrap(),
            ],
        );
        f.registry = config;
        let mut request = f.request()["request"].clone();
        request["owner"] = json!("alicecase");
        request["target"] = json!({"mode":"assigned","host":"github.com","login":"AliceCase"});
        let plan = preview(
            &f,
            json!({"method":"publish_preview","request":request}),
            false,
        );
        assert_eq!(plan["can_create"], false, "{kind}");
        if kind == "worktree" || kind == "gitdir" {
            preview(
                &f,
                json!({"method":"publish_create","request":request,"basis":plan["basis"],"acknowledge_partial":true}),
                true,
            );
        }
        assert!(!f.root.join("post-log").exists());
    }
}

#[test]
fn durable_creation_receipt_wins_over_forged_legacy_record() {
    use sha2::{Digest, Sha256};
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new();
    f.install_stub();
    let id = create_publication(&f, "alicecase", 101, false);
    let registry: Value = serde_json::from_slice(&fs::read(&f.registry).unwrap()).unwrap();
    let record = &registry["records"][0];
    let hash = |v: &[u8]| format!("{:x}", Sha256::digest(v));
    let namespace = hash(f.registry.as_os_str().as_bytes());
    let dir_id = hash(json!([namespace, record["id"]]).to_string().as_bytes());
    let base = f.repo.join(".git/desktop-publish-receipts");
    let directory = base.join(dir_id);
    fs::create_dir_all(&directory).unwrap();
    fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let forged = json!({"version":1,"id":id,"repository_id":hash(f.repo.as_os_str().as_bytes()),"namespace":namespace,"record_id":record["id"],"phase":"complete","host":"github.com","owner":"ExampleOrg","name":"example-repo","error":null,"acknowledged":false});
    let path = directory.join(format!("{id}.json"));
    fs::write(&path, forged.to_string()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let history = preview(&f, json!({"method":"publish_history"}), false);
    assert_eq!(history["receipts"].as_array().unwrap().len(), 1);
    assert_eq!(history["receipts"][0]["id"], id);
    assert_eq!(history["receipts"][0]["phase"], "created");
    assert_eq!(
        fs::read_to_string(f.root.join("post-log")).unwrap(),
        "POST\n"
    );
}
