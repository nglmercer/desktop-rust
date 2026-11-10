//! Port of the URL helpers at the bottom of `app/src/lib/api.ts`
//! (`getDotComAPIEndpoint`, `getEnterpriseAPIURL`, `getHTMLURL`, …) and
//! `app/src/lib/endpoint-capabilities.ts`.

pub const DOTCOM_API_ENDPOINT: &str = "https://api.github.com";
pub const DOTCOM_HTML_URL: &str = "https://github.com";
pub const USER_AGENT: &str = "GitHubDesktop-Rust/0.1";

pub fn get_dot_com_api_endpoint() -> String {
    std::env::var("DESKTOP_GITHUB_DOTCOM_API_ENDPOINT")
        .unwrap_or_else(|_| DOTCOM_API_ENDPOINT.to_string())
}

/// GHES uses `https://<host>/api/v3`; GHE.com uses `https://api.<host>/`.
pub fn get_enterprise_api_url(host: &str) -> String {
    if host.ends_with(".ghe.com") {
        format!("https://api.{host}")
    } else {
        format!("https://{host}/api/v3")
    }
}

pub fn get_html_url(endpoint: &str) -> String {
    if endpoint == get_dot_com_api_endpoint() || endpoint == DOTCOM_API_ENDPOINT {
        return DOTCOM_HTML_URL.to_string();
    }
    endpoint
        .trim_end_matches('/')
        .trim_start_matches("https://")
        .trim_start_matches("api.")
        .trim_end_matches("/api/v3")
        .pipe(|host| format!("https://{host}"))
}

pub fn get_endpoint_for_repository(url: &str) -> String {
    if is_github_dot_com(url) {
        get_dot_com_api_endpoint()
    } else {
        let host = url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_start_matches("git@")
            .split(['/', ':'])
            .next()
            .unwrap_or("");
        get_enterprise_api_url(host)
    }
}

pub fn is_github_dot_com(url: &str) -> bool {
    url.contains("github.com") && !url.contains("api.github.com")
}

pub fn is_ghe(host: &str) -> bool {
    host.ends_with(".ghe.com")
}
pub fn is_ghes(host: &str) -> bool {
    !is_ghe(host) && !host.contains("github.com")
}

pub fn get_oauth_authorization_url(endpoint: &str, state: &str, login: Option<&str>) -> String {
    let html = get_html_url(endpoint);
    let client_id = std::env::var("DESKTOP_OAUTH_CLIENT_ID")
        .unwrap_or_else(|_| "de0e3c7e9973e1c4dd77".to_string());
    let mut url = format!(
        "{html}/login/oauth/authorize?client_id={client_id}&scope=repo%20user:email&state={state}"
    );
    if let Some(login) = login {
        url.push_str(&format!("&login={login}"));
    }
    url
}

/// HTTP(S) absolute URL for an API path.
pub fn get_absolute_url(endpoint: &str, path: &str) -> String {
    let base = endpoint.trim_end_matches('/');
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    format!("{base}/{}", path.trim_start_matches('/'))
}

/// Parse a `Link:` header value into the next page URL.
pub fn get_next_page_path_from_link(link: &str) -> Option<String> {
    link.split(',').find_map(|part| {
        if !part.contains("rel=\"next\"") {
            return None;
        }
        part.split(';').next().and_then(|url| {
            let url = url.trim().trim_start_matches('<').trim_end_matches('>');
            (!url.is_empty()).then(|| url.to_string())
        })
    })
}

trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}
impl<T> Pipe for T {}
