//! Public release notes fetched by the browser bridge from a fixed origin.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::OnceLock, time::Duration};
use tokio::sync::Semaphore;

const CHANGELOG_URL: &str = "https://central.github.com/deployments/desktop/desktop/changelog.json";
const MAX_RESPONSE: usize = 2 * 1024 * 1024;
static REQUESTS: Semaphore = Semaphore::const_new(4);

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Channel {
    Beta,
    Test,
}

#[derive(Deserialize, Serialize)]
struct Release {
    name: String,
    notes: Vec<String>,
    pub_date: String,
    version: String,
}

fn build_client(builder: reqwest::ClientBuilder) -> Result<reqwest::Client> {
    Ok(builder
        // A redirect must not turn this fixed request into an arbitrary host request.
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent("GitHubDesktop-Rust")
        .build()?)
}

fn client() -> Result<&'static reqwest::Client> {
    static CLIENT: OnceLock<Result<reqwest::Client, ()>> = OnceLock::new();
    CLIENT
        .get_or_init(|| build_client(reqwest::Client::builder()).map_err(|_| ()))
        .as_ref()
        .map_err(|_| anyhow::anyhow!("Unable to initialize release notes client"))
}

fn changelog_url(channel: Option<Channel>, limit: Option<u16>) -> Result<reqwest::Url> {
    ensure!(
        limit.is_none_or(|limit| limit <= 250),
        "Invalid changelog limit"
    );
    let mut url = reqwest::Url::parse(CHANGELOG_URL).expect("static URL is valid");
    if channel.is_none() && limit.is_none() {
        return Ok(url);
    }
    {
        let mut query = url.query_pairs_mut();
        if let Some(channel) = channel {
            query.append_pair(
                "env",
                match channel {
                    Channel::Beta => "beta",
                    Channel::Test => "test",
                },
            );
        }
        if let Some(limit) = limit {
            query.append_pair("limit", &limit.to_string());
        }
    }
    Ok(url)
}

async fn fetch_from(client: &reqwest::Client, url: reqwest::Url) -> Result<Value> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Unable to download release notes"))?;
    // Preserve the renderer's existing behavior for unsuccessful responses;
    // a redirect is unsuccessful here because redirects are disabled.
    if !response.status().is_success() {
        return Ok(json!([]));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE as u64)
    {
        bail!("Release notes exceed the supported response size");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("Unable to read release notes"))?
    {
        ensure!(
            chunk.len() <= MAX_RESPONSE - bytes.len(),
            "Release notes exceed the supported response size"
        );
        bytes.extend_from_slice(&chunk);
    }
    let releases: Vec<Release> = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Invalid release notes response"))?;
    Ok(serde_json::to_value(releases)?)
}

pub(crate) async fn fetch(channel: Option<Channel>, limit: Option<u16>) -> Result<Value> {
    let url = changelog_url(channel, limit)?;
    let _permit = REQUESTS
        .try_acquire()
        .context("Too many release note requests")?;
    desktop_core::cancellation::wait(fetch_from(client()?, url)).await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn test_client() -> reqwest::Client {
        build_client(reqwest::Client::builder().no_proxy()).unwrap()
    }

    async fn server(response: Vec<u8>) -> (reqwest::Url, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let count = stream.read(&mut request).await.unwrap();
            let _ = stream.write_all(&response).await;
            String::from_utf8_lossy(&request[..count]).into_owned()
        });
        (
            reqwest::Url::parse(&format!("http://{address}/changelog")).unwrap(),
            task,
        )
    }

    fn response(status: &str, body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    #[test]
    fn constructs_only_the_fixed_endpoint_with_supported_options() {
        assert_eq!(changelog_url(None, None).unwrap().as_str(), CHANGELOG_URL);
        let url = changelog_url(Some(Channel::Beta), Some(250)).unwrap();
        assert_eq!(url.as_str(), format!("{CHANGELOG_URL}?env=beta&limit=250"));
        assert_eq!(
            changelog_url(Some(Channel::Test), None).unwrap().query(),
            Some("env=test")
        );
        assert!(changelog_url(None, Some(251)).is_err());
    }

    #[tokio::test]
    async fn reads_valid_releases_without_following_redirects() {
        let body = br#"[{"name":"Release","notes":["[Fixed] Example"],"pub_date":"2025-01-01","version":"3.0.0"}]"#;
        let (url, task) = server(response("200 OK", body)).await;
        let value = fetch_from(&test_client(), url).await.unwrap();
        assert_eq!(value[0]["notes"][0], "[Fixed] Example");
        assert!(task.await.unwrap().starts_with("GET /changelog HTTP/1.1"));
        let (url, task) = server(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()).await;
        assert_eq!(fetch_from(&test_client(), url).await.unwrap(), json!([]));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_invalid_json_and_large_responses() {
        let (url, task) = server(response("200 OK", br#"[{"name":null}]"#)).await;
        assert!(fetch_from(&test_client(), url).await.is_err());
        task.await.unwrap();
        let (url, task) = server(response("200 OK", &vec![b'x'; MAX_RESPONSE + 1])).await;
        assert!(
            fetch_from(&test_client(), url)
                .await
                .unwrap_err()
                .to_string()
                .contains("response size")
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_oversized_chunked_responses_without_a_length_header() {
        let mut body =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
        body.extend_from_slice(format!("{:x}\r\n", MAX_RESPONSE + 1).as_bytes());
        body.extend(std::iter::repeat_n(b'x', MAX_RESPONSE + 1));
        body.extend_from_slice(b"\r\n0\r\n\r\n");
        let (url, task) = server(body).await;
        assert!(
            fetch_from(&test_client(), url)
                .await
                .unwrap_err()
                .to_string()
                .contains("response size")
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn cancels_a_pending_http_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = reqwest::Url::parse(&format!(
            "http://{}/changelog",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let (ready, received) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut data = [0_u8; 4096];
            let _ = stream.read(&mut data).await;
            let _ = ready.send(());
            tokio::time::sleep(Duration::from_secs(2)).await;
        });
        let token = desktop_core::cancellation::Cancellation::default();
        let scoped = token.clone();
        let task = tokio::spawn(async move {
            let client = test_client();
            scoped
                .scope(desktop_core::cancellation::wait(fetch_from(&client, url)))
                .await
        });
        received.await.unwrap();
        token.cancel();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        server.abort();
    }
}
