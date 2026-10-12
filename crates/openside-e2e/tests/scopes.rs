//! PP-20: a client in another user namespace (uid 12345) cannot reach the private socket and
//! can do exactly what its scoped token allows over loopback TCP.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{path::Path, process::Output};

use openside_e2e::Harness;
use serde_json::json;
use tokio::process::Command;

const SANDBOX_UID: &str = "12345";

fn bwrap(root: &Path) -> Command {
    let mut command = Command::new("bwrap");
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .args(["--unshare-user", "--uid", SANDBOX_UID, "--gid", SANDBOX_UID])
        .args(["--dev-bind", "/", "/"])
        // The daemon's runtime directory (socket) is not part of the sandbox.
        .arg("--tmpfs")
        .arg(root.join("run"))
        .arg("--");
    command
}

async fn sandboxed(root: &Path, args: &[&str]) -> Output {
    let mut command = bwrap(root);
    command.args(args);
    tokio::time::timeout(std::time::Duration::from_secs(60), command.output())
        .await
        .expect("sandboxed command deadline")
        .expect("bwrap spawn")
}

/// Whether unprivileged user namespaces work here; prints the skip reason otherwise.
async fn sandbox_available(root: &Path) -> bool {
    match Command::new("bwrap").arg("--version").output().await {
        Ok(output) if output.status.success() => {}
        _ => {
            eprintln!("SKIP: PP-20 sandbox test needs bubblewrap (bwrap) on PATH");
            return false;
        }
    }
    let output = sandboxed(root, &["id", "-u"]).await;
    if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim() != SANDBOX_UID {
        eprintln!(
            "SKIP: unprivileged user namespaces are unavailable: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return false;
    }
    true
}

fn browser() -> Option<std::path::PathBuf> {
    openside_config::discover_browser(&openside_config::Config::default()).ok()
}

#[tokio::test]
async fn sandboxed_client_needs_a_token_and_stays_inside_its_scope() {
    let Some(mut harness) = Harness::prepare(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":[]}),
    ) else {
        return;
    };
    let browser = browser();
    if let Some(executable) = &browser {
        harness.append_config(&format!("[browser]\nexecutable = {}\n", json!(executable)));
    } else {
        eprintln!("NOTE: Chromium missing; the browse call is checked only for authorization");
    }
    harness.launch().await;
    if !sandbox_available(harness.root()).await {
        harness.kill().await;
        return;
    }
    let cli = env!("CARGO_BIN_EXE_e2e-openside");
    let minted = harness
        .cli(
            cli,
            &["token", "create", "--scope", "browse", "--name", "sandbox"],
            None,
        )
        .await;
    assert!(minted.status.success(), "{minted:?}");
    let minted = String::from_utf8(minted.stdout).unwrap();
    let (_, token) = minted.trim().split_once('\t').unwrap();
    let session = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(session.status, 201, "{}", session.body);
    let session = session.body["id"].as_str().unwrap().to_owned();
    let port = std::fs::read_to_string(harness.data_dir.join("opensided.port")).unwrap();
    let url = format!("http://127.0.0.1:{}", port.trim());
    let socket = harness.socket.to_str().unwrap().to_owned();

    // The private socket is outside the sandbox; nothing falls back to it.
    let output = sandboxed(
        harness.root(),
        &[cli, "--socket", &socket, "ticket", "list"],
    )
    .await;
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("could not connect to daemon socket"),
        "{output:?}"
    );

    // TCP without a valid token is refused by the daemon.
    for args in [
        vec![
            cli,
            "--url",
            &url,
            "--token",
            "not-a-token",
            "ticket",
            "list",
        ],
        vec![
            cli,
            "--url",
            &url,
            "--token",
            "not-a-token",
            "repl",
            &session,
            "--eval",
            "1 + 1",
        ],
    ] {
        let output = sandboxed(harness.root(), &args).await;
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("HTTP 401"),
            "{output:?}"
        );
    }

    // The browse token succeeds on its route...
    let output = sandboxed(
        harness.root(),
        &[
            cli, "--url", &url, "--token", token, "repl", &session, "--eval", "1 + 1",
        ],
    )
    .await;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("HTTP 401") && !stderr.contains("HTTP 403"),
        "{output:?}"
    );
    if browser.is_some() {
        assert!(output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains('2'),
            "{output:?}"
        );
    }

    // ...and everything outside it, including minting a broader token, is refused.
    for args in [
        vec!["ticket", "list"],
        vec!["policy", "show"],
        vec!["token", "create", "--scope", "admin"],
        vec!["routine", "list"],
    ] {
        let mut full = vec![cli, "--url", &url, "--token", token];
        full.extend(args.iter().copied());
        let output = sandboxed(harness.root(), &full).await;
        assert!(!output.status.success(), "{args:?}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("HTTP 403"),
            "{args:?}: {output:?}"
        );
    }
    harness.kill().await;
}
