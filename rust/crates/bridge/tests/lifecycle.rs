use desktop_bridge::start;
use desktop_core::Registry;
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

#[tokio::test]
async fn browser_entry_reports_unavailable_without_exposing_the_desktop_bundle() {
    let directory =
        std::env::temp_dir().join(format!("desktop-lifecycle-{}", uuid::Uuid::new_v4()));
    let server = start(None, Registry::new(directory.join("registry.json")), 0)
        .await
        .unwrap();
    let address = server.session_url().split('/').nth(2).unwrap();
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(
            format!("GET / HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 503"));
    assert!(response.contains("browser frontend is not available"));
    assert!(response.contains("cache-control: no-store"));
    server.shutdown().await.unwrap();
    if directory.exists() {
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[tokio::test]
async fn shutdown_closes_authenticated_and_unauthenticated_sessions_and_releases_port() {
    let directory =
        std::env::temp_dir().join(format!("desktop-lifecycle-{}", uuid::Uuid::new_v4()));
    let server = start(None, Registry::new(directory.join("registry.json")), 0)
        .await
        .unwrap();
    let url = server.session_url();
    let (http_url, token) = url.split_once("/#token=").unwrap();
    let address = http_url.strip_prefix("http://").unwrap().to_owned();
    let mut request = format!("ws://{address}/ws").into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Origin", http_url.parse().unwrap());
    let (mut authenticated, _) = connect_async(request.clone()).await.unwrap();
    authenticated
        .send(Message::text(
            serde_json::json!({"token":token}).to_string(),
        ))
        .await
        .unwrap();
    let ready = authenticated
        .next()
        .await
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&ready).unwrap()["ready"],
        true
    );
    let (mut unauthenticated, _) = connect_async(request).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), server.shutdown())
        .await
        .unwrap()
        .unwrap();
    for socket in [&mut authenticated, &mut unauthenticated] {
        let closed = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap();
        assert!(!matches!(closed, Some(Ok(Message::Text(_)))));
    }
    assert!(TcpStream::connect(&address).await.is_err());
    // The listener port is immediately reusable after orderly shutdown.
    let rebound = tokio::net::TcpListener::bind(&address).await.unwrap();
    drop(rebound);
    if directory.exists() {
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[tokio::test]
async fn dropping_handle_stops_listener_without_ctrl_c() {
    let directory =
        std::env::temp_dir().join(format!("desktop-lifecycle-{}", uuid::Uuid::new_v4()));
    let server = start(None, Registry::new(directory.join("registry.json")), 0)
        .await
        .unwrap();
    let address = server
        .session_url()
        .strip_prefix("http://")
        .unwrap()
        .split('/')
        .next()
        .unwrap()
        .to_owned();
    drop(server);
    tokio::time::timeout(Duration::from_secs(2), async {
        while TcpStream::connect(&address).await.is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    if directory.exists() {
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[tokio::test]
async fn unauthenticated_connections_are_bounded_and_shutdown_drains_them() {
    let directory =
        std::env::temp_dir().join(format!("desktop-lifecycle-{}", uuid::Uuid::new_v4()));
    let server = start(None, Registry::new(directory.join("registry.json")), 0)
        .await
        .unwrap();
    let url = server.session_url().split("/#token=").next().unwrap();
    let mut request = url.replacen("http://", "ws://", 1).to_owned();
    request.push_str("/ws");
    let mut request = request.into_client_request().unwrap();
    request.headers_mut().insert("Origin", url.parse().unwrap());
    let mut sockets = Vec::new();
    for _ in 0..16 {
        sockets.push(connect_async(request.clone()).await.unwrap().0);
    }
    match connect_async(request).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status(), 429)
        }
        _ => panic!("unbounded unauthenticated connections"),
    }
    tokio::time::timeout(Duration::from_secs(2), server.shutdown())
        .await
        .unwrap()
        .unwrap();
    drop(sockets);
    if directory.exists() {
        std::fs::remove_dir_all(directory).unwrap();
    }
}
