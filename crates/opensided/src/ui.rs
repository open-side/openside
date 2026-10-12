//! Optional static web UI (`[ui] dir`) served beside the API with an SPA fallback.

use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use axum::{
    extract::Request,
    http::{header, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    Router,
};

use crate::api::HttpError;

// The bundle loads only same-origin scripts/styles and talks only to this daemon;
// nothing inline is allowed, so injected markup cannot execute.
const CSP: &str =
    "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; \
connect-src 'self'; manifest-src 'self'; worker-src 'self'; font-src 'self'; object-src 'none'; \
base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

struct Root(PathBuf);

enum Resolved {
    File(PathBuf),
    /// Nothing exists at the path, so client-side routes may fall back to the shell.
    Missing,
    /// Something exists but is not a file inside the bundle (escaping symlink, special file).
    Denied,
}

/// Canonicalize `candidate` and require the target to stay inside `root`.
async fn contained(root: &Path, candidate: &Path) -> Result<PathBuf, Resolved> {
    match tokio::fs::canonicalize(candidate).await {
        Ok(path) if path.starts_with(root) => Ok(path),
        Ok(_) => Err(Resolved::Denied),
        // A dangling symlink is still an entry; only a truly absent path is "missing".
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match tokio::fs::symlink_metadata(candidate).await {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Err(Resolved::Missing)
                }
                _ => Err(Resolved::Denied),
            }
        }
        Err(_) => Err(Resolved::Denied),
    }
}

/// Resolve `candidate` to a regular file inside `root`, following symlinks only when
/// their target stays inside; a directory resolves to its contained `index.html`.
async fn resolve(root: &Path, candidate: &Path) -> Resolved {
    let mut path = match contained(root, candidate).await {
        Ok(path) => path,
        Err(resolved) => return resolved,
    };
    if path.is_dir() {
        path = match contained(root, &path.join("index.html")).await {
            Ok(index) => index,
            Err(resolved) => return resolved,
        };
    }
    if path.is_file() {
        Resolved::File(path)
    } else {
        Resolved::Denied
    }
}

/// Add the UI fallback when a directory containing `index.html` is configured.
pub(crate) fn attach(router: Router, dir: Option<&Path>) -> Router {
    let Some(dir) = dir else { return router };
    let root = std::fs::canonicalize(dir).ok().filter(|root| {
        std::fs::canonicalize(root.join("index.html"))
            .is_ok_and(|index| index.starts_with(root) && index.is_file())
    });
    match root {
        Some(root) => {
            let root = Arc::new(Root(root));
            router.fallback(move |request: Request| serve(root.clone(), request))
        }
        None => {
            crate::diag!(
                "opensided: ui.dir {} has no index.html inside it; web UI disabled",
                dir.display()
            );
            router
        }
    }
}

async fn serve(root: Arc<Root>, request: Request) -> Response {
    let path = request.uri().path();
    // API paths never fall through to the SPA, so clients see JSON 404s for typos.
    if path == "/v1" || path.starts_with("/v1/") {
        return HttpError::not_found().into_response();
    }
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return secured(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }
    let Some(relative) = relative_path(path) else {
        return secured(StatusCode::BAD_REQUEST.into_response());
    };
    let candidate = root.0.join(&relative);
    let file = match resolve(&root.0, &candidate).await {
        Resolved::File(file) => file,
        // Client-side routes have no extension; missing assets stay 404.
        Resolved::Missing if relative.extension().is_none() => {
            match resolve(&root.0, &root.0.join("index.html")).await {
                Resolved::File(index) => index,
                _ => return secured(StatusCode::NOT_FOUND.into_response()),
            }
        }
        Resolved::Missing | Resolved::Denied => {
            return secured(StatusCode::NOT_FOUND.into_response())
        }
    };
    let Ok(body) = tokio::fs::read(&file).await else {
        return secured(StatusCode::NOT_FOUND.into_response());
    };
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let immutable = file
        .parent()
        .is_some_and(|parent| parent == root.0.join("assets"));
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(name)),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if immutable {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        }),
    );
    secured(response)
}

/// Decode a URL path into normal components only; any traversal or root marker is rejected.
fn relative_path(path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(path)?;
    if decoded.contains('\0') || decoded.contains('\\') {
        return None;
    }
    let mut relative = PathBuf::new();
    for component in Path::new(decoded.trim_start_matches('/')).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(relative)
}

fn percent_decode(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn secured(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
}
