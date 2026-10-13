//! Live daemon policy (D-1, D-2, PP-12): `GET`/`PUT /v1/policy`, `POST /v1/policy/test`.
//!
//! The live document holds the default session mode, origin allow/deny lists and the
//! agent-level rule layer. Each tool call composes it with the session's own mode, approved
//! workspace and session rules (see [`effective`]), so a `PUT` applies to the next call of
//! every session without restarting sidecars. `PUT` persists `<data_dir>/policy.json`
//! (0600, atomic rename); when present at startup it replaces `[policy]` from config.toml.
//! An unreadable or invalid file stops startup instead of silently widening access.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{extract::State, http::StatusCode, Extension, Json};
use openside_policy::{Action, Capability, Effect, OriginLists, Policy, Rule, RuleLayer};
use openside_proto::{
    PolicyCapability, PolicyDecision, PolicyEffect, PolicyMode, PolicySettings, PolicyTestRequest,
};
use parking_lot::RwLock;

use crate::{api::HttpError, config::Resolved, engine::Shared, persistence::Result};

const FILE: &str = openside_config::data_layout::DataPath::Policy.relative();
const MAX_ENTRIES: usize = 512;
const MAX_PATTERN: usize = 1024;

pub(crate) struct LivePolicy {
    live: RwLock<Live>,
    path: PathBuf,
    // One PUT at a time owns persist-then-publish, so disk and live state never diverge.
    pub(crate) writer: tokio::sync::Mutex<()>,
}

/// The document as served by `GET` plus its origin lists compiled once per `PUT`.
struct Live {
    settings: PolicySettings,
    origins: OriginLists,
}

impl LivePolicy {
    pub(crate) fn load(config: &Resolved) -> Result<Self> {
        let path = openside_config::data_layout::DataPath::Policy.path(&config.data_dir);
        let settings = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<PolicySettings>(&bytes)
                .map_err(|_| format!("{} is not a valid policy document", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let policy = &config.loaded.config.policy;
                PolicySettings {
                    default_mode: config.policy_mode(),
                    allow_origins: policy.allow_origins.clone(),
                    deny_origins: policy.deny_origins.clone(),
                    rules: Vec::new(),
                }
            }
            Err(error) => return Err(error.into()),
        };
        let origins =
            validate(&settings).map_err(|reason| format!("{}: {reason}", path.display()))?;
        Ok(Self {
            live: RwLock::new(Live { settings, origins }),
            path,
            writer: tokio::sync::Mutex::new(()),
        })
    }

    pub(crate) fn default_mode(&self) -> PolicyMode {
        self.live.read().settings.default_mode
    }

    /// Live decision for a browser navigation to `origin`, used by the browser controller for
    /// every top-frame request of agent tabs (redirects and popups included). Sessions add no
    /// rule layers and `Browse` defaults to Allow in every mode, so the session does not enter.
    pub(crate) fn browse(&self, origin: &str) -> Effect {
        effective_with(&self.live.read(), &Policy::default())
            .decide(&Action::new(Capability::Browse, origin))
            .effect
    }
}

/// Session policy as evaluated now: live agent rules beneath the session's layers, live
/// origin lists, and the session's own mode and approved roots.
pub(crate) fn effective(shared: &Shared, session: &Policy) -> Policy {
    effective_with(&shared.policy.live.read(), session)
}

fn effective_with(live: &Live, session: &Policy) -> Policy {
    let mut layers = vec![RuleLayer {
        rules: live.settings.rules.iter().map(rule).collect(),
    }];
    layers.extend(session.layers.iter().cloned());
    Policy {
        mode: session.mode,
        approved_roots: session.approved_roots.clone(),
        layers,
        origins: live.origins.clone(),
    }
}

fn rule(rule: &openside_proto::PolicyRule) -> Rule {
    Rule::new(
        rule.effect.into(),
        rule.capability.into(),
        rule.pattern.clone(),
    )
}

/// Checks limits and rule shapes and compiles the origin lists; origin entries go through the
/// same canonicalization as browser targets ([`openside_policy::OriginPattern`]), so an entry
/// that could never match is rejected instead of silently restricting nothing.
fn validate(settings: &PolicySettings) -> std::result::Result<OriginLists, String> {
    if settings.rules.len() + settings.allow_origins.len() + settings.deny_origins.len()
        > MAX_ENTRIES
    {
        return Err("too many rules or origins".into());
    }
    if settings
        .allow_origins
        .iter()
        .chain(&settings.deny_origins)
        .any(|origin| origin.len() > MAX_PATTERN)
    {
        return Err("origin pattern is too long".into());
    }
    for rule in &settings.rules {
        if rule.pattern.is_empty() || rule.pattern.len() > MAX_PATTERN {
            return Err("rule patterns must be 1..=1024 characters".into());
        }
        // Sensitive actions always reach a human; an allow/ask rule would only mislead.
        if rule.capability == PolicyCapability::Sensitive && rule.effect != PolicyEffect::Deny {
            return Err("sensitive rules may only deny".into());
        }
    }
    OriginLists::new(&settings.allow_origins, &settings.deny_origins)
}

pub(crate) async fn get(State(shared): State<Shared>) -> Json<PolicySettings> {
    Json(shared.policy.live.read().settings.clone())
}

pub(crate) async fn put(
    State(shared): State<Shared>,
    Extension(local): Extension<crate::remote::LocalAuthority>,
    Json(settings): Json<PolicySettings>,
) -> std::result::Result<Json<PolicySettings>, HttpError> {
    local.require()?;
    let policy = Arc::clone(&shared.policy);
    // The whole persist-then-publish transaction runs in its own task: a client that drops the
    // request cannot release the writer lock while the blocking worker is still renaming, nor
    // leave a document on disk that the live policy never adopted.
    tokio::spawn(async move {
        let _writer = policy.writer.lock().await;
        replace_locked(&policy, settings).await.map(Json)
    })
    .await
    .map_err(|_| HttpError::internal())?
}

/// Validate, persist and publish `settings`. The caller holds `policy.writer` (a `PUT`, or a
/// sync run that keeps policy writers out for its whole duration).
pub(crate) async fn replace_locked(
    policy: &Arc<LivePolicy>,
    settings: PolicySettings,
) -> std::result::Result<PolicySettings, HttpError> {
    let origins =
        validate(&settings).map_err(|_| HttpError(StatusCode::BAD_REQUEST, "invalid_policy"))?;
    let bytes = serde_json::to_vec_pretty(&settings).map_err(|_| HttpError::internal())?;
    let path = policy.path.clone();
    tokio::task::spawn_blocking(move || persist(&path, &bytes))
        .await
        .map_err(|_| HttpError::internal())?
        .map_err(|_| HttpError::internal())?;
    *policy.live.write() = Live {
        settings: settings.clone(),
        origins,
    };
    Ok(settings)
}

/// The live document, for sync snapshots.
pub(crate) fn current(policy: &LivePolicy) -> PolicySettings {
    policy.live.read().settings.clone()
}

pub(crate) async fn test(
    State(shared): State<Shared>,
    Json(request): Json<PolicyTestRequest>,
) -> std::result::Result<Json<PolicyDecision>, HttpError> {
    let workspace = match request.workspace {
        Some(path) if Path::new(&path).is_absolute() => Some(PathBuf::from(path)),
        Some(_) => return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_workspace")),
        None => None,
    };
    let mode = request.mode.unwrap_or_else(|| shared.policy.default_mode());
    let session = Policy {
        mode: mode.into(),
        approved_roots: workspace.into_iter().collect(),
        layers: Vec::new(),
        origins: OriginLists::default(),
    };
    let decision = effective(&shared, &session)
        .decide(&Action::new(request.capability.into(), request.target));
    Ok(Json(decision.into()))
}

pub(crate) async fn validate_document(
    Json(request): Json<openside_proto::ValidatePolicyRequest>,
) -> Json<openside_proto::PolicyValidation> {
    use openside_proto::{PolicyList, PolicyProblem};
    let mut problems = Vec::new();
    if let Some(policy) = request.policy {
        for (list, entries) in [
            (PolicyList::AllowOrigins, &policy.allow_origins),
            (PolicyList::DenyOrigins, &policy.deny_origins),
        ] {
            for (index, entry) in entries.iter().enumerate() {
                if entry.len() > MAX_PATTERN
                    || OriginLists::new(&[entry.as_str()], &[] as &[&str]).is_err()
                {
                    problems.push(PolicyProblem {
                        list,
                        index: index as u32,
                        message: "Enter a valid origin pattern.".into(),
                    });
                }
            }
        }
        for (index, rule) in policy.rules.iter().enumerate() {
            if rule.pattern.is_empty()
                || rule.pattern.len() > MAX_PATTERN
                || (rule.capability == PolicyCapability::Sensitive
                    && rule.effect != PolicyEffect::Deny)
            {
                problems.push(PolicyProblem { list: PolicyList::Rules, index: index as u32, message: "Use a non-empty pattern up to 1024 characters; sensitive rules may only deny.".into() });
            }
        }
        if policy.rules.len() + policy.allow_origins.len() + policy.deny_origins.len() > MAX_ENTRIES
        {
            problems.push(PolicyProblem {
                list: PolicyList::Rules,
                index: 0,
                message: "Use at most 512 rules and origins in total.".into(),
            });
        }
    }
    if let Some(allow) = request.network_allow {
        for (index, entry) in allow.iter().enumerate() {
            if openside_policy::Egress::new([entry.as_str()]).is_err() {
                problems.push(PolicyProblem {
                    list: PolicyList::NetworkAllow,
                    index: index as u32,
                    message: "Enter a host, wildcard domain, host:port, or HTTP(S) origin.".into(),
                });
            }
        }
    }
    Json(openside_proto::PolicyValidation {
        valid: problems.is_empty(),
        problems,
    })
}

pub(crate) async fn network(
    State(s): State<Shared>,
) -> std::result::Result<Json<openside_proto::NetworkPolicy>, HttpError> {
    let allow = crate::settings_api::stored(&s).await?.network.allow;
    let restart_required = allow != s.config.loaded.config.network.allow;
    Ok(Json(openside_proto::NetworkPolicy {
        allow,
        restart_required,
    }))
}

pub(crate) async fn put_network(
    State(s): State<Shared>,
    Extension(local): Extension<crate::remote::LocalAuthority>,
    Json(request): Json<openside_proto::UpdateNetworkPolicyRequest>,
) -> std::result::Result<Json<openside_proto::NetworkPolicy>, HttpError> {
    local.require()?;
    openside_policy::Egress::new(request.allow.iter().map(String::as_str))
        .map_err(|_| HttpError(StatusCode::BAD_REQUEST, "invalid_network_allow"))?;
    tokio::spawn(async move {
        let _writer = crate::settings_api::WRITE.lock().await;
        let mut config = crate::settings_api::stored(&s).await?;
        config.network.allow = request.allow;
        crate::settings_api::persist(
            &s,
            serde_json::to_value(config).map_err(|_| HttpError::internal())?,
        )
        .await?;
        crate::pairing::audit(&s, "network_policy", "written", serde_json::json!({})).await;
        network(State(s)).await
    })
    .await
    .map_err(|_| HttpError::internal())?
}

/// Atomically replaces `path`. `Ok` means the rename happened, so the new document is what a
/// restart reads; a failure before it leaves the old file untouched.
fn persist(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    // Unique create_new temp: a stale or concurrent temp can never be written through.
    let temporary = parent.join(format!(".{FILE}.{}.tmp", crate::persistence::random_id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    let written = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return written;
    }
    // The rename is already visible; failing here would leave live=old and disk=new, so the
    // PUT still succeeds and only crash-durability of the directory entry is unconfirmed.
    let _ = std::fs::File::open(parent).and_then(|dir| dir.sync_all());
    Ok(())
}
