//! Reading `tailscale serve status --json` for openside's HTTPS handler. The daemon and
//! `openside remote setup tailscale` share this so both refuse the same ports: openside owns
//! a whole HTTPS port with exactly one root (`/`) proxy and never shares one with another app.

use serde_json::Value;

/// HTTPS port openside asks Tailscale Serve for unless `remote.tailscale.https_port` says otherwise.
pub const DEFAULT_HTTPS_PORT: u16 = 443;

/// Ports suggested first when the configured one is taken: the ones Tailscale documents for
/// Serve and Funnel, so the suggestion also works on clients that still restrict ports.
const SUGGESTED_HTTPS_PORTS: [u16; 3] = [443, 8443, 10000];

/// What an HTTPS port holds, from openside's point of view.
#[derive(Debug, PartialEq, Eq)]
pub enum ServeSlot {
    /// Tailscale Serve has nothing on this port.
    Free,
    /// Only openside's root proxy to `target`; the value is its HTTPS origin.
    Ours(String),
    /// Anything else (another app, another path, another host, a TCP forward or Funnel);
    /// the value describes it for the person.
    Taken(String),
}

/// `https://host` for 443, `https://host:port` otherwise.
pub fn serve_origin(host: &str, port: u16) -> String {
    if port == 443 {
        format!("https://{host}")
    } else {
        format!("https://{host}:{port}")
    }
}

/// Classify `port` in a `tailscale serve status --json` document for an openside handler
/// proxying to `target` (`http://127.0.0.1:47820`). The port is ours only when its single
/// web host has a single handler, `/`, proxying to exactly `target`, without Funnel.
pub fn serve_slot(status: &Value, port: u16, target: &str) -> ServeSlot {
    let suffix = format!(":{port}");
    let tcp = status["TCP"].get(port.to_string());
    if let Some(forward) = tcp
        .and_then(|tcp| tcp.get("TCPForward"))
        .and_then(Value::as_str)
    {
        return ServeSlot::Taken(format!("TCP forward to {forward}"));
    }
    if tcp.is_some_and(|tcp| tcp != &serde_json::json!({"HTTPS": true})) {
        return ServeSlot::Taken("a non-HTTPS TCP listener".into());
    }
    let funnel = status
        .get("AllowFunnel")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .any(|(key, on)| key.ends_with(&suffix) && on.as_bool() == Some(true));
    if funnel {
        return ServeSlot::Taken("Funnel, open to the internet".into());
    }
    let hosts: Vec<(&str, &serde_json::Map<String, Value>)> = status
        .get("Web")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(key, web)| {
            let host = key.strip_suffix(&suffix)?;
            let handlers = web.get("Handlers").and_then(Value::as_object)?;
            (!handlers.is_empty()).then_some((host, handlers))
        })
        .collect();
    let describe = |path: &str, handler: &Value| match handler.get("Proxy").and_then(Value::as_str)
    {
        Some(proxy) => format!("{path} proxy to {proxy}"),
        None => format!("{path} file or text handler"),
    };
    match hosts.as_slice() {
        [] => ServeSlot::Free,
        [(host, handlers)] if handlers.len() == 1 => match handlers.iter().next() {
            Some((path, handler))
                if path == "/" && handler.get("Proxy").and_then(Value::as_str) == Some(target) =>
            {
                ServeSlot::Ours(serve_origin(host, port))
            }
            Some((path, handler)) => ServeSlot::Taken(describe(path, handler)),
            None => ServeSlot::Free,
        },
        many => {
            // Report another app's handler rather than our own when both are present.
            let foreign =
                many.iter()
                    .flat_map(|(_, handlers)| handlers.iter())
                    .find(|(path, handler)| {
                        !(path.as_str() == "/"
                            && handler.get("Proxy").and_then(Value::as_str) == Some(target))
                    });
            ServeSlot::Taken(foreign.map_or_else(
                || "the same proxy on several hosts".into(),
                |(path, handler)| describe(path, handler),
            ))
        }
    }
}

/// First HTTPS port with no Serve configuration at all: 443, 8443, 10000, then 10001 upwards.
pub fn free_https_port(status: &Value) -> Option<u16> {
    let used = |port: u16| {
        status["TCP"].get(port.to_string()).is_some()
            || serve_slot(status, port, "") != ServeSlot::Free
    };
    SUGGESTED_HTTPS_PORTS
        .into_iter()
        .chain(10001..=u16::MAX)
        .find(|port| !used(*port))
}

/// Explanation for a taken port, naming a free one to configure instead.
pub fn conflict_message(status: &Value, port: u16, holder: &str) -> String {
    let fix = match free_https_port(status) {
        Some(free) => format!(
            "Set remote.tailscale.https_port = {free} (free on this computer), then turn Tailscale on again."
        ),
        None => "Set remote.tailscale.https_port to a port Tailscale Serve does not use yet.".into(),
    };
    format!("Tailscale Serve port {port} on this computer belongs to another app ({holder}); openside never replaces it. {fix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const OURS: &str = "http://127.0.0.1:47820";

    fn host_like_this_one() -> Value {
        json!({
            "TCP": {"443": {"HTTPS": true}, "8443": {"HTTPS": true}, "9443": {"HTTPS": true}},
            "Web": {
                "desk.example.ts.net:443": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:7317"}}},
                "desk.example.ts.net:8443": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:7878"}}},
                "desk.example.ts.net:9443": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:9000"}}}
            }
        })
    }

    #[test]
    fn other_apps_on_443_and_8443_leave_10000_as_the_suggestion() {
        let status = host_like_this_one();
        assert_eq!(
            serve_slot(&status, 443, OURS),
            ServeSlot::Taken("/ proxy to http://127.0.0.1:7317".into())
        );
        assert!(matches!(
            serve_slot(&status, 8443, OURS),
            ServeSlot::Taken(_)
        ));
        assert_eq!(serve_slot(&status, 10000, OURS), ServeSlot::Free);
        assert_eq!(free_https_port(&status), Some(10000));
        let message = conflict_message(&status, 443, "proxy to http://127.0.0.1:7317");
        assert!(
            message.contains("remote.tailscale.https_port = 10000"),
            "{message}"
        );
        assert_eq!(free_https_port(&json!({})), Some(443));
    }

    #[test]
    fn our_handler_is_recognised_with_the_port_in_its_origin() {
        let mut status = host_like_this_one();
        status["TCP"]["10000"] = json!({"HTTPS": true});
        status["Web"]["desk.example.ts.net:10000"] = json!({"Handlers": {"/": {"Proxy": OURS}}});
        assert_eq!(
            serve_slot(&status, 10000, OURS),
            ServeSlot::Ours("https://desk.example.ts.net:10000".into())
        );
        assert_eq!(free_https_port(&status), Some(10001));
        let root =
            json!({"Web": {"desk.example.ts.net:443": {"Handlers": {"/": {"Proxy": OURS}}}}});
        assert_eq!(
            serve_slot(&root, 443, OURS),
            ServeSlot::Ours("https://desk.example.ts.net".into())
        );
    }

    #[test]
    fn a_port_with_any_other_path_host_or_funnel_is_taken() {
        let other_path_only = json!({
            "TCP": {"10000": {"HTTPS": true}},
            "Web": {"desk.example.ts.net:10000": {"Handlers": {"/other": {"Proxy": "http://127.0.0.1:9"}}}}
        });
        assert_eq!(
            serve_slot(&other_path_only, 10000, OURS),
            ServeSlot::Taken("/other proxy to http://127.0.0.1:9".into())
        );
        assert_eq!(
            free_https_port(
                &json!({"TCP": {"443": {"HTTPS": true}}, "Web": other_path_only["Web"].clone()})
            ),
            Some(8443)
        );
        let beside_ours = json!({
            "Web": {"desk.example.ts.net:10000": {"Handlers": {"/": {"Proxy": OURS}, "/other": {"Path": "/tmp/x"}}}}
        });
        assert_eq!(
            serve_slot(&beside_ours, 10000, OURS),
            ServeSlot::Taken("/other file or text handler".into())
        );
        let two_hosts = json!({"Web": {
            "desk.example.ts.net:10000": {"Handlers": {"/": {"Proxy": OURS}}},
            "svc:other:10000": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:9"}}}
        }});
        assert_eq!(
            serve_slot(&two_hosts, 10000, OURS),
            ServeSlot::Taken("/ proxy to http://127.0.0.1:9".into())
        );
        let funnel = json!({
            "Web": {"desk.example.ts.net:443": {"Handlers": {"/": {"Proxy": OURS}}}},
            "AllowFunnel": {"desk.example.ts.net:443": true}
        });
        assert!(matches!(
            serve_slot(&funnel, 443, OURS),
            ServeSlot::Taken(_)
        ));
    }

    #[test]
    fn tcp_forwards_and_non_proxy_handlers_are_taken() {
        let status = json!({
            "TCP": {"10000": {"TCPForward": "127.0.0.1:22"}},
            "Web": {"desk.example.ts.net:8443": {"Handlers": {"/": {"Path": "/srv"}}}}
        });
        assert!(matches!(
            serve_slot(&status, 10000, OURS),
            ServeSlot::Taken(_)
        ));
        assert!(matches!(
            serve_slot(&status, 8443, OURS),
            ServeSlot::Taken(_)
        ));
        assert_eq!(free_https_port(&status), Some(443));
    }
}
