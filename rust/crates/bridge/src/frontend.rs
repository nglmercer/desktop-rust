//! Immutable renderer assets for the opt-in loopback frontend.
use axum::{
    Json,
    body::Body,
    extract::{Path, Query},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use include_dir::{Dir, include_dir};
use serde::Deserialize;
use serde_json::{Value, json};

static ASSETS: Dir = include_dir!("$DESKTOP_EMBEDDED_UI_DIR");

fn valid_path(path: &str) -> bool {
    !path.starts_with('/')
        && !path.contains(['\\', ':'])
        && !path.chars().any(char::is_control)
        && path.split('/').all(|part| part != "." && part != "..")
}

fn mime(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("md" | "txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn file(path: &str) -> Response {
    if !valid_path(path) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match ASSETS.get_file(path) {
        Some(file) => (
            [
                (header::CONTENT_TYPE, mime(path)),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            Body::from(file.contents()),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn index() -> Response {
    file("index.html")
}

pub async fn asset(Path(path): Path<String>) -> Response {
    file(&path)
}

#[derive(Deserialize)]
pub struct BundleQuery {
    #[serde(default)]
    path: String,
}

pub async fn entry(Query(query): Query<BundleQuery>) -> Json<Value> {
    let path = query.path.trim_matches('/');
    if !valid_path(path) {
        return Json(Value::Null);
    }
    if let Some(file) = ASSETS.get_file(path) {
        return Json(json!({"kind":"file", "size":file.contents().len()}));
    }
    let directory = if path.is_empty() {
        Some(&ASSETS)
    } else {
        ASSETS.get_dir(path)
    };
    let Some(directory) = directory else {
        return Json(Value::Null);
    };
    let mut entries: Vec<Value> = directory.dirs().map(|entry| json!({"name": entry.path().file_name().unwrap().to_string_lossy(), "isDirectory":true}))
        .chain(directory.files().map(|entry| json!({"name": entry.path().file_name().unwrap().to_string_lossy(), "isDirectory":false})))
        .collect();
    entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Json(json!({"kind":"directory","entries":entries}))
}
