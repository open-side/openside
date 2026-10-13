//! Runtime-only child environments, excluding daemon-owned credential sources.

use std::{collections::HashSet, ffi::OsString};

use serde_json::Value;

const RUNTIME_NAMES: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "SYSTEMROOT",
    "WINDIR",
    "PATHEXT",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_DIRS",
    "XDG_DATA_DIRS",
    "APPDATA",
    "LOCALAPPDATA",
    "PYTHONPATH",
    "PYTHONHOME",
    "VIRTUAL_ENV",
    "UV_CACHE_DIR",
    "UV_PYTHON",
    "UV_PYTHON_INSTALL_DIR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "TMPDIR",
    "TEMP",
    "TMP",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "OPENSIDE_AGENT_SCRIPT",
    "OPENSIDE_CLAUDE_CODE_VERSION",
    "OPENSIDE_TEST_STDERR",
];

fn credential_names(value: &Value, names: &mut HashSet<String>) {
    if let Some(env) = value.get("env").and_then(Value::as_object) {
        for variable in env.values() {
            if let Some(name) = variable.as_str() {
                names.insert(name.into());
            }
            if let Some(variables) = variable.as_array() {
                for name in variables.iter().filter_map(Value::as_str) {
                    names.insert(name.into());
                }
            }
        }
    }
    if value.get("source").and_then(Value::as_str) == Some("env") {
        if let Some(name) = value.get("env").and_then(Value::as_str) {
            names.insert(name.into());
        }
    }
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(
                    key.as_str(),
                    "token_env" | "key_env" | "api_key_env" | "env_name"
                ) {
                    if let Some(name) = value.as_str() {
                        names.insert(name.into());
                    }
                }
                credential_names(value, names);
            }
        }
        Value::Array(values) => {
            for value in values {
                credential_names(value, names);
            }
        }
        _ => {}
    }
}

/// Capture only runtime variables for sidecar/index children; credential aliases win exclusion.
///
/// Proxy variables are omitted: the current network policy does not authorize an inherited proxy.
/// Socket/nonce and sanitized diagnostic metadata are added explicitly by the trusted launcher.
pub fn runtime_environment(credentials: &Value) -> Vec<(String, String)> {
    runtime_environment_with(credentials, &|name| std::env::var_os(name))
}

fn runtime_environment_with(
    credentials: &Value,
    env: &impl Fn(&str) -> Option<OsString>,
) -> Vec<(String, String)> {
    let mut excluded = HashSet::new();
    credential_names(credentials, &mut excluded);
    RUNTIME_NAMES
        .iter()
        .filter(|name| !excluded.contains(**name))
        .filter_map(|name| {
            env(name)
                .and_then(|value| value.into_string().ok())
                .map(|value| ((*name).into(), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn runtime_allowlist_never_inherits_default_or_custom_credential_variables() {
        let config = json!({"env":{"anthropic":"HOME"},"omp_broker":{"token_env":"CUSTOM_BEARER"}});
        let values = runtime_environment_with(&config, &|name| {
            Some(match name {
                "HOME" | "CUSTOM_BEARER" | "OPENSIDE_BROKER_TOKEN" => {
                    "credential-env-canary-741".into()
                }
                "PATH" => "/runtime/bin".into(),
                _ => "runtime-value".into(),
            })
        });
        assert!(values.iter().all(|(name, value)| name != "HOME"
            && name != "CUSTOM_BEARER"
            && name != "OPENSIDE_BROKER_TOKEN"
            && !value.contains("credential-env-canary-741")));
        assert!(values
            .iter()
            .any(|(name, value)| name == "PATH" && value == "/runtime/bin"));
        assert!(values
            .iter()
            .all(|(name, _)| !matches!(name.as_str(), "HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY")));
    }
}
