//! End-to-end typed Git operations against an isolated repository.
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("desktop-direct-git-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let repo = root.join("repository with spaces");
        let fixture = Self { root, repo };
        fixture.run(
            "git",
            &["init", "-q", "-b", "main", fixture.repo.to_str().unwrap()],
            false,
        );
        fixture.git(&["config", "user.name", "CLI fixture"]);
        fixture.git(&["config", "user.email", "cli@example.invalid"]);
        fixture.git(&["config", "commit.gpgsign", "false"]);
        fixture
    }

    fn run(&self, executable: &str, args: &[&str], expect_failure: bool) -> Output {
        let mut command = Command::new(executable);
        command
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_SYSTEM")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("global"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("command timed out: {executable} {args:?}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.success(),
            !expect_failure,
            "{executable} {args:?}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn git(&self, args: &[&str]) -> String {
        let mut command = vec!["-C", self.repo.to_str().unwrap()];
        command.extend_from_slice(args);
        String::from_utf8(self.run("git", &command, false).stdout)
            .unwrap()
            .trim()
            .to_owned()
    }

    fn cli(&self, args: &[&str], expect_failure: bool) {
        let registry = self.root.join("registry.json");
        let mut command = vec![
            "-C",
            self.repo.to_str().unwrap(),
            "--config",
            registry.to_str().unwrap(),
        ];
        command.extend_from_slice(args);
        let output = self.run(env!("CARGO_BIN_EXE_desktop-rs"), &command, expect_failure);
        if !expect_failure {
            let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                value
                    .get("code")
                    .and_then(|code| code.as_i64())
                    .unwrap_or(0),
                0,
                "{value}"
            );
        }
    }

    fn commit(&self, path: &str, content: &str) -> String {
        fs::write(self.repo.join(path), content).unwrap();
        self.git(&["add", "--", path]);
        self.git(&["commit", "-qm", path]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn file(&self, path: &str) -> PathBuf {
        self.repo.join(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn content(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn merge_rebase_cherry_pick_revert_and_conflict_recovery() {
    let f = Fixture::new();
    f.commit("base.txt", "base\n");
    f.git(&["switch", "-qc", "merge-source"]);
    f.commit("merged.txt", "merge content\n");
    f.git(&["switch", "-q", "main"]);
    f.cli(&["merge", "merge-source"], false);
    assert_eq!(content(&f.file("merged.txt")), "merge content\n");

    f.git(&["switch", "-qc", "pick-source"]);
    let picked = f.commit("picked.txt", "picked content\n");
    f.git(&["switch", "-q", "main"]);
    f.cli(&["cherry-pick", &picked], false);
    assert_eq!(content(&f.file("picked.txt")), "picked content\n");
    f.cli(&["revert", "HEAD"], false);
    assert!(!f.file("picked.txt").exists());

    f.git(&["switch", "-qc", "merge-parent-source"]);
    f.commit("merge-parent.txt", "merge parent\n");
    f.git(&["switch", "-q", "main"]);
    f.git(&[
        "merge",
        "--no-ff",
        "-qm",
        "merge parent fixture",
        "merge-parent-source",
    ]);
    let merge = f.git(&["rev-parse", "HEAD"]);
    f.git(&["switch", "-qc", "pick-merge", &format!("{merge}^1")]);
    f.cli(&["cherry-pick", &merge, "--mainline", "1"], false);
    assert!(f.file("merge-parent.txt").exists());
    f.git(&["switch", "-q", "main"]);
    f.cli(&["revert", &merge, "--mainline", "1"], false);
    assert!(!f.file("merge-parent.txt").exists());
    let before = f.git(&["rev-parse", "HEAD"]);
    f.cli(&["revert", &merge, "--mainline", "0"], true);
    assert_eq!(f.git(&["rev-parse", "HEAD"]), before);

    f.git(&["switch", "-qc", "rebase-topic"]);
    f.commit("topic.txt", "topic\n");
    f.git(&["switch", "-q", "main"]);
    f.commit("new-main.txt", "new main\n");
    f.git(&["switch", "-q", "rebase-topic"]);
    f.cli(&["rebase", "main"], false);
    assert!(f.file("topic.txt").exists() && f.file("new-main.txt").exists());

    f.git(&["switch", "-q", "main"]);
    f.commit("conflict.txt", "base\n");
    f.git(&["switch", "-qc", "conflict-side"]);
    let side = f.commit("conflict.txt", "side\n");
    f.git(&["switch", "-q", "main"]);
    f.commit("conflict.txt", "main\n");
    for (operation, target, recovery) in [
        ("merge", "conflict-side", "merge"),
        ("rebase", "conflict-side", "rebase"),
        ("cherry-pick", side.as_str(), "cherry_pick"),
    ] {
        f.cli(&[operation, target], true);
        assert_eq!(
            f.git(&["diff", "--name-only", "--diff-filter=U"]),
            "conflict.txt"
        );
        f.cli(&["recover", recovery, "abort"], false);
        assert_eq!(content(&f.file("conflict.txt")), "main\n");
    }
    assert_eq!(f.git(&["status", "--porcelain"]), "");
}
