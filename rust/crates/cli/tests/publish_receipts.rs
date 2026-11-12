//! Publication recovery history and acknowledgement safety.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::{
        ffi::OsStrExt,
        fs::{PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
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
            std::env::temp_dir().join(format!("desktop-publish-receipts-{}", uuid::Uuid::new_v4()));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let repo = root.join("history repo");
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
        f.git(&["config", "user.name", "Receipt Test"]);
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
        let real = String::from_utf8(f.run("sh", &["-c", "command -v git"])).unwrap();
        fs::create_dir(f.bin.join("native")).unwrap();
        symlink(real.trim(), f.bin.join("native/git")).unwrap();
        let wrapper = f.bin.join("git");
        fs::write(&wrapper, "#!/bin/sh\nfor arg do case \"$arg\" in push|pull|fetch|clone|ls-remote) printf attempted > \"$NETWORK_MARKER\"; exit 97;; esac; done\nexec \"$NATIVE_GIT\" \"$@\"\n").unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        let gh = f.bin.join("gh");
        fs::write(&gh, "#!/bin/sh\nprintf called > \"$GH_MARKER\"\nexit 97\n").unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o700)).unwrap();
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
            .env("NATIVE_GIT", self.bin.join("native/git"))
            .env("NETWORK_MARKER", self.root.join("network-attempted"))
            .env("GH_MARKER", self.root.join("gh-attempted"))
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
    fn rpc(&self, method: &str, mut params: Value, fail: bool) -> Output {
        params["method"] = json!(method);
        let mut child = self
            .command(env!("CARGO_BIN_EXE_desktop-rs"))
            .args([
                "--config",
                self.registry.to_str().unwrap(),
                "-C",
                self.repo.to_str().unwrap(),
                "repo",
                "-",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(params.to_string().as_bytes())
            .unwrap();
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
    fn value(&self, method: &str, params: Value) -> Value {
        serde_json::from_slice(&self.rpc(method, params, false).stdout).unwrap()
    }
    fn snapshot(&self) -> [Vec<u8>; 5] {
        [
            fs::read(self.repo.join(".git/config")).unwrap(),
            fs::read(self.repo.join(".git/HEAD")).unwrap(),
            self.git(&["for-each-ref", "--format=%(refname) %(objectname)"]),
            fs::read(self.repo.join(".git/index")).unwrap(),
            fs::read(&self.registry).unwrap(),
        ]
    }
    fn receipt(&self, phase: &str, error: Value) -> (PathBuf, Value) {
        use sha2::{Digest, Sha256};
        let hash = |data: &[u8]| format!("{:x}", Sha256::digest(data));
        let registry: Value = serde_json::from_slice(&fs::read(&self.registry).unwrap()).unwrap();
        let record = registry["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["path"] == self.repo.to_str().unwrap())
            .unwrap();
        let record_id = record["id"].as_str().unwrap();
        let namespace = hash(self.registry.as_os_str().as_bytes());
        let repository_id = hash(self.repo.as_os_str().as_bytes());
        let dir_id = hash(json!([namespace, record_id]).to_string().as_bytes());
        let directory = self.repo.join(".git/desktop-publish-receipts").join(dir_id);
        fs::create_dir_all(&directory).unwrap();
        fs::set_permissions(
            self.repo.join(".git/desktop-publish-receipts"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let value = json!({"version":1,"id":id,"repository_id":repository_id,"namespace":namespace,"record_id":record_id,"phase":phase,"host":"github.com","owner":"ExampleOrg","name":"example-repo","error":error,"acknowledged":false});
        let path = directory.join(format!("{id}.json"));
        private(&path, value.to_string().as_bytes());
        (path, value)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let network = self.root.join("network-attempted").exists();
        let gh = self.root.join("gh-attempted").exists();
        let _ = fs::remove_dir_all(&self.root);
        if !std::thread::panicking() {
            assert!(
                !network && !gh,
                "receipt history attempted a network operation"
            );
        }
    }
}
fn private(path: &Path, value: &[u8]) {
    fs::write(path, value).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn history_and_acknowledgement_are_idempotent_and_read_only() {
    let f = Fixture::new();
    let (first, receipt) = f.receipt("created", json!("network"));
    let (second, unknown) = f.receipt("creation_unknown", Value::Null);
    let before = f.snapshot();
    let untouched = fs::read(&second).unwrap();
    let history = f.value("publish_history", json!({}));
    let rows = history["receipts"].as_array().unwrap();
    for original in [&receipt, &unknown] {
        let row = rows.iter().find(|r| r["id"] == original["id"]).unwrap();
        assert_eq!(row["phase"], original["phase"]);
        assert_eq!(row["acknowledged"], false);
        assert!(row["url"].is_null());
    }
    let cli = f.run(
        env!("CARGO_BIN_EXE_desktop-rs"),
        &[
            "--config",
            f.registry.to_str().unwrap(),
            "-C",
            f.repo.to_str().unwrap(),
            "publish",
            "history",
        ],
    );
    assert_eq!(serde_json::from_slice::<Value>(&cli).unwrap(), history);
    f.value("acknowledge_publish", json!({"receipt_id":receipt["id"]}));
    let once = fs::read(&first).unwrap();
    let acknowledged = f.value("publish_history", json!({}));
    let rows = acknowledged["receipts"].as_array().unwrap();
    assert_eq!(
        rows.iter().find(|r| r["id"] == receipt["id"]).unwrap()["acknowledged"],
        true
    );
    assert_eq!(
        rows.iter().find(|r| r["id"] == unknown["id"]).unwrap()["acknowledged"],
        false
    );
    assert_eq!(fs::read(&second).unwrap(), untouched);
    assert_eq!(f.snapshot(), before);
    f.run(
        env!("CARGO_BIN_EXE_desktop-rs"),
        &[
            "--config",
            f.registry.to_str().unwrap(),
            "-C",
            f.repo.to_str().unwrap(),
            "publish",
            "acknowledge",
            receipt["id"].as_str().unwrap(),
        ],
    );
    assert_eq!(fs::read(&first).unwrap(), once);
    assert_eq!(f.value("publish_history", json!({})), acknowledged);
    assert_eq!(fs::metadata(first).unwrap().permissions().mode() & 0o077, 0);
    assert_eq!(f.snapshot(), before);
}

#[test]
fn invalid_foreign_and_oversize_receipts_are_not_acknowledged_or_disclosed() {
    let f = Fixture::new();
    let (path, receipt) = f.receipt("created", Value::Null);
    for (key, bad) in [
        ("version", json!(2)),
        ("repository_id", json!("0".repeat(64))),
        ("namespace", json!("0".repeat(64))),
        ("record_id", json!(uuid::Uuid::new_v4().to_string())),
        ("phase", json!("unrecognized")),
        ("host", json!("github.com@private.invalid")),
        ("owner", json!("owner/elsewhere")),
        ("name", json!("repo?token=private-marker")),
        ("error", json!("private-marker")),
    ] {
        let mut record = receipt.clone();
        record[key] = bad;
        private(&path, record.to_string().as_bytes());
        let before = fs::read(&path).unwrap();
        let history = f.value("publish_history", json!({}));
        assert!(!history.to_string().contains("private-marker"));
        let output = f.rpc(
            "acknowledge_publish",
            json!({"receipt_id":receipt["id"]}),
            true,
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("private-marker"));
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    private(&path, &vec![b'x'; 1024 * 1024 + 1]);
    f.value("publish_history", json!({}));
    f.rpc(
        "acknowledge_publish",
        json!({"receipt_id":receipt["id"]}),
        true,
    );
    for id in [
        "../private-marker".to_string(),
        "/tmp/private-marker".to_string(),
        receipt["id"].as_str().unwrap().to_ascii_uppercase(),
        "not-a-uuid".to_string(),
    ] {
        let output = f.rpc("acknowledge_publish", json!({"receipt_id":id}), true);
        assert!(!String::from_utf8_lossy(&output.stderr).contains("private-marker"));
    }
}

#[test]
fn receipt_symlinks_fifos_and_directory_symlinks_are_refused() {
    let f = Fixture::new();
    let (path, receipt) = f.receipt("created", Value::Null);
    let good = fs::read(&path).unwrap();
    let outside = f.root.join("outside");
    fs::write(&outside, b"outside-sentinel").unwrap();
    fs::remove_file(&path).unwrap();
    symlink(&outside, &path).unwrap();
    assert!(
        !f.value("publish_history", json!({}))
            .to_string()
            .contains("outside-sentinel")
    );
    f.rpc(
        "acknowledge_publish",
        json!({"receipt_id":receipt["id"]}),
        true,
    );
    assert_eq!(fs::read(&outside).unwrap(), b"outside-sentinel");
    assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
    fs::remove_file(&path).unwrap();
    f.run("mkfifo", &[path.to_str().unwrap()]);
    // Limit FIFO reads so an unsafe blocking open cannot hang the suite.
    for (request, success) in [
        (json!({"method":"publish_history"}), true),
        (
            json!({"method":"acknowledge_publish","receipt_id":receipt["id"]}),
            false,
        ),
    ] {
        let mut child = f
            .command("timeout")
            .args([
                "5",
                env!("CARGO_BIN_EXE_desktop-rs"),
                "--config",
                f.registry.to_str().unwrap(),
                "-C",
                f.repo.to_str().unwrap(),
                "repo",
                "-",
            ])
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
        let status = child.wait_with_output().unwrap().status;
        assert_ne!(status.code(), Some(124), "FIFO operation timed out");
        assert_eq!(status.success(), success);
    }
    fs::remove_file(&path).unwrap();
    private(&path, &good);
    let directory = path.parent().unwrap();
    fs::remove_file(&path).unwrap();
    fs::remove_dir(directory).unwrap();
    let other = f.root.join("outside-directory");
    fs::create_dir(&other).unwrap();
    symlink(&other, directory).unwrap();
    f.rpc("publish_history", json!({}), true);
    f.rpc(
        "acknowledge_publish",
        json!({"receipt_id":receipt["id"]}),
        true,
    );
    assert_eq!(fs::read_dir(&other).unwrap().count(), 0);
    assert_eq!(fs::read(&outside).unwrap(), b"outside-sentinel");
    fs::remove_file(directory).unwrap();
    fs::remove_dir(f.repo.join(".git/desktop-publish-receipts")).unwrap();
    symlink(&other, f.repo.join(".git/desktop-publish-receipts")).unwrap();
    f.rpc("publish_history", json!({}), true);
    assert_eq!(fs::read_dir(&other).unwrap().count(), 0);
}

#[test]
fn history_is_scoped_to_registry_and_saved_record() {
    let mut f = Fixture::new();
    let (path, receipt) = f.receipt("created", Value::Null);
    let before = fs::read(&path).unwrap();
    let original = f.registry.clone();
    let original_bytes = fs::read(&original).unwrap();
    let alternate = f.root.join("alternate/repositories.json");
    f.run(
        env!("CARGO_BIN_EXE_desktop-rs"),
        &[
            "--config",
            alternate.to_str().unwrap(),
            "repositories",
            "add",
            f.repo.to_str().unwrap(),
        ],
    );
    f.registry = alternate;
    assert_eq!(f.value("publish_history", json!({}))["receipts"], json!([]));
    f.rpc(
        "acknowledge_publish",
        json!({"receipt_id":receipt["id"]}),
        true,
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read(&original).unwrap(), original_bytes);
    f.registry = original;
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
    assert_eq!(f.value("publish_history", json!({}))["receipts"], json!([]));
    f.rpc(
        "acknowledge_publish",
        json!({"receipt_id":receipt["id"]}),
        true,
    );
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn untrusted_receipt_identity_is_never_reflected() {
    let f = Fixture::new();
    let (path, mut receipt) = f.receipt("created", Value::Null);
    receipt["name"] = json!("SyntheticAlphaToken123456");
    private(&path, receipt.to_string().as_bytes());
    let history = f.value("publish_history", json!({}));
    assert!(history["receipts"][0]["url"].is_null());
    assert!(!history.to_string().contains("SyntheticAlphaToken123456"));
    let value = f.value("acknowledge_publish", json!({"receipt_id":receipt["id"]}));
    assert!(value["receipt"]["url"].is_null());
    assert!(!value.to_string().contains("SyntheticAlphaToken123456"));
}

#[test]
fn registry_replacement_during_history_or_acknowledgement_is_rejected() {
    for method in ["publish_history", "acknowledge_publish"] {
        let f = Fixture::new();
        let (path, receipt) = f.receipt("created", Value::Null);
        let retained = fs::read(&path).unwrap();
        let real = f.bin.join("native/git");
        let wrapper = f.bin.join("git");
        let quoted = real.to_str().unwrap().replace('\'', "'\\''");
        fs::write(&wrapper, format!("#!/bin/sh\nfor arg do [ \"$arg\" != --absolute-git-dir ] || printf ready > \"$GITDIR_MARKER\"; done\nexec '{quoted}' \"$@\"\n")).unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        let lock_ready = f.root.join("lock-ready");
        let marker = f.root.join("gitdir-marker");
        let mut guard = f
            .command("flock")
            .args([
                "-x",
                f.registry.with_extension("lock").to_str().unwrap(),
                "sh",
                "-c",
                "printf ready > \"$LOCK_READY\"; exec cat",
            ])
            .env("LOCK_READY", &lock_ready)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !lock_ready.exists() && Instant::now() < deadline {
            assert!(guard.try_wait().unwrap().is_none());
            thread::sleep(Duration::from_millis(20));
        }
        assert!(lock_ready.exists());
        let mut request = json!({"method":method});
        if method == "acknowledge_publish" {
            request["receipt_id"] = receipt["id"].clone();
        }
        let mut child = f
            .command(env!("CARGO_BIN_EXE_desktop-rs"))
            .args([
                "--config",
                f.registry.to_str().unwrap(),
                "-C",
                f.repo.to_str().unwrap(),
                "repo",
                "-",
            ])
            .env("GITDIR_MARKER", &marker)
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
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() && Instant::now() < deadline {
            assert!(child.try_wait().unwrap().is_none());
            thread::sleep(Duration::from_millis(20));
        }
        if !marker.exists() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("receipt operation never inspected Git directory");
        }
        let mut registry: Value = serde_json::from_slice(&fs::read(&f.registry).unwrap()).unwrap();
        let row = registry["records"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|r| r["path"] == f.repo.to_str().unwrap())
            .unwrap();
        row["id"] = json!(uuid::Uuid::new_v4().to_string());
        private(&f.registry, registry.to_string().as_bytes());
        drop(guard.stdin.take());
        assert!(guard.wait().unwrap().success());
        let deadline = Instant::now() + Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        if child.try_wait().unwrap().is_none() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("receipt operation did not complete");
        }
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(fs::read(&path).unwrap(), retained);
    }
}
