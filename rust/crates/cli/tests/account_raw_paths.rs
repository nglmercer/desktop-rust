//! Saved-account identity preserves distinct non-UTF-8 repository paths.
#![cfg(unix)]

use serde_json::{Value, json};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::PermissionsExt,
    },
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

struct Fixture {
    root: PathBuf,
    registry: PathBuf,
    path: OsString,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("desktop-account-raw-{}", uuid::Uuid::new_v4()));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let gh = bin.join("gh");
        fs::write(&gh, r##"#!/bin/sh
case "$*" in
  'auth status'*) printf '%s\n' '{"hosts":{"github.com":[{"host":"github.com","login":"AliceCase","active":true,"state":"success"}]}}';;
  'auth token'*'--user'*) printf '%s\n' 'synthetic-named-token';;
  'auth token'*) printf '%s\n' 'synthetic-effective-token';;
  'api '*'/user'*) [ "${GH_TOKEN-}" = synthetic-named-token ] || exit 91; printf '%s\n' '{"id":101,"login":"alicecase","plan":{"name":"pro"}}';;
  'api '*'/repos/fork/repo'*) [ "${GH_TOKEN-}" = synthetic-named-token ] || exit 92; printf '%s\n' '{"id":2001,"full_name":"fork/repo","name":"repo","html_url":"https://github.com/fork/repo","owner":{"login":"fork"},"private":true,"fork":false,"parent":null}';;
  *) exit 93;;
esac
"##).unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o700)).unwrap();
        let path = std::env::join_paths(
            std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        Self {
            registry: root.join("repositories.json"),
            root,
            path,
        }
    }
    fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut cmd = Command::new(program);
        cmd.env("HOME", &self.root)
            .env("PATH", &self.path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("global.config"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GH_TOKEN", "synthetic-effective-token")
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_ENTERPRISE_TOKEN")
            .env_remove("GITHUB_ENTERPRISE_TOKEN")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GH_HOST")
            .env_remove("GH_REPO");
        cmd
    }
    fn run(&self, mut command: Command, input: Option<&[u8]>) -> Output {
        command
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        if let Some(bytes) = input {
            child.stdin.take().unwrap().write_all(bytes).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        for text in [&output.stdout, &output.stderr] {
            let text = String::from_utf8_lossy(text);
            assert!(
                !text.contains("synthetic-named-token")
                    && !text.contains("synthetic-effective-token"),
                "credential was exposed"
            );
        }
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    fn git(&self, repo: &Path, args: &[&str]) {
        let mut cmd = self.command("git");
        cmd.arg("-C").arg(repo).args(args);
        self.run(cmd, None);
    }
    fn cli(&self, args: &[&OsStr]) -> Value {
        let mut cmd = self.command(env!("CARGO_BIN_EXE_desktop-rs"));
        cmd.arg("--config").arg(&self.registry).args(args);
        serde_json::from_slice(&self.run(cmd, None).stdout).unwrap()
    }
    fn rpc(&self, repo: &Path, request: Value) -> Value {
        let mut cmd = self.command(env!("CARGO_BIN_EXE_desktop-rs"));
        cmd.arg("--config")
            .arg(&self.registry)
            .arg("-C")
            .arg(repo)
            .args(["repo", "-"]);
        serde_json::from_slice(&self.run(cmd, Some(request.to_string().as_bytes())).stdout).unwrap()
    }
    fn seed(&self, suffix: u8) -> PathBuf {
        let mut bytes = self.root.as_os_str().as_bytes().to_vec();
        bytes.extend_from_slice(b"/raw-account-");
        bytes.push(suffix);
        let repo = PathBuf::from(OsString::from_vec(bytes));
        let mut init = self.command("git");
        init.args(["init", "-q", "-b", "main"]).arg(&repo);
        self.run(init, None);
        self.git(&repo, &["config", "user.name", "Raw Path Test"]);
        self.git(&repo, &["config", "user.email", "raw@example.invalid"]);
        fs::write(repo.join("file"), b"base\n").unwrap();
        self.git(&repo, &["add", "--all"]);
        self.git(&repo, &["commit", "-qm", "checkpoint"]);
        self.git(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/fork/repo.git",
            ],
        );
        self.cli(&[
            OsStr::new("repositories"),
            OsStr::new("add"),
            repo.as_os_str(),
        ]);
        repo
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn separate_invalid_utf8_paths_keep_distinct_account_scopes() {
    let f = Fixture::new();
    let one = f.seed(0xff);
    let two = f.seed(0xfe);
    for (repo, target) in [
        (
            &one,
            json!({"mode":"assigned", "host":"github.com", "login":"AliceCase"}),
        ),
        (&two, json!({"mode":"none"})),
    ] {
        let plan = f.rpc(
            repo,
            json!({"method":"repository_account_preview", "target":target}),
        );
        assert_eq!(plan["can_apply"], true, "{plan}");
        f.rpc(
            repo,
            json!({"method":"set_repository_account", "target":target, "basis":plan["basis"]}),
        );
    }
    let alice = f.rpc(&one, json!({"method":"repository_account"}));
    let disabled = f.rpc(&two, json!({"method":"repository_account"}));
    assert_ne!(alice["scope_id"], disabled["scope_id"]);
    assert_eq!(alice["assignment"]["id"], 101);
    assert_eq!(disabled["assignment"]["mode"], "none");
    let registry: Value = serde_json::from_slice(&fs::read(&f.registry).unwrap()).unwrap();
    let records = registry["records"].as_array().unwrap();
    for repo in [&one, &two] {
        assert!(
            records
                .iter()
                .any(|record| record["path"]["bytes"] == json!(repo.as_os_str().as_bytes()))
        );
    }
}
