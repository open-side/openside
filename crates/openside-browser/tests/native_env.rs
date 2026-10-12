//! The managed browser keeps the user's XDG environment: native-messaging hosts (Bitwarden's
//! `desktop_proxy` reads `$XDG_CACHE_HOME`, KeePassXC's proxy `$XDG_RUNTIME_DIR`) inherit the
//! browser's environment and must find the user's desktop app, not a directory inside the
//! managed profile. Runs against any discovered Chromium; the gateway is optional, so a stock
//! browser exercises the same launch path as the patched one.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
mod common;

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use openside_browser::{Browser, GatewayConfig};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

use common::{browser_config, serial};

const VARIABLES: [&str; 3] = ["XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_RUNTIME_DIR"];
const HOST: &str = "com.openside.envprobe";

fn environment(bytes: &[u8]) -> HashMap<OsString, OsString> {
    bytes
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let split = entry.iter().position(|byte| *byte == b'=')?;
            Some((
                OsStr::from_bytes(&entry[..split]).to_owned(),
                OsStr::from_bytes(&entry[split + 1..]).to_owned(),
            ))
        })
        .collect()
}

fn assert_user_values(
    seen: &HashMap<OsString, OsString>,
    profile: &Path,
    runtime: &Path,
    who: &str,
) {
    for name in VARIABLES {
        let value = seen.get(OsStr::new(name));
        assert_eq!(
            value,
            std::env::var_os(name).as_ref(),
            "{who} got a different {name} than the user"
        );
        if let Some(value) = value {
            let value = Path::new(value);
            assert!(
                !value.starts_with(profile) && !value.starts_with(runtime),
                "{who} {name} points into openside's directories: {}",
                value.display()
            );
        }
    }
}

struct Cdp {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    next: u64,
}

impl Cdp {
    async fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        self.next += 1;
        let mut message = json!({"id":self.next,"method":method,"params":params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        self.socket
            .send(Message::Text(message.to_string().into()))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Message::Text(text) = self.socket.next().await.unwrap().unwrap() {
                    let reply: Value = serde_json::from_str(&text).unwrap();
                    if reply["id"] == self.next {
                        assert!(reply.get("error").is_none(), "{method} failed: {reply}");
                        return reply["result"].clone();
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{method} did not answer"))
    }
}

#[tokio::test]
async fn native_messaging_hosts_inherit_the_users_xdg_environment() {
    let _serial = serial().await;
    let Some(mut config) = browser_config() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("profile");
    let runtime = root.path().join("runtime");
    std::fs::create_dir(&runtime).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
    let extension = root.path().join("extension");
    std::fs::create_dir(&extension).unwrap();
    std::fs::write(
        extension.join("manifest.json"),
        json!({"manifest_version":3,"name":"env probe","version":"1",
            "permissions":["nativeMessaging"],"background":{"service_worker":"sw.js"}})
        .to_string(),
    )
    .unwrap();
    std::fs::write(extension.join("sw.js"), "// native host probe\n").unwrap();
    let report = root.path().join("host-env");
    let host = root.path().join("host.sh");
    std::fs::write(
        &host,
        format!(
            "#!/bin/sh\nenv -0 > '{0}.tmp' && mv '{0}.tmp' '{0}'\n",
            report.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o700)).unwrap();

    // A private runtime directory makes the launcher take its gateway path, which used to
    // replace XDG_RUNTIME_DIR; the gateway stays optional so a stock browser still runs.
    let mut gateway = GatewayConfig::new("env-session");
    gateway.runtime_dir = Some(runtime.clone());
    gateway.required = false;
    config.gateway = Some(gateway);
    config.profile_dir = Some(profile.clone());
    config.extensions = vec![extension];
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let profile = std::fs::canonicalize(&profile).unwrap();

    // Chromium rewrites its process title, so the switch is matched in the joined text.
    let cmdline = String::from_utf8_lossy(
        &std::fs::read(format!("/proc/{}/cmdline", browser.process_id())).unwrap(),
    )
    .replace('\0', " ");
    for (switch, what) in [
        (
            format!("--disk-cache-dir={}", profile.join("cache").display()),
            "cache",
        ),
        (
            format!(
                "--breakpad-dump-location={}",
                profile.join("Crash Reports").display()
            ),
            "crash database",
        ),
    ] {
        assert!(
            cmdline.contains(&switch),
            "the browser {what} is not pinned inside the managed profile: {cmdline}"
        );
    }

    let (socket, _) = tokio_tungstenite::connect_async(browser.endpoint().ws_url)
        .await
        .unwrap();
    let mut cdp = Cdp { socket, next: 0 };
    let deadline = Instant::now() + Duration::from_secs(10);
    let (worker, origin) = loop {
        let targets = cdp.call(None, "Target.getTargets", json!({})).await;
        let found = targets["targetInfos"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|target| {
                let url = target["url"].as_str()?;
                let id = url
                    .strip_prefix("chrome-extension://")?
                    .strip_suffix("/sw.js")?;
                (target["type"] == "service_worker").then(|| {
                    (
                        target["targetId"].as_str().unwrap().to_owned(),
                        format!("chrome-extension://{id}/"),
                    )
                })
            });
        if let Some(found) = found {
            break found;
        }
        assert!(Instant::now() < deadline, "probe extension never started");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    // Chromium with --user-data-dir reads host manifests only from the profile.
    let manifests = profile.join("NativeMessagingHosts");
    std::fs::create_dir_all(&manifests).unwrap();
    std::fs::write(
        manifests.join(format!("{HOST}.json")),
        json!({"name":HOST,"description":"openside env probe","path":host,
            "type":"stdio","allowed_origins":[origin]})
        .to_string(),
    )
    .unwrap();
    let session = cdp
        .call(
            None,
            "Target.attachToTarget",
            json!({"targetId":worker,"flatten":true}),
        )
        .await["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    // connectNative from a worker that had not activated yet was seen to never start the host.
    let deadline = Instant::now() + Duration::from_secs(10);
    while cdp
        .call(
            Some(&session),
            "Runtime.evaluate",
            json!({"expression":"self.serviceWorker?.state === 'activated'","returnByValue":true}),
        )
        .await["result"]["value"]
        != true
    {
        assert!(Instant::now() < deadline, "probe worker never activated");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cdp.call(
        Some(&session),
        "Runtime.evaluate",
        json!({"expression":format!(
            "globalThis.outcome = 'connecting'; chrome.runtime.connectNative('{HOST}')\
             .onDisconnect.addListener(() => {{ globalThis.outcome = \
             chrome.runtime.lastError?.message ?? 'disconnected'; }}); true"
        )}),
    )
    .await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !report.exists() {
        if Instant::now() >= deadline {
            let outcome = cdp
                .call(
                    Some(&session),
                    "Runtime.evaluate",
                    json!({"expression":"globalThis.outcome","returnByValue":true}),
                )
                .await;
            panic!("native host never ran: {outcome}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_user_values(
        &environment(&std::fs::read(&report).unwrap()),
        &profile,
        &runtime,
        "the native-messaging host",
    );
    drop(cdp);
    browser.shutdown().await.unwrap();
}
