//! Minimal WebDAV-style file server for tests: `MKCOL`, `PROPFIND` (Depth 1, `getetag`
//! only), `GET` and conditional `PUT` (`If-Match` / `If-None-Match: *`, strong ETags =
//! truncated SHA-256 of the content), optional HTTP Basic credentials. It stores exactly the
//! bytes it receives under `<dir>/<path>`, so tests can scan its storage for plaintext. It is
//! not a production server; real deployments point `sync.webdav_url` at any WebDAV share.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::backend::{etag, write_atomic};

/// Largest accepted resource body.
pub const MAX_BLOB: usize = 1024 * 1024;

struct Server {
    root: PathBuf,
    credentials: Option<String>,
    // Serializes check-then-write so conditional PUTs are atomic.
    writer: tokio::sync::Mutex<()>,
}

fn resolve(root: &Path, uri: &Uri) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    for segment in uri.path().split('/').filter(|s| !s.is_empty()) {
        let valid = !segment.starts_with('.')
            && segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if !valid {
            return None;
        }
        path.push(segment);
    }
    Some(path)
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn unquote(tag: &str) -> &str {
    tag.trim().trim_matches('"')
}

async fn handle(
    State(server): State<Arc<Server>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(expected) = &server.credentials {
        if header(&headers, "authorization") != Some(expected.as_str()) {
            return (
                StatusCode::UNAUTHORIZED,
                [("WWW-Authenticate", "Basic realm=\"openside-sync\"")],
            )
                .into_response();
        }
    }
    let Some(path) = resolve(&server.root, &uri) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match method.as_str() {
        "MKCOL" => {
            if path.exists() {
                return StatusCode::METHOD_NOT_ALLOWED.into_response();
            }
            match std::fs::create_dir_all(&path) {
                Ok(()) => StatusCode::CREATED.into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        "PROPFIND" => propfind(&path, uri.path()),
        "GET" => match std::fs::read(&path) {
            Ok(bytes) if path.is_file() => {
                let tag = format!("\"{}\"", etag(&bytes));
                (StatusCode::OK, [("ETag", tag)], bytes).into_response()
            }
            _ => StatusCode::NOT_FOUND.into_response(),
        },
        "PUT" => {
            if body.len() > MAX_BLOB {
                return StatusCode::PAYLOAD_TOO_LARGE.into_response();
            }
            if !path.parent().is_some_and(Path::is_dir) {
                return StatusCode::CONFLICT.into_response();
            }
            let _writer = server.writer.lock().await;
            let current = std::fs::read(&path).ok().map(|bytes| etag(&bytes));
            let allowed = match (
                header(&headers, "if-match"),
                header(&headers, "if-none-match"),
            ) {
                (Some(tag), _) => current.as_deref() == Some(unquote(tag)),
                (None, Some("*")) => current.is_none(),
                (None, _) => true,
            };
            if !allowed {
                return StatusCode::PRECONDITION_FAILED.into_response();
            }
            if write_atomic(&path, &body).is_err() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            let status = if current.is_some() {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::CREATED
            };
            (status, [("ETag", format!("\"{}\"", etag(&body)))]).into_response()
        }
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

fn propfind(path: &Path, href: &str) -> Response {
    let Ok(entries) = std::fs::read_dir(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let base = href.trim_end_matches('/');
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<d:multistatus xmlns:d=\"DAV:\">\
         <d:response><d:href>{base}/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/>\
         </d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
    );
    for entry in entries.flatten() {
        let file = entry.path();
        let (Some(name), Ok(bytes)) = (
            file.file_name().and_then(|n| n.to_str()).map(str::to_owned),
            std::fs::read(&file),
        ) else {
            continue;
        };
        xml.push_str(&format!(
            "<d:response><d:href>{base}/{name}</d:href><d:propstat><d:prop>\
             <d:getetag>&quot;{}&quot;</d:getetag></d:prop>\
             <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>",
            etag(&bytes)
        ));
    }
    xml.push_str("</d:multistatus>");
    (
        StatusCode::MULTI_STATUS,
        [("Content-Type", "application/xml; charset=utf-8")],
        xml,
    )
        .into_response()
}

/// Router serving `dir`; `credentials` = `(username, password)` enables HTTP Basic.
pub fn router(dir: PathBuf, credentials: Option<(&str, &str)>) -> Router {
    let credentials = credentials
        .map(|(user, password)| format!("Basic {}", STANDARD.encode(format!("{user}:{password}"))));
    Router::new()
        .fallback(handle)
        .layer(DefaultBodyLimit::max(MAX_BLOB + 1))
        .with_state(Arc::new(Server {
            root: dir,
            credentials,
            writer: tokio::sync::Mutex::new(()),
        }))
}

/// Serve `dir` on `addr` in the current runtime; returns the bound address and the task.
pub async fn spawn(
    addr: SocketAddr,
    dir: PathBuf,
    credentials: Option<(&str, &str)>,
) -> std::io::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    std::fs::create_dir_all(&dir)?;
    let app = router(dir, credentials);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((bound, handle))
}
