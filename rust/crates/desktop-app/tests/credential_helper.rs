//! The GUI executable is also the Git credential-helper entry point.
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[test]
fn credential_helper_dispatches_without_starting_the_gui() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_github-desktop-rs"))
        .args(["__credential-helper", "invalid"])
        .env_remove("DESKTOP_RS_GIT_BROKER")
        .env_remove("DESKTOP_RS_GIT_CAPABILITY")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start helper");
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll helper") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("credential helper did not exit without a GUI");
        }
        thread::sleep(Duration::from_millis(25));
    };
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(status.success(), "helper failed: {stderr}");
    assert_eq!(stdout, "quit=true\n\n");
    assert!(stderr.contains("Assigned Git credentials refused"));
}

#[test]
fn credential_helper_relays_an_authenticated_broker_response() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let broker = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) => panic!("helper did not contact broker: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut length = [0u8; 4];
        stream.read_exact(&mut length).unwrap();
        let mut payload = vec![0; u32::from_be_bytes(length) as usize];
        stream.read_exact(&mut payload).unwrap();
        let request: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(request["capability"], "test-capability");
        assert_eq!(request["operation"], "get");
        let response =
            br#"{"output":"dXNlcm5hbWU9dGVzdApwYXNzd29yZD1zeW50aGV0aWMKCg==","error":null}"#;
        stream
            .write_all(&(response.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(response).unwrap();
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_github-desktop-rs"))
        .args(["__credential-helper", "get"])
        .env("DESKTOP_RS_GIT_BROKER", address.to_string())
        .env("DESKTOP_RS_GIT_CAPABILITY", "test-capability")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"protocol=https\nhost=example.invalid\n\n")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("credential helper did not finish");
        }
        thread::sleep(Duration::from_millis(25));
    }
    let output = child.wait_with_output().unwrap();
    broker.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"username=test\npassword=synthetic\n\n");
}
