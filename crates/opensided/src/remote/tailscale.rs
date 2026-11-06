//! openside's Tailscale Serve handler: the root (`/`) of one HTTPS port
//! (`remote.tailscale.https_port`) proxied to `daemon.listen`. Another app's handler on that
//! port is never replaced or reset; the person picks a free port instead.

use std::{future::Future, net::SocketAddr, path::Path, time::Duration};

use axum::http::StatusCode;
use openside_config::tailscale::{conflict_message, serve_slot, ServeSlot};
use openside_proto::TailscaleStatus;
use serde_json::Value;
use tokio::process::Command;

use crate::api::HttpError;

fn unavailable() -> HttpError {
    HttpError(StatusCode::SERVICE_UNAVAILABLE, "transport_unavailable")
}

async fn command(bin: &Path, args: &[&str]) -> Result<Vec<u8>, HttpError> {
    let mut command = Command::new(bin);
    command.args(args).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())?;
    if !output.status.success() {
        return Err(unavailable());
    }
    Ok(output.stdout)
}

async fn serve_status(bin: &Path) -> Result<Value, HttpError> {
    serde_json::from_slice(&command(bin, &["serve", "status", "--json"]).await?)
        .map_err(|_| unavailable())
}

fn target(listen: SocketAddr) -> String {
    format!("http://{listen}")
}

pub(crate) async fn status(listen: Option<SocketAddr>, port: u16) -> TailscaleStatus {
    let Some(bin) = openside_config::find_on_path("tailscale") else {
        return TailscaleStatus {
            installed: false,
            state: "unavailable".into(),
            url: None,
            last_error: None,
        };
    };
    status_with(&bin, listen, port).await
}

async fn status_with(bin: &Path, listen: Option<SocketAddr>, port: u16) -> TailscaleStatus {
    let value = match serve_status(bin).await {
        Ok(value) => value,
        Err(_) => {
            return TailscaleStatus {
                installed: true,
                state: "failed".into(),
                url: None,
                last_error: Some("Tailscale Serve status is unavailable; check that Tailscale is running and signed in.".into()),
            }
        }
    };
    let slot = serve_slot(&value, port, &listen.map(target).unwrap_or_default());
    let (state, url, last_error) = match slot {
        ServeSlot::Ours(url) => ("running", Some(url), None),
        ServeSlot::Taken(holder) => (
            "failed",
            None,
            Some(conflict_message(&value, port, &holder)),
        ),
        ServeSlot::Free => ("stopped", None, None),
    };
    TailscaleStatus {
        installed: true,
        state: state.into(),
        url,
        last_error,
    }
}

/// Turn openside's handler on `port` on or off, then run `commit` with its HTTPS origin (when
/// on) to save the settings. If `commit` fails, the Serve change is undone so Serve and the
/// saved settings still agree. Proxying to `listen` before the daemon listens there is fine:
/// Serve forwards lazily.
pub(crate) async fn configure<F, Fut>(
    enabled: bool,
    listen: SocketAddr,
    port: u16,
    commit: F,
) -> Result<(), HttpError>
where
    F: FnOnce(Option<String>) -> Fut,
    Fut: Future<Output = Result<(), HttpError>>,
{
    let Some(bin) = openside_config::find_on_path("tailscale") else {
        return if enabled {
            Err(unavailable())
        } else {
            commit(None).await
        };
    };
    configure_with(&bin, enabled, listen, port, commit).await
}

async fn configure_with<F, Fut>(
    bin: &Path,
    enabled: bool,
    listen: SocketAddr,
    port: u16,
    commit: F,
) -> Result<(), HttpError>
where
    F: FnOnce(Option<String>) -> Fut,
    Fut: Future<Output = Result<(), HttpError>>,
{
    let (url, changed) = apply(bin, enabled, listen, port).await?;
    let Err(error) = commit(url).await else {
        return Ok(());
    };
    if changed {
        if let Err(undo) = apply(bin, !enabled, listen, port).await {
            crate::diag!(
                "opensided: saving remote settings failed and Tailscale Serve port {port} could not be put back ({})",
                undo.1
            );
        }
    }
    Err(error)
}

/// Turn openside's handler on `port` back on (Remote Control turned on); the saved settings
/// are the caller's business. Returns the HTTPS origin it serves.
pub(crate) async fn start(
    bin: &Path,
    listen: SocketAddr,
    port: u16,
) -> Result<Option<String>, HttpError> {
    apply(bin, true, listen, port).await.map(|(url, _)| url)
}

/// Turn openside's handler on `port` off (Remote Control turned off) and keep the saved
/// settings. Another app's handler on that port is left alone.
pub(crate) async fn stop(bin: &Path, listen: SocketAddr, port: u16) -> Result<(), HttpError> {
    apply(bin, false, listen, port).await.map(|_| ())
}

/// Apply the Serve change; returns the origin when on and whether Serve was changed.
async fn apply(
    bin: &Path,
    enabled: bool,
    listen: SocketAddr,
    port: u16,
) -> Result<(Option<String>, bool), HttpError> {
    let target = target(listen);
    let https = format!("--https={port}");
    match (
        enabled,
        serve_slot(&serve_status(bin).await?, port, &target),
    ) {
        (true, ServeSlot::Ours(url)) => Ok((Some(url), false)),
        (true, ServeSlot::Taken(_)) => {
            Err(HttpError(StatusCode::CONFLICT, "tailscale_serve_conflict"))
        }
        (true, ServeSlot::Free) => {
            command(bin, &["serve", "--bg", &https, "--set-path=/", &target]).await?;
            match serve_status(bin)
                .await
                .map(|status| serve_slot(&status, port, &target))
            {
                Ok(ServeSlot::Ours(url)) => Ok((Some(url), true)),
                _ => {
                    // The port was free, so its root handler is the one just added.
                    if command(bin, &["serve", &https, "--set-path=/", "off"])
                        .await
                        .is_err()
                    {
                        crate::diag!(
                            "opensided: Tailscale Serve port {port} did not report openside's handler and it could not be turned off"
                        );
                    }
                    Err(unavailable())
                }
            }
        }
        (false, ServeSlot::Ours(_)) => {
            command(bin, &["serve", &https, "--set-path=/", "off"]).await?;
            Ok((None, true))
        }
        (false, _) => Ok((None, false)),
    }
}

#[cfg(all(test, unix))]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Keeps one file per HTTPS port holding its root proxy target, like tailscaled's Serve
    /// config, and logs every invocation. A `skew` file makes an add record another target,
    /// as if Serve accepted the command but reports something else afterwards.
    const FAKE: &str = r#"#!/bin/sh
dir=$(dirname "$0")
echo "$*" >> "$dir/calls"
if [ "$1 $2" = "serve status" ]; then
  printf '{"TCP":{'; sep=
  for f in "$dir"/web-*; do [ -e "$f" ] || continue; printf '%s"%s":{"HTTPS":true}' "$sep" "${f##*/web-}"; sep=,; done
  printf '},"Web":{'; sep=
  for f in "$dir"/web-*; do [ -e "$f" ] || continue; printf '%s"desk.example.ts.net:%s":{"Handlers":{"/":{"Proxy":"%s"}}}' "$sep" "${f##*/web-}" "$(cat "$f")"; sep=,; done
  printf '}}\n'; exit 0
fi
port=; last=
for a in "$@"; do case "$a" in --https=*) port=${a#--https=};; esac; last=$a; done
[ -n "$port" ] || exit 2
if [ "$last" = off ]; then rm "$dir/web-$port" || exit 1
elif [ -e "$dir/skew" ]; then printf %s "$last/skewed" > "$dir/web-$port"
else printf %s "$last" > "$dir/web-$port"; fi
"#;

    struct Fake {
        dir: tempfile::TempDir,
        bin: std::path::PathBuf,
    }

    impl Fake {
        fn new(others: &[(u16, &str)]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let bin = dir.path().join("tailscale");
            std::fs::write(&bin, FAKE).unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
            for (port, proxy) in others {
                std::fs::write(dir.path().join(format!("web-{port}")), proxy).unwrap();
            }
            Self { dir, bin }
        }
        fn handler(&self, port: u16) -> Option<String> {
            std::fs::read_to_string(self.dir.path().join(format!("web-{port}"))).ok()
        }
        fn changes(&self) -> Vec<String> {
            std::fs::read_to_string(self.dir.path().join("calls"))
                .unwrap_or_default()
                .lines()
                .filter(|line| !line.starts_with("serve status"))
                .map(str::to_owned)
                .collect()
        }
    }

    const LISTEN: &str = "127.0.0.1:47820";
    const APP_443: &str = "http://127.0.0.1:7317";
    const APP_8443: &str = "http://127.0.0.1:7878";

    /// `configure_with` whose save always succeeds, returning the origin it was given.
    async fn turn(
        bin: &Path,
        enabled: bool,
        listen: SocketAddr,
        port: u16,
    ) -> Result<Option<String>, HttpError> {
        let saved = std::sync::Mutex::new(None);
        configure_with(bin, enabled, listen, port, |url| {
            *saved.lock().unwrap() = url;
            async { Ok(()) }
        })
        .await?;
        Ok(saved.into_inner().unwrap())
    }

    #[tokio::test]
    async fn refuses_a_port_another_app_serves_and_names_a_free_one() {
        let fake = Fake::new(&[(443, APP_443), (8443, APP_8443)]);
        let listen = LISTEN.parse().unwrap();
        for port in [443, 8443] {
            let error = turn(&fake.bin, true, listen, port).await.unwrap_err();
            assert_eq!(
                (error.0, error.1),
                (StatusCode::CONFLICT, "tailscale_serve_conflict")
            );
            // Turning off on someone else's port is a no-op, not an `off` for their handler.
            assert_eq!(turn(&fake.bin, false, listen, port).await.unwrap(), None);
        }
        assert!(fake.changes().is_empty(), "{:?}", fake.changes());
        assert_eq!(fake.handler(443).as_deref(), Some(APP_443));
        assert_eq!(fake.handler(8443).as_deref(), Some(APP_8443));
        let status = status_with(&fake.bin, Some(listen), 443).await;
        assert_eq!(status.state, "failed");
        let hint = status.last_error.unwrap();
        assert!(
            hint.contains("remote.tailscale.https_port = 10000"),
            "{hint}"
        );
        assert!(hint.contains(APP_443), "{hint}");
    }

    #[tokio::test]
    async fn configured_port_serves_beside_other_apps_idempotently_and_turns_off_alone() {
        let fake = Fake::new(&[(443, APP_443), (8443, APP_8443)]);
        let listen = LISTEN.parse().unwrap();
        let url = turn(&fake.bin, true, listen, 10000).await.unwrap();
        assert_eq!(url.as_deref(), Some("https://desk.example.ts.net:10000"));
        assert_eq!(
            fake.handler(10000).as_deref(),
            Some("http://127.0.0.1:47820")
        );
        // A restart or a repeated turn-on finds its own handler and runs no Serve change.
        let again = turn(&fake.bin, true, listen, 10000).await.unwrap();
        assert_eq!(again, url);
        assert_eq!(
            fake.changes(),
            ["serve --bg --https=10000 --set-path=/ http://127.0.0.1:47820"]
        );
        let status = status_with(&fake.bin, Some(listen), 10000).await;
        assert_eq!(
            (status.state.as_str(), status.url.clone()),
            ("running", url)
        );

        assert_eq!(turn(&fake.bin, false, listen, 10000).await.unwrap(), None);
        assert_eq!(fake.handler(10000), None);
        assert_eq!(fake.handler(443).as_deref(), Some(APP_443));
        assert_eq!(fake.handler(8443).as_deref(), Some(APP_8443));
        assert_eq!(turn(&fake.bin, false, listen, 10000).await.unwrap(), None);
        assert_eq!(
            fake.changes(),
            [
                "serve --bg --https=10000 --set-path=/ http://127.0.0.1:47820",
                "serve --https=10000 --set-path=/ off"
            ]
        );
        assert_eq!(
            status_with(&fake.bin, Some(listen), 10000).await.state,
            "stopped"
        );
    }

    #[tokio::test]
    async fn default_port_443_has_no_port_in_the_url() {
        let fake = Fake::new(&[]);
        let url = turn(&fake.bin, true, LISTEN.parse().unwrap(), 443)
            .await
            .unwrap();
        assert_eq!(url.as_deref(), Some("https://desk.example.ts.net"));
    }

    #[tokio::test]
    async fn a_failed_save_puts_serve_back_in_both_directions() {
        let fake = Fake::new(&[(443, APP_443)]);
        let listen: SocketAddr = LISTEN.parse().unwrap();
        let refuse = |_| async { Err(HttpError::internal()) };

        let error = configure_with(&fake.bin, true, listen, 10000, refuse)
            .await
            .unwrap_err();
        assert_eq!(error.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(fake.handler(10000), None, "enable left an orphaned handler");

        turn(&fake.bin, true, listen, 10000).await.unwrap();
        configure_with(&fake.bin, false, listen, 10000, refuse)
            .await
            .unwrap_err();
        assert_eq!(
            fake.handler(10000).as_deref(),
            Some("http://127.0.0.1:47820")
        );
        assert_eq!(fake.handler(443).as_deref(), Some(APP_443));
        assert_eq!(
            fake.changes(),
            [
                "serve --bg --https=10000 --set-path=/ http://127.0.0.1:47820",
                "serve --https=10000 --set-path=/ off",
                "serve --bg --https=10000 --set-path=/ http://127.0.0.1:47820",
                "serve --https=10000 --set-path=/ off",
                "serve --bg --https=10000 --set-path=/ http://127.0.0.1:47820",
            ]
        );
    }

    #[tokio::test]
    async fn an_add_serve_does_not_confirm_is_turned_off_again() {
        let fake = Fake::new(&[(443, APP_443)]);
        std::fs::write(fake.dir.path().join("skew"), "").unwrap();
        let error = turn(&fake.bin, true, LISTEN.parse().unwrap(), 10000)
            .await
            .unwrap_err();
        assert_eq!(
            (error.0, error.1),
            (StatusCode::SERVICE_UNAVAILABLE, "transport_unavailable")
        );
        assert_eq!(
            fake.handler(10000),
            None,
            "the unconfirmed handler was left behind"
        );
        assert_eq!(fake.handler(443).as_deref(), Some(APP_443));
        assert_eq!(
            fake.changes(),
            [
                "serve --bg --https=10000 --set-path=/ http://127.0.0.1:47820",
                "serve --https=10000 --set-path=/ off",
            ]
        );
    }
}
