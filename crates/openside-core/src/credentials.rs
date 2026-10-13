//! Daemon-owned request credentials; these values never become tools or public API replies.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Bounds one logical request so pool exhaustion cannot create an infinite retry loop.
pub const MAX_CREDENTIAL_ATTEMPTS: u32 = 64;

/// Durable source-local account reference, without upstream identity or credential material.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct ModelAccountRef {
    /// Credential plugin namespace, not an upstream authentication scheme.
    pub source: String,
    /// Opaque durable id; reordering must never change the account it identifies.
    pub account_id: String,
}

/// One attempt's private credential resolution request; session authority is connection-bound.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CredentialRequest {
    /// Stable logical request id shared across its bounded retry sequence.
    pub request_id: String,
    /// Explicit SDK provider namespace.
    pub provider: String,
    /// Provider-qualified model, checked against the session's permitted models.
    pub model: String,
    /// Zero-based outbound attempt, strictly below [`MAX_CREDENTIAL_ATTEMPTS`].
    pub attempt: u32,
    /// Daemon-known child id for applying its account-pool restriction, or the parent.
    pub subagent: Option<String>,
}

/// Request authentication shape; OAuth refresh material has no representation here.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialAuthKind {
    /// Provider-native API-key authentication.
    ApiKey,
    /// Anthropic subscription access token with Claude request shaping.
    ClaudeCode,
    /// Codex subscription access token with its private request account id.
    CodexCli,
    /// Provider-native access bearer without client-side refresh.
    Bearer,
}

/// Ephemeral authentication; Debug is redacted and owned strings are zeroized on drop.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialAuth {
    /// Authentication adapter needed for this selected account.
    pub kind: CredentialAuthKind,
    /// Selected API key or access token, never a refresh token.
    pub value: String,
    /// Private upstream request identity when required; never public account metadata.
    pub upstream_account_id: Option<String>,
    /// Access-token expiry in Unix milliseconds, absent for static API keys.
    pub expires_at_ms: Option<u64>,
}

impl std::fmt::Debug for CredentialAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialAuth")
            .field("kind", &self.kind)
            .field("value", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl Drop for CredentialAuth {
    fn drop(&mut self) {
        self.value.zeroize();
        self.upstream_account_id.zeroize();
    }
}

/// Fresh private authentication snapshot leased to one request attempt.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialSnapshot {
    /// Logical request whose current attempt owns this snapshot.
    pub request_id: String,
    /// Opaque lease binding reports to this session/account/revision.
    pub lease_id: String,
    /// Daemon credential generation observed while selecting.
    pub generation: u64,
    /// Selected durable account reference.
    pub account: ModelAccountRef,
    /// Ephemeral selected authentication, absent from public metadata.
    pub auth: CredentialAuth,
}

/// Sanitized attempt classification; provider bodies and exception text never cross this API.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialOutcome {
    /// Completed upstream operation.
    Success,
    /// Authentication failure eligible for owner recovery or sibling rotation.
    Auth,
    /// Account-scoped or otherwise unclassified rate-limit failure.
    RateLimit,
    /// Classified exhausted usage allowance.
    Quota,
    /// Account/model permission denial, not a credential expiry.
    Policy,
    /// Transport/server/concurrency failure that must not rotate credentials.
    Transient,
    /// Caller cancellation; cleanup may report without awaiting acknowledgement.
    Cancelled,
}

/// Scope in which an attributed account block affects eligibility.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialScope {
    /// Account-wide authentication, allowance or rate-limit state.
    Account,
    /// Only the lease's model is affected by a classified model-specific denial.
    Model,
}

/// Observed nonnegative token counts attributed to the leased account.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CredentialUsage {
    /// Prompt/input tokens reported for this attempt.
    pub input_tokens: u64,
    /// Completion/output tokens reported for this attempt.
    pub output_tokens: u64,
    /// Prompt-cache reads reported for this attempt.
    pub cache_read_tokens: u64,
    /// Prompt-cache writes reported for this attempt.
    pub cache_write_tokens: u64,
}

/// Sanitized provider usage window; missing measurements are never manufactured as zero.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CredentialUsageWindow {
    /// Safe window label without upstream account identity.
    pub name: String,
    /// Inclusive percentage0–100, absent when the provider did not measure usage.
    pub used_percent: Option<f64>,
    /// Provider-reported Unix milliseconds, absent when the reset is unknown.
    pub reset_at_ms: Option<f64>,
}

/// Lease-bound attempt report committed by the daemon before another credential resolution.
#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CredentialReport {
    /// Logical request whose attempt produced this outcome.
    pub request_id: String,
    /// Daemon-issued lease; delayed reports cannot affect a replacement credential.
    pub lease_id: String,
    /// Sanitized outcome, never an upstream response body.
    pub outcome: CredentialOutcome,
    /// Once a stream was yielded, the operation cannot be replayed on another account.
    pub stream_started: bool,
    /// Provider-reported retry duration, absent when no authoritative hint exists.
    pub retry_after_ms: Option<u64>,
    /// Account or model scope for an attributed block.
    pub scope: CredentialScope,
    /// Observed counts, absent when the provider supplied none.
    pub usage: Option<CredentialUsage>,
    /// Sanitized measured windows; unknown windows remain absent/null.
    pub usage_windows: Vec<CredentialUsageWindow>,
}

/// Daemon decision after committing a report; no decision carries credential material.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialAction {
    /// Terminal success/cancellation; no credential resolution follows.
    Complete,
    /// Re-resolve after the owner's single refresh-current recovery.
    Refresh,
    /// Re-resolve a distinct eligible account in the effective explicit pool.
    Rotate,
    /// Fail closed; no credential retry/local-source fallback follows.
    Stop,
}

/// Acknowledgement of committed report effects, before a permitted subsequent resolve.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CredentialReportAck {
    /// Logical request associated with the report.
    pub request_id: String,
    /// Lease whose report is acknowledged.
    pub lease_id: String,
    /// Credential generation after committing report effects.
    pub generation: u64,
    /// Authoritative next action; the sidecar has no separate selector.
    pub action: CredentialAction,
    /// Earliest authoritative Unix-millisecond retry time, or unknown.
    pub retry_at_ms: Option<u64>,
    /// Fixed/sanitized explanation, without provider exception text.
    pub reason: Option<String>,
}

/// Fixed denial classes for private credential resolution/report validation.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialErrorCode {
    /// No eligible usable credential under the current explicit trust boundary.
    AuthUnavailable,
    /// Authorized accounts are blocked until an authoritative retry/reset.
    AccountBlocked,
    /// Requested account is outside the effective provider/Agent pool.
    AccountNotInPool,
    /// The selected provider/source is unavailable and cannot implicitly downgrade.
    ProviderUnavailable,
    /// Invalid model/session/lease/attempt relationship.
    InvalidCredentialRequest,
}

/// Safe private-RPC denial; it contains no provider response, identity or token.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CredentialError {
    /// Fixed machine-readable denial class.
    pub code: CredentialErrorCode,
    /// Fixed/sanitized explanation, never provider exception text.
    pub reason: String,
    /// Authoritative Unix-millisecond retry time, absent when unknown.
    pub retry_at_ms: Option<u64>,
    /// Daemon credential generation observed while denying.
    pub generation: u64,
}

impl CredentialError {
    /// Fail closed for a host that has no configured credential policy owner.
    pub fn unavailable() -> Self {
        Self {
            code: CredentialErrorCode::AuthUnavailable,
            reason: "No daemon credential policy is configured".into(),
            retry_at_ms: None,
            generation: 0,
        }
    }
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for CredentialError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HostToolHandler, ToolCall, ToolOutcome};
    use std::{
        future::Future,
        pin::Pin,
        sync::Arc,
        task::{Context, Poll, Wake, Waker},
    };

    struct Host;
    impl HostToolHandler for Host {
        fn call(&self, _: ToolCall) -> Pin<Box<dyn Future<Output = ToolOutcome> + Send + '_>> {
            Box::pin(async { panic!("credential callbacks must not invoke model tools") })
        }
    }
    struct Wakeup;
    impl Wake for Wakeup {
        fn wake(self: Arc<Self>) {}
    }

    #[test]
    fn private_credential_snapshot_debug_redacts_tokens_and_upstream_identity() {
        let snapshot = CredentialSnapshot {
            request_id: "request".into(),
            lease_id: "lease".into(),
            generation: 1,
            account: ModelAccountRef {
                source: "codex-cli".into(),
                account_id: "opaque".into(),
            },
            auth: CredentialAuth {
                kind: CredentialAuthKind::CodexCli,
                value: "private-access-canary-1742".into(),
                upstream_account_id: Some("private-upstream-identity-841".into()),
                expires_at_ms: Some(1),
            },
        };
        let text = format!("{snapshot:?}");
        assert!(!text.contains("private-access-canary-1742"));
        assert!(!text.contains("private-upstream-identity-841"));
        assert!(text.contains("[REDACTED]"));
    }

    #[test]
    fn unconfigured_host_credential_callbacks_fail_closed_without_tool_calls() {
        let waker = Waker::from(Arc::new(Wakeup));
        let mut context = Context::from_waker(&waker);
        let host = Host;
        let mut resolve = host.credentials(CredentialRequest {
            request_id: "request".into(),
            provider: "anthropic".into(),
            model: "anthropic:model".into(),
            attempt: 0,
            subagent: None,
        });
        assert!(matches!(
            resolve.as_mut().poll(&mut context),
            Poll::Ready(Err(CredentialError {
                code: CredentialErrorCode::AuthUnavailable,
                ..
            }))
        ));
        let mut report = host.credential_report(CredentialReport {
            request_id: "request".into(),
            lease_id: "lease".into(),
            outcome: CredentialOutcome::Success,
            stream_started: false,
            retry_after_ms: None,
            scope: CredentialScope::Account,
            usage: None,
            usage_windows: vec![],
        });
        assert!(matches!(
            report.as_mut().poll(&mut context),
            Poll::Ready(Err(CredentialError {
                code: CredentialErrorCode::AuthUnavailable,
                ..
            }))
        ));
    }
}
