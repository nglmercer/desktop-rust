//! Exercise repository registry persistence through the actual CLI.
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    root: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("desktop-registry-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let config = root.join("config/repositories.json");
        Self { root, config }
    }

    fn cli(&self, args: &[&str], fail: bool) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_desktop-rs"))
            .arg("--config")
            .arg(&self.config)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_SYSTEM")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("gitconfig"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            !fail,
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_slice(&self.cli(args, false).stdout).unwrap()
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_SYSTEM")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("gitconfig"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn initialization_clone_concurrent_registry_and_corruption_preservation() {
    let f = Fixture::new();
    assert_eq!(
        f.json(&["repositories", "list"]),
        json!({"version": 1, "selected": null, "repositories": []})
    );
    assert!(!f.config.exists(), "a read must not create the config");

    let source = f.root.join("source");
    assert_eq!(f.json(&["init", text(&source)])["selected"], text(&source));
    assert!(source.join(".git").is_dir());
    f.git(&["-C", text(&source), "config", "user.name", "Registry Test"]);
    f.git(&[
        "-C",
        text(&source),
        "config",
        "user.email",
        "registry@example.invalid",
    ]);
    fs::write(source.join("file"), "data").unwrap();
    f.git(&["-C", text(&source), "add", "--", "file"]);
    f.git(&[
        "-C",
        text(&source),
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-qm",
        "Initial",
    ]);

    let clone = f.root.join("cloned");
    assert_eq!(
        f.json(&["clone", text(&source), text(&clone)])["selected"],
        text(&clone)
    );
    assert_eq!(fs::read_to_string(clone.join("file")).unwrap(), "data");
    assert_eq!(f.json(&["--selected", "snapshot"])["root"], text(&clone));
    f.cli(&["repositories", "select", text(&source)], false);
    assert_eq!(f.json(&["--selected", "snapshot"])["root"], text(&source));
    f.cli(&["repositories", "add", text(&source)], false);
    assert_eq!(
        f.json(&["repositories", "list"])["repositories"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    f.cli(&["init", text(&source)], true);
    f.cli(&["clone", text(&source), text(&clone)], true);
    let bad = f.root.join("bad");
    f.cli(&["clone", "ext::bad", text(&bad)], true);
    assert!(!bad.exists());

    let added: Vec<_> = (0..6).map(|i| f.root.join(format!("repo-{i}"))).collect();
    for path in &added {
        f.git(&["init", "-q", text(path)]);
    }
    std::thread::scope(|scope| {
        let jobs: Vec<_> = added
            .iter()
            .map(|path| scope.spawn(|| f.cli(&["repositories", "add", text(path)], false)))
            .collect();
        for job in jobs {
            job.join().unwrap();
        }
    });
    let listed = f.json(&["repositories", "list"]);
    assert_eq!(listed["repositories"].as_array().unwrap().len(), 8);
    for path in &added {
        assert!(
            listed["repositories"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry == text(path)),
            "concurrent update lost {}",
            path.display()
        );
    }
    assert_eq!(listed["selected"], text(&source));
    f.cli(&["repositories", "forget", text(&source)], false);
    assert_eq!(fs::read_to_string(source.join("file")).unwrap(), "data");
    assert!(f.json(&["repositories", "list"])["selected"].is_null());
    f.cli(&["--selected", "snapshot"], true);

    fs::write(&f.config, "{broken").unwrap();
    let failure = f.cli(&["repositories", "add", text(&source)], true);
    assert!(String::from_utf8_lossy(&failure.stderr).contains("not overwritten"));
    assert_eq!(fs::read_to_string(&f.config).unwrap(), "{broken");
    assert!(
        !fs::read_dir(f.config.parent().unwrap())
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|extension| extension == "tmp"))
    );
}
