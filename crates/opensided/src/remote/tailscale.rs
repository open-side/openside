use std::{net::SocketAddr, path::Path, time::Duration};

use axum::http::StatusCode;
use openside_proto::TailscaleStatus;
use serde_json::Value;
use tokio::process::Command;

use crate::api::HttpError;

async fn command(bin: &Path, args: &[&str]) -> Result<Vec<u8>, HttpError> {
    let mut command = Command::new(bin);
    command.args(args).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .map_err(|_| HttpError(StatusCode::SERVICE_UNAVAILABLE, "transport_unavailable"))?
        .map_err(|_| HttpError(StatusCode::SERVICE_UNAVAILABLE, "transport_unavailable"))?;
    if !output.status.success() {
        return Err(HttpError(
            StatusCode::SERVICE_UNAVAILABLE,
            "transport_unavailable",
        ));
    }
    Ok(output.stdout)
}

fn root_handler(status: &Value) -> Option<(&str, &Value)> {
    status
        .get("Web")?
        .as_object()?
        .iter()
        .find_map(|(host, web)| {
            web.get("Handlers")?
                .get("/")
                .map(|handler| (host.as_str(), handler))
        })
}

fn our_url(status: &Value, listen: SocketAddr) -> Option<String> {
    let (host, handler) = root_handler(status)?;
    (handler.get("Proxy")?.as_str()? == format!("http://{listen}"))
        .then(|| format!("https://{}", host.strip_suffix(":443").unwrap_or(host)))
}

pub(crate) async fn status(listen: Option<SocketAddr>) -> TailscaleStatus {
    let Some(bin) = openside_config::find_on_path("tailscale") else {
        return TailscaleStatus {
            installed: false,
            state: "unavailable".into(),
            url: None,
            last_error: None,
        };
    };
    match command(&bin, &["serve", "status", "--json"]).await
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(|_| HttpError::internal())) {
        Ok(value) => {
            let url = listen.and_then(|listen| our_url(&value, listen));
            TailscaleStatus { installed: true, state: if url.is_some() { "running" } else { "stopped" }.into(), url, last_error: None }
        }
        Err(_) => TailscaleStatus { installed: true, state: "failed".into(), url: None, last_error: Some("Tailscale Serve status is unavailable; check that Tailscale is running and signed in.".into()) },
    }
}

pub(crate) async fn configure(
    enabled: bool,
    listen: SocketAddr,
) -> Result<Option<String>, HttpError> {
    let Some(bin) = openside_config::find_on_path("tailscale") else {
        return if enabled {
            Err(HttpError(
                StatusCode::SERVICE_UNAVAILABLE,
                "transport_unavailable",
            ))
        } else {
            Ok(None)
        };
    };
    configure_with(&bin, enabled, listen).await
}

async fn configure_with(
    bin: &Path,
    enabled: bool,
    listen: SocketAddr,
) -> Result<Option<String>, HttpError> {
    let before: Value =
        serde_json::from_slice(&command(bin, &["serve", "status", "--json"]).await?)
            .map_err(|_| HttpError(StatusCode::SERVICE_UNAVAILABLE, "transport_unavailable"))?;
    let existing = our_url(&before, listen);
    // Never replace or reset another application's Serve handler.
    if root_handler(&before).is_some() && existing.is_none() {
        return if enabled {
            Err(HttpError(StatusCode::CONFLICT, "tailscale_serve_conflict"))
        } else {
            Ok(None)
        };
    }
    let target = format!("http://{listen}");
    if enabled {
        command(
            bin,
            &["serve", "--bg", "--https=443", "--set-path=/", &target],
        )
        .await?;
        let after: Value =
            serde_json::from_slice(&command(bin, &["serve", "status", "--json"]).await?)
                .map_err(|_| HttpError(StatusCode::SERVICE_UNAVAILABLE, "transport_unavailable"))?;
        our_url(&after, listen).map(Some).ok_or(HttpError(
            StatusCode::SERVICE_UNAVAILABLE,
            "transport_unavailable",
        ))
    } else {
        if existing.is_some() {
            command(bin, &["serve", "--https=443", "--set-path=/", "off"]).await?;
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serves_only_matching_backend_and_preserves_other_handlers() {
        let status = json!({"Web":{"desk.example.ts.net:443":{"Handlers":{
            "/":{"Proxy":"http://127.0.0.1:47821"}, "/other":{"Path":"/tmp/other"}
        }}}});
        assert_eq!(
            our_url(&status, "127.0.0.1:47821".parse().unwrap()),
            Some("https://desk.example.ts.net".into())
        );
        assert_eq!(our_url(&status, "127.0.0.1:47822".parse().unwrap()), None);
    }
}
