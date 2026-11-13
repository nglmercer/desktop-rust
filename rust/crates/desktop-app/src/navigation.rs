//! Keep the privileged webview on the application document.
use url::Url;

fn app_origin(url: &Url) -> bool {
    let scheme = url.scheme();
    let host = url.host_str();
    url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && if cfg!(any(target_os = "windows", target_os = "android")) {
            scheme == "http" && host == Some("desktop.localhost")
        } else {
            scheme == "desktop" && host == Some("localhost")
        }
}

pub fn internal(value: &str) -> bool {
    Url::parse(value)
        .is_ok_and(|url| app_origin(&url) && url.path() == "/index.html" && url.query().is_none())
}

pub fn external(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && !app_origin(&url)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_application_document_is_privileged() {
        let app = if cfg!(any(target_os = "windows", target_os = "android")) {
            "http://desktop.localhost/index.html#test"
        } else {
            "desktop://localhost/index.html#test"
        };
        assert!(internal(app));
        for url in [
            "desktop://attacker/index.html",
            "desktop://localhost/other",
            "desktop://localhost/index.html?x=1",
            "http://desktop.attacker.invalid/index.html",
            "http://desktop.localhost.attacker.invalid/index.html",
            "http://desktop.localhost@attacker.invalid/index.html",
            "http://desktop.localhost:123/index.html",
            "about:blank",
            "file:///etc/passwd",
        ] {
            assert!(!internal(url), "{url}");
        }
        assert!(external("https://github.com/desktop/desktop"));
        assert!(!external("https://user:secret@github.com/"));
        assert!(!external("javascript:alert(1)"));
        if cfg!(any(target_os = "windows", target_os = "android")) {
            assert!(!external("http://desktop.localhost/index.html"));
        } else {
            assert!(!internal("http://desktop.localhost/index.html"));
        }
    }
}
