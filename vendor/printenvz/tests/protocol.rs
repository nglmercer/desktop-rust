#[cfg(unix)]
#[test]
fn executable_preserves_environment_bytes() {
    use std::os::unix::ffi::OsStringExt;
    use std::{ffi::OsString, process::Command};

    let output = Command::new(env!("CARGO_BIN_EXE_printenvz"))
        .env_clear()
        .env("EMPTY", "")
        .env("MULTILINE", "line 1\nline=2 ☃")
        .env(
            OsString::from_vec(b"N\xff".to_vec()),
            OsString::from_vec(b"V\xfe".to_vec()),
        )
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let contents = output.stdout.strip_prefix(b"--printenvz--begin\n").unwrap();
    let contents = contents.strip_suffix(b"\n--printenvz--end\n").unwrap();
    let entries: Vec<_> = contents.split(|byte| *byte == 0).collect();
    assert!(entries.contains(&&b"EMPTY="[..]));
    assert!(entries.contains(&&b"MULTILINE=line 1\nline=2 \xe2\x98\x83"[..]));
    assert!(entries.contains(&&b"N\xff=V\xfe"[..]));
    assert_eq!(entries.last(), Some(&&b""[..]));
}
