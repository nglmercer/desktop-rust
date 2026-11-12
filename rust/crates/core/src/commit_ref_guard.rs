//! Reference-transaction fence for a future selected-commit transaction.
//!
//! Git's configured hooks are used so existing hook files and configured
//! callbacks keep their normal behavior. The caller must retain this guard
//! until Git and all of its hooks have finished.
use crate::{Repository, args, discard_fs, os_from_bytes, process};
use anyhow::Result;
use std::{ffi::OsString, fs, path::PathBuf};

pub struct CommitReferenceGuard {
    directory: PathBuf,
    configuration: Vec<OsString>,
    hash_length: usize,
}

impl CommitReferenceGuard {
    /// Git `-c` arguments; they must be placed before the commit subcommand.
    pub fn configuration(&self) -> &[OsString] {
        &self.configuration
    }

    /// The object Git prepared under its reference lock, if the guard ran.
    /// This is not proof of publication: Git or a later hook may still fail.
    /// A caller must verify the actual reference and retain recovery state.
    pub fn prepared_oid(&self) -> Result<Option<String>> {
        match fs::read(self.directory.join("prepared")) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
            Ok(bytes) => {
                let value = std::str::from_utf8(&bytes)?.trim_end_matches('\n');
                anyhow::ensure!(
                    valid_oid(value, self.hash_length),
                    "invalid guarded reference receipt"
                );
                Ok(Some(value.to_owned()))
            }
        }
    }
}

impl Drop for CommitReferenceGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn valid_oid(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn quote(value: &[u8]) -> Vec<u8> {
    let mut output = vec![b'\''];
    for b in value {
        if *b == b'\'' {
            output.extend_from_slice(b"'\\''");
        } else {
            output.push(*b);
        }
    }
    output.push(b'\'');
    output
}

impl Repository {
    /// Fence a later Git commit's reference update to the reviewed HEAD and
    /// parent. `branch` is None for detached HEAD. `parent` is the resulting
    /// commit's parent, which differs from HEAD when amending. `tree` is
    /// checked against the prepared commit under the ref lock. The
    /// caller must retain the guard, reject a missing receipt, and reconcile
    /// the index and refs after Git and its hooks finish.
    pub async fn commit_reference_guard(
        &self,
        branch: Option<&str>,
        head: Option<&str>,
        parent: Option<&str>,
        tree: &str,
        hash_length: usize,
    ) -> Result<CommitReferenceGuard> {
        anyhow::ensure!(
            matches!(hash_length, 40 | 64),
            "unsupported Git object format"
        );
        anyhow::ensure!(
            head.into_iter()
                .chain(parent)
                .chain(Some(tree))
                .all(|oid| valid_oid(oid, hash_length)),
            "invalid reviewed commit object ID"
        );
        if let Some(branch) = branch {
            anyhow::ensure!(
                branch.starts_with("refs/heads/") && !branch.contains(['\0', '\n', '\r']),
                "invalid reviewed branch"
            );
            let check = self.run(&args(&["check-ref-format", branch])).await?;
            anyhow::ensure!(check.code == 0, "invalid reviewed branch");
        }
        let base = self.git_directory().await?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let directory = base.join(format!("desktop-commit-guard-{id}"));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
        let mut guard = CommitReferenceGuard {
            directory,
            configuration: Vec::new(),
            hash_length,
        };
        let shell = self.run(&args(&["var", "GIT_SHELL_PATH"])).await?;
        anyhow::ensure!(shell.code == 0, "cannot resolve Git hook shell");
        let shell = shell.stdout.strip_suffix(b"\n").unwrap_or(&shell.stdout);
        #[cfg(not(windows))]
        anyhow::ensure!(
            !shell.is_empty() && !shell.iter().any(u8::is_ascii_whitespace),
            "unsupported Git hook shell path"
        );
        let probe = format!("desktop-commit-probe-{id}");
        let mut script = b"#!".to_vec();
        #[cfg(windows)]
        script.extend_from_slice(b"/bin/sh");
        #[cfg(not(windows))]
        script.extend_from_slice(shell);
        script.extend_from_slice(b"\nif [ \"$1\" = ");
        script.extend(quote(probe.as_bytes()));
        script.extend_from_slice(b" ]; then printf ok > ");
        script.extend(quote(
            guard.directory.join("probe").as_os_str().as_encoded_bytes(),
        ));
        script.extend_from_slice(
            b"; exit $?; fi\ncase \"$1\" in preparing|prepared) ;; *) exit 0 ;; esac\ninput=",
        );
        script.extend(quote(
            guard.directory.join("input").as_os_str().as_encoded_bytes(),
        ));
        script.extend_from_slice(b".$$\nobject=");
        script.extend(quote(
            guard
                .directory
                .join("object")
                .as_os_str()
                .as_encoded_bytes(),
        ));
        script.extend_from_slice(
            b".$$\ntrap 'rm -f \"$input\" \"$object\"' 0\ncat >\"$input\" || exit 1\ntarget=",
        );
        script.extend(quote(branch.unwrap_or("HEAD").as_bytes()));
        script.extend_from_slice(b"\nexpected=");
        script.extend(quote(head.unwrap_or(&"0".repeat(hash_length)).as_bytes()));
        script.extend_from_slice(b"\nparent=");
        script.extend(quote(
            parent
                .map(|oid| format!("parent {oid}"))
                .unwrap_or_default()
                .as_bytes(),
        ));
        script.extend_from_slice(b"\nexpected_tree=");
        script.extend(quote(tree.as_bytes()));
        script.extend_from_slice(b"\nwhile IFS=' ' read -r old new ref; do\n if { [ \"$1\" = preparing ] && [ \"$ref\" = HEAD ]; } || { [ \"$1\" = prepared ] && [ \"$ref\" = \"$target\" ]; }; then\n  actual=$(git symbolic-ref --quiet HEAD)\n  status=$?\n");
        if let Some(branch) = branch {
            script.extend_from_slice(b"  [ \"$status\" = 0 ] && [ \"$actual\" = ");
            script.extend(quote(branch.as_bytes()));
            script.extend_from_slice(b" ] || { echo 'Reviewed branch changed; commit refused' >&2; exit 1; }\n  git symbolic-ref --quiet \"$target\" >/dev/null\n  [ \"$?\" = 1 ] || { echo 'Reviewed branch is not a direct ref' >&2; exit 1; }\n");
        } else {
            script.extend_from_slice(b"  [ \"$status\" = 1 ] || { echo 'Detached HEAD changed; commit refused' >&2; exit 1; }\n");
        }
        script.extend_from_slice(b"  [ \"$old\" = \"$expected\" ] || { echo 'Reviewed HEAD changed; commit refused' >&2; exit 1; }\n  [ ${#new} = ");
        script.extend_from_slice(hash_length.to_string().as_bytes());
        script.extend_from_slice(b" ] || exit 1\n  case \"$new\" in *[!0-9a-fA-F]*|'') exit 1 ;; esac\n  git --no-replace-objects cat-file commit \"$new\" >\"$object\" || exit 1\n  parents=$(sed -n '/^$/q; /^parent /p' \"$object\") || exit 1\n  [ \"$parents\" = \"$parent\" ] || { echo 'Commit parent differs from review; commit refused' >&2; exit 1; }\n");
        script.extend_from_slice(b"  actual_tree=$(sed -n '1s/^tree //p' \"$object\") || exit 1\n  [ \"$actual_tree\" = \"$expected_tree\" ] || { echo 'Commit tree differs from review; commit refused' >&2; exit 1; }\n");
        script.extend_from_slice(b"  if [ \"$1\" = prepared ]; then printf '%s\\n' \"$new\" > ");
        script.extend(quote(
            guard
                .directory
                .join("prepared")
                .as_os_str()
                .as_encoded_bytes(),
        ));
        script.extend_from_slice(b" || exit 1; fi\n fi\ndone <\"$input\"\n");
        let hook = guard.directory.join("guard");
        discard_fs::private_write(&hook, &script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o700))?;
        }
        let name = format!("hook.desktopcommit{id}");
        let mut command = format!("{name}.command=").into_bytes();
        command.extend(quote(hook.as_os_str().as_encoded_bytes()));
        guard.configuration = vec![
            "-c".into(),
            os_from_bytes(&command)?,
            "-c".into(),
            format!("{name}.event=reference-transaction").into(),
            "-c".into(),
            format!("{name}.event={probe}").into(),
            "-c".into(),
            format!("{name}.enabled=true").into(),
        ];
        let mut test = guard.configuration.clone();
        test.extend(args(&[
            "hook",
            "run",
            "--allow-unknown-hook-name",
            "--ignore-missing",
            &probe,
            "--",
            &probe,
        ]));
        let result = process::run_checkout(&self.path, &test).await?;
        anyhow::ensure!(
            result.code == 0
                && fs::read(guard.directory.join("probe")).ok().as_deref() == Some(b"ok"),
            "Git does not support the selected-commit reference guard"
        );
        Ok(guard)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture {
        root: PathBuf,
    }
    impl Fixture {
        fn git(&self, arguments: &[&str]) -> String {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(arguments)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", self.root.join("global.config"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        }
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("desktop-commit-ref-'{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            fs::write(root.join("global.config"), "").unwrap();
            let fixture = Self { root };
            fixture.git(&["init", "-q"]);
            fixture.git(&["config", "user.name", "Test"]);
            fixture.git(&["config", "user.email", "test@example.invalid"]);
            fixture.git(&["config", "commit.gpgsign", "false"]);
            fs::write(fixture.root.join("file"), "initial\n").unwrap();
            fixture.git(&["add", "file"]);
            fixture.git(&["commit", "-qm", "initial"]);
            for (name, content) in [
                ("pre-commit", "#!/bin/sh\nprintf pre >> .git/pre-events\n"),
                (
                    "reference-transaction",
                    "#!/bin/sh\nprintf '%s\\n' \"$1\" >> .git/ref-events\ncat >/dev/null\n",
                ),
            ] {
                let hook = fixture.root.join(".git/hooks").join(name);
                fs::write(&hook, content).unwrap();
                fs::set_permissions(hook, fs::Permissions::from_mode(0o700)).unwrap();
            }
            fixture
        }
        async fn commit(&self, guard: &CommitReferenceGuard, extra: &[&str]) -> process::RawOutput {
            let mut command = guard.configuration().to_vec();
            command.extend(args(&[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "guarded",
            ]));
            command.extend(args(extra));
            process::run_commit(&self.root, &command).await.unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn commit_reference_fence_rejects_tree_changed_by_pre_commit_hook() {
        let f = Fixture::new();
        let repo = Repository::open(&f.root).await.unwrap();
        let branch = f.git(&["symbolic-ref", "HEAD"]);
        let head = f.git(&["rev-parse", "HEAD"]);
        fs::write(f.root.join("file"), b"reviewed\n").unwrap();
        f.git(&["add", "file"]);
        let tree = f.git(&["write-tree"]);
        assert!(
            repo.commit_reference_guard(
                Some(&branch),
                Some(&head),
                Some(&head),
                "invalid",
                head.len()
            )
            .await
            .is_err()
        );
        let guard = match repo
            .commit_reference_guard(Some(&branch), Some(&head), Some(&head), &tree, head.len())
            .await
        {
            Ok(guard) => guard,
            Err(error) if error.to_string().contains("does not support") => {
                eprintln!("configured Git hooks are unavailable; guard rejected this Git version");
                return;
            }
            Err(error) => panic!("{error:#}"),
        };
        let hook = f.root.join(".git/hooks/pre-commit");
        fs::write(
            &hook,
            "#!/bin/sh\nprintf 'unreviewed\\n' > file\ngit add -- file\n",
        )
        .unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
        let result = f.commit(&guard, &[]).await;
        assert_ne!(result.code, 0);
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("Commit tree differs from review")
        );
        assert_eq!(f.git(&["rev-parse", "HEAD"]), head);
        assert!(guard.prepared_oid().unwrap().is_none());
        assert_ne!(f.git(&["write-tree"]), tree);

        fs::write(f.root.join("file"), b"reviewed\n").unwrap();
        f.git(&["add", "file"]);
        fs::write(&hook, "#!/bin/sh\nprintf pre >> .git/pre-events\n").unwrap();
        let valid = repo
            .commit_reference_guard(Some(&branch), Some(&head), Some(&head), &tree, head.len())
            .await
            .unwrap();
        let result = f.commit(&valid, &[]).await;
        assert_eq!(
            result.code,
            0,
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(f.git(&["rev-parse", "HEAD^{tree}"]), tree);
        assert_eq!(
            valid.prepared_oid().unwrap().as_deref(),
            Some(f.git(&["rev-parse", "HEAD"]).as_str())
        );
    }

    #[tokio::test]
    async fn commit_reference_fence_preserves_hooks_and_rejects_changed_heads() {
        let f = Fixture::new();
        let repo = Repository::open(&f.root).await.unwrap();
        let branch = f.git(&["symbolic-ref", "HEAD"]);
        let old = f.git(&["rev-parse", "HEAD"]);
        let tree = f.git(&["write-tree"]);
        let guard = match repo
            .commit_reference_guard(Some(&branch), Some(&old), Some(&old), &tree, old.len())
            .await
        {
            Ok(guard) => guard,
            Err(error) if error.to_string().contains("does not support") => {
                eprintln!("configured Git hooks are unavailable; guard rejected this Git version");
                return;
            }
            Err(error) => panic!("{error:#}"),
        };
        let result = f.commit(&guard, &[]).await;
        assert_eq!(
            result.code,
            0,
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            guard.prepared_oid().unwrap().as_deref(),
            Some(f.git(&["rev-parse", "HEAD"]).as_str())
        );
        assert!(f.root.join(".git/pre-events").exists());
        let events = fs::read_to_string(f.root.join(".git/ref-events")).unwrap();
        assert!(events.contains("prepared\n") && events.contains("committed\n"));

        let previous = f.git(&["rev-parse", "HEAD"]);
        let stale = repo
            .commit_reference_guard(
                Some(&branch),
                Some(&previous),
                Some(&previous),
                &tree,
                previous.len(),
            )
            .await
            .unwrap();
        f.git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "other",
        ]);
        let current = f.git(&["rev-parse", "HEAD"]);
        assert_ne!(f.commit(&stale, &[]).await.code, 0);
        assert_eq!(f.git(&["rev-parse", "HEAD"]), current);
        assert!(stale.prepared_oid().unwrap().is_none());

        let wrong_parent = repo
            .commit_reference_guard(Some(&branch), Some(&current), Some(&old), &tree, old.len())
            .await
            .unwrap();
        assert_ne!(f.commit(&wrong_parent, &[]).await.code, 0);
        assert_eq!(f.git(&["rev-parse", "HEAD"]), current);
        assert!(wrong_parent.prepared_oid().unwrap().is_none());

        let switch_guard = repo
            .commit_reference_guard(
                Some(&branch),
                Some(&current),
                Some(&current),
                &tree,
                current.len(),
            )
            .await
            .unwrap();
        f.git(&["switch", "-qc", "other-branch"]);
        assert_ne!(f.commit(&switch_guard, &[]).await.code, 0);
        assert_eq!(f.git(&["rev-parse", "HEAD"]), current);
        assert!(switch_guard.prepared_oid().unwrap().is_none());
    }

    #[tokio::test]
    async fn commit_reference_fence_supports_detached_and_amend() {
        let f = Fixture::new();
        let repo = Repository::open(&f.root).await.unwrap();
        let original = f.git(&["rev-parse", "HEAD"]);
        let tree = f.git(&["write-tree"]);
        f.git(&["checkout", "-q", "--detach"]);
        let guard = match repo
            .commit_reference_guard(
                None,
                Some(&original),
                Some(&original),
                &tree,
                original.len(),
            )
            .await
        {
            Ok(guard) => guard,
            Err(error) if error.to_string().contains("does not support") => {
                eprintln!("configured Git hooks are unavailable; guard rejected this Git version");
                return;
            }
            Err(error) => panic!("{error:#}"),
        };
        let result = f.commit(&guard, &[]).await;
        assert_eq!(
            result.code,
            0,
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let tip = f.git(&["rev-parse", "HEAD"]);
        let amend = repo
            .commit_reference_guard(None, Some(&tip), Some(&original), &tree, tip.len())
            .await
            .unwrap();
        let result = f.commit(&amend, &["--amend"]).await;
        assert_eq!(
            result.code,
            0,
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            amend.prepared_oid().unwrap().as_deref(),
            Some(f.git(&["rev-parse", "HEAD"]).as_str())
        );
        assert_eq!(f.git(&["rev-parse", "HEAD^"]), original);
    }

    #[tokio::test]
    async fn commit_reference_fence_refuses_a_head_changed_by_a_hook() {
        let f = Fixture::new();
        let repo = Repository::open(&f.root).await.unwrap();
        let branch = f.git(&["symbolic-ref", "HEAD"]);
        let old = f.git(&["rev-parse", "HEAD"]);
        let tree = f.git(&["rev-parse", "HEAD^{tree}"]);
        let other = f.git(&["commit-tree", &tree, "-p", &old, "-m", "external change"]);
        let guard = match repo
            .commit_reference_guard(Some(&branch), Some(&old), Some(&old), &tree, old.len())
            .await
        {
            Ok(guard) => guard,
            Err(error) if error.to_string().contains("does not support") => {
                eprintln!("configured Git hooks are unavailable; guard rejected this Git version");
                return;
            }
            Err(error) => panic!("{error:#}"),
        };
        let hook = f.root.join(".git/hooks/pre-commit");
        fs::write(
            &hook,
            format!("#!/bin/sh\nenv -u GIT_CONFIG_PARAMETERS git update-ref HEAD {other} {old}\n"),
        )
        .unwrap();
        fs::set_permissions(hook, fs::Permissions::from_mode(0o700)).unwrap();
        let result = f.commit(&guard, &[]).await;
        assert_ne!(result.code, 0);
        assert_eq!(f.git(&["rev-parse", "HEAD"]), other);
        assert!(guard.prepared_oid().unwrap().is_none());
    }

    #[tokio::test]
    async fn commit_reference_fence_supports_unborn_sha1_and_sha256() {
        for format in ["sha1", "sha256"] {
            let root =
                std::env::temp_dir().join(format!("desktop-unborn-guard-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            fs::write(root.join("global.config"), "").unwrap();
            let init = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["init", "-q", &format!("--object-format={format}")])
                .output()
                .unwrap();
            if !init.status.success() && format == "sha256" {
                fs::remove_dir_all(root).unwrap();
                continue;
            }
            assert!(init.status.success());
            let f = Fixture { root };
            let branch = f.git(&["symbolic-ref", "HEAD"]);
            let repo = Repository::open(&f.root).await.unwrap();
            let length = if format == "sha1" { 40 } else { 64 };
            let tree = f.git(&["write-tree"]);
            let guard = match repo
                .commit_reference_guard(Some(&branch), None, None, &tree, length)
                .await
            {
                Ok(guard) => guard,
                Err(error) if error.to_string().contains("does not support") => {
                    eprintln!(
                        "configured Git hooks are unavailable; guard rejected this Git version"
                    );
                    return;
                }
                Err(error) => panic!("{error:#}"),
            };
            let result = f.commit(&guard, &["--no-verify"]).await;
            assert_eq!(
                result.code,
                0,
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(
                guard.prepared_oid().unwrap().as_deref(),
                Some(f.git(&["rev-parse", "HEAD"]).as_str())
            );
            assert_eq!(
                f.git(&["rev-list", "--parents", "-1", "HEAD"])
                    .split_whitespace()
                    .count(),
                1
            );
        }
    }
}
