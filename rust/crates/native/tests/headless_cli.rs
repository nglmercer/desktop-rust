//! The feature-free native binary explains how to run on headless hosts.
#![cfg(not(feature = "native"))]

use std::process::Command;

#[test]
fn headless_binary_prints_help_and_guides_users_to_the_bridge() {
    let binary = env!("CARGO_BIN_EXE_desktop-native");
    let help = Command::new(binary).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--selected"));
    let unsupported = Command::new(binary).arg("--empty").output().unwrap();
    assert!(!unsupported.status.success());
    assert!(String::from_utf8_lossy(&unsupported.stderr).contains("desktop-rs serve --empty"));
}
