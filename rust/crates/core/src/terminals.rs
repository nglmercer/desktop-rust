//! Launch known graphical terminals at a repository root without a shell command.
use crate::{Registry, Repository, cancellation};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

#[derive(Clone, Copy)]
struct Terminal {
    name: &'static str,
    program: &'static str,
    args: &'static [&'static str],
    use_cwd: bool,
}

// Paths and arguments follow the existing Linux integrations. Never accept an
// executable or argument vector from the browser or webview.
#[cfg(target_os = "linux")]
const TERMINALS: &[Terminal] = &[
    Terminal {
        name: "GNOME Terminal",
        program: "/usr/bin/gnome-terminal",
        args: &["--working-directory"],
        use_cwd: false,
    },
    Terminal {
        name: "GNOME Console",
        program: "/usr/bin/kgx",
        args: &["--working-directory"],
        use_cwd: false,
    },
    Terminal {
        name: "Ptyxis",
        program: "/usr/bin/ptyxis",
        args: &["--new-window", "--working-directory"],
        use_cwd: false,
    },
    Terminal {
        name: "MATE Terminal",
        program: "/usr/bin/mate-terminal",
        args: &["--working-directory"],
        use_cwd: false,
    },
    Terminal {
        name: "Tilix",
        program: "/usr/bin/tilix",
        args: &["--working-directory"],
        use_cwd: false,
    },
    Terminal {
        name: "Terminator",
        program: "/usr/bin/terminator",
        args: &["--working-directory"],
        use_cwd: false,
    },
    Terminal {
        name: "URxvt",
        program: "/usr/bin/urxvt",
        args: &["-cd"],
        use_cwd: false,
    },
    Terminal {
        name: "Konsole",
        program: "/usr/bin/konsole",
        args: &["--workdir"],
        use_cwd: false,
    },
    Terminal {
        name: "XTerm",
        program: "/usr/bin/xterm",
        args: &["-e", "/bin/bash"],
        use_cwd: true,
    },
    Terminal {
        name: "Terminology",
        program: "/usr/bin/terminology",
        args: &["-d"],
        use_cwd: false,
    },
    Terminal {
        name: "Deepin Terminal",
        program: "/usr/bin/deepin-terminal",
        args: &["-w"],
        use_cwd: false,
    },
    Terminal {
        name: "Elementary Terminal",
        program: "/usr/bin/io.elementary.terminal",
        args: &["-w"],
        use_cwd: false,
    },
    Terminal {
        name: "XFCE Terminal",
        program: "/usr/bin/xfce4-terminal",
        args: &["--working-directory"],
        use_cwd: false,
    },
    Terminal {
        name: "Alacritty",
        program: "/usr/bin/alacritty",
        args: &["--working-directory"],
        use_cwd: false,
    },
    Terminal {
        name: "Kitty",
        program: "/usr/bin/kitty",
        args: &["--single-instance", "--directory"],
        use_cwd: false,
    },
    Terminal {
        name: "LXDE Terminal",
        program: "/usr/bin/lxterminal",
        args: &["--working-directory="],
        use_cwd: false,
    },
    Terminal {
        name: "Warp",
        program: "/usr/bin/warp-terminal",
        args: &[],
        use_cwd: true,
    },
    Terminal {
        name: "Ghostty",
        program: "/usr/bin/ghostty",
        args: &["--working-directory="],
        use_cwd: false,
    },
];
#[cfg(not(target_os = "linux"))]
const TERMINALS: &[Terminal] = &[];

fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// List only known terminals installed on the host.
pub async fn available_terminals() -> Result<Value> {
    cancellation::blocking(|| {
        let terminals: Vec<_> = TERMINALS
            .iter()
            .filter(|item| executable(Path::new(item.program)))
            .map(|item| json!({"name": item.name, "path": item.program}))
            .collect();
        Ok(json!({"terminals": terminals}))
    })
    .await
}

/// Launch only from a repository already saved in the native registry.
pub async fn open_registered_terminal(
    registry: Registry,
    repository_path: PathBuf,
    name: String,
    directory: PathBuf,
) -> Result<Value> {
    ensure!(
        repository_path.is_absolute(),
        "repository path must be absolute"
    );
    let canonical = cancellation::blocking(move || {
        repository_path
            .canonicalize()
            .context("resolve repository root")
    })
    .await?;
    let root = canonical.clone();
    let saved = cancellation::blocking(move || registry.record_for_path(&root)).await?;
    ensure!(
        saved.is_some(),
        "Repository is not saved in the native registry; add it before opening a terminal"
    );
    let repository = Repository::open(&canonical).await?;
    ensure!(
        repository.path() == canonical,
        "select a repository root, not a nested path"
    );
    repository.open_terminal(name, directory).await
}

impl Repository {
    /// Open a known terminal in an existing directory within the repository.
    pub async fn open_terminal(&self, name: String, directory: PathBuf) -> Result<Value> {
        ensure!(
            !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control),
            "invalid terminal name"
        );
        let terminal = TERMINALS
            .iter()
            .copied()
            .find(|item| item.name == name)
            .context("Unknown terminal")?;
        ensure!(
            executable(Path::new(terminal.program)),
            "Selected terminal was not found"
        );
        let root = self.path().to_owned();
        let directory =
            cancellation::blocking(move || confined_directory(&root, &directory)).await?;
        launch_terminal(terminal, directory).await
    }
}

fn confined_directory(root: &Path, directory: &Path) -> Result<PathBuf> {
    let root = root.canonicalize().context("resolve repository root")?;
    ensure!(
        directory.is_absolute(),
        "terminal directory must be absolute"
    );
    let directory = directory
        .canonicalize()
        .context("resolve terminal directory")?;
    ensure!(
        directory.starts_with(&root) && directory.is_dir(),
        "terminal directory must be within the selected repository"
    );
    Ok(directory)
}

async fn launch_terminal(terminal: Terminal, directory: PathBuf) -> Result<Value> {
    let mut command = tokio::process::Command::new(terminal.program);
    command
        .current_dir(&directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if terminal.use_cwd {
        command.args(terminal.args);
    } else if terminal.args.last() == Some(&"--working-directory=") {
        command.args(&terminal.args[..terminal.args.len() - 1]);
        let mut option = std::ffi::OsString::from("--working-directory=");
        option.push(&directory);
        command.arg(option);
    } else {
        command.args(terminal.args).arg(&directory);
    }
    #[cfg(unix)]
    command.process_group(0);
    cancellation::Cancellation::current().check()?;
    let mut child = command.spawn().context("launch configured terminal")?;
    match tokio::time::timeout(std::time::Duration::from_millis(200), child.wait()).await {
        Ok(Ok(status)) if !status.success() => {
            bail!("Terminal exited unsuccessfully. Check its installation and settings.")
        }
        Ok(Err(error)) => return Err(error).context("wait for terminal launch"),
        Ok(Ok(_)) => return Ok(json!({"launched": true})),
        Err(_) => {}
    }
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(json!({"launched": true}))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn terminal_directory_must_be_a_directory_inside_the_repository() {
        use std::os::unix::fs::symlink;
        let root =
            std::env::temp_dir().join(format!("desktop-terminal-path-{}", uuid::Uuid::new_v4()));
        let repository = root.join("repository");
        let nested = repository.join("nested");
        let outside = root.join("repository-other");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(repository.join("file"), "x").unwrap();
        symlink(&outside, repository.join("link-out")).unwrap();
        assert_eq!(
            confined_directory(&repository, &nested).unwrap(),
            nested.canonicalize().unwrap()
        );
        assert!(confined_directory(&repository, Path::new("nested")).is_err());
        assert!(confined_directory(&repository, &outside).is_err());
        assert!(confined_directory(&repository, &repository.join("link-out")).is_err());
        assert!(confined_directory(&repository, &repository.join("file")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn terminal_receives_literal_repository_path_and_runs_at_its_root() {
        let root =
            std::env::temp_dir().join(format!("desktop-terminal-test-{}", uuid::Uuid::new_v4()));
        let repository = root.join("repo with spaces ; dollar $()");
        std::fs::create_dir_all(&repository).unwrap();
        let script = root.join("terminal");
        let output = root.join("result");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$@\" > '{}'\n",
                output.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let program: &'static str =
            Box::leak(script.to_string_lossy().into_owned().into_boxed_str());
        let terminal = Terminal {
            name: "fixture",
            program,
            args: &["--working-directory"],
            use_cwd: false,
        };
        launch_terminal(terminal, repository.clone()).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(output).unwrap(),
            format!(
                "{}\n--working-directory\n{}\n",
                repository.display(),
                repository.display()
            )
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn unknown_terminal_names_cannot_launch_a_program() {
        let root =
            std::env::temp_dir().join(format!("desktop-terminal-repo-{}", uuid::Uuid::new_v4()));
        let result = std::process::Command::new("git")
            .args(["-c", "init.templateDir=", "init", "-q"])
            .arg(&root)
            .status()
            .unwrap();
        assert!(result.success());
        let repository = Repository::open(&root).await.unwrap();
        assert!(
            repository
                .open_terminal("/bin/sh".into(), root.clone())
                .await
                .is_err()
        );
        assert!(
            repository
                .open_terminal("invalid\nname".into(), root.clone())
                .await
                .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn explicit_terminal_requests_require_a_saved_repository() {
        let root = std::env::temp_dir().join(format!(
            "desktop-terminal-registry-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&root).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["-c", "init.templateDir=", "init", "-q"])
                .arg(&repository)
                .status()
                .unwrap()
                .success()
        );
        let registry = Registry::new(root.join("registry.json"));
        let error = open_registered_terminal(
            registry.clone(),
            repository.clone(),
            "/bin/sh".into(),
            repository.clone(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("not saved"));
        registry
            .add(repository.clone().canonicalize().unwrap(), false)
            .unwrap();
        let error = open_registered_terminal(
            registry.clone(),
            repository.clone(),
            "/bin/sh".into(),
            repository,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Unknown terminal"));
        assert!(registry.read().unwrap().selected.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
