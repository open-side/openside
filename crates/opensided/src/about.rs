//! `GET /v1/about` and `POST /v1/about/update-check`: installed versions and whether a newer
//! release is published.
//!
//! The check is the only request here that leaves the computer, and it runs only when a person
//! asks for it.

use std::{future::Future, time::Duration};

use axum::{http::StatusCode, Json};
use openside_proto::{About, UpdateCheck};
use std::sync::RwLock;

use crate::api::HttpError;

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/open-side/openside/releases/latest";

static RUNTIME_VERSION: RwLock<Option<String>> = RwLock::new(None);

/// The agent runtime version from the latest sidecar handshake; fake runtimes report none.
pub(crate) fn record_runtime_version(version: &str) {
    if version.is_empty() {
        return;
    }
    let mut recorded = RUNTIME_VERSION.write().unwrap_or_else(|e| e.into_inner());
    if recorded.as_deref() != Some(version) {
        *recorded = Some(version.to_owned());
    }
}

fn releases_url() -> String {
    format!("{REPOSITORY}/releases")
}

pub(crate) async fn get() -> Json<About> {
    Json(About {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        runtime_versions: openside_agent_sidecar::supported_agent_versions(),
        runtime_version: RUNTIME_VERSION
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        releases_url: releases_url(),
    })
}

/// How long one answer from the release feed is reused.
const CHECK_TTL: Duration = Duration::from_secs(600);
/// The release document is a few KiB; anything bigger is not the feed.
const MAX_RELEASE_BYTES: usize = 256 * 1024;

type LastCheck = tokio::sync::Mutex<Option<(std::time::Instant, UpdateCheck)>>;

/// The last answer; holding the lock across the request makes concurrent checks share it.
static LAST_CHECK: LastCheck = tokio::sync::Mutex::const_new(None);

pub(crate) async fn update_check() -> Result<Json<UpdateCheck>, HttpError> {
    cached(&LAST_CHECK, CHECK_TTL, || fetch(LATEST_RELEASE_API))
        .await
        .map(Json)
}

/// The answer in `last` while younger than `ttl`, otherwise a fresh `fetch`; failures are not
/// kept, so the next check asks again.
async fn cached<F, Fut>(last: &LastCheck, ttl: Duration, fetch: F) -> Result<UpdateCheck, HttpError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<UpdateCheck, HttpError>>,
{
    let mut last = last.lock().await;
    if let Some((at, check)) = last.as_ref() {
        if at.elapsed() < ttl {
            return Ok(check.clone());
        }
    }
    let check = fetch().await?;
    *last = Some((std::time::Instant::now(), check.clone()));
    Ok(check)
}

async fn fetch(feed: &str) -> Result<UpdateCheck, HttpError> {
    let unavailable = || HttpError(StatusCode::BAD_GATEWAY, "update_check_unavailable");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        // The feed URL is fixed; a redirect could only lead somewhere else.
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("opensided/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| HttpError::internal())?;
    let mut response = client
        .get(feed)
        .header("accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| unavailable())?;
    // GitHub answers 404 while a repository has no published release.
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(compare(env!("CARGO_PKG_VERSION"), None));
    }
    if !response.status().is_success() {
        return Err(unavailable());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if body.len() + chunk.len() > MAX_RELEASE_BYTES {
            return Err(unavailable());
        }
        body.extend_from_slice(&chunk);
    }
    let release: serde_json::Value = serde_json::from_slice(&body).map_err(|_| unavailable())?;
    let tag = release
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(unavailable)?;
    let mut check = compare(env!("CARGO_PKG_VERSION"), Some(tag));
    if let Some(url) = release
        .get("html_url")
        .and_then(serde_json::Value::as_str)
        .and_then(release_page)
    {
        check.url = Some(url);
    }
    Ok(check)
}

/// `url` when it is an https page of this repository on its own host; anything else (another
/// host, a lookalike repository name, credentials, a port) is dropped.
fn release_page(url: &str) -> Option<String> {
    let repository = reqwest::Url::parse(REPOSITORY).ok()?;
    let page = reqwest::Url::parse(url).ok()?;
    let prefix = format!("{}/", repository.path().trim_end_matches('/'));
    (page.scheme() == "https"
        && page.host_str() == repository.host_str()
        && page.port().is_none()
        && page.username().is_empty()
        && page.password().is_none()
        && page.path().starts_with(&prefix))
    .then(|| page.to_string())
}

/// One dot-separated prerelease identifier; numeric ones sort before alphanumeric ones.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Identifier {
    Numeric(u64),
    Text(String),
}

/// Major, minor and patch; whether it is a release; the prerelease identifiers.
type VersionKey = ((u64, u64, u64), bool, Vec<Identifier>);

/// Semver precedence key of `1.2.3`, `v1.2.3` or `1.2.3-rc.1+build`: a release sorts after
/// all of its prereleases, and build metadata is ignored.
fn version_key(version: &str) -> Option<VersionKey> {
    let version = version.trim().trim_start_matches('v');
    let version = version.split('+').next()?;
    let (core, prerelease) = match version.split_once('-') {
        Some((core, prerelease)) => (core, Some(prerelease)),
        None => (version, None),
    };
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let triple = (parts.next()??, parts.next()??, parts.next()??);
    if parts.next().is_some() {
        return None;
    }
    let identifiers = match prerelease {
        None => Vec::new(),
        Some(prerelease) => prerelease
            .split('.')
            .map(|part| match part.parse::<u64>() {
                Ok(number) => Some(Identifier::Numeric(number)),
                Err(_) if !part.is_empty() => Some(Identifier::Text(part.to_owned())),
                Err(_) => None,
            })
            .collect::<Option<Vec<_>>>()?,
    };
    Some((triple, prerelease.is_none(), identifiers))
}

pub(crate) fn compare(current: &str, latest_tag: Option<&str>) -> UpdateCheck {
    let latest = latest_tag.map(|tag| tag.trim().trim_start_matches('v').to_owned());
    let update_available = match (
        version_key(current),
        latest.as_deref().and_then(version_key),
    ) {
        (Some(current), Some(latest)) => latest > current,
        _ => false,
    };
    UpdateCheck {
        current: current.to_owned(),
        url: latest.as_ref().map(|_| releases_url()),
        latest,
        update_available,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_repositorys_https_pages_are_release_links() {
        let good = format!("{REPOSITORY}/releases/tag/v0.2.0");
        assert_eq!(release_page(&good), Some(good.clone()));
        for bad in [
            format!("{REPOSITORY}.evil/releases/tag/v0.2.0"),
            format!("{REPOSITORY}-fork/releases"),
            good.replacen("https://", "http://", 1),
            good.replacen("github.com", "github.com.evil.example", 1),
            good.replacen("github.com", "github.com:8443", 1),
            good.replacen("https://", "https://user@", 1),
            "https://example.com/open-side/openside/releases".into(),
            "not a url".into(),
        ] {
            assert_eq!(release_page(&bad), None, "{bad}");
        }
    }

    #[test]
    fn newer_tags_are_updates_and_unparsable_ones_are_not() {
        assert!(compare("0.0.1", Some("v0.0.2")).update_available);
        assert!(compare("0.0.1", Some("0.1.0-rc1")).update_available);
        assert!(!compare("0.1.0", Some("v0.1.0")).update_available);
        assert!(!compare("0.2.0", Some("v0.1.9")).update_available);
        // A release is newer than its own prereleases; prereleases order by identifier.
        assert!(compare("1.2.3-rc1", Some("v1.2.3")).update_available);
        assert!(!compare("1.2.3", Some("v1.2.3-rc1")).update_available);
        assert!(compare("1.2.3-rc.2", Some("1.2.3-rc.10")).update_available);
        assert!(compare("1.2.3-alpha", Some("1.2.3-alpha.1")).update_available);
        assert!(compare("1.2.3-1", Some("1.2.3-beta")).update_available);
        assert!(!compare("1.2.3+build.5", Some("1.2.3+build.9")).update_available);
        assert!(!compare("0.0.1", Some("nightly")).update_available);
        let none = compare("0.0.1", None);
        assert_eq!(
            (none.latest, none.url, none.update_available),
            (None, None, false)
        );
        let check = compare("0.0.1", Some("v0.0.2"));
        assert_eq!(check.latest.as_deref(), Some("0.0.2"));
        assert_eq!(check.url, Some(format!("{REPOSITORY}/releases")));
    }

    #[test]
    fn an_empty_runtime_version_is_not_recorded() {
        record_runtime_version("0.2.0");
        record_runtime_version("");
        assert_eq!(RUNTIME_VERSION.read().unwrap().as_deref(), Some("0.2.0"));
    }

    fn answer(latest: &str) -> UpdateCheck {
        compare("0.0.1", Some(latest))
    }

    #[tokio::test]
    async fn concurrent_and_repeated_checks_share_one_request_within_the_ttl() {
        let last = LastCheck::const_new(None);
        let requests = std::sync::atomic::AtomicUsize::new(0);
        let counter = &requests;
        let slow = move || async move {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(answer("0.0.2"))
        };
        let ttl = Duration::from_secs(600);
        let (first, second) = tokio::join!(cached(&last, ttl, slow), cached(&last, ttl, slow));
        assert_eq!(first.unwrap().latest.as_deref(), Some("0.0.2"));
        assert_eq!(second.unwrap().latest.as_deref(), Some("0.0.2"));
        let again = cached(&last, ttl, || async { Ok(answer("9.9.9")) })
            .await
            .unwrap();
        assert_eq!(
            again.latest.as_deref(),
            Some("0.0.2"),
            "reused inside the TTL"
        );
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        let expired = cached(&last, Duration::ZERO, || async { Ok(answer("0.0.3")) }).await;
        assert_eq!(expired.unwrap().latest.as_deref(), Some("0.0.3"));
    }

    #[tokio::test]
    async fn a_failed_check_is_not_cached() {
        let last = LastCheck::const_new(None);
        let ttl = Duration::from_secs(600);
        let failed = cached(&last, ttl, || async {
            Err(HttpError(
                StatusCode::BAD_GATEWAY,
                "update_check_unavailable",
            ))
        })
        .await;
        assert!(failed.is_err());
        let retried = cached(&last, ttl, || async { Ok(answer("0.0.2")) }).await;
        assert_eq!(retried.unwrap().latest.as_deref(), Some("0.0.2"));
    }

    /// A loopback feed answering `/ok`, `/none`, `/moved` and `/huge`.
    async fn feed() -> String {
        use axum::{response::Redirect, routing::get, Router};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let page = format!("{REPOSITORY}/releases/tag/v99.0.0");
        let router = Router::new()
            .route(
                "/ok",
                get(move || async move {
                    Json(serde_json::json!({"tag_name": "v99.0.0", "html_url": page}))
                }),
            )
            .route("/none", get(|| async { StatusCode::NOT_FOUND }))
            .route("/moved", get(|| async { Redirect::temporary("/ok") }))
            .route(
                "/huge",
                get(|| async {
                    let filler = "x".repeat(MAX_RELEASE_BYTES);
                    Json(serde_json::json!({"tag_name": "v99.0.0", "body": filler}))
                }),
            );
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        base
    }

    #[tokio::test]
    async fn the_feed_is_read_without_redirects_and_within_its_size_cap() {
        let base = feed().await;
        let ok = fetch(&format!("{base}/ok")).await.unwrap();
        assert!(ok.update_available);
        assert_eq!(ok.latest.as_deref(), Some("99.0.0"));
        assert_eq!(ok.url, Some(format!("{REPOSITORY}/releases/tag/v99.0.0")));
        let none = fetch(&format!("{base}/none")).await.unwrap();
        assert!(!none.update_available && none.latest.is_none());
        for refused in ["moved", "huge"] {
            let error = fetch(&format!("{base}/{refused}")).await.unwrap_err();
            assert_eq!(
                (error.0, error.1),
                (StatusCode::BAD_GATEWAY, "update_check_unavailable"),
                "{refused}"
            );
        }
    }
}
