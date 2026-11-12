//! Reviewed branch renames preserve saved changes and recovery receipts.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    os::unix::{
        ffi::OsStringExt,
        fs::{PermissionsExt, symlink},
    },
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
            std::env::temp_dir().join(format!("desktop-branch-rename-{}", uuid::Uuid::new_v4()));
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
            .envs(&self.env);
        command
    }
    fn run<I, S>(&self, executable: &str, args: I, input: Option<&[u8]>, fail: bool) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command(executable);
        command
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
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
    fn init(&self, name: &str, options: &[&str]) -> PathBuf {
        let repo = self.root.join(name);
        let mut args: Vec<OsString> = ["init", "-q", "-b", "main"]
            .into_iter()
            .map(Into::into)
            .collect();
        args.extend(options.iter().map(OsString::from));
        args.push(repo.as_os_str().into());
        self.run("git", args, None, false);
        self.git(&repo, &["config", "user.name", "Rename Test"]);
        self.git(&repo, &["config", "user.email", "rename@example.invalid"]);
        self.git(&repo, &["config", "commit.gpgsign", "false"]);
        fs::write(repo.join("file"), b"base\n").unwrap();
        self.commit(&repo);
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
    fn review(&self, repo: &Path, name: &str, new: &str) -> Value {
        self.rpc(
            repo,
            "rename_branch_preview",
            json!({"name":name,"new_name":new}),
        )
    }
    fn rename(&self, repo: &Path, name: &str, new: &str, fail: bool) -> Output {
        let plan = self.review(repo, name, new);
        self.rpc_raw(
            repo,
            "rename_branch",
            json!({"name":name,"new_name":new,"basis":plan["basis"]}),
            fail,
        )
    }
    fn save(&self, repo: &Path, content: &[u8], body: Value) -> Value {
        fs::write(repo.join("file"), content).unwrap();
        self.rpc(repo, "stash_save", body)["oid"].clone()
    }
    fn entries(&self, repo: &Path) -> Value {
        self.rpc(repo, "stash_list", json!({}))["entries"].clone()
    }
    fn wrapper(&mut self, repo: &Path, label: &str, script: &str) -> PathBuf {
        let directory = self.root.join(label);
        fs::create_dir(&directory).unwrap();
        let wrapper = directory.join("git");
        fs::write(&wrapper, script).unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        let real = String::from_utf8(self.run("sh", ["-c", "command -v git"], None, false).stdout)
            .unwrap()
            .trim()
            .to_owned();
        let path = std::env::join_paths(std::iter::once(directory).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        self.env.insert("PATH".into(), path);
        self.env.insert("RENAME_REAL_GIT".into(), real.into());
        self.env
            .insert("RENAME_REPO".into(), repo.as_os_str().into());
        wrapper
    }
    fn unmap_wrapper(&mut self) {
        for key in [
            "PATH",
            "RENAME_REAL_GIT",
            "RENAME_REPO",
            "RENAME_NEW_TIP",
            "RENAME_MODE",
            "RENAME_MARKER",
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

#[test]
fn saved_stashes_reviews_and_native_ref_formats() {
    let s = Suite::new();
    let repo = s.init("empty", &[]);
    assert_eq!(s.rpc(&repo, "branch_rename_history", json!({})), json!([]));
    assert!(!repo.join(".git/desktop-branch-renames").exists());
    let same: Value =
        serde_json::from_slice(&s.rename(&repo, "main", "main", false).stdout).unwrap();
    assert_eq!(same["unchanged"], true);
    assert!(!repo.join(".git/desktop-branch-renames").exists());
    s.git(&repo, &["tag", "main"]);
    fs::write(repo.join("file"), b"index saved\n").unwrap();
    s.git(&repo, &["add", "--", "file"]);
    fs::write(repo.join("file"), b"working saved\n").unwrap();
    let raw = OsString::from_vec(b"raw-\xff\n".to_vec());
    fs::write(repo.join(&raw), b"untracked saved\0").unwrap();
    let first = s.rpc(
        &repo,
        "stash_save",
        json!({"desktop":true,"include_untracked":true}),
    )["oid"]
        .clone();
    let ordinary = s.save(&repo, b"ordinary\n", json!({"message":"user stash"}));
    let second = s.save(&repo, b"newer branch stash\n", json!({"desktop":true}));
    let destination = s.save(
        &repo,
        b"destination saved\n",
        json!({"message":"!!GitHub_Desktop<renamed>"}),
    );
    let old = s.entries(&repo);
    let old_first = old
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["oid"] == first)
        .unwrap();
    let old_second = old
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["oid"] == second)
        .unwrap();
    fs::write(repo.join("file"), b"current index\n").unwrap();
    s.git(&repo, &["add", "--", "file"]);
    fs::write(repo.join("file"), b"current working\n").unwrap();
    fs::write(repo.join("loose"), b"keep loose").unwrap();
    let index = fs::read(repo.join(".git/index")).unwrap();
    let status = s
        .git_result(&repo, &["status", "--porcelain=v2", "-z"], false)
        .stdout;
    s.git(&repo, &["config", "branch.main.remote", "origin"]);
    s.git(&repo, &["config", "branch.main.merge", "refs/heads/main"]);
    let plan = s.review(&repo, "refs/heads/main", "renamed");
    assert_eq!(
        plan["saved_stashes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["oid"].clone())
            .collect::<Vec<_>>(),
        vec![second.clone(), first.clone()]
    );
    assert_eq!(plan["destination_stashes"][0]["oid"], destination);
    let result = s.rpc(
        &repo,
        "rename_branch",
        json!({"name":"refs/heads/main","new_name":"renamed","basis":plan["basis"]}),
    );
    assert_eq!(result["receipt"]["phase"], "complete");
    assert_eq!(s.rpc(&repo, "snapshot", json!({}))["branch"], "renamed");
    assert_eq!(
        s.git(&repo, &["config", "branch.renamed.merge"]),
        "refs/heads/main"
    );
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
    assert_eq!(
        s.git_result(&repo, &["status", "--porcelain=v2", "-z"], false)
            .stdout,
        status
    );
    let now = s.entries(&repo);
    let entries = now.as_array().unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0]["tree"], old_second["tree"]);
    assert_eq!(entries[1]["tree"], old_first["tree"]);
    assert_eq!(entries[0]["parents"], old_second["parents"]);
    assert_eq!(entries[1]["parents"], old_first["parents"]);
    assert!(
        entries[..2]
            .iter()
            .all(|entry| entry["branch"] == "renamed")
    );
    assert!(entries[2..].iter().any(|entry| entry["oid"] == ordinary));
    assert!(entries[2..].iter().any(|entry| entry["oid"] == destination));
    let history = s.rpc(&repo, "branch_rename_history", json!({}));
    assert_eq!(history.as_array().unwrap().len(), 1);
    assert_eq!(history[0]["saved_stashes"].as_array().unwrap().len(), 2);
    let receipt = history[0]["receipt"]["id"].as_str().unwrap();
    let metadata = repo
        .join(".git/desktop-branch-renames")
        .join(format!("{receipt}.json"));
    assert_eq!(
        fs::metadata(metadata).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let before = s
        .git_result(&repo, &["stash", "list", "--format=%H"], false)
        .stdout;
    s.rpc(
        &repo,
        "acknowledge_branch_rename",
        json!({"receipt_id":receipt}),
    );
    assert_eq!(
        s.git_result(&repo, &["stash", "list", "--format=%H"], false)
            .stdout,
        before
    );
    assert_eq!(s.rpc(&repo, "branch_rename_history", json!({})), json!([]));
    s.git(&repo, &["reset", "--hard", "-q"]);
    fs::remove_file(repo.join("loose")).unwrap();
    let moved = entries
        .iter()
        .find(|entry| entry["parents"] == old_first["parents"])
        .unwrap();
    s.rpc(
        &repo,
        "stash_apply",
        json!({"oid":moved["oid"],"restore_index":true}),
    );
    assert_eq!(
        s.git_result(&repo, &["show", ":file"], false).stdout,
        b"index saved\n"
    );
    assert_eq!(fs::read(repo.join("file")).unwrap(), b"working saved\n");
    assert_eq!(fs::read(repo.join(&raw)).unwrap(), b"untracked saved\0");
    for kind in ["config", "ref", "stash", "worktree"] {
        let repo = s.init(&format!("stale-{kind}"), &[]);
        s.save(&repo, b"saved", json!({"desktop":true}));
        let plan = s.review(&repo, "main", "renamed");
        match kind {
            "config" => {
                s.git(&repo, &["config", "branch.main.description", "changed"]);
            }
            "ref" => {
                let oid = s.git(&repo, &["commit-tree", "HEAD^{tree}", "-m", "changed ref"]);
                s.git(&repo, &["update-ref", "refs/heads/main", &oid]);
            }
            "stash" => {
                s.save(&repo, b"external saved", json!({"message":"external"}));
            }
            "worktree" => {
                s.git(
                    &repo,
                    &[
                        "worktree",
                        "add",
                        "-qb",
                        "other",
                        s.root.join("new-worktree").to_str().unwrap(),
                    ],
                );
            }
            _ => unreachable!(),
        }
        let before = s
            .git_result(&repo, &["stash", "list", "--format=%H"], false)
            .stdout;
        s.rpc_raw(
            &repo,
            "rename_branch",
            json!({"name":"main","new_name":"renamed","basis":plan["basis"]}),
            true,
        );
        assert_eq!(s.git(&repo, &["symbolic-ref", "HEAD"]), "refs/heads/main");
        assert_eq!(
            s.git_result(&repo, &["stash", "list", "--format=%H"], false)
                .stdout,
            before
        );
        assert!(!repo.join(".git/desktop-branch-renames").exists());
    }
    let repo = s.init("invalid", &[]);
    s.save(&repo, b"saved", json!({"desktop":true}));
    s.git(&repo, &["branch", "existing"]);
    for new in ["existing", "--help", "bad name", "@{-1}"] {
        s.rpc_raw(
            &repo,
            "rename_branch_preview",
            json!({"name":"main","new_name":new}),
            true,
        );
    }
    assert_eq!(s.entries(&repo).as_array().unwrap().len(), 1);
    let repo = s.init("no-identity", &[]);
    let original = s.save(&repo, b"keep original", json!({"desktop":true}));
    s.git(&repo, &["config", "user.useConfigOnly", "true"]);
    s.git(&repo, &["config", "--unset", "user.name"]);
    s.git(&repo, &["config", "--unset", "user.email"]);
    s.rename(&repo, "main", "renamed", true);
    assert_eq!(s.entries(&repo)[0]["oid"], original);
    assert_eq!(s.git(&repo, &["symbolic-ref", "HEAD"]), "refs/heads/main");
    for label in ["sha256", "split", "linked", "reftable", "detached"] {
        let options: Vec<&str> = match label {
            "sha256" => vec!["--object-format=sha256"],
            "reftable" => vec!["--ref-format=reftable"],
            _ => vec![],
        };
        let mut repo = s.init(label, &options);
        s.save(&repo, b"saved content\0", json!({"desktop":true}));
        if label == "split" {
            s.git(&repo, &["update-index", "--split-index"]);
        }
        if label == "linked" {
            let sibling = s.root.join("sibling");
            s.git(
                &repo,
                &["worktree", "add", "-qb", "other", sibling.to_str().unwrap()],
            );
            repo = sibling;
        }
        if label == "detached" {
            s.git(&repo, &["switch", "--detach", "-q"]);
        }
        s.rename(&repo, "main", "renamed", false);
        s.git(
            &repo,
            &["show-ref", "--verify", "--hash", "refs/heads/renamed"],
        );
        let entries = s.entries(&repo);
        assert_eq!(entries.as_array().unwrap().len(), 1);
        assert_eq!(entries[0]["branch"], "renamed");
        s.rpc(&repo, "stash_apply", json!({"oid":entries[0]["oid"]}));
        assert_eq!(fs::read(repo.join("file")).unwrap(), b"saved content\0");
        if label == "linked" {
            assert_eq!(
                s.git(&s.root.join(label), &["symbolic-ref", "HEAD"]),
                "refs/heads/renamed"
            );
        }
        if label == "detached" {
            assert!(s.rpc(&repo, "snapshot", json!({}))["branch"].is_null());
        }
    }
    let repo = s.init("canonical-source", &[]);
    s.git(&repo, &["branch", "refs/heads/main"]);
    s.rename(
        &repo,
        "refs/heads/refs/heads/main",
        "canonical-renamed",
        false,
    );
    assert_eq!(s.git(&repo, &["symbolic-ref", "HEAD"]), "refs/heads/main");
    s.git(
        &repo,
        &["show-ref", "--verify", "refs/heads/canonical-renamed"],
    );
    s.git_result(
        &repo,
        &["show-ref", "--verify", "refs/heads/refs/heads/main"],
        true,
    );
}

const WRAPPER: &str = "#!/bin/sh\nbranch=0; move=0; stash=0; store=0; drop=0\nfor arg in \"$@\"; do\n case \"$arg\" in branch) branch=1;; -m) move=1;; stash) stash=1;; store) store=1;; drop) drop=1;; esac\ndone\nif [ \"$RENAME_MODE\" = before-rename ] && [ \"$branch$move\" = 11 ]; then exit 73; fi\nif [ \"$RENAME_MODE\" = drop ] && [ \"$stash$drop\" = 11 ]; then exit 74; fi\n\"$RENAME_REAL_GIT\" \"$@\"\nresult=$?\nif [ \"$result\" = 0 ]; then\n if [ \"$RENAME_MODE\" = after-rename ] && [ \"$branch$move\" = 11 ]; then exit 73; fi\n if [ \"$RENAME_MODE\" = concurrent ] && [ \"$stash$store\" = 11 ]; then \"$RENAME_REAL_GIT\" -C \"$RENAME_REPO\" update-ref refs/heads/main \"$RENAME_NEW_TIP\" || exit 95; fi\n if { [ \"$RENAME_MODE\" = cancel-store ] && [ \"$stash$store\" = 11 ]; } || { [ \"$RENAME_MODE\" = cancel-rename ] && [ \"$branch$move\" = 11 ]; }; then printf done > \"$RENAME_MARKER\"; sleep 60; fi\nfi\nexit \"$result\"\n";

#[test]
fn failures_cancellation_and_receipt_safety() {
    let mut s = Suite::new();
    let repo = s.init("receipt-files", &[]);
    fs::write(repo.join("file"), "untouched").unwrap();
    s.commit(&repo);
    let result: Value =
        serde_json::from_slice(&s.rename(&repo, "main", "renamed", false).stdout).unwrap();
    let receipt = result["receipt"]["id"].as_str().unwrap();
    let metadata = repo
        .join(".git/desktop-branch-renames")
        .join(format!("{receipt}.json"));
    let original = fs::read(&metadata).unwrap();
    let external = s.root.join("external-receipt");
    fs::write(&external, &original).unwrap();
    fs::remove_file(&metadata).unwrap();
    symlink(&external, &metadata).unwrap();
    assert!(s.rpc(&repo, "branch_rename_history", json!({}))[0]["error"].is_string());
    s.rpc_raw(
        &repo,
        "acknowledge_branch_rename",
        json!({"receipt_id":receipt}),
        true,
    );
    assert_eq!(fs::read(external).unwrap(), original);
    fs::remove_file(&metadata).unwrap();
    s.run("mkfifo", [metadata.as_os_str()], None, false);
    assert!(s.rpc(&repo, "branch_rename_history", json!({}))[0]["error"].is_string());
    s.rpc_raw(
        &repo,
        "acknowledge_branch_rename",
        json!({"receipt_id":receipt}),
        true,
    );
    fs::remove_file(&metadata).unwrap();
    fs::write(&metadata, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    assert!(
        s.rpc(&repo, "branch_rename_history", json!({}))[0]["error"]
            .as_str()
            .unwrap()
            .contains("size")
    );
    fs::write(metadata, original).unwrap();
    s.rpc(
        &repo,
        "acknowledge_branch_rename",
        json!({"receipt_id":receipt}),
    );
    assert_eq!(fs::read(repo.join("file")).unwrap(), b"untouched");
    assert_eq!(
        s.git(&repo, &["symbolic-ref", "HEAD"]),
        "refs/heads/renamed"
    );
    for phase in ["before-rename", "after-rename", "drop", "concurrent"] {
        let repo = s.init(phase, &[]);
        let original = s.save(&repo, b"saved payload", json!({"desktop":true}));
        let tip = (phase == "concurrent")
            .then(|| s.git(&repo, &["commit-tree", "HEAD^{tree}", "-m", "concurrent"]));
        s.wrapper(&repo, &format!("wrapper-{phase}"), WRAPPER);
        s.env.insert("RENAME_MODE".into(), phase.into());
        if let Some(tip) = tip {
            s.env.insert("RENAME_NEW_TIP".into(), tip.into());
        }
        s.rename(&repo, "main", "renamed", true);
        s.unmap_wrapper();
        let entries = s.entries(&repo);
        assert_eq!(entries.as_array().unwrap().len(), 2);
        assert!(
            entries
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["oid"] == original)
        );
        let history = s.rpc(&repo, "branch_rename_history", json!({}));
        assert!(history[0]["receipt"]["error"].is_string());
        assert_eq!(history[0]["saved_stashes"].as_array().unwrap().len(), 2);
        assert_eq!(
            !history[0]["destination_oid"].is_null(),
            ["after-rename", "drop"].contains(&phase)
        );
        for entry in entries.as_array().unwrap() {
            assert_eq!(
                s.git_result(
                    &repo,
                    &["show", &format!("{}:file", entry["oid"].as_str().unwrap())],
                    false
                )
                .stdout,
                b"saved payload"
            );
        }
    }
    for phase in ["store", "rename"] {
        let repo = s.init(&format!("cancel-{phase}"), &[]);
        let original = s.save(&repo, b"cancelled payload", json!({"desktop":true}));
        let marker = s.root.join(format!("started-{phase}"));
        s.wrapper(&repo, &format!("wrapper-cancel-{phase}"), WRAPPER);
        s.env
            .insert("RENAME_MODE".into(), format!("cancel-{phase}").into());
        s.env
            .insert("RENAME_MARKER".into(), marker.as_os_str().into());
        let mut child = s
            .command(env!("CARGO_BIN_EXE_desktop-rs"))
            .args([
                "-C",
                repo.to_str().unwrap(),
                "rename-branch",
                "main",
                "renamed",
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
                panic!("rename did not reach cancellation marker");
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "rename exited before marker"
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
                panic!("cancelled rename did not stop");
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
        s.unmap_wrapper();
        let history = s.rpc(&repo, "branch_rename_history", json!({}));
        assert_eq!(history[0]["saved_stashes"].as_array().unwrap().len(), 2);
        assert_eq!(
            history[0]["receipt"]["phase"],
            if phase == "store" {
                "storing"
            } else {
                "renaming"
            }
        );
        assert!(
            s.entries(&repo)
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["oid"] == original)
        );
    }
    let repo = s.init("lock", &[]);
    s.save(&repo, b"locked payload", json!({"desktop":true}));
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(repo.join(".git/desktop-stash.lock"))
        .unwrap();
    lock.lock().unwrap();
    let mut child = s
        .command(env!("CARGO_BIN_EXE_desktop-rs"))
        .args([
            "-C",
            repo.to_str().unwrap(),
            "rename-branch",
            "main",
            "renamed",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(child.try_wait().unwrap().is_none());
    assert!(!repo.join(".git/desktop-branch-renames").exists());
    drop(lock);
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("rename did not resume after lock release");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let repo = s.init("symlinks", &[]);
    let external = s.root.join("external");
    fs::create_dir(&external).unwrap();
    symlink(&external, repo.join(".git/desktop-branch-renames")).unwrap();
    s.rename(&repo, "main", "renamed", true);
    s.rpc_raw(&repo, "branch_rename_history", json!({}), true);
    assert_eq!(fs::read_dir(external).unwrap().count(), 0);
}

#[test]
fn lfs_saved_changes_keep_index_and_working_content() {
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
    let repo = s.init("lfs", &[]);
    s.rpc(&repo, "lfs_install", json!({}));
    s.git(&repo, &["lfs", "track", "*.lfs"]);
    let payload = repo.join("payload.lfs");
    fs::write(&payload, b"base payload").unwrap();
    s.commit(&repo);
    fs::write(&payload, b"index payload").unwrap();
    s.git(&repo, &["add", "--", "payload.lfs"]);
    fs::write(&payload, b"working payload").unwrap();
    fs::write(repo.join("untracked.lfs"), b"untracked payload").unwrap();
    s.rpc(
        &repo,
        "stash_save",
        json!({"desktop":true,"include_untracked":true}),
    );
    let before = s.entries(&repo)[0].clone();
    s.rename(&repo, "main", "renamed", false);
    let entry = s.entries(&repo)[0].clone();
    assert_eq!(entry["tree"], before["tree"]);
    assert_eq!(entry["parents"], before["parents"]);
    s.rpc(
        &repo,
        "stash_apply",
        json!({"oid":entry["oid"],"restore_index":true}),
    );
    assert_eq!(fs::read(payload).unwrap(), b"working payload");
    assert_eq!(
        fs::read(repo.join("untracked.lfs")).unwrap(),
        b"untracked payload"
    );
    assert_eq!(
        s.git_result(&repo, &["show", ":payload.lfs"], false).stdout,
        s.git_result(
            &repo,
            &[
                "show",
                &format!("{}:payload.lfs", before["parents"][1].as_str().unwrap())
            ],
            false
        )
        .stdout
    );
}
