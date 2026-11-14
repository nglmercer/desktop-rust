#![cfg(feature = "packaged-ui")]

use std::fs;
use std::process::Command;

#[test]
fn successful_non_window_commands_acknowledge_package_supervisor() {
    for argument in ["--version", "--help", "--self-test", "--no-window"] {
        let root =
            std::env::temp_dir().join(format!("desktop-package-early-{}", uuid::Uuid::new_v4()));
        let control = root.join("control");
        fs::create_dir_all(&control).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
        }

        let output = Command::new(env!("CARGO_BIN_EXE_github-desktop-rs"))
            .arg(argument)
            .env("DESKTOP_PACKAGE_QUIT_FILE", control.join("quit"))
            .env("DESKTOP_USER_DATA", root.join("user-data"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{argument}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(control.join("result")).unwrap(),
            "drained",
            "{argument}"
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn early_exit_refuses_to_replace_existing_acknowledgement() {
    let root = std::env::temp_dir().join(format!("desktop-package-early-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(root.join("result"), "preserve").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_github-desktop-rs"))
        .arg("--version")
        .env("DESKTOP_PACKAGE_QUIT_FILE", root.join("quit"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(root.join("result")).unwrap(), "preserve");
    fs::remove_dir_all(root).unwrap();
}
