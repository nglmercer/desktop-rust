//! Exercise CLI signal handling against an actual authenticated bridge request.
#![cfg(unix)]

use futures_util::{SinkExt, StreamExt};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::TcpStream,
    process::Command,
    time::{Instant, sleep, timeout},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

struct Fixture {
    root: PathBuf,
    child_pid: Option<u32>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let pid = self.child_pid.or_else(|| {
            fs::read_to_string(self.root.join("gh.pid"))
                .ok()
                .and_then(|value| value.parse().ok())
        });
        if let Some(pid) = pid.filter(|pid| alive(*pid)) {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn alive(pid: u32) -> bool {
    // Linux can leave a terminated orphan as a zombie until its parent reaps it.
    if let Ok(status) = fs::read_to_string(format!("/proc/{pid}/status"))
        && status
            .lines()
            .any(|line| line.starts_with("State:") && line.contains('Z'))
    {
        return false;
    }
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

async fn wait_for(label: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "{label}");
        sleep(Duration::from_millis(20)).await;
    }
}

fn fake_gh(root: &Path) -> PathBuf {
    let directory = root.join("bin");
    fs::create_dir_all(&directory).unwrap();
    let program = directory.join("gh");
    fs::write(
        &program,
        "#!/bin/sh\nprintf '%s' \"$$\" > \"$LIFECYCLE_PID\"\nexec sleep 60\n",
    )
    .unwrap();
    fs::set_permissions(program, fs::Permissions::from_mode(0o700)).unwrap();
    directory
}

#[tokio::test]
async fn sigint_stops_inflight_gh_and_closes_sessions_and_listener() {
    let root = std::env::temp_dir().join(format!("desktop-cli-shutdown-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let mut fixture = Fixture {
        root,
        child_pid: None,
    };
    let directory = fake_gh(&fixture.root);
    let pid_file = fixture.root.join("gh.pid");
    let path = std::env::join_paths(std::iter::once(directory).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let mut server = Command::new(env!("CARGO_BIN_EXE_desktop-rs"))
        .args([
            "--config",
            fixture.root.join("registry.json").to_str().unwrap(),
            "serve",
            "--empty",
            "--port",
            "0",
        ])
        .env("PATH", path)
        .env("LIFECYCLE_PID", &pid_file)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", fixture.root.join("gitconfig"))
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(server.stderr.take().unwrap()).lines();
    let line = timeout(Duration::from_secs(10), stderr.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let url = line
        .strip_prefix("Open ")
        .expect("CLI did not announce a session URL");
    let (origin, token) = url.split_once("/#token=").expect("invalid session URL");
    let address = origin.strip_prefix("http://").unwrap();
    let mut request = format!("ws://{address}/ws").into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Origin", origin.parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    socket
        .send(Message::text(
            serde_json::json!({"token":token}).to_string(),
        ))
        .await
        .unwrap();
    let ready = timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&ready).unwrap()["ready"],
        true
    );
    socket.send(Message::text(serde_json::json!({"id":1,"method":"github","request":{"action":{"type":"auth_status","hostname":"github.com"}}}).to_string())).await.unwrap();
    wait_for("gh request did not start", || {
        fs::read_to_string(&pid_file)
            .ok()
            .and_then(|text| text.parse::<u32>().ok())
            .is_some()
    })
    .await;
    let pid: u32 = fs::read_to_string(&pid_file).unwrap().parse().unwrap();
    fixture.child_pid = Some(pid);
    assert!(alive(pid));
    let sent = std::process::Command::new("kill")
        .args(["-INT", &server.id().unwrap().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    let status = timeout(Duration::from_secs(10), server.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success(), "server exited with {status}");
    wait_for("server left gh running", || !alive(pid)).await;
    fs::remove_file(&pid_file).unwrap();
    fixture.child_pid = None;
    assert!(TcpStream::connect(address).await.is_err());
    let closed = timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap();
    assert!(!matches!(closed, Some(Ok(Message::Text(_)))));
}
