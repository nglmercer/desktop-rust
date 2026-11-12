//! Staged blob review and consent through the real CLI and Git index.
#![cfg(unix)]

use serde_json::{Value, json};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::Write,
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

const LIMIT: u64 = 100 * 1024 * 1024;
struct Fixture {
    root: PathBuf,
    repo: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("desktop-commit-review-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let fixture = Self {
            repo: root.join("repo"),
            root,
        };
        fixture.init(&fixture.repo, &[]);
        fixture
    }
    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .env("HOME", &self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("global"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_LITERAL_PATHSPECS")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_CONFIG_KEY_")
                || name.to_string_lossy().starts_with("GIT_CONFIG_VALUE_")
            {
                command.env_remove(name);
            }
        }
        command
    }
    fn run<I, S>(
        &self,
        program: &str,
        args: I,
        input: Option<&[u8]>,
        fail: bool,
        literal: bool,
    ) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command(program);
        command
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if literal {
            command.env("GIT_LITERAL_PATHSPECS", "1");
        }
        let mut child = command.spawn().unwrap();
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
    fn git<I, S>(&self, repo: &Path, args: I) -> Vec<u8>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut argv = vec![OsString::from("-C"), repo.as_os_str().to_owned()];
        argv.extend(args.into_iter().map(|s| s.as_ref().to_owned()));
        self.run("git", argv, None, false, false).stdout
    }
    fn init(&self, repo: &Path, options: &[&str]) {
        let mut args: Vec<OsString> = ["init", "-q", "-b", "main"]
            .into_iter()
            .map(Into::into)
            .collect();
        args.extend(options.iter().map(OsString::from));
        args.push(repo.as_os_str().to_owned());
        self.run("git", args, None, false, false);
        self.git(repo, ["config", "user.name", "Commit Review"]);
        self.git(repo, ["config", "user.email", "review@example.invalid"]);
        self.git(repo, ["config", "commit.gpgsign", "false"]);
    }
    fn rpc_raw(
        &self,
        repo: &Path,
        method: &str,
        mut params: Value,
        fail: bool,
        literal: bool,
    ) -> Output {
        params["method"] = json!(method);
        self.run(
            env!("CARGO_BIN_EXE_desktop-rs"),
            [
                OsString::from("-C"),
                repo.as_os_str().to_owned(),
                OsString::from("repo"),
                OsString::from("-"),
            ],
            Some(params.to_string().as_bytes()),
            fail,
            literal,
        )
    }
    fn rpc(&self, repo: &Path, method: &str, params: Value) -> Value {
        serde_json::from_slice(&self.rpc_raw(repo, method, params, false, false).stdout).unwrap()
    }
    fn preview(&self, repo: &Path) -> Value {
        self.rpc(repo, "commit_preview", json!({}))
    }
    fn commit(&self, repo: &Path, basis: Option<&Value>, large: bool) -> Value {
        let mut params = json!({"summary":"reviewed commit", "allow_large_files":large});
        if let Some(basis) = basis {
            params["basis"] = basis["basis"].clone();
        }
        self.rpc(repo, "commit", params)
    }
    fn stage(&self, repo: &Path, mode: &str, oid: &str, name: &OsStr) {
        self.git(
            repo,
            [
                OsStr::new("update-index"),
                OsStr::new("--add"),
                OsStr::new("--cacheinfo"),
                OsStr::new(mode),
                OsStr::new(oid),
                name,
            ],
        );
    }
    fn text(&self, repo: &Path, args: &[&str]) -> String {
        String::from_utf8(self.git(repo, args))
            .unwrap()
            .trim()
            .to_owned()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn staged_blob_review_preserves_consent_and_native_git_behavior() {
    let f = Fixture::new();
    let repo = &f.repo;
    assert_eq!(f.preview(repo)["staged_count"], 0);
    let payload = f.root.join("payload");
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&payload)
        .unwrap();
    file.set_len(LIMIT).unwrap();
    let exact = f.text(
        repo,
        &["hash-object", "-w", "--", payload.to_str().unwrap()],
    );
    file.set_len(LIMIT + 1).unwrap();
    drop(file);
    let large = f.text(
        repo,
        &["hash-object", "-w", "--", payload.to_str().unwrap()],
    );
    f.stage(repo, "100644", &exact, OsStr::new("exact.bin"));
    assert_eq!(f.preview(repo)["large_files"], json!([]));
    assert_eq!(f.commit(repo, None, false)["code"], 0);
    let old_head = f.git(repo, ["rev-parse", "HEAD"]);
    f.stage(repo, "100644", &large, OsStr::new("oversized.bin"));
    let plan = f.preview(repo);
    assert_eq!(plan["staged_count"], 1);
    assert_eq!(plan["threshold_bytes"], LIMIT);
    assert_eq!(plan["large_files"][0]["size"], LIMIT + 1);
    assert_eq!(plan["large_files"][0]["display_path"], "oversized.bin");
    assert_eq!(
        plan["large_files"][0]["path"]["bytes"],
        json!(b"oversized.bin".to_vec())
    );
    assert_eq!(plan["large_files"][0]["oid"], large);
    assert_eq!(plan["large_files"][0]["tracked_by_lfs"], false);
    let index = fs::read(repo.join(".git/index")).unwrap();
    fs::hard_link(&payload, repo.join("oversized.bin")).unwrap();
    let selected = f.rpc(
        repo,
        "selected_commit_preview",
        json!({"paths":["oversized.bin"]}),
    );
    assert_eq!(selected["review"]["staged_count"], 1);
    assert_eq!(selected["review"]["large_files"][0]["size"], LIMIT + 1);
    assert_eq!(selected["review"]["large_files"][0]["oid"], large);
    assert_eq!(
        selected["review"]["files"][0]["bytes"],
        json!(b"oversized.bin".to_vec())
    );
    fs::write(repo.join(".gitattributes"), "*.bin filter=lfs\n").unwrap();
    let with_lfs = f.rpc(
        repo,
        "selected_commit_preview",
        json!({"paths":["oversized.bin"]}),
    );
    assert_eq!(with_lfs["review"]["large_files"][0]["tracked_by_lfs"], true);
    assert_ne!(with_lfs["basis"], selected["basis"]);
    fs::remove_file(repo.join(".gitattributes")).unwrap();
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
    fs::remove_file(repo.join("oversized.bin")).unwrap();
    let denied = f.rpc_raw(repo, "commit", json!({"summary":"blocked"}), true, false);
    let denied: Value = serde_json::from_slice(&denied.stdout).unwrap();
    assert_eq!(denied["code"], 2);
    assert_eq!(denied["requires_confirmation"], true);
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
    assert_eq!(f.git(repo, ["rev-parse", "HEAD"]), old_head);
    f.rpc_raw(
        repo,
        "commit",
        json!({"summary":"unreviewed", "allow_large_files":true}),
        true,
        false,
    );
    let cli = f.run(
        env!("CARGO_BIN_EXE_desktop-rs"),
        ["-C", repo.to_str().unwrap(), "commit-preview"],
        None,
        false,
        false,
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&cli.stdout).unwrap()["basis"],
        plan["basis"]
    );
    fs::write(repo.join("normal.txt"), "small\n").unwrap();
    let small = f.text(repo, &["hash-object", "-w", "--", "normal.txt"]);
    f.git(repo, ["replace", &large, &small]);
    assert!(
        f.text(repo, &["cat-file", "-s", &large])
            .parse::<u64>()
            .unwrap()
            < LIMIT
    );
    assert_eq!(f.preview(repo)["large_files"][0]["size"], LIMIT + 1);
    f.git(repo, ["replace", "-d", &large]);
    fs::write(repo.join(".gitattributes"), "*.bin filter=lfs\n").unwrap();
    assert_eq!(f.preview(repo)["large_files"][0]["tracked_by_lfs"], true);
    f.rpc_raw(
        repo,
        "commit",
        json!({"summary":"stale attributes", "basis":plan["basis"], "allow_large_files":true}),
        true,
        false,
    );
    fs::remove_file(repo.join(".gitattributes")).unwrap();
    f.git(repo, ["branch", "same-head"]);
    f.git(repo, ["symbolic-ref", "HEAD", "refs/heads/same-head"]);
    f.rpc_raw(
        repo,
        "commit",
        json!({"summary":"wrong branch", "basis":plan["basis"], "allow_large_files":true}),
        true,
        false,
    );
    f.git(repo, ["symbolic-ref", "HEAD", "refs/heads/main"]);
    fs::write(repo.join(".git/MERGE_MSG"), "new operation context\n").unwrap();
    f.rpc_raw(
        repo,
        "commit",
        json!({"summary":"stale operation", "basis":plan["basis"], "allow_large_files":true}),
        true,
        false,
    );
    fs::remove_file(repo.join(".git/MERGE_MSG")).unwrap();
    f.git(repo, ["add", "--", "normal.txt"]);
    f.rpc_raw(
        repo,
        "commit",
        json!({"summary":"stale index", "basis":plan["basis"], "allow_large_files":true}),
        true,
        false,
    );
    f.git(repo, ["reset", "-q", "--", "normal.txt"]);
    let plan = f.preview(repo);
    let override_result = f.run(
        env!("CARGO_BIN_EXE_desktop-rs"),
        [
            "-C",
            repo.to_str().unwrap(),
            "commit",
            "reviewed CLI override",
            "--allow-large-files",
            "--basis",
            plan["basis"].as_str().unwrap(),
        ],
        None,
        false,
        false,
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&override_result.stdout).unwrap()["code"],
        0
    );
    assert_eq!(
        f.text(
            repo,
            &[
                "--no-replace-objects",
                "cat-file",
                "-s",
                "HEAD:oversized.bin"
            ]
        ),
        (LIMIT + 1).to_string()
    );
    f.git(repo, ["add", "--", "normal.txt"]);
    assert_eq!(f.preview(repo)["large_files"], json!([]));
    assert_eq!(f.commit(repo, Some(&f.preview(repo)), false)["code"], 0);
    let pointer = repo.join("pointer.bin");
    fs::write(
        &pointer,
        format!(
            "version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize {}\n",
            "a".repeat(64),
            LIMIT + 1
        ),
    )
    .unwrap();
    f.git(repo, ["add", "--", "pointer.bin"]);
    OpenOptions::new()
        .write(true)
        .open(&pointer)
        .unwrap()
        .set_len(LIMIT + 1)
        .unwrap();
    assert_eq!(f.preview(repo)["large_files"], json!([]));
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(repo.join("intent.bin"))
        .unwrap()
        .set_len(LIMIT + 1)
        .unwrap();
    f.git(repo, ["add", "-N", "--", "intent.bin"]);
    f.stage(
        repo,
        "160000",
        &"b".repeat(large.len()),
        OsStr::new("module"),
    );
    f.stage(repo, "120000", &large, OsStr::new("link"));
    assert_eq!(f.preview(repo)["large_files"], json!([]));
    f.git(repo, ["reset", "-q", "HEAD", "--"]);
    let raw = OsString::from_vec(b"raw-\xff\n.bin".to_vec());
    f.stage(repo, "100644", &large, &raw);
    assert_eq!(
        f.preview(repo)["large_files"][0]["path"]["bytes"],
        json!(b"raw-\xff\n.bin".to_vec())
    );
    f.git(repo, ["reset", "-q", "HEAD", "--"]);
    f.git(
        repo,
        ["update-index", "--force-remove", "--", "oversized.bin"],
    );
    f.stage(repo, "100644", &large, OsStr::new("renamed.bin"));
    assert_eq!(
        f.preview(repo)["large_files"][0]["display_path"],
        "renamed.bin"
    );
    f.git(repo, ["reset", "-q", "HEAD", "--"]);
    f.git(repo, ["add", "--", "normal.txt"]);
    let plan = f.preview(repo);
    let old = f.text(repo, &["rev-parse", "HEAD"]);
    let new = f.text(
        repo,
        &["commit-tree", "HEAD^{tree}", "-m", "different HEAD"],
    );
    f.git(repo, ["update-ref", "HEAD", &new]);
    f.rpc_raw(
        repo,
        "commit",
        json!({"summary":"stale HEAD", "basis":plan["basis"]}),
        true,
        false,
    );
    f.git(repo, ["update-ref", "HEAD", &old]);
    fs::write(repo.join("normal.txt"), "changed\n").unwrap();
    f.git(repo, ["add", "--", "normal.txt"]);
    let hook = repo.join(".git/hooks/pre-commit");
    fs::write(&hook, "#!/bin/sh\ngit ls-files \"*.txt\" > hook-paths\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    let plan = f.preview(repo);
    f.rpc_raw(
        repo,
        "commit",
        json!({"summary":"hook test", "basis":plan["basis"]}),
        false,
        true,
    );
    assert_eq!(
        fs::read_to_string(repo.join("hook-paths")).unwrap(),
        "normal.txt\n"
    );
    fs::remove_file(hook).unwrap();
    for (name, options) in [
        ("split", vec![]),
        ("sha256", vec!["--object-format=sha256"]),
    ] {
        let other = f.root.join(name);
        f.init(&other, &options);
        fs::write(other.join("file"), "first\n").unwrap();
        f.git(&other, ["add", "--all"]);
        f.git(&other, ["commit", "-qm", "initial"]);
        if name == "split" {
            f.git(&other, ["update-index", "--split-index"]);
        }
        fs::write(other.join("file"), "second\n").unwrap();
        f.git(&other, ["add", "--", "file"]);
        assert_eq!(f.commit(&other, Some(&f.preview(&other)), false)["code"], 0);
        if name == "sha256" {
            let oid = f.text(
                &other,
                &["hash-object", "-w", "--", payload.to_str().unwrap()],
            );
            f.stage(&other, "100644", &oid, OsStr::new("big"));
            assert_eq!(f.preview(&other)["large_files"][0]["size"], LIMIT + 1);
            assert_eq!(f.commit(&other, Some(&f.preview(&other)), true)["code"], 0);
        }
    }
    let linked = f.root.join("linked");
    f.git(
        repo,
        ["worktree", "add", "-qb", "linked", linked.to_str().unwrap()],
    );
    fs::write(linked.join("normal.txt"), "linked\n").unwrap();
    f.git(&linked, ["add", "--", "normal.txt"]);
    assert_eq!(
        f.commit(&linked, Some(&f.preview(&linked)), false)["code"],
        0
    );
}
