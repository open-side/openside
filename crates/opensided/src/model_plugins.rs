//! Credential-source plugins (`openside_config::PluginSource`): relays described by local
//! manifests that are reachable only from some networks. While a manifest's probe answers,
//! each provider it serves gets one `plugin:<id>` account that joins that provider's pool as
//! an extra, unpersisted source; when the probe fails the account silently drops out. The
//! source has no Settings surface; `openside doctor` reports counts only.

use crate::{
    engine::Shared,
    model_accounts::{Account, Inventory},
};
use openside_config::{PluginAuth, PluginOutcome, PluginSource};
use openside_core::{
    CredentialAuth, CredentialAuthKind, CredentialErrorRule, CredentialErrorRules,
    CredentialRuleOutcome, ModelAccountRef, SessionId,
};
use openside_proto::ProviderPool;
use std::{
    collections::{BTreeMap, HashMap},
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

pub(crate) const SOURCE: &str = "plugin";
const RELOAD: Duration = Duration::from_secs(10);
const RETRY_FAILED: Duration = Duration::from_secs(10);
const ROUTE_CHECK: Duration = Duration::from_secs(2);
const RESOLVE_TIMEOUT: Duration = Duration::from_millis(300);

struct Probe {
    at: Instant,
    ok: bool,
    revision: u64,
    route: Option<IpAddr>,
    route_at: Instant,
}

#[derive(Default)]
struct State {
    loaded_at: Option<Instant>,
    sources: Vec<Arc<PluginSource>>,
    probes: HashMap<String, Probe>,
}

/// Loaded manifests and their latest availability probes.
#[derive(Default)]
pub(crate) struct Cache(Mutex<State>);

fn revision(source: &PluginSource) -> u64 {
    crate::model_accounts::revision(&format!("{}\n{}", source.url, source.identity))
}

/// The local address this host would use to reach the relay. It changes when the machine
/// joins or leaves a network (office LAN, tailnet), which re-probes at once instead of
/// waiting for the interval. No packet is sent.
async fn route(origin: &str) -> Option<IpAddr> {
    let url = url::Url::parse(origin).ok()?;
    let port = url.port_or_known_default()?;
    let target: SocketAddr = match url.host()? {
        url::Host::Ipv4(ip) => (ip, port).into(),
        url::Host::Ipv6(ip) => (ip, port).into(),
        url::Host::Domain(name) => {
            tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host((name, port)))
                .await
                .ok()?
                .ok()?
                .next()?
        }
    };
    let socket = std::net::UdpSocket::bind(if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })
    .ok()?;
    socket.connect(target).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

async fn probe(source: &PluginSource) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_millis(source.probe.timeout_ms))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
    else {
        return false;
    };
    let mut request = client.get(source.probe_url());
    if let Some(header) = &source.probe.identity_header {
        request = request.header(header.as_str(), source.identity.as_str());
    }
    request
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

/// Manifests whose probe currently succeeds, re-reading the directory and re-probing as due.
async fn available(s: &Shared) -> Vec<Arc<PluginSource>> {
    let mut state = s.models.plugins.0.lock().await;
    if state.loaded_at.map_or(true, |at| at.elapsed() >= RELOAD) {
        let dir = openside_config::plugin_dir(&s.config.loaded.paths.config_file);
        let load = openside_config::load_plugin_sources(&dir);
        state.sources = load.sources.into_iter().map(Arc::new).collect();
        state.loaded_at = Some(Instant::now());
        let ids: Vec<String> = state.sources.iter().map(|s| s.id.clone()).collect();
        state.probes.retain(|id, _| ids.contains(id));
    }
    let mut due = Vec::new();
    for source in state.sources.clone() {
        let current = revision(&source);
        let previous = state.probes.get(&source.id);
        let mut route_now = previous.map(|p| p.route);
        if previous.map_or(true, |p| p.route_at.elapsed() >= ROUTE_CHECK) {
            route_now = Some(route(&source.url).await);
        }
        let stale = previous.map_or(true, |p| {
            p.revision != current
                || Some(p.route) != route_now
                || p.at.elapsed()
                    >= if p.ok {
                        Duration::from_secs(source.probe.interval_seconds)
                    } else {
                        RETRY_FAILED
                    }
        });
        if stale {
            due.push((source, current, route_now.flatten()));
        } else if let (Some(probe), Some(route)) = (state.probes.get_mut(&source.id), route_now) {
            probe.route = route;
            probe.route_at = Instant::now();
        }
    }
    let mut probes = tokio::task::JoinSet::new();
    for (source, current, route) in due {
        probes.spawn(async move { (source.id.clone(), probe(&source).await, current, route) });
    }
    while let Some(Ok((id, ok, revision, route))) = probes.join_next().await {
        let now = Instant::now();
        state.probes.insert(
            id,
            Probe {
                at: now,
                ok,
                revision,
                route,
                route_at: now,
            },
        );
    }
    state
        .sources
        .iter()
        .filter(|source| state.probes.get(&source.id).is_some_and(|probe| probe.ok))
        .cloned()
        .collect()
}

async fn find(s: &Shared, id: &str) -> Option<Arc<PluginSource>> {
    available(s)
        .await
        .into_iter()
        .find(|source| source.id == id)
}

/// One account per provider of each reachable manifest.
pub(crate) async fn accounts(s: &Shared) -> Vec<Account> {
    let mut accounts = Vec::new();
    for source in available(s).await {
        for provider in source.providers.keys() {
            accounts.push(Account {
                provider: provider.clone(),
                reference: ModelAccountRef {
                    source: SOURCE.into(),
                    account_id: source.id.clone(),
                },
                label: format!("Plugin source {}", source.id),
                enabled: true,
                ready: true,
                reason: "Ready".into(),
                models: vec![],
                reserve: 10,
                windows: vec![],
                blocked_until: None,
                model_blocks: vec![],
                revision: revision(&source),
            });
        }
    }
    accounts
}

fn rules(source: &PluginSource) -> Option<CredentialErrorRules> {
    (!source.errors.rules.is_empty()).then(|| CredentialErrorRules {
        hint_header: source.errors.hint_header.clone(),
        rules: source
            .errors
            .rules
            .iter()
            .map(|rule| CredentialErrorRule {
                status: rule.status,
                hint: rule.hint.clone(),
                body_contains: rule.body_contains.clone(),
                outcome: match rule.outcome {
                    PluginOutcome::UnsupportedClient => CredentialRuleOutcome::UnsupportedClient,
                    PluginOutcome::SignIn => CredentialRuleOutcome::SignIn,
                    PluginOutcome::Capacity => CredentialRuleOutcome::Capacity,
                    PluginOutcome::RateLimit => CredentialRuleOutcome::RateLimit,
                },
                message: rule.message.clone(),
            })
            .collect(),
    })
}

fn expand(template: &str, session: &SessionId, subagent: Option<&str>, limit: usize) -> String {
    let clean = |value: &str| -> String {
        value
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "._:-".contains(c) {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    };
    let mut value = template.replace("{session}", &clean(session.as_str()));
    value = value.replace("{subagent}", &clean(subagent.unwrap_or("")));
    value.truncate(limit.min(value.len()));
    value
}

/// The lease for `provider` through manifest `id`: the identity as the key, the manifest's
/// endpoint as private `base_url`, its per-session headers and its refusal rules.
pub(crate) async fn resolve(
    s: &Shared,
    id: &str,
    provider: &str,
    session: &SessionId,
    subagent: Option<&str>,
) -> Option<(CredentialAuth, u64)> {
    let source = find(s, id).await?;
    let route = source.providers.get(provider)?;
    let kind = match route.auth {
        PluginAuth::ApiKey => CredentialAuthKind::ApiKey,
        PluginAuth::ClaudeCode => CredentialAuthKind::ClaudeCode,
        PluginAuth::CodexCli => CredentialAuthKind::CodexCli,
        PluginAuth::Bearer => CredentialAuthKind::Bearer,
    };
    let headers: BTreeMap<String, String> = source
        .session_headers
        .iter()
        .map(|header| {
            (
                header.name.clone(),
                expand(&header.template, session, subagent, header.max_length),
            )
        })
        .collect();
    Some((
        CredentialAuth {
            kind,
            value: source.identity.clone(),
            upstream_account_id: None,
            expires_at_ms: None,
            base_url: source.base_url(provider),
            headers: (!headers.is_empty()).then_some(headers),
            error_rules: rules(&source),
            request_metadata: None,
        },
        revision(&source),
    ))
}

/// `configured` plus the provider's reachable plugin accounts at the end, unpersisted. An
/// explicitly empty pool stays empty (routing disabled); a provider without a pool gets one
/// of plugin accounts only.
pub(crate) fn effective(
    configured: Option<&ProviderPool>,
    inventory: &Inventory,
    provider: &str,
) -> Option<ProviderPool> {
    let mut pool = configured.cloned();
    if pool.as_ref().is_some_and(|pool| pool.accounts.is_empty()) {
        return pool;
    }
    for account in &inventory.accounts {
        if account.provider != provider || account.reference.source != SOURCE {
            continue;
        }
        let pool = pool.get_or_insert_with(|| ProviderPool {
            provider: provider.into(),
            accounts: vec![],
        });
        if !pool.accounts.contains(&account.reference) {
            pool.accounts.push(account.reference.clone());
        }
    }
    pool
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_header_values_are_bounded_and_sanitized() {
        let value = expand(
            "openside:{session}:{subagent}",
            &SessionId::new("s 1/é".repeat(20)),
            Some("child"),
            64,
        );
        assert!(value.starts_with("openside:s-1-"));
        assert!(value.len() <= 64);
        assert!(value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b)));
    }
}
