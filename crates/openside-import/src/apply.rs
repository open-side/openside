//! Writes cookies into an openside browser profile through the profile's own Chromium.
//!
//! Chromium encrypts and persists the cookies with its own key, so the profile stays valid
//! for the browser that will use it. The importer launches the profile headless, sends
//! `Storage.setCookies` (one cookie per call, so one invalid cookie cannot reject the rest),
//! and closes the browser gracefully so the cookie database is flushed to disk.

use std::path::Path;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use openside_browser::{Browser, BrowserConfig, BrowserError};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

use crate::error::{ImportError, Result};
use crate::model::{Cookie, SameSite};

/// Session cookies die with the browser, and the target browser is stopped right after the
/// import, so they are persisted with this lifetime instead of being lost.
pub(crate) const SESSION_COOKIE_TTL_SECS: i64 = 7 * 24 * 60 * 60;
const CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Cookies the target accepted, and those Chromium refused (host and name only).
pub(crate) struct Applied {
    pub(crate) accepted: u64,
    pub(crate) rejected: Vec<(String, String)>,
}

/// CDP `Storage.setCookies` parameters for one cookie, or `None` when it already expired.
pub(crate) fn cdp_cookie(cookie: &Cookie, now: i64) -> Option<Value> {
    let expires = match cookie.expires {
        Some(expires) if expires <= now => return None,
        Some(expires) => expires,
        None => now + SESSION_COOKIE_TTL_SECS,
    };
    let mut params = json!({
        "name": cookie.name,
        "value": cookie.value,
        "path": cookie.path,
        "secure": cookie.secure,
        "httpOnly": cookie.http_only,
        "expires": expires,
    });
    if cookie.host.starts_with('.') {
        params["domain"] = json!(cookie.host);
    } else {
        // Without a domain Chromium creates a host-only cookie for the URL's host.
        let scheme = if cookie.secure { "https" } else { "http" };
        params["url"] = json!(format!("{scheme}://{}{}", cookie.host, cookie.path));
    }
    // Chromium rejects SameSite=None without Secure.
    match cookie.same_site {
        Some(SameSite::None) if cookie.secure => params["sameSite"] = json!("None"),
        Some(SameSite::Lax) => params["sameSite"] = json!("Lax"),
        Some(SameSite::Strict) => params["sameSite"] = json!("Strict"),
        _ => {}
    }
    Some(params)
}

fn browser_error(error: &BrowserError) -> ImportError {
    ImportError::Browser(error.to_string())
}

#[cfg(unix)]
/// True when a live Chromium holds the profile. Chromium keeps `SingletonLock` as a symlink
/// to `<host>-<pid>` while it runs; a lock left by a dead process is stale and ignored.
/// This catches browsers started without a debugging port, which `Browser::launch` cannot see.
pub(crate) fn profile_in_use(profile_dir: &Path) -> bool {
    let Ok(target) = std::fs::read_link(profile_dir.join("SingletonLock")) else {
        return false;
    };
    let Some(pid) = target
        .to_string_lossy()
        .rsplit('-')
        .next()
        .and_then(|pid| pid.parse::<i32>().ok())
        .filter(|pid| *pid > 0)
    else {
        return false;
    };
    // EPERM still means the process exists.
    !matches!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    )
}

#[cfg(not(unix))]
pub(crate) fn ensure_profile_free(_profile_name: &str, _profile_dir: &Path) -> Result<()> {
    Err(ImportError::Browser(
        "Browser profile locking is unsupported on this platform".into(),
    ))
}

#[cfg(unix)]
pub(crate) fn ensure_profile_free(profile_name: &str, profile_dir: &Path) -> Result<()> {
    if profile_in_use(profile_dir) {
        return Err(ImportError::ProfileInUse(profile_name.to_owned()));
    }
    Ok(())
}

pub(crate) async fn apply(
    executable: &Path,
    profile_name: &str,
    profile_dir: &Path,
    cookies: Vec<Value>,
) -> Result<Applied> {
    let mut config = BrowserConfig::new(executable);
    config.profile_dir = Some(profile_dir.to_owned());
    ensure_profile_free(profile_name, profile_dir)?;
    let (browser, _events) = Browser::launch(config).await.map_err(|error| match error {
        BrowserError::InvalidEndpoint(_) => ImportError::ProfileInUse(profile_name.to_owned()),
        // A browser that won the lock between the check above and the launch exits at once.
        #[cfg(unix)]
        _ if profile_in_use(profile_dir) => ImportError::ProfileInUse(profile_name.to_owned()),
        other => browser_error(&other),
    })?;
    let outcome = send(&browser.endpoint().ws_url, cookies).await;
    // Shut down even after a failure so no Chromium outlives the import.
    let shutdown = browser
        .shutdown()
        .await
        .map_err(|error| browser_error(&error));
    let applied = outcome?;
    shutdown?;
    Ok(applied)
}

async fn send(ws_url: &str, cookies: Vec<Value>) -> Result<Applied> {
    let (mut socket, _) =
        tokio::time::timeout(CALL_TIMEOUT, tokio_tungstenite::connect_async(ws_url))
            .await
            .map_err(|_| ImportError::Browser("timed out connecting to the target browser".into()))?
            .map_err(|error| ImportError::Browser(error.to_string()))?;
    let mut applied = Applied {
        accepted: 0,
        rejected: Vec::new(),
    };
    for (index, cookie) in cookies.into_iter().enumerate() {
        let id = u64::try_from(index).unwrap_or(0) + 1;
        let label = (
            cookie
                .get("domain")
                .or_else(|| cookie.get("url"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            cookie["name"].as_str().unwrap_or_default().to_owned(),
        );
        let request =
            json!({"id": id, "method": "Storage.setCookies", "params": {"cookies": [cookie]}});
        socket
            .send(Message::text(request.to_string()))
            .await
            .map_err(|error| ImportError::Browser(error.to_string()))?;
        let reply = tokio::time::timeout(CALL_TIMEOUT, async {
            while let Some(message) = socket.next().await {
                let Message::Text(text) = message.map_err(|error| error.to_string())? else {
                    continue;
                };
                let value: Value =
                    serde_json::from_str(&text).map_err(|error| error.to_string())?;
                if value["id"].as_u64() == Some(id) {
                    return Ok(value);
                }
            }
            Err("the target browser closed the connection".to_owned())
        })
        .await
        .map_err(|_| ImportError::Browser("timed out waiting for the target browser".into()))?
        .map_err(ImportError::Browser)?;
        if reply.get("error").is_some() {
            applied.rejected.push(label);
        } else {
            applied.accepted += 1;
        }
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(host: &str) -> Cookie {
        Cookie {
            host: host.into(),
            name: "sid".into(),
            value: "v".into(),
            path: "/a".into(),
            expires: None,
            secure: false,
            http_only: true,
            same_site: Some(SameSite::None),
        }
    }

    #[test]
    fn host_only_cookies_use_a_url_and_domain_cookies_a_domain() {
        let host_only = cdp_cookie(&cookie("127.0.0.1"), 1_000).unwrap();
        assert_eq!(host_only["url"], "http://127.0.0.1/a");
        assert!(host_only.get("domain").is_none());
        let domain = cdp_cookie(&cookie(".example.test"), 1_000).unwrap();
        assert_eq!(domain["domain"], ".example.test");
        assert!(domain.get("url").is_none());
    }

    #[test]
    fn session_cookies_get_a_finite_lifetime_and_expired_ones_are_dropped() {
        let session = cdp_cookie(&cookie("a.test"), 1_000).unwrap();
        assert_eq!(session["expires"], 1_000 + SESSION_COOKIE_TTL_SECS);
        let mut expired = cookie("a.test");
        expired.expires = Some(999);
        assert!(cdp_cookie(&expired, 1_000).is_none());
    }

    #[test]
    fn samesite_none_without_secure_is_omitted() {
        let params = cdp_cookie(&cookie("a.test"), 1_000).unwrap();
        assert!(params.get("sameSite").is_none());
        let mut secure = cookie("a.test");
        secure.secure = true;
        let params = cdp_cookie(&secure, 1_000).unwrap();
        assert_eq!(params["sameSite"], "None");
        assert_eq!(params["url"], "https://a.test/a");
    }
}
