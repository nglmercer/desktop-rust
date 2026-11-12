//! Verified upstream setup with isolated local Git configuration.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Suite {
    root: PathBuf,
    env: HashMap<OsString, OsString>,
}
impl Suite {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("desktop-upstream-setup-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self {
            root,
            env: HashMap::new(),
        }
    }
    fn command(&self, program: &str) -> Command {
        let mut cmd = Command::new(program);
        cmd.env("HOME", &self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("global.config"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .envs(&self.env);
        cmd
    }
    fn run<I, S>(&self, program: &str, args: I, input: Option<&[u8]>, fail: bool) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = self.command(program);
        cmd.args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        if let Some(input) = input {
            child.stdin.take().unwrap().write_all(input).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert_eq!(
            out.status.success(),
            !fail,
            "stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }
    fn git<I, S>(&self, repo: &Path, args: I, fail: bool) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut argv = vec![OsString::from("-C"), repo.as_os_str().into()];
        argv.extend(args.into_iter().map(|v| v.as_ref().to_owned()));
        self.run("git", argv, None, fail)
    }
    fn text(&self, repo: &Path, args: &[&str]) -> String {
        String::from_utf8(self.git(repo, args, false).stdout)
            .unwrap()
            .trim()
            .to_owned()
    }
    fn init(&self, name: &str, options: &[&str]) -> PathBuf {
        let repo = self.root.join(name);
        let mut args: Vec<OsString> = ["init", "-q", "-b", "main"]
            .into_iter()
            .map(Into::into)
            .collect();
        args.extend(options.iter().map(OsString::from));
        args.push(repo.as_os_str().into());
        self.run("git", args, None, false);
        self.git(&repo, ["config", "user.name", "Conflict Test"], false);
        self.git(
            &repo,
            ["config", "user.email", "conflict@example.invalid"],
            false,
        );
        self.git(&repo, ["config", "commit.gpgsign", "false"], false);
        repo
    }
    fn commit(&self, repo: &Path) {
        self.git(repo, ["add", "--all"], false);
        self.git(repo, ["commit", "-qm", "checkpoint"], false);
    }
    fn rpc_raw(&self, repo: &Path, method: &str, mut body: Value, fail: bool) -> Output {
        body["method"] = json!(method);
        self.run(
            env!("CARGO_BIN_EXE_desktop-rs"),
            [
                OsString::from("-C"),
                repo.as_os_str().into(),
                OsString::from("repo"),
                OsString::from("-"),
            ],
            Some(body.to_string().as_bytes()),
            fail,
        )
    }
    fn rpc(&self, repo: &Path, method: &str, body: Value) -> Value {
        serde_json::from_slice(&self.rpc_raw(repo, method, body, false).stdout).unwrap()
    }
}
impl Drop for Suite {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

const CANONICAL: &str = "https://github.example/parent/repo.git";
const SECRET: &str = "SETUP_PRIVATE_SECRET_6c19";
fn fixture() -> Suite {
    let mut s = Suite::new();
    let bin = s.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let response = s.root.join("response.json");
    fs::write(&response,json!({"nameWithOwner":"fork/repo","url":"https://github.example/fork/repo","isFork":true,"parent":{"nameWithOwner":"parent/repo","url":"https://github.example/parent/repo"}}).to_string()).unwrap();
    let gh = bin.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$GH_LOG\"\nif [ -e \"$GH_GATE\" ]; then printf started > \"$GH_STARTED\"; while [ ! -e \"$GH_RELEASE\" ]; do sleep 0.02; done; fi\nif [ -e \"$GH_ERROR\" ]; then cat \"$GH_ERROR\" >&2; exit 4; fi\ncat \"$GH_RESPONSE\"\n",
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o700)).unwrap();
    let real = String::from_utf8(s.run("sh", ["-c", "command -v git"], None, false).stdout)
        .unwrap()
        .trim()
        .to_owned();
    let git = bin.join("git");
    fs::write(&git,"#!/bin/sh\nread_remote=0; get_url=0\nfor arg do case \"$arg\" in fetch|push|pull|clone) printf attempted > \"$GIT_TRANSFER\"; exit 97;; ls-remote) read_remote=1;; --get-url) get_url=1;; esac; done\nif [ \"$read_remote\" = 1 ] && [ \"$get_url\" != 1 ]; then printf attempted > \"$GIT_TRANSFER\"; exit 97; fi\ncase \"$*\" in *'remote add -- upstream '*|*'remote add upstream '*) printf 'add\\n' >> \"$GIT_ADDS\"; case \"${GIT_MODE-}\" in fail_before) printf '%s' \"$GIT_PRIVATE_DIAG\" >&2; exit 1;; partial_url) \"$REAL_GIT\" config remote.upstream.url \"$PARENT_URL\" || exit; printf '%s' \"$GIT_PRIVATE_DIAG\" >&2; exit 1;; fail_after) \"$REAL_GIT\" \"$@\" || exit; printf '%s' \"$GIT_PRIVATE_DIAG\" >&2; exit 1;; esac;; esac\nexec \"$REAL_GIT\" \"$@\"\n").unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o700)).unwrap();
    let paths = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    for (key, value) in [
        ("PATH", paths),
        ("REAL_GIT", real.into()),
        ("GH_RESPONSE", response.into_os_string()),
        ("GH_ERROR", s.root.join("gh-error").into_os_string()),
        ("GH_LOG", s.root.join("gh-log").into_os_string()),
        ("GH_GATE", s.root.join("gh-gate").into_os_string()),
        ("GH_STARTED", s.root.join("gh-started").into_os_string()),
        ("GH_RELEASE", s.root.join("gh-release").into_os_string()),
        ("GIT_TRANSFER", s.root.join("git-transfer").into_os_string()),
        ("GIT_ADDS", s.root.join("git-adds").into_os_string()),
        ("PARENT_URL", CANONICAL.into()),
        (
            "GIT_PRIVATE_DIAG",
            format!("https://private:{SECRET}@elsewhere.invalid/repo?token={SECRET}").into(),
        ),
    ] {
        s.env.insert(key.into(), value);
    }
    s
}
fn fork(s: &Suite, name: &str, options: &[&str]) -> PathBuf {
    let repo = s.init(name, options);
    fs::write(repo.join("file"), b"base\n").unwrap();
    s.commit(&repo);
    s.git(
        &repo,
        [
            "remote",
            "add",
            "origin",
            "https://github.example/fork/repo.git",
        ],
        false,
    );
    repo
}
fn verified_missing(s: &Suite, name: &str, options: &[&str]) -> PathBuf {
    let repo = fork(s, name, options);
    assert_eq!(
        s.rpc(&repo, "refresh_repository_context", json!({}))["status"],
        "verified"
    );
    s.git(&repo, ["remote", "remove", "upstream"], false);
    repo
}
fn state(s: &Suite, repo: &Path) -> [Vec<u8>; 4] {
    let index = s.text(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    );
    [
        s.git(
            repo,
            [
                "for-each-ref",
                "--format=%(refname)%00%(objectname)%00%(symref)",
            ],
            false,
        )
        .stdout,
        s.git(repo, ["symbolic-ref", "--quiet", "HEAD"], false)
            .stdout,
        fs::read(index).unwrap_or_default(),
        fs::read(repo.join("file")).unwrap(),
    ]
}
fn review(s: &Suite, repo: &Path) -> Value {
    s.rpc(repo, "upstream_setup_preview", json!({}))
}
fn setup(s: &Suite, repo: &Path, basis: &Value, fail: bool) -> Output {
    s.rpc_raw(repo, "setup_upstream", json!({"basis":basis}), fail)
}
fn clean(value: &str) {
    for secret in [SECRET, "private:", "token="] {
        assert!(!value.contains(secret));
    }
}

#[test]
fn automatic_and_reviewed_setup_preserve_dirty_worktree() {
    let s = fixture();
    let repo = fork(&s, "automatic", &[]);
    fs::write(repo.join("file"), b"index data\n").unwrap();
    s.git(&repo, ["add", "--", "file"], false);
    fs::write(repo.join("file"), b"working data\n").unwrap();
    fs::write(repo.join("untracked"), b"keep\0").unwrap();
    let before = state(&s, &repo);
    let result = s.rpc(&repo, "refresh_repository_context", json!({}));
    assert_eq!(result["status"], "verified");
    assert!(result["upstream_setup_error"].is_null());
    assert_eq!(
        s.text(&repo, &["config", "--get-all", "remote.upstream.url"]),
        CANONICAL
    );
    assert_eq!(
        s.text(&repo, &["config", "--get-all", "remote.upstream.fetch"]),
        "+refs/heads/*:refs/remotes/upstream/*"
    );
    assert_eq!(state(&s, &repo), before);
    assert_eq!(fs::read(repo.join("untracked")).unwrap(), b"keep\0");
    assert!(
        s.git(
            &repo,
            [
                "for-each-ref",
                "--format=%(refname)",
                "refs/remotes/upstream"
            ],
            false
        )
        .stdout
        .is_empty()
    );
    s.git(&repo, ["remote", "remove", "upstream"], false);
    let before = state(&s, &repo);
    let config = fs::read(repo.join(".git/config")).unwrap();
    let plan = review(&s, &repo);
    assert_eq!(plan["status"], "add");
    assert_eq!(plan["remote"], "upstream");
    assert_eq!(plan["url"], CANONICAL);
    assert_eq!(
        plan["parent"],
        json!({"host":"github.example","owner":"parent","name":"repo"})
    );
    s.rpc_raw(
        &repo,
        "setup_upstream",
        json!({"basis":plan["basis"],"url":"https://elsewhere.invalid/untrusted/repo.git"}),
        true,
    );
    assert_eq!(fs::read(repo.join(".git/config")).unwrap(), config);
    let output = setup(&s, &repo, &plan["basis"], false);
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["status"], "added");
    assert_eq!(result["context"]["status"], "verified");
    assert_eq!(state(&s, &repo), before);
    let after = fs::read(repo.join(".git/config")).unwrap();
    assert_eq!(
        s.rpc(&repo, "setup_upstream", json!({}))["status"],
        "already_configured"
    );
    assert_eq!(fs::read(repo.join(".git/config")).unwrap(), after);
    setup(&s, &repo, &plan["basis"], true);
    assert_eq!(fs::read(repo.join(".git/config")).unwrap(), after);
    assert!(!s.root.join("git-transfer").exists());
}

#[test]
fn occupied_upstream_configuration_and_url_rewrites_are_blocked_privately() {
    for kind in [
        "local-push",
        "local-reset",
        "local-proxy",
        "wrong-url",
        "global",
        "worktree",
        "conditional",
        "command",
        "rewrite",
    ] {
        let mut s = fixture();
        let repo = verified_missing(&s, kind, &[]);
        let value = format!("https://private:{SECRET}@other.invalid/owner/repo.git?token={SECRET}");
        match kind {
            "local-push" => {
                s.git(&repo, ["config", "remote.upstream.pushurl", &value], false);
            }
            "local-reset" => {
                s.git(&repo, ["config", "remote.upstream.url", ""], false);
            }
            "local-proxy" => {
                s.git(&repo, ["config", "remote.upstream.proxy", &value], false);
            }
            "wrong-url" => {
                s.git(&repo, ["remote", "add", "upstream", &value], false);
            }
            "global" => {
                s.git(
                    &repo,
                    ["config", "--global", "remote.upstream.pushurl", &value],
                    false,
                );
            }
            "conditional" => {
                let include = s.root.join("conditional.cfg");
                fs::write(
                    &include,
                    format!("[remote \"upstream\"]\n pushurl = {value}\n"),
                )
                .unwrap();
                fs::set_permissions(&include, fs::Permissions::from_mode(0o600)).unwrap();
                s.git(
                    &repo,
                    [
                        "config",
                        "--global",
                        &format!("includeIf.gitdir:{}/.git.path", repo.display()),
                        include.to_str().unwrap(),
                    ],
                    false,
                );
                assert_eq!(s.text(&repo, &["config", "remote.upstream.pushurl"]), value);
            }
            "command" => {
                s.env.insert("GIT_CONFIG_COUNT".into(), "1".into());
                s.env
                    .insert("GIT_CONFIG_KEY_0".into(), "remote.upstream.pushurl".into());
                s.env
                    .insert("GIT_CONFIG_VALUE_0".into(), value.clone().into());
            }
            "worktree" => {
                s.git(
                    &repo,
                    ["config", "extensions.worktreeConfig", "true"],
                    false,
                );
                s.git(
                    &repo,
                    ["config", "--worktree", "remote.upstream.pushurl", &value],
                    false,
                );
            }
            _ => {
                s.git(
                    &repo,
                    [
                        "config",
                        "url.https://elsewhere.invalid/.insteadOf",
                        "https://github.example/parent/",
                    ],
                    false,
                );
            }
        }
        let before = state(&s, &repo);
        let config = fs::read(repo.join(".git/config")).unwrap();
        let plan = review(&s, &repo);
        assert_eq!(plan["status"], "blocked", "{kind}");
        clean(&plan.to_string());
        let failure = setup(&s, &repo, &plan["basis"], true);
        clean(&String::from_utf8_lossy(&failure.stderr));
        let refreshed = s.rpc(&repo, "refresh_repository_context", json!({}));
        assert_eq!(refreshed["status"], "verified");
        assert!(refreshed["upstream_setup_error"].is_string());
        clean(&refreshed.to_string());
        assert_eq!(state(&s, &repo), before);
        assert_eq!(fs::read(repo.join(".git/config")).unwrap(), config);
        assert!(!s.root.join("git-transfer").exists());
    }
}

#[test]
fn matching_upstream_retains_private_configuration_and_unrelated_namespaces() {
    let s = fixture();
    let repo = verified_missing(&s, "matching", &[]);
    s.git(
        &repo,
        [
            "remote",
            "add",
            "upstream",
            "git@GitHub.Example:Parent/Repo.git",
        ],
        false,
    );
    s.git(
        &repo,
        [
            "config",
            "--replace-all",
            "remote.upstream.fetch",
            "+refs/heads/trunk:refs/remotes/upstream/custom",
        ],
        false,
    );
    s.git(
        &repo,
        [
            "config",
            "--add",
            "remote.upstream.fetch",
            "^refs/heads/excluded",
        ],
        false,
    );
    let private = format!("https://private:{SECRET}@other.invalid/other/repo.git?token={SECRET}");
    s.git(
        &repo,
        ["config", "--add", "remote.upstream.url", &private],
        false,
    );
    s.git(
        &repo,
        ["config", "--add", "remote.upstream.pushurl", &private],
        false,
    );
    let before = state(&s, &repo);
    let config = fs::read(repo.join(".git/config")).unwrap();
    let plan = review(&s, &repo);
    assert_eq!(plan["status"], "already_configured");
    clean(&plan.to_string());
    let result: Value =
        serde_json::from_slice(&setup(&s, &repo, &plan["basis"], false).stdout).unwrap();
    assert_eq!(result["status"], "already_configured");
    assert_eq!(state(&s, &repo), before);
    assert_eq!(fs::read(repo.join(".git/config")).unwrap(), config);
    let repo = verified_missing(&s, "neighbors", &[]);
    s.git(
        &repo,
        [
            "remote",
            "add",
            "upstream.extra",
            "https://neighbor.invalid/owner/repo.git",
        ],
        false,
    );
    s.git(
        &repo,
        ["config", "remote.upstream.extra.pushurl", &private],
        false,
    );
    let before = state(&s, &repo);
    let plan = review(&s, &repo);
    assert_eq!(plan["status"], "add");
    s.git(
        &repo,
        [
            "remote",
            "set-url",
            "upstream.extra",
            "https://changed.invalid/owner/repo.git",
        ],
        false,
    );
    s.git(
        &repo,
        [
            "config",
            "remote.upstream.extra.fetch",
            "+refs/heads/only:refs/remotes/upstream.extra/only",
        ],
        false,
    );
    assert_eq!(review(&s, &repo)["basis"], plan["basis"]);
    let result: Value =
        serde_json::from_slice(&setup(&s, &repo, &plan["basis"], false).stdout).unwrap();
    assert_eq!(result["status"], "added");
    assert_eq!(state(&s, &repo), before);
    assert_eq!(
        s.text(&repo, &["config", "remote.upstream.extra.url"]),
        "https://changed.invalid/owner/repo.git"
    );
}

#[test]
fn stale_review_and_foreign_config_lock_do_not_mutate() {
    for kind in ["namespace", "origin", "preference", "cache", "lock"] {
        let s = fixture();
        let repo = verified_missing(&s, kind, &[]);
        let plan = review(&s, &repo);
        assert_eq!(plan["status"], "add");
        match kind {
            "namespace" => {
                s.git(
                    &repo,
                    [
                        "config",
                        "remote.upstream.pushurl",
                        "https://elsewhere.invalid/other/repo.git",
                    ],
                    false,
                );
            }
            "origin" => {
                s.git(
                    &repo,
                    [
                        "remote",
                        "set-url",
                        "origin",
                        "https://github.example/new/repo.git",
                    ],
                    false,
                );
            }
            "preference" => {
                let ctx = s.rpc(&repo, "repository_context", json!({}));
                s.rpc(
                    &repo,
                    "set_fork_contribution_target",
                    json!({"target":"self","basis":ctx["basis"]}),
                );
            }
            "cache" => {
                let path = repo.join(".git/desktop-repository.json");
                let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                value["revision"] = json!(value["revision"].as_u64().unwrap() + 1);
                fs::write(&path, value.to_string()).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
            }
            _ => {
                fs::write(repo.join(".git/config.lock"), b"outside-owned config lock").unwrap();
            }
        }
        let before = state(&s, &repo);
        let config = fs::read(repo.join(".git/config")).unwrap();
        setup(&s, &repo, &plan["basis"], true);
        assert_eq!(state(&s, &repo), before);
        assert_eq!(fs::read(repo.join(".git/config")).unwrap(), config);
        if kind == "lock" {
            assert_eq!(
                fs::read(repo.join(".git/config.lock")).unwrap(),
                b"outside-owned config lock"
            );
            fs::remove_file(repo.join(".git/config.lock")).unwrap();
            let result: Value =
                serde_json::from_slice(&setup(&s, &repo, &plan["basis"], false).stdout).unwrap();
            assert_eq!(result["status"], "added");
        }
    }
}

#[test]
fn native_ref_formats_and_linked_worktrees_preserve_state() {
    let s = fixture();
    for (kind, options) in [
        ("sha256", vec!["--object-format=sha256"]),
        ("reftable", vec!["--ref-format=reftable"]),
        ("packed", vec![]),
        ("broken-index", vec![]),
    ] {
        let repo = verified_missing(&s, kind, &options);
        if kind == "packed" {
            s.git(&repo, ["pack-refs", "--all", "--prune"], false);
        }
        if kind == "broken-index" {
            fs::write(repo.join(".git/index"), b"corrupt index preserved").unwrap();
        }
        let before = state(&s, &repo);
        let plan = review(&s, &repo);
        let result: Value =
            serde_json::from_slice(&setup(&s, &repo, &plan["basis"], false).stdout).unwrap();
        assert_eq!(result["status"], "added");
        assert_eq!(state(&s, &repo), before);
        if kind != "broken-index" {
            let tip = s.text(&repo, &["rev-parse", "HEAD"]);
            for (remote, branch) in [("origin", "fork-main"), ("upstream", "parent-main")] {
                s.git(
                    &repo,
                    [
                        "update-ref",
                        &format!("refs/remotes/{remote}/{branch}"),
                        &tip,
                    ],
                    false,
                );
                s.git(
                    &repo,
                    [
                        "symbolic-ref",
                        &format!("refs/remotes/{remote}/HEAD"),
                        &format!("refs/remotes/{remote}/{branch}"),
                    ],
                    false,
                );
            }
            for (remote, branch) in [("origin", "fork-main"), ("upstream", "parent-main")] {
                s.git(&repo, ["branch", branch], false);
                s.git(
                    &repo,
                    [
                        "branch",
                        "--set-upstream-to",
                        &format!("{remote}/{branch}"),
                        branch,
                    ],
                    false,
                );
            }
            let ctx = s.rpc(&repo, "refresh_repository_context", json!({}));
            assert_eq!(
                s.rpc(&repo, "branch_policy", json!({}))["default_branch"]["name"],
                "parent-main"
            );
            s.rpc(
                &repo,
                "set_fork_contribution_target",
                json!({"target":"self","basis":ctx["basis"]}),
            );
            assert_eq!(
                s.rpc(&repo, "branch_policy", json!({}))["default_branch"]["name"],
                "fork-main"
            );
        }
    }
    let repo = verified_missing(&s, "linked-parent", &[]);
    let linked = s.root.join("linked-worktree");
    s.git(
        &repo,
        ["worktree", "add", "-qb", "linked", linked.to_str().unwrap()],
        false,
    );
    let before = state(&s, &linked);
    let plan = review(&s, &linked);
    let result: Value =
        serde_json::from_slice(&setup(&s, &linked, &plan["basis"], false).stdout).unwrap();
    assert_eq!(result["status"], "added");
    assert_eq!(state(&s, &linked), before);
    assert_eq!(review(&s, &repo)["status"], "already_configured");
    assert_eq!(
        s.rpc(&repo, "repository_context", json!({}))["status"],
        "verified"
    );
}

#[test]
fn unverified_and_nonfork_repositories_do_not_add_upstream() {
    let s = fixture();
    let repo = fork(&s, "not-fork", &[]);
    let config = fs::read(repo.join(".git/config")).unwrap();
    assert_eq!(review(&s, &repo)["status"], "blocked");
    s.rpc_raw(&repo, "setup_upstream", json!({}), true);
    assert_eq!(fs::read(repo.join(".git/config")).unwrap(), config);
    assert!(!s.root.join("gh-log").exists());
    fs::write(s.root.join("response.json"),json!({"nameWithOwner":"fork/repo","url":"https://github.example/fork/repo","isFork":false,"parent":null}).to_string()).unwrap();
    assert_eq!(
        s.rpc(&repo, "refresh_repository_context", json!({}))["status"],
        "verified"
    );
    assert_eq!(review(&s, &repo)["status"], "not_fork");
    assert_eq!(
        s.rpc(&repo, "setup_upstream", json!({}))["status"],
        "not_fork"
    );
    assert_eq!(fs::read(repo.join(".git/config")).unwrap(), config);
}

#[test]
fn failed_git_remote_add_preserves_partial_state_without_retry() {
    for mode in ["fail_before", "fail_after", "partial_url"] {
        let mut s = fixture();
        let repo = verified_missing(&s, mode, &[]);
        let before = state(&s, &repo);
        let original = fs::read(repo.join(".git/config")).unwrap();
        let plan = review(&s, &repo);
        let count = fs::read_to_string(s.root.join("git-adds"))
            .unwrap()
            .lines()
            .count();
        s.env.insert("GIT_MODE".into(), mode.into());
        let failure = setup(&s, &repo, &plan["basis"], true);
        clean(&String::from_utf8_lossy(&failure.stderr));
        s.env.remove(OsStr::new("GIT_MODE"));
        assert_eq!(state(&s, &repo), before);
        assert_eq!(
            fs::read_to_string(s.root.join("git-adds"))
                .unwrap()
                .lines()
                .count(),
            count + 1
        );
        if mode == "fail_before" {
            assert_eq!(fs::read(repo.join(".git/config")).unwrap(), original);
            assert_eq!(review(&s, &repo)["status"], "add");
        } else {
            assert_eq!(s.text(&repo, &["config", "remote.upstream.url"]), CANONICAL);
            if mode == "partial_url" {
                s.git(&repo, ["config", "--get", "remote.upstream.fetch"], true);
            }
            assert_eq!(review(&s, &repo)["status"], "already_configured");
            let original = fs::read(repo.join(".git/config")).unwrap();
            assert_eq!(
                s.rpc(&repo, "setup_upstream", json!({}))["status"],
                "already_configured"
            );
            assert_eq!(fs::read(repo.join(".git/config")).unwrap(), original);
            assert_eq!(
                fs::read_to_string(s.root.join("git-adds"))
                    .unwrap()
                    .lines()
                    .count(),
                count + 1
            );
        }
    }
}

#[test]
fn canonical_parent_respects_preference_ports_and_redacts_credentials() {
    let s = fixture();
    let repo = fork(&s, "self-target", &[]);
    let ctx = s.rpc(&repo, "repository_context", json!({}));
    s.rpc(
        &repo,
        "set_fork_contribution_target",
        json!({"target":"self","basis":ctx["basis"]}),
    );
    let before = state(&s, &repo);
    let ctx = s.rpc(&repo, "refresh_repository_context", json!({}));
    assert_eq!(ctx["status"], "verified");
    assert_eq!(ctx["fork_contribution_target"], "self");
    assert_eq!(ctx["contribution_remote"], "origin");
    assert_eq!(state(&s, &repo), before);
    assert_eq!(s.text(&repo, &["config", "remote.upstream.url"]), CANONICAL);
    let repo = fork(&s, "custom-port", &[]);
    s.git(
        &repo,
        [
            "remote",
            "set-url",
            "origin",
            "https://localhost:34567/fork/repo.git",
        ],
        false,
    );
    fs::write(s.root.join("response.json"),json!({"nameWithOwner":"fork/repo","url":"https://localhost:34567/fork/repo","isFork":true,"parent":{"nameWithOwner":"parent/repo","url":"https://localhost:34567/parent/repo"}}).to_string()).unwrap();
    assert_eq!(
        s.rpc(&repo, "refresh_repository_context", json!({}))["status"],
        "verified"
    );
    assert_eq!(
        s.text(&repo, &["config", "remote.upstream.url"]),
        "https://localhost:34567/parent/repo.git"
    );
    let repo = fork(&s, "private-metadata", &[]);
    let url = format!("https://private:{SECRET}@github.example/fork/repo.git?token={SECRET}");
    s.git(&repo, ["remote", "set-url", "origin", &url], false);
    fs::write(s.root.join("response.json"),json!({"nameWithOwner":"fork/repo","url":"https://github.example/fork/repo","isFork":true,"parent":{"nameWithOwner":"Parent/Repo","url":format!("https://private:{SECRET}@GitHub.Example/Parent/Repo.git?token={SECRET}#private")}}).to_string()).unwrap();
    let ctx = s.rpc(&repo, "refresh_repository_context", json!({}));
    clean(&ctx.to_string());
    assert_eq!(s.text(&repo, &["config", "remote.upstream.url"]), CANONICAL);
    clean(&review(&s, &repo).to_string());
}

#[test]
fn cli_upstream_setup_uses_the_reviewed_basis() {
    let s = fixture();
    let repo = verified_missing(&s, "cli", &[]);
    let before = state(&s, &repo);
    let config = fs::read(repo.join(".git/config")).unwrap();
    let command = |args: &[&str], fail| {
        let mut all = vec![
            OsString::from("-C"),
            repo.as_os_str().into(),
            "upstream-setup".into(),
        ];
        all.extend(args.iter().map(OsString::from));
        s.run(env!("CARGO_BIN_EXE_desktop-rs"), all, None, fail)
    };
    let plan: Value = serde_json::from_slice(&command(&["--preview"], false).stdout).unwrap();
    assert_eq!(plan["status"], "add");
    assert_eq!(fs::read(repo.join(".git/config")).unwrap(), config);
    let result: Value = serde_json::from_slice(
        &command(&["--basis", plan["basis"].as_str().unwrap()], false).stdout,
    )
    .unwrap();
    assert_eq!(result["status"], "added");
    assert_eq!(result["code"], 0);
    assert_eq!(state(&s, &repo), before);
}

#[test]
fn cached_fork_selection_survives_nonorigin_remote_and_legacy_cache() {
    for target in ["parent", "self"] {
        let s = fixture();
        let repo = fork(&s, target, &[]);
        s.git(&repo, ["remote", "rename", "origin", "z-fork"], false);
        let ctx = s.rpc(&repo, "repository_context", json!({}));
        s.rpc(
            &repo,
            "set_fork_contribution_target",
            json!({"target":target,"basis":ctx["basis"]}),
        );
        let ctx = s.rpc(&repo, "refresh_repository_context", json!({}));
        assert_eq!(ctx["status"], "verified");
        assert_eq!(ctx["remote"], "z-fork");
        assert_eq!(ctx["fork_contribution_target"], target);
        assert_eq!(
            ctx["contribution_remote"],
            if target == "parent" {
                "upstream"
            } else {
                "z-fork"
            }
        );
        assert_eq!(
            s.rpc(&repo, "refresh_repository_context", json!({}))["status"],
            "verified"
        );
        assert!(
            fs::read_to_string(s.root.join("gh-log"))
                .unwrap()
                .lines()
                .last()
                .unwrap()
                .ends_with("github.example/fork/repo")
        );
        let path = repo.join(".git/desktop-repository.json");
        let mut cache: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        cache["association"]
            .as_object_mut()
            .unwrap()
            .remove("remote_name");
        fs::write(&path, cache.to_string()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        s.git(&repo, ["remote", "remove", "upstream"], false);
        let ctx = s.rpc(&repo, "repository_context", json!({}));
        assert_eq!(ctx["status"], "verified");
        assert_eq!(ctx["remote"], "z-fork");
        let plan = review(&s, &repo);
        assert_eq!(plan["status"], "add");
        let calls = fs::read_to_string(s.root.join("gh-log")).unwrap();
        let result: Value =
            serde_json::from_slice(&setup(&s, &repo, &plan["basis"], false).stdout).unwrap();
        assert_eq!(result["status"], "added");
        assert_eq!(result["context"]["status"], "verified");
        assert_eq!(result["context"]["remote"], "z-fork");
        assert_eq!(fs::read_to_string(s.root.join("gh-log")).unwrap(), calls);
        s.git(
            &repo,
            [
                "remote",
                "add",
                "origin",
                "https://github.example/explicit/repo.git",
            ],
            false,
        );
        let ctx = s.rpc(&repo, "repository_context", json!({}));
        assert_eq!(ctx["remote"], "origin");
        assert_eq!(ctx["status"], "stale");
        let config = fs::read(repo.join(".git/config")).unwrap();
        assert_eq!(review(&s, &repo)["status"], "blocked");
        s.rpc_raw(&repo, "setup_upstream", json!({}), true);
        assert_eq!(fs::read(repo.join(".git/config")).unwrap(), config);
    }
}

#[test]
fn repository_context_is_read_only_until_verified_and_preferences_are_reviewed() {
    let s = fixture();
    let empty = s.init("no-remote", &[]);
    fs::write(empty.join("file"), b"base\n").unwrap();
    s.commit(&empty);
    let context = s.rpc(&empty, "repository_context", json!({}));
    assert_eq!(context["status"], "unverified");
    assert!(context["remote"].is_null());
    assert!(context["contribution_remote"].is_null());
    assert!(context["repository"].is_null());
    assert_eq!(
        s.rpc(&empty, "branch_policy", json!({}))["default_branch"]["name"],
        "main"
    );
    s.rpc_raw(&empty, "refresh_repository_context", json!({}), true);
    for name in [
        "desktop-repository.json",
        "desktop-repository.lock",
        "desktop-repository.id",
    ] {
        assert!(!empty.join(".git").join(name).exists());
    }
    assert!(!s.root.join("gh-log").exists());
    let repo = fork(&s, "readonly", &[]);
    let tip = s.text(&repo, &["rev-parse", "HEAD"]);
    for (remote, branch, url) in [
        ("origin", "fork-main", ""),
        ("upstream", "parent-main", CANONICAL),
    ] {
        if remote == "upstream" {
            s.git(&repo, ["remote", "add", remote, url], false);
        }
        s.git(
            &repo,
            [
                "update-ref",
                &format!("refs/remotes/{remote}/{branch}"),
                &tip,
            ],
            false,
        );
        s.git(
            &repo,
            [
                "symbolic-ref",
                &format!("refs/remotes/{remote}/HEAD"),
                &format!("refs/remotes/{remote}/{branch}"),
            ],
            false,
        );
        s.git(&repo, ["branch", branch], false);
        s.git(
            &repo,
            [
                "branch",
                "--set-upstream-to",
                &format!("{remote}/{branch}"),
                branch,
            ],
            false,
        );
    }
    let record = repo.join(".git/desktop-repository.json");
    let lock = repo.join(".git/desktop-repository.lock");
    let marker = repo.join(".git/desktop-repository.id");
    let before = s
        .git(
            &repo,
            ["for-each-ref", "--format=%(refname) %(objectname)"],
            false,
        )
        .stdout;
    let context = s.rpc(&repo, "repository_context", json!({}));
    assert_eq!(context["status"], "unverified");
    assert_eq!(context["remote"], "origin");
    assert_eq!(context["fork_contribution_target"], "parent");
    assert_eq!(context["contribution_remote"], "origin");
    assert_eq!(
        s.rpc(&repo, "branch_policy", json!({}))["default_branch"]["name"],
        "fork-main"
    );
    assert_eq!(
        s.rpc(&repo, "snapshot", json!({}))["repository_context"],
        context
    );
    assert!(!record.exists() && !lock.exists() && !marker.exists());
    assert!(!s.root.join("gh-log").exists());
    s.rpc_raw(
        &repo,
        "set_fork_contribution_target",
        json!({"target":"self","is_fork":true}),
        true,
    );
    assert!(!record.exists());
    let verified = s.rpc(&repo, "refresh_repository_context", json!({}));
    assert_eq!(verified["status"], "verified");
    assert_eq!(
        verified["repository"]["identity"],
        json!({"host":"github.example","owner":"fork","name":"repo"})
    );
    assert_eq!(verified["upstream_status"], "matches_parent");
    assert_eq!(verified["contribution_remote"], "upstream");
    assert_eq!(
        s.rpc(&repo, "branch_policy", json!({}))["default_branch"]["name"],
        "parent-main"
    );
    for path in [&record, &lock, &marker] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(
        s.git(
            &repo,
            ["for-each-ref", "--format=%(refname) %(objectname)"],
            false
        )
        .stdout,
        before
    );
    let changed = s.rpc(
        &repo,
        "set_fork_contribution_target",
        json!({"target":"self","basis":verified["basis"]}),
    );
    assert_eq!(changed["contribution_remote"], "origin");
    assert_ne!(changed["basis"], verified["basis"]);
    let saved = fs::read(&record).unwrap();
    s.rpc_raw(
        &repo,
        "set_fork_contribution_target",
        json!({"target":"parent","basis":verified["basis"]}),
        true,
    );
    assert_eq!(fs::read(&record).unwrap(), saved);
    let changed = s.rpc(
        &repo,
        "set_fork_contribution_target",
        json!({"target":"parent","basis":changed["basis"]}),
    );
    assert_eq!(changed["contribution_remote"], "upstream");
    let cli = |args: &[&str], fail| {
        let mut command = vec![
            OsString::from("-C"),
            repo.as_os_str().into(),
            OsString::from("repository-context"),
        ];
        command.extend(args.iter().map(OsString::from));
        s.run(env!("CARGO_BIN_EXE_desktop-rs"), command, None, fail)
    };
    let refreshed: Value = serde_json::from_slice(&cli(&["--refresh"], false).stdout).unwrap();
    assert_eq!(refreshed["status"], "verified");
    cli(&["--refresh", "--target", "self"], true);
    let basis = refreshed["basis"].as_str().unwrap();
    let selected: Value =
        serde_json::from_slice(&cli(&["--target", "self", "--basis", basis], false).stdout)
            .unwrap();
    assert_eq!(selected["fork_contribution_target"], "self");
    let basis = selected["basis"].as_str().unwrap();
    cli(&["--target", "parent", "--basis", basis], false);
    s.git(
        &repo,
        ["symbolic-ref", "--delete", "refs/remotes/upstream/HEAD"],
        false,
    );
    assert_eq!(
        s.rpc(&repo, "branch_policy", json!({}))["default_branch"]["name"],
        "main"
    );
    s.git(
        &repo,
        [
            "remote",
            "set-url",
            "upstream",
            "https://github.example/unrelated/repo.git",
        ],
        false,
    );
    let config = fs::read(repo.join(".git/config")).unwrap();
    assert_eq!(
        s.rpc(&repo, "repository_context", json!({}))["upstream_status"],
        "mismatch"
    );
    assert_eq!(
        s.rpc(&repo, "branch_policy", json!({}))["default_branch"]["name"],
        "main"
    );
    assert_eq!(fs::read(repo.join(".git/config")).unwrap(), config);
    s.git(&repo, ["remote", "set-url", "upstream", CANONICAL], false);
    s.git(
        &repo,
        [
            "symbolic-ref",
            "refs/remotes/upstream/HEAD",
            "refs/remotes/upstream/parent-main",
        ],
        false,
    );
    fs::write(s.root.join("response.json"), json!({"nameWithOwner":"fork/repo","url":"https://github.example/fork/repo","isFork":false,"parent":null}).to_string()).unwrap();
    let nonfork = s.rpc(&repo, "refresh_repository_context", json!({}));
    assert!(nonfork["repository"]["parent"].is_null());
    assert_eq!(nonfork["contribution_remote"], "origin");
    assert_eq!(
        s.rpc(&repo, "branch_policy", json!({}))["default_branch"]["name"],
        "fork-main"
    );
}

#[test]
fn repository_identity_changes_expire_cached_metadata_and_do_not_leak_urls() {
    let s = fixture();
    let repo = verified_missing(&s, "stale-context", &[]);
    let record = repo.join(".git/desktop-repository.json");
    let good = fs::read(&record).unwrap();
    s.git(
        &repo,
        [
            "remote",
            "set-url",
            "origin",
            "https://github.example/another/repo.git",
        ],
        false,
    );
    let context = s.rpc(&repo, "repository_context", json!({}));
    assert_eq!(context["status"], "stale");
    assert!(context["repository"].is_null());
    s.rpc_raw(&repo, "refresh_repository_context", json!({}), true);
    assert_eq!(fs::read(&record).unwrap(), good);
    s.git(
        &repo,
        [
            "remote",
            "set-url",
            "origin",
            "ssh://git@github.example:2222/fork/repo.git",
        ],
        false,
    );
    assert_eq!(
        s.rpc(&repo, "repository_context", json!({}))["status"],
        "stale"
    );
    assert_eq!(
        s.rpc(&repo, "refresh_repository_context", json!({}))["status"],
        "verified"
    );
    s.git(
        &repo,
        [
            "config",
            "url.https://github.example/.insteadOf",
            "gh-short:",
        ],
        false,
    );
    s.git(
        &repo,
        ["remote", "set-url", "origin", "gh-short:fork/repo.git"],
        false,
    );
    assert_eq!(
        s.rpc(&repo, "repository_context", json!({}))["status"],
        "stale"
    );
    assert_eq!(
        s.rpc(&repo, "refresh_repository_context", json!({}))["status"],
        "verified"
    );
    s.git(
        &repo,
        [
            "remote",
            "set-url",
            "origin",
            "https://private:password@github.example/fork/repo.git?token=secret#private-fragment",
        ],
        false,
    );
    let context = s
        .rpc(&repo, "refresh_repository_context", json!({}))
        .to_string();
    for secret in ["password", "secret", "private-fragment"] {
        assert!(!context.contains(secret));
        assert!(
            !fs::read_to_string(s.root.join("gh-log"))
                .unwrap()
                .contains(secret)
        );
    }
    let good = fs::read(&record).unwrap();
    let fork = json!({"nameWithOwner":"fork/repo","url":"https://github.example/fork/repo","isFork":true,"parent":{"nameWithOwner":"parent/repo","url":"https://github.example/parent/repo"}});
    let mut invalid = Vec::new();
    for (key, value) in [
        ("url", json!("https://github.example/wrong/repo")),
        ("nameWithOwner", json!("wrong/repo")),
        ("parent", Value::Null),
        ("isFork", json!(false)),
        (
            "parent",
            json!({"nameWithOwner":"parent/repo","url":"https://other.example/parent/repo"}),
        ),
        (
            "parent",
            json!({"nameWithOwner":"fork/repo","url":"https://github.example/fork/repo"}),
        ),
        (
            "parent",
            json!({"nameWithOwner":"../repo","url":"https://github.example/../repo"}),
        ),
    ] {
        let mut response = fork.clone();
        response[key] = value;
        invalid.push(response.to_string());
    }
    invalid.push(json!({"isFork":true}).to_string());
    invalid.push("{broken private-secret".to_owned());
    for response in invalid {
        fs::write(s.root.join("response.json"), response).unwrap();
        let result = s.rpc_raw(&repo, "refresh_repository_context", json!({}), true);
        assert_eq!(fs::read(&record).unwrap(), good);
        let error = String::from_utf8_lossy(&result.stderr);
        for secret in ["password", "private-secret", "token=secret"] {
            assert!(!error.contains(secret));
        }
    }
    let gh_error = s.root.join("gh-error");
    fs::write(
        &gh_error,
        "https://private:password@example.invalid/repo?token=secret",
    )
    .unwrap();
    let result = s.rpc_raw(&repo, "refresh_repository_context", json!({}), true);
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(!error.contains("password") && !error.contains("token=secret"));
    assert_eq!(fs::read(&record).unwrap(), good);
    fs::remove_file(gh_error).unwrap();
    fs::write(s.root.join("response.json"), fork.to_string()).unwrap();
    let local = s.root.join("local-only.git");
    s.git(
        &repo,
        [
            OsStr::new("remote"),
            OsStr::new("set-url"),
            OsStr::new("origin"),
            local.as_os_str(),
        ],
        false,
    );
    let calls = fs::read(s.root.join("gh-log")).unwrap();
    let context = s.rpc(&repo, "repository_context", json!({}));
    assert_eq!(context["status"], "stale");
    assert!(context["repository"].is_null());
    s.rpc_raw(&repo, "refresh_repository_context", json!({}), true);
    assert_eq!(fs::read(s.root.join("gh-log")).unwrap(), calls);
    assert_eq!(fs::read(record).unwrap(), good);
    let unknown = s.init("unrecognized", &[]);
    let local = s.root.join("uncontacted.git");
    s.git(
        &unknown,
        [
            OsStr::new("remote"),
            OsStr::new("add"),
            OsStr::new("origin"),
            local.as_os_str(),
        ],
        false,
    );
    assert_eq!(
        s.rpc(&unknown, "repository_context", json!({}))["status"],
        "unrecognized_remote"
    );
    s.rpc_raw(&unknown, "refresh_repository_context", json!({}), true);
    assert!(!unknown.join(".git/desktop-repository.json").exists());
}

#[test]
fn context_preferences_invalidate_reviews_and_are_shared_by_worktrees() {
    let s = fixture();
    let repo = fork(&s, "review-context", &[]);
    s.git(&repo, ["branch", "feature"], false);
    assert_eq!(
        s.rpc(&repo, "refresh_repository_context", json!({}))["status"],
        "verified"
    );
    let review = s.rpc(&repo, "delete_branch_preview", json!({"name":"feature"}));
    let ctx = s.rpc(&repo, "repository_context", json!({}));
    s.rpc(
        &repo,
        "set_fork_contribution_target",
        json!({"target":"self","basis":ctx["basis"]}),
    );
    s.rpc_raw(
        &repo,
        "delete_branch",
        json!({"name":"feature","force":true,"basis":review["basis"]}),
        true,
    );
    assert!(!repo.join(".git/desktop-branch-deletions").exists());
    let review = s.rpc(
        &repo,
        "rename_branch_preview",
        json!({"name":"feature","new_name":"renamed-feature"}),
    );
    let ctx = s.rpc(&repo, "repository_context", json!({}));
    s.rpc(
        &repo,
        "set_fork_contribution_target",
        json!({"target":"parent","basis":ctx["basis"]}),
    );
    s.rpc_raw(
        &repo,
        "rename_branch",
        json!({"name":"feature","new_name":"renamed-feature","basis":review["basis"]}),
        true,
    );
    assert!(!repo.join(".git/desktop-branch-renames").exists());
    let linked = s.root.join("linked-context");
    s.git(
        &repo,
        ["worktree", "add", "-qb", "linked", linked.to_str().unwrap()],
        false,
    );
    let context = s.rpc(&repo, "repository_context", json!({}));
    assert_eq!(s.rpc(&linked, "repository_context", json!({})), context);
    let changed = s.rpc(
        &linked,
        "set_fork_contribution_target",
        json!({"target":"self","basis":context["basis"]}),
    );
    assert_eq!(s.rpc(&repo, "repository_context", json!({})), changed);
    let index = repo.join(".git/index");
    fs::write(&index, b"corrupt index preserved").unwrap();
    let config = s.root.join("broken-registry.json");
    let prefs = s.root.join("broken-registry.preferences.json");
    fs::write(&prefs, b"{broken").unwrap();
    let command = |args: &[&str]| {
        let mut all = vec![
            OsString::from("-C"),
            repo.as_os_str().into(),
            "--config".into(),
            config.as_os_str().into(),
            "repository-context".into(),
        ];
        all.extend(args.iter().map(OsString::from));
        let out = s.run(env!("CARGO_BIN_EXE_desktop-rs"), all, None, false);
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    let context = command(&[]);
    assert_eq!(context["fork_contribution_target"], "self");
    assert!(!config.exists());
    command(&[
        "--target",
        "parent",
        "--basis",
        context["basis"].as_str().unwrap(),
    ]);
    assert_eq!(fs::read(index).unwrap(), b"corrupt index preserved");
    assert_eq!(fs::read(prefs).unwrap(), b"{broken");
}

#[test]
fn context_relocation_and_legacy_owner_upgrade_preserve_preferences() {
    use sha2::{Digest, Sha256};
    use std::os::unix::ffi::OsStrExt;
    let s = fixture();
    let repo = fork(&s, "relocation", &[]);
    s.rpc(&repo, "refresh_repository_context", json!({}));
    let ctx = s.rpc(&repo, "repository_context", json!({}));
    let changed = s.rpc(
        &repo,
        "set_fork_contribution_target",
        json!({"target":"self","basis":ctx["basis"]}),
    );
    let marker = fs::read(repo.join(".git/desktop-repository.id")).unwrap();
    let calls = fs::read_to_string(s.root.join("gh-log")).unwrap();
    let moved = s.root.join("relocated-repository");
    fs::rename(&repo, &moved).unwrap();
    assert_eq!(s.rpc(&moved, "repository_context", json!({})), changed);
    assert_eq!(
        fs::read(moved.join(".git/desktop-repository.id")).unwrap(),
        marker
    );
    let changed = s.rpc(&moved, "repository_context", json!({}));
    s.rpc(
        &moved,
        "set_fork_contribution_target",
        json!({"target":"parent","basis":changed["basis"]}),
    );
    assert_eq!(fs::read_to_string(s.root.join("gh-log")).unwrap(), calls);
    for kind in ["legacy-owner", "legacy-marker-window"] {
        let repo = fork(&s, kind, &[]);
        s.rpc(&repo, "refresh_repository_context", json!({}));
        let marker = repo.join(".git/desktop-repository.id");
        let record = repo.join(".git/desktop-repository.json");
        let marker_bytes = fs::read(&marker).unwrap();
        let mut data: Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
        let canonical = repo.join(".git").canonicalize().unwrap();
        data["repository_id"] = json!(format!(
            "{:x}",
            Sha256::digest(canonical.as_os_str().as_bytes())
        ));
        let legacy = data.to_string();
        fs::write(&record, &legacy).unwrap();
        fs::set_permissions(&record, fs::Permissions::from_mode(0o600)).unwrap();
        if kind == "legacy-owner" {
            fs::remove_file(&marker).unwrap();
        }
        let ctx = s.rpc(&repo, "repository_context", json!({}));
        assert_eq!(ctx["status"], "verified");
        assert_eq!(fs::read_to_string(&record).unwrap(), legacy);
        if kind == "legacy-owner" {
            assert!(!marker.exists());
        } else {
            assert_eq!(fs::read(&marker).unwrap(), marker_bytes);
        }
        s.rpc(
            &repo,
            "set_fork_contribution_target",
            json!({"target":"self","basis":ctx["basis"]}),
        );
        let id = fs::read_to_string(&marker).unwrap();
        let parsed = uuid::Uuid::parse_str(id.trim()).unwrap();
        assert_eq!(parsed.to_string(), id.trim());
        let updated: Value = serde_json::from_slice(&fs::read(record).unwrap()).unwrap();
        assert_eq!(updated["repository_id"], id.trim());
    }
}

fn private_file(path: &Path, content: &[u8]) {
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
fn bounded_rpc(s: &Suite, repo: &Path, method: &str, mut body: Value, fail: bool) -> Output {
    body["method"] = json!(method);
    let mut child = s
        .command(env!("CARGO_BIN_EXE_desktop-rs"))
        .arg("-C")
        .arg(repo)
        .args(["repo", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.to_string().as_bytes())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("{method} blocked on unsafe metadata");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.success(),
        !fail,
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn malformed_and_unsafe_repository_storage_is_never_overwritten() {
    let s = fixture();
    let repo = verified_missing(&s, "storage", &[]);
    let record = repo.join(".git/desktop-repository.json");
    let lock = repo.join(".git/desktop-repository.lock");
    let original = fs::read(&record).unwrap();
    let good: Value = serde_json::from_slice(&original).unwrap();
    for content in [
        b"{broken".to_vec(),
        vec![b'x'; 64 * 1024 + 1],
        {
            let mut v = good.clone();
            v["version"] = json!(2);
            v.to_string().into_bytes()
        },
        {
            let mut v = good.clone();
            v["repository_id"] = json!("0".repeat(64));
            v.to_string().into_bytes()
        },
        {
            let mut v = good.clone();
            v["unknown"] = json!(true);
            v.to_string().into_bytes()
        },
    ] {
        private_file(&record, &content);
        bounded_rpc(&s, &repo, "repository_context", json!({}), true);
        bounded_rpc(
            &s,
            &repo,
            "set_fork_contribution_target",
            json!({"target":"self"}),
            true,
        );
        assert_eq!(fs::read(&record).unwrap(), content);
    }
    private_file(&record, &original);
    let other = fork(&s, "foreign-storage", &[]);
    let other_record = other.join(".git/desktop-repository.json");
    private_file(&other_record, &original);
    bounded_rpc(&s, &other, "repository_context", json!({}), true);
    assert_eq!(fs::read(other_record).unwrap(), original);
    for (name, path) in [("record", &record), ("lock", &lock)] {
        let baseline = fs::read(path).unwrap();
        let outside = s.root.join(format!("outside-{name}"));
        private_file(&outside, &baseline);
        for fault in ["symlink", "fifo", "directory", "mode", "hardlink"] {
            fs::remove_file(path).unwrap();
            match fault {
                "symlink" => symlink(&outside, path).unwrap(),
                "fifo" => {
                    s.run("mkfifo", [path.as_os_str()], None, false);
                }
                "directory" => fs::create_dir(path).unwrap(),
                "mode" => {
                    private_file(path, &baseline);
                    fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
                }
                _ => fs::hard_link(&outside, path).unwrap(),
            }
            if name == "record" {
                bounded_rpc(&s, &repo, "repository_context", json!({}), true);
            } else {
                let out = bounded_rpc(&s, &repo, "repository_context", json!({}), false);
                assert_eq!(
                    serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"],
                    "verified"
                );
            }
            bounded_rpc(
                &s,
                &repo,
                "set_fork_contribution_target",
                json!({"target":"self"}),
                true,
            );
            assert_eq!(fs::read(&outside).unwrap(), baseline);
            if name == "lock" {
                assert_eq!(fs::read(&record).unwrap(), original);
            }
            if fault == "directory" {
                fs::remove_dir(path).unwrap();
            } else {
                fs::remove_file(path).unwrap();
            }
            private_file(path, &baseline);
        }
    }
    assert_eq!(fs::read(&record).unwrap(), original);
    assert!(!fs::read_dir(record.parent().unwrap()).unwrap().any(|v| {
        let name = v.unwrap().file_name().to_string_lossy().into_owned();
        name.starts_with(".desktop-repository-") && name.ends_with(".tmp")
    }));
}

#[test]
fn missing_invalid_and_unsafe_repository_identity_markers_are_refused() {
    let s = fixture();
    let repo = verified_missing(&s, "marker-storage", &[]);
    let record = repo.join(".git/desktop-repository.json");
    let marker = repo.join(".git/desktop-repository.id");
    let original_record = fs::read(&record).unwrap();
    let original_marker = fs::read(&marker).unwrap();
    fs::remove_file(&marker).unwrap();
    bounded_rpc(&s, &repo, "repository_context", json!({}), true);
    bounded_rpc(
        &s,
        &repo,
        "set_fork_contribution_target",
        json!({"target":"self"}),
        true,
    );
    assert!(!marker.exists());
    assert_eq!(fs::read(&record).unwrap(), original_record);
    private_file(&marker, &original_marker);
    for content in [
        vec![],
        b"not-a-uuid".to_vec(),
        vec![b'x'; 1024],
        format!("{}\n", uuid::Uuid::new_v4()).into_bytes(),
    ] {
        private_file(&marker, &content);
        bounded_rpc(&s, &repo, "repository_context", json!({}), true);
        bounded_rpc(
            &s,
            &repo,
            "set_fork_contribution_target",
            json!({"target":"self"}),
            true,
        );
        assert_eq!(fs::read(&marker).unwrap(), content);
        assert_eq!(fs::read(&record).unwrap(), original_record);
    }
    private_file(&marker, &original_marker);
    let outside = s.root.join("outside-marker");
    private_file(&outside, &original_marker);
    for fault in ["symlink", "fifo", "directory", "mode", "hardlink"] {
        fs::remove_file(&marker).unwrap();
        match fault {
            "symlink" => symlink(&outside, &marker).unwrap(),
            "fifo" => {
                s.run("mkfifo", [marker.as_os_str()], None, false);
            }
            "directory" => fs::create_dir(&marker).unwrap(),
            "mode" => {
                private_file(&marker, &original_marker);
                fs::set_permissions(&marker, fs::Permissions::from_mode(0o644)).unwrap();
            }
            _ => fs::hard_link(&outside, &marker).unwrap(),
        }
        bounded_rpc(&s, &repo, "repository_context", json!({}), true);
        bounded_rpc(
            &s,
            &repo,
            "set_fork_contribution_target",
            json!({"target":"self"}),
            true,
        );
        assert_eq!(fs::read(&outside).unwrap(), original_marker);
        assert_eq!(fs::read(&record).unwrap(), original_record);
        if fault == "directory" {
            fs::remove_dir(&marker).unwrap();
        } else {
            fs::remove_file(&marker).unwrap();
        }
        private_file(&marker, &original_marker);
    }
    assert_eq!(
        s.rpc(&repo, "repository_context", json!({}))["status"],
        "verified"
    );
}

fn wait_for_file(path: &Path, child: &mut std::process::Child) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.exists() && Instant::now() < deadline {
        assert!(
            child.try_wait().unwrap().is_none(),
            "child exited before marker"
        );
        thread::sleep(Duration::from_millis(20));
    }
    if !path.exists() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("marker wait timed out");
    }
}
fn wait_child(mut child: std::process::Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    if child.try_wait().unwrap().is_none() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("child did not stop");
    }
    child.wait_with_output().unwrap()
}

#[test]
fn delayed_refresh_fences_remote_and_preserves_concurrent_preference() {
    for kind in ["remote", "preference"] {
        let s = fixture();
        let repo = verified_missing(&s, kind, &[]);
        let record = repo.join(".git/desktop-repository.json");
        let before = fs::read(&record).unwrap();
        fs::write(s.root.join("gh-gate"), "on").unwrap();
        let mut child = s
            .command(env!("CARGO_BIN_EXE_desktop-rs"))
            .args([
                "-C",
                repo.to_str().unwrap(),
                "repository-context",
                "--refresh",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        wait_for_file(&s.root.join("gh-started"), &mut child);
        if kind == "remote" {
            s.git(
                &repo,
                [
                    "remote",
                    "set-url",
                    "origin",
                    "https://github.example/new/repo.git",
                ],
                false,
            );
        } else {
            let ctx = s.rpc(&repo, "repository_context", json!({}));
            s.rpc(
                &repo,
                "set_fork_contribution_target",
                json!({"target":"self","basis":ctx["basis"]}),
            );
        }
        fs::write(s.root.join("gh-release"), "release").unwrap();
        let out = wait_child(child);
        if kind == "remote" {
            assert!(!out.status.success());
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("changed"),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(fs::read(&record).unwrap(), before);
        } else {
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let value: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(value["status"], "verified");
            assert_eq!(value["fork_contribution_target"], "self");
            assert_eq!(
                s.rpc(&repo, "repository_context", json!({}))["fork_contribution_target"],
                "self"
            );
        }
    }
}

#[test]
fn cancellation_while_waiting_for_context_lock_preserves_record() {
    let s = fixture();
    let repo = verified_missing(&s, "lock-cancel", &[]);
    let record = repo.join(".git/desktop-repository.json");
    let before = fs::read(&record).unwrap();
    let lock = repo.join(".git/desktop-repository.lock");
    let marker = s.root.join("lock-ready");
    let mut guard = s
        .command("flock")
        .args([
            "-x",
            lock.to_str().unwrap(),
            "sh",
            "-c",
            "printf ready > \"$LOCK_MARKER\"; exec cat",
        ])
        .env("LOCK_MARKER", &marker)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for_file(&marker, &mut guard);
    let mut child = s
        .command(env!("CARGO_BIN_EXE_desktop-rs"))
        .args([
            "-C",
            repo.to_str().unwrap(),
            "repository-context",
            "--target",
            "self",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_millis(200));
    assert!(child.try_wait().unwrap().is_none());
    s.run("kill", ["-INT", &child.id().to_string()], None, false);
    let output = wait_child(child);
    assert_eq!(output.status.code(), Some(130));
    assert_eq!(fs::read(&record).unwrap(), before);
    drop(guard.stdin.take());
    assert!(guard.wait().unwrap().success());
    assert_eq!(
        s.rpc(
            &repo,
            "set_fork_contribution_target",
            json!({"target":"self"})
        )["fork_contribution_target"],
        "self"
    );
}
