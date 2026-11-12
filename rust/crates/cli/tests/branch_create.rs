//! Reviewed branch creation, checkout and recovery with native Git.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

struct Suite {
    root: PathBuf,
    env: HashMap<OsString, OsString>,
}
impl Suite {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("desktop-branch-create-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self {
            root,
            env: HashMap::new(),
        }
    }
    fn command(&self, executable: &str) -> Command {
        let mut command = Command::new(executable);
        command
            .env("HOME", &self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("global"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_LITERAL_PATHSPECS")
            .envs(&self.env);
        command
    }
    fn run<I, S>(&self, executable: &str, args: I, input: Option<&[u8]>, fail: bool) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = self.command(executable);
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
    fn git_result(&self, repo: &Path, args: &[&str], fail: bool) -> Output {
        let mut all: Vec<OsString> = vec!["-C".into(), repo.as_os_str().into()];
        all.extend(args.iter().map(OsString::from));
        self.run("git", all, None, fail)
    }
    fn git(&self, repo: &Path, args: &[&str]) -> String {
        String::from_utf8(self.git_result(repo, args, false).stdout)
            .unwrap()
            .trim()
            .to_owned()
    }
    fn init(&self, name: &str, options: &[&str], committed: bool) -> PathBuf {
        let repo = self.root.join(name);
        let mut args: Vec<OsString> = ["init", "-q", "-b", "main"]
            .into_iter()
            .map(Into::into)
            .collect();
        args.extend(options.iter().map(OsString::from));
        args.push(repo.as_os_str().into());
        self.run("git", args, None, false);
        self.git(&repo, &["config", "user.name", "Creation Test"]);
        self.git(&repo, &["config", "user.email", "create@example.invalid"]);
        self.git(&repo, &["config", "commit.gpgsign", "false"]);
        if committed {
            fs::write(
                repo.join("file"),
                (0..30).map(|i| format!("line {i}\n")).collect::<String>(),
            )
            .unwrap();
            self.commit(&repo);
        }
        repo
    }
    fn commit(&self, repo: &Path) {
        self.git(repo, &["add", "--all"]);
        self.git(repo, &["commit", "-qm", "checkpoint"]);
    }
    fn rpc_raw(&self, repo: &Path, method: &str, mut body: Value, fail: bool) -> Output {
        body["method"] = json!(method);
        self.run(
            env!("CARGO_BIN_EXE_desktop-rs"),
            [
                OsString::from("-C"),
                repo.as_os_str().into(),
                "repo".into(),
                "-".into(),
            ],
            Some(body.to_string().as_bytes()),
            fail,
        )
    }
    fn rpc(&self, repo: &Path, method: &str, body: Value) -> Value {
        serde_json::from_slice(&self.rpc_raw(repo, method, body, false).stdout).unwrap()
    }
    fn review(&self, repo: &Path, name: &str, params: &Value) -> Value {
        let mut body = json!({"name":name});
        for key in ["start_point", "checkout", "no_track"] {
            if let Some(value) = params.get(key) {
                body[key] = value.clone();
            }
        }
        self.rpc(repo, "create_branch_preview", body)
    }
    fn create_raw(&self, repo: &Path, name: &str, mut params: Value, fail: bool) -> Output {
        let plan = self.review(repo, name, &params);
        params["name"] = json!(name);
        params["basis"] = plan["basis"].clone();
        self.rpc_raw(repo, "create_branch", params, fail)
    }
    fn create(&self, repo: &Path, name: &str, params: Value) -> Value {
        serde_json::from_slice(&self.create_raw(repo, name, params, false).stdout).unwrap()
    }
    fn reference(&self, repo: &Path, name: &str) -> bool {
        self.command("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ])
            .status()
            .unwrap()
            .success()
    }
    fn wrapper(&mut self, repo: &Path, label: &str, script: &str) {
        let directory = self.root.join(label);
        fs::create_dir(&directory).unwrap();
        let tool = directory.join("git");
        fs::write(&tool, script).unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
        let real = String::from_utf8(self.run("sh", ["-c", "command -v git"], None, false).stdout)
            .unwrap()
            .trim()
            .to_owned();
        let path = std::env::join_paths(std::iter::once(directory).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        self.env.insert("PATH".into(), path);
        self.env.insert("CREATE_REAL_GIT".into(), real.into());
        self.env
            .insert("CREATE_REPO".into(), repo.as_os_str().into());
    }
    fn clear_wrapper(&mut self) {
        for key in [
            "PATH",
            "CREATE_REAL_GIT",
            "CREATE_REPO",
            "CREATE_MODE",
            "CREATE_MARKER",
            "CREATE_TIP",
            "CREATE_NAME",
        ] {
            self.env.remove(OsStr::new(key));
        }
    }
}
impl Drop for Suite {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
const LEAVE: &str = "StashOnCurrentBranch";
const BRING: &str = "MoveToNewBranch";

#[test]
fn creation_preserves_changes_tracking_and_review_fences() {
    let s = Suite::new();
    let repo = s.init("no-checkout", &[], true);
    fs::write(repo.join("file"), "staged").unwrap();
    s.git(&repo, &["add", "--", "file"]);
    fs::write(repo.join("file"), "working").unwrap();
    fs::write(repo.join("loose"), "loose").unwrap();
    let index = fs::read(repo.join(".git/index")).unwrap();
    let status = s
        .git_result(&repo, &["status", "--porcelain=v2", "-z"], false)
        .stdout;
    let plan = s.review(&repo, "new", &json!({}));
    assert_eq!(plan["dirty"], false);
    assert!(!repo.join(".git/desktop-checkout").exists());
    let result = s.create(&repo, "new", json!({}));
    assert_eq!(result["created"], true);
    assert_eq!(result["checked_out"], false);
    assert!(s.reference(&repo, "new"));
    assert_eq!(s.git(&repo, &["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
    assert_eq!(
        s.git_result(&repo, &["status", "--porcelain=v2", "-z"], false)
            .stdout,
        status
    );
    assert_eq!(s.rpc(&repo, "stash_list", json!({}))["entries"], json!([]));
    let repo = s.init("leave", &[], true);
    fs::write(repo.join("file"), "previous").unwrap();
    let previous = s.rpc(&repo, "stash_save", json!({"desktop":true}))["oid"].clone();
    fs::write(repo.join("file"), "index").unwrap();
    s.git(&repo, &["add", "--", "file"]);
    fs::write(repo.join("file"), "working").unwrap();
    let raw = OsString::from_vec(b"raw-\xff".to_vec());
    fs::write(repo.join(&raw), b"loose\0").unwrap();
    let plan = s.review(&repo, "new", &json!({"checkout":true}));
    let result = s.rpc_raw(
        &repo,
        "create_branch",
        json!({"name":"new","checkout":true,"basis":plan["basis"]}),
        true,
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap()["requires_confirmation"],
        true
    );
    assert!(!s.reference(&repo, "new"));
    assert!(!repo.join(".git/desktop-checkout").exists());
    let result = s.create_raw(
        &repo,
        "new",
        json!({"checkout":true,"strategy":LEAVE}),
        true,
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap()["requires_confirmation"],
        true
    );
    assert!(!s.reference(&repo, "new"));
    let result = s.create(
        &repo,
        "new",
        json!({"checkout":true,"strategy":LEAVE,"replace_stash":true}),
    );
    assert_eq!(result["created"], true);
    assert_eq!(result["checked_out"], true);
    assert_eq!(s.rpc(&repo, "snapshot", json!({}))["branch"], "new");
    let entries = s.rpc(&repo, "stash_list", json!({}));
    let entries = entries["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["branch"], "main");
    assert_ne!(entries[0]["oid"], previous);
    let oid = entries[0]["oid"].as_str().unwrap();
    let parent = entries[0]["parents"][1].as_str().unwrap();
    assert_eq!(
        s.git_result(&repo, &["show", &format!("{oid}:file")], false)
            .stdout,
        b"working"
    );
    assert_eq!(
        s.git_result(&repo, &["show", &format!("{parent}:file")], false)
            .stdout,
        b"index"
    );
    let mut raw_revision =
        OsString::from(format!("{}:", entries[0]["parents"][2].as_str().unwrap()));
    raw_revision.push(&raw);
    let output = s.run(
        "git",
        [
            OsString::from("-C"),
            repo.as_os_str().into(),
            OsString::from("show"),
            raw_revision,
        ],
        None,
        false,
    );
    assert_eq!(output.stdout, b"loose\0");
    assert!(!repo.join(&raw).exists());
    let repo = s.init("direct", &[], true);
    fs::write(repo.join("file"), "index").unwrap();
    s.git(&repo, &["add", "--", "file"]);
    fs::write(repo.join("file"), "working").unwrap();
    fs::write(repo.join("loose"), "loose").unwrap();
    s.create(&repo, "new", json!({"checkout":true,"strategy":BRING}));
    assert_eq!(
        s.git_result(&repo, &["show", ":file"], false).stdout,
        b"index"
    );
    assert_eq!(fs::read(repo.join("file")).unwrap(), b"working");
    assert_eq!(s.rpc(&repo, "stash_list", json!({}))["entries"], json!([]));
    let repo = s.init("fallback", &[], true);
    let base = fs::read_to_string(repo.join("file")).unwrap();
    s.git(&repo, &["switch", "-qc", "target"]);
    fs::write(repo.join("file"), base.replace("line 29", "target 29")).unwrap();
    s.commit(&repo);
    s.git(&repo, &["switch", "-q", "main"]);
    fs::write(repo.join("file"), base.replace("line 0", "index 0")).unwrap();
    s.git(&repo, &["add", "--", "file"]);
    fs::write(
        repo.join("file"),
        base.replace("line 0", "index 0")
            .replace("line 1\n", "working 1\n"),
    )
    .unwrap();
    fs::write(repo.join("loose"), "carry").unwrap();
    s.create(&repo, "new", json!({"start_point":"refs/heads/target","checkout":true,"strategy":BRING,"restore_index":true}));
    assert_eq!(s.rpc(&repo, "snapshot", json!({}))["branch"], "new");
    assert_eq!(
        fs::read_to_string(repo.join("file")).unwrap(),
        base.replace("line 0", "index 0")
            .replace("line 1\n", "working 1\n")
            .replace("line 29", "target 29")
    );
    let staged = s.git_result(&repo, &["show", ":file"], false).stdout;
    assert!(
        staged
            .windows(b"index 0\n".len())
            .any(|w| w == b"index 0\n")
    );
    assert!(
        !staged
            .windows(b"working 1\n".len())
            .any(|w| w == b"working 1\n")
    );
    assert_eq!(s.rpc(&repo, "stash_list", json!({}))["entries"], json!([]));
    let repo = s.init("conflict", &[], true);
    s.git(&repo, &["switch", "-qc", "target"]);
    fs::write(repo.join("file"), "target").unwrap();
    s.commit(&repo);
    s.git(&repo, &["switch", "-q", "main"]);
    fs::write(repo.join("file"), "local").unwrap();
    s.create_raw(
        &repo,
        "new",
        json!({"start_point":"target","checkout":true,"strategy":BRING}),
        true,
    );
    let history = s.rpc(&repo, "checkout_history", json!({}));
    assert_eq!(history[0]["receipt"]["phase"], "applying");
    assert_eq!(
        history[0]["receipt"]["target"]["creation"]["start_point"],
        "target"
    );
    assert!(!history[0]["saved_stashes"].as_array().unwrap().is_empty());
    assert!(s.reference(&repo, "new"));
    assert!(
        !s.git_result(&repo, &["ls-files", "-u"], false)
            .stdout
            .is_empty()
    );
    assert_eq!(s.rpc(&repo, "snapshot", json!({}))["branch"], "new");
    for (mode, source, name) in [
        ("true", "refs/remotes/origin/topic", "topic"),
        ("simple", "refs/remotes/origin/topic", "different"),
        ("always", "refs/heads/main", "always"),
        ("inherit", "refs/heads/main", "inherited"),
    ] {
        let repo = s.init(&format!("tracking-{mode}"), &[], true);
        s.git(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                s.root.join("not-contacted").to_str().unwrap(),
            ],
        );
        let head = s.git(&repo, &["rev-parse", "HEAD"]);
        s.git(&repo, &["update-ref", "refs/remotes/origin/topic", &head]);
        s.git(&repo, &["branch", "refs/remotes/origin/topic"]);
        s.git(&repo, &["config", "branch.autoSetupMerge", mode]);
        s.git(&repo, &["config", "branch.autoSetupRebase", "always"]);
        s.git(&repo, &["config", "branch.main.remote", "origin"]);
        s.git(&repo, &["config", "branch.main.merge", "refs/heads/topic"]);
        s.git(
            &repo,
            &[
                "-c",
                "core.warnAmbiguousRefs=false",
                "branch",
                "--",
                &format!("{name}-control"),
                source,
            ],
        );
        s.create(&repo, name, json!({"start_point":source}));
        if mode != "simple" {
            for key in ["remote", "merge", "rebase"] {
                let actual = s
                    .command("git")
                    .args([
                        "-C",
                        repo.to_str().unwrap(),
                        "config",
                        &format!("branch.{name}.{key}"),
                    ])
                    .output()
                    .unwrap();
                let expected = s
                    .command("git")
                    .args([
                        "-C",
                        repo.to_str().unwrap(),
                        "config",
                        &format!("branch.{name}-control.{key}"),
                    ])
                    .output()
                    .unwrap();
                assert_eq!(actual.status.code(), expected.status.code());
                assert_eq!(actual.stdout, expected.stdout);
            }
        }
        if mode == "simple" {
            assert!(
                s.git_result(&repo, &["config", "branch.different.remote"], true)
                    .stdout
                    .is_empty()
            );
        }
        s.create(
            &repo,
            "no-tracking",
            json!({"start_point":source,"no_track":true}),
        );
        assert!(
            s.git_result(&repo, &["config", "branch.no-tracking.remote"], true)
                .stdout
                .is_empty()
        );
        if mode == "true" {
            assert_eq!(
                s.git(&repo, &["config", "branch.topic.merge"]),
                "refs/heads/topic"
            );
        }
    }
    let repo = s.init("revisions", &[], true);
    let first = s.git(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("more"), "more").unwrap();
    s.commit(&repo);
    s.create(&repo, "new", json!({"start_point":"HEAD~1"}));
    assert_eq!(s.git(&repo, &["rev-parse", "refs/heads/new"]), first);
    s.git(&repo, &["switch", "--detach", "-q"]);
    s.create(&repo, "from-detached", json!({"checkout":true}));
    assert_eq!(
        s.rpc(&repo, "snapshot", json!({}))["branch"],
        "from-detached"
    );
    s.git(&repo, &["tag", "-a", "tagged", "-m", "first tag"]);
    let plan = s.review(
        &repo,
        "from-tag",
        &json!({"start_point":"refs/tags/tagged"}),
    );
    s.git(&repo, &["tag", "-fa", "tagged", "-m", "different tag"]);
    s.rpc_raw(
        &repo,
        "create_branch",
        json!({"name":"from-tag","start_point":"refs/tags/tagged","basis":plan["basis"]}),
        true,
    );
    assert!(!s.reference(&repo, "from-tag"));
    for kind in [
        "bytes",
        "index",
        "head",
        "source-ref",
        "config",
        "stash",
        "destination",
    ] {
        let repo = s.init(&format!("stale-{kind}"), &[], true);
        s.git(&repo, &["branch", "start"]);
        let loose = repo.join("loose");
        fs::write(&loose, "one").unwrap();
        let plan = s.review(
            &repo,
            "new",
            &json!({"start_point":"start","checkout":true}),
        );
        match kind {
            "bytes" => {
                let meta = fs::metadata(&loose).unwrap();
                fs::write(&loose, "two").unwrap();
                fs::File::options()
                    .write(true)
                    .open(&loose)
                    .unwrap()
                    .set_times(
                        fs::FileTimes::new()
                            .set_accessed(meta.accessed().unwrap())
                            .set_modified(meta.modified().unwrap()),
                    )
                    .unwrap();
            }
            "index" => {
                s.git(&repo, &["add", "--", "loose"]);
            }
            "head" => {
                s.git(&repo, &["switch", "-qc", "other"]);
            }
            "source-ref" => {
                let oid = s.git(&repo, &["commit-tree", "HEAD^{tree}", "-m", "new start"]);
                s.git(&repo, &["update-ref", "refs/heads/start", &oid]);
            }
            "config" => {
                s.git(&repo, &["config", "branch.autoSetupMerge", "always"]);
            }
            "stash" => {
                fs::write(repo.join("file"), "saved").unwrap();
                s.rpc(&repo, "stash_save", json!({"message":"external"}));
            }
            "destination" => {
                s.git(&repo, &["branch", "new"]);
            }
            _ => unreachable!(),
        }
        let before = s
            .git_result(&repo, &["stash", "list", "--format=%H"], false)
            .stdout;
        s.rpc_raw(&repo, "create_branch", json!({"name":"new","start_point":"start","checkout":true,"strategy":BRING,"basis":plan["basis"]}), true);
        assert_eq!(
            s.git_result(&repo, &["stash", "list", "--format=%H"], false)
                .stdout,
            before
        );
        assert!(!repo.join(".git/desktop-checkout").exists());
        assert_eq!(s.reference(&repo, "new"), kind == "destination");
    }
}

#[test]
fn conflicts_unborn_sources_and_native_ref_backends() {
    let s = Suite::new();
    let repo = s.init("during-merge", &[], true);
    s.git(&repo, &["switch", "-qc", "side"]);
    fs::write(repo.join("file"), "side").unwrap();
    s.commit(&repo);
    s.git(&repo, &["switch", "-q", "main"]);
    fs::write(repo.join("file"), "main").unwrap();
    s.commit(&repo);
    s.git_result(&repo, &["merge", "side"], true);
    let index = fs::read(repo.join(".git/index")).unwrap();
    let working = fs::read(repo.join("file")).unwrap();
    let unmerged = s.git_result(&repo, &["ls-files", "-u", "-z"], false).stdout;
    s.create(&repo, "ref-only-conflicted", json!({}));
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(repo.join("file")).unwrap(), working);
    assert_eq!(
        s.git_result(&repo, &["ls-files", "-u", "-z"], false).stdout,
        unmerged
    );
    assert_eq!(
        s.rpc(&repo, "snapshot", json!({}))["operation"]["kind"],
        "merge"
    );
    s.create_raw(
        &repo,
        "switch-conflicted",
        json!({"checkout":true,"strategy":BRING}),
        true,
    );
    assert!(!s.reference(&repo, "switch-conflicted"));
    let repo = s.init("corrupt-index", &[], true);
    let index = repo.join(".git/index");
    fs::write(&index, b"corrupt index retained exactly").unwrap();
    s.git(&repo, &["branch", "native-control"]);
    let result = s.create(&repo, "rust-ref", json!({}));
    assert_eq!(result["created"], true);
    assert_eq!(fs::read(index).unwrap(), b"corrupt index retained exactly");
    assert_eq!(s.git(&repo, &["symbolic-ref", "HEAD"]), "refs/heads/main");
    s.rpc_raw(
        &repo,
        "create_branch_preview",
        json!({"name":"requires-index","checkout":true}),
        true,
    );
    let repo = s.init("corrupt-stash", &[], true);
    fs::write(repo.join(".git/refs/stash"), "broken ref\n").unwrap();
    s.create(&repo, "without-stash", json!({}));
    assert_eq!(
        fs::read_to_string(repo.join(".git/refs/stash")).unwrap(),
        "broken ref\n"
    );
    let source = s.init("unborn-source", &[], true);
    fs::write(source.join("file"), "fetched base").unwrap();
    s.commit(&source);
    for (name, start) in [
        ("main", "FETCH_HEAD"),
        ("from-remote", "refs/remotes/origin/main"),
    ] {
        let repo = s.init(&format!("unborn-{name}"), &[], false);
        s.git(
            &repo,
            &["remote", "add", "origin", source.to_str().unwrap()],
        );
        s.git(
            &repo,
            &["fetch", "-q", "origin", "main:refs/remotes/origin/main"],
        );
        fs::write(repo.join("prepared"), "staged data").unwrap();
        s.git(&repo, &["add", "--", "prepared"]);
        fs::write(repo.join("loose"), "untracked data").unwrap();
        let args = json!({"start_point":start,"checkout":true});
        let plan = s.review(&repo, name, &args);
        assert_eq!(plan["same_branch"], false);
        assert_eq!(plan["target"]["creation"]["defer_to_switch"], true);
        s.rpc(&repo, "create_branch", json!({"name":name,"start_point":start,"checkout":true,"strategy":BRING,"basis":plan["basis"]}));
        assert_eq!(s.rpc(&repo, "snapshot", json!({}))["branch"], name);
        assert_eq!(fs::read(repo.join("file")).unwrap(), b"fetched base");
        assert_eq!(
            s.git_result(&repo, &["show", ":prepared"], false).stdout,
            b"staged data"
        );
        assert_eq!(fs::read(repo.join("loose")).unwrap(), b"untracked data");
        if name == "from-remote" {
            assert_eq!(
                s.git(&repo, &["config", "branch.from-remote.remote"]),
                "origin"
            );
        }
    }
    let repo = s.init("unborn-current-ref", &[], false);
    s.git(&repo, &["fetch", "-q", source.to_str().unwrap(), "main"]);
    fs::write(repo.join("loose"), "kept").unwrap();
    s.create(&repo, "main", json!({"start_point":"FETCH_HEAD"}));
    assert!(s.reference(&repo, "main"));
    assert!(!repo.join("file").exists());
    assert_eq!(fs::read(repo.join("loose")).unwrap(), b"kept");
    let repo = s.init("unborn", &[], false);
    fs::write(repo.join("file"), "staged").unwrap();
    s.git(&repo, &["add", "--", "file"]);
    fs::write(repo.join("loose"), "loose").unwrap();
    s.rpc_raw(&repo, "create_branch_preview", json!({"name":"new"}), true);
    s.create(&repo, "new", json!({"checkout":true}));
    assert_eq!(s.git(&repo, &["symbolic-ref", "HEAD"]), "refs/heads/new");
    assert_eq!(
        s.git_result(&repo, &["show", ":file"], false).stdout,
        b"staged"
    );
    assert_eq!(fs::read(repo.join("loose")).unwrap(), b"loose");
    assert!(!s.reference(&repo, "new"));
    assert!(s.rpc(&repo, "snapshot", json!({}))["head"].is_null());
    for label in ["sha256", "split", "linked", "reftable"] {
        let options: Vec<&str> = match label {
            "sha256" => vec!["--object-format=sha256"],
            "reftable" => vec!["--ref-format=reftable"],
            _ => vec![],
        };
        let mut repo = s.init(label, &options, true);
        if label == "split" {
            s.git(&repo, &["update-index", "--split-index"]);
        }
        if label == "linked" {
            let linked = s.root.join("linked-tree");
            s.git(
                &repo,
                &["worktree", "add", "-qb", "other", linked.to_str().unwrap()],
            );
            repo = linked;
        }
        fs::write(repo.join("file"), "carry").unwrap();
        s.create(&repo, "new", json!({"checkout":true,"strategy":BRING}));
        assert_eq!(s.rpc(&repo, "snapshot", json!({}))["branch"], "new");
        assert_eq!(fs::read(repo.join("file")).unwrap(), b"carry");
    }
}

const WRAPPER: &str = "#!/bin/sh\nbranch=0; target=0\nfor arg in \"$@\"; do [ \"$arg\" = branch ] && branch=1; [ \"$arg\" = \"$CREATE_NAME\" ] && target=1; done\nif [ \"$branch$target\" = 11 ]; then\n if [ \"$CREATE_MODE\" = source ]; then \"$CREATE_REAL_GIT\" -C \"$CREATE_REPO\" update-ref refs/heads/start \"$CREATE_TIP\" || exit 95; fi\n if [ \"$CREATE_MODE\" = before ]; then exit 72; fi\nfi\n\"$CREATE_REAL_GIT\" \"$@\"\nresult=$?\nif [ \"$branch$target\" = 11 ] && [ \"$result\" = 0 ]; then\n if [ \"$CREATE_MODE\" = after ]; then exit 72; fi\n if [ \"$CREATE_MODE\" = cancel ]; then printf created > \"$CREATE_MARKER\"; sleep 60; fi\nfi\nexit \"$result\"\n";

#[test]
fn failures_cancellation_and_recovery_metadata() {
    let mut s = Suite::new();
    let repo = s.init("failed-checkout-hook", &[], true);
    fs::write(repo.join("file"), "keep working").unwrap();
    fs::write(repo.join("loose"), "keep untracked").unwrap();
    let hook = repo.join(".git/hooks/post-checkout");
    fs::write(&hook, "#!/bin/sh\nexit 65\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    s.create_raw(
        &repo,
        "created-before-hook",
        json!({"checkout":true,"strategy":LEAVE}),
        true,
    );
    let history = s.rpc(&repo, "checkout_history", json!({}));
    assert_eq!(history[0]["receipt"]["phase"], "switching");
    assert!(!history[0]["saved_stashes"].as_array().unwrap().is_empty());
    assert_eq!(
        s.rpc(&repo, "snapshot", json!({}))["branch"],
        "created-before-hook"
    );
    let saved = &history[0]["saved_stashes"][0];
    assert_eq!(
        s.git_result(
            &repo,
            &["show", &format!("{}:file", saved["oid"].as_str().unwrap())],
            false
        )
        .stdout,
        b"keep working"
    );
    assert_eq!(
        s.git_result(
            &repo,
            &[
                "show",
                &format!("{}:loose", saved["parents"][2].as_str().unwrap())
            ],
            false
        )
        .stdout,
        b"keep untracked"
    );
    let repo = s.init("source-race", &[], true);
    s.git(&repo, &["branch", "start"]);
    fs::write(repo.join("file"), "source remains").unwrap();
    fs::write(repo.join("loose"), "loose remains").unwrap();
    let tip = s.git(
        &repo,
        &["commit-tree", "HEAD^{tree}", "-m", "changed source"],
    );
    s.wrapper(&repo, "source-race-wrapper", WRAPPER);
    s.env.insert("CREATE_MODE".into(), "source".into());
    s.env.insert("CREATE_NAME".into(), "raced".into());
    s.env.insert("CREATE_TIP".into(), tip.into());
    s.create_raw(
        &repo,
        "raced",
        json!({"start_point":"refs/heads/start","checkout":true,"strategy":LEAVE}),
        true,
    );
    s.clear_wrapper();
    assert_eq!(s.rpc(&repo, "snapshot", json!({}))["branch"], "main");
    assert_eq!(fs::read(repo.join("file")).unwrap(), b"source remains");
    assert_eq!(fs::read(repo.join("loose")).unwrap(), b"loose remains");
    assert_eq!(s.rpc(&repo, "stash_list", json!({}))["entries"], json!([]));
    assert!(!s.reference(&repo, "raced"));
    assert_eq!(
        s.rpc(&repo, "checkout_history", json!({}))[0]["receipt"]["phase"],
        "creating_branch"
    );
    for when in ["before", "after"] {
        let repo = s.init(&format!("failure-{when}"), &[], true);
        fs::write(repo.join("file"), "retained").unwrap();
        s.wrapper(&repo, &format!("wrapper-{when}"), WRAPPER);
        s.env.insert("CREATE_MODE".into(), when.into());
        s.env.insert("CREATE_NAME".into(), "new".into());
        s.create_raw(
            &repo,
            "new",
            json!({"checkout":true,"strategy":BRING}),
            true,
        );
        s.clear_wrapper();
        assert_eq!(fs::read(repo.join("file")).unwrap(), b"retained");
        assert_eq!(s.rpc(&repo, "snapshot", json!({}))["branch"], "main");
        assert_eq!(s.reference(&repo, "new"), when == "after");
        assert_eq!(
            s.rpc(&repo, "checkout_history", json!({}))[0]["receipt"]["phase"],
            "creating_branch"
        );
    }
    let repo = s.init("cancel", &[], true);
    fs::write(repo.join("file"), "cancel data").unwrap();
    let marker = s.root.join("created");
    s.wrapper(&repo, "cancel-wrapper", WRAPPER);
    s.env.insert("CREATE_MODE".into(), "cancel".into());
    s.env.insert("CREATE_NAME".into(), "new".into());
    s.env
        .insert("CREATE_MARKER".into(), marker.as_os_str().into());
    let mut child = s
        .command(env!("CARGO_BIN_EXE_desktop-rs"))
        .args([
            "-C",
            repo.to_str().unwrap(),
            "create-branch",
            "new",
            "--checkout",
            "--strategy",
            "bring",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !marker.exists() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("branch creation did not reach cancellation marker");
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "branch creation exited before marker"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("cancelled creation did not stop");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let result = child.wait_with_output().unwrap();
    assert_eq!(
        result.status.code(),
        Some(130),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    s.clear_wrapper();
    assert!(s.reference(&repo, "new"));
    assert_eq!(fs::read(repo.join("file")).unwrap(), b"cancel data");
    assert_eq!(
        s.rpc(&repo, "checkout_history", json!({}))[0]["receipt"]["phase"],
        "creating_branch"
    );
    let repo = s.init("prefs", &[], true);
    let config = s.root.join("registry.json");
    let preferences = config.with_extension("preferences.json");
    fs::write(&preferences, "{bad").unwrap();
    s.run(
        env!("CARGO_BIN_EXE_desktop-rs"),
        [
            OsString::from("-C"),
            repo.as_os_str().into(),
            "--config".into(),
            config.as_os_str().into(),
            "create-branch".into(),
            "previewed".into(),
            "--preview".into(),
            "--checkout".into(),
        ],
        None,
        false,
    );
    assert_eq!(fs::read_to_string(&preferences).unwrap(), "{bad");
    assert!(!s.reference(&repo, "previewed"));
    s.run(
        env!("CARGO_BIN_EXE_desktop-rs"),
        [
            OsString::from("-C"),
            repo.as_os_str().into(),
            "--config".into(),
            config.as_os_str().into(),
            "create-branch".into(),
            "ref-only".into(),
        ],
        None,
        false,
    );
    let history = s.rpc(&repo, "checkout_history", json!({}));
    let receipt = history[0]["receipt"]["id"].as_str().unwrap();
    let path = repo
        .join(".git/desktop-checkout")
        .join(format!("{receipt}.json"));
    fs::remove_file(&path).unwrap();
    s.run("mkfifo", [path.as_os_str()], None, false);
    assert!(s.rpc(&repo, "checkout_history", json!({}))[0]["error"].is_string());
    s.rpc_raw(
        &repo,
        "acknowledge_checkout",
        json!({"receipt_id":receipt}),
        true,
    );
}

#[test]
fn lfs_creation_preserves_staged_and_untracked_content() {
    let mut s = Suite::new();
    if let Some(tool) = std::env::var_os("GIT_LFS_BIN") {
        let directory = PathBuf::from(tool).parent().unwrap().to_path_buf();
        let path = std::env::join_paths(std::iter::once(directory).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        s.env.insert("PATH".into(), path);
    }
    let lfs = s
        .command("git")
        .args(["lfs", "version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    if !lfs.success() {
        assert!(
            std::env::var_os("DESKTOP_TEST_REQUIRE_LFS").is_none(),
            "Git LFS is required for this test"
        );
        return;
    }
    let repo = s.init("lfs", &[], true);
    s.rpc(&repo, "lfs_install", json!({}));
    s.git(&repo, &["lfs", "track", "*.lfs"]);
    let payload = repo.join("data.lfs");
    fs::write(&payload, b"base").unwrap();
    s.commit(&repo);
    fs::write(&payload, b"index").unwrap();
    s.git(&repo, &["add", "--", "data.lfs"]);
    fs::write(&payload, b"working").unwrap();
    fs::write(repo.join("new.lfs"), b"untracked").unwrap();
    s.env.insert("GIT_LITERAL_PATHSPECS".into(), "1".into());
    s.create(&repo, "new", json!({"checkout":true,"strategy":LEAVE}));
    s.env.remove(OsStr::new("GIT_LITERAL_PATHSPECS"));
    let entries = s.rpc(&repo, "stash_list", json!({}));
    s.rpc(
        &repo,
        "stash_apply",
        json!({"oid":entries["entries"][0]["oid"],"restore_index":true}),
    );
    assert_eq!(fs::read(payload).unwrap(), b"working");
    assert_eq!(fs::read(repo.join("new.lfs")).unwrap(), b"untracked");
}
