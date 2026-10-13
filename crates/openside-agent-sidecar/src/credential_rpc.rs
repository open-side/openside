use openside_core::{
    CredentialError, CredentialErrorCode, CredentialReport, CredentialRequest, HostToolHandler,
    MAX_CREDENTIAL_ATTEMPTS,
};

use crate::protocol::{DaemonMessage, SidecarMessage};

pub(crate) fn correlation(message: &SidecarMessage) -> Option<(&str, &str)> {
    match message {
        SidecarMessage::CredentialRequest { id, request_id, .. }
        | SidecarMessage::CredentialReport { id, request_id, .. }
            if bounded_id(id) && bounded_id(request_id) =>
        {
            Some((id, request_id))
        }
        _ => None,
    }
}

fn bounded_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128
}

fn invalid() -> CredentialError {
    CredentialError {
        code: CredentialErrorCode::InvalidCredentialRequest,
        reason: "Invalid private credential request".into(),
        retry_at_ms: None,
        generation: 0,
    }
}

pub(crate) fn denial(id: String, request_id: String, error: CredentialError) -> DaemonMessage {
    DaemonMessage::CredentialError {
        id,
        request_id,
        code: error.code,
        reason: error.reason,
        retry_at_ms: error.retry_at_ms,
        generation: error.generation,
    }
}

pub(crate) async fn reply(host: &dyn HostToolHandler, message: SidecarMessage) -> DaemonMessage {
    match message {
        SidecarMessage::CredentialRequest {
            id,
            request_id,
            provider,
            model,
            attempt,
            subagent,
        } => {
            if !bounded_id(&id)
                || !bounded_id(&request_id)
                || provider.is_empty()
                || provider.len() > 64
                || model.len() > 256
                || !model
                    .split_once(':')
                    .is_some_and(|(namespace, name)| namespace == provider && !name.is_empty())
                || attempt >= MAX_CREDENTIAL_ATTEMPTS
                || subagent.as_deref().is_some_and(|value| !bounded_id(value))
            {
                return denial(id, request_id, invalid());
            }
            let request = CredentialRequest {
                request_id,
                provider,
                model,
                attempt,
                subagent,
            };
            let expected = request.request_id.clone();
            match host.credentials(request).await {
                Ok(snapshot) if snapshot.request_id == expected => {
                    DaemonMessage::CredentialResponse {
                        id,
                        request_id: snapshot.request_id,
                        lease_id: snapshot.lease_id,
                        generation: snapshot.generation,
                        account: snapshot.account,
                        auth: snapshot.auth,
                    }
                }
                Ok(_) => denial(id, expected, invalid()),
                Err(error) => denial(id, expected, error),
            }
        }
        SidecarMessage::CredentialReport {
            id,
            request_id,
            lease_id,
            outcome,
            stream_started,
            retry_after_ms,
            scope,
            usage,
            usage_windows,
        } => {
            if !bounded_id(&id)
                || !bounded_id(&request_id)
                || !bounded_id(&lease_id)
                || retry_after_ms.is_some_and(|value| value > 86_400_000)
                || usage_windows.len() > 64
                || usage_windows.iter().any(|window| {
                    window.name.is_empty()
                        || window.name.len() > 80
                        || window.used_percent.is_some_and(|value| {
                            !value.is_finite() || !(0.0..=100.0).contains(&value)
                        })
                        || window
                            .reset_at_ms
                            .is_some_and(|value| !value.is_finite() || value < 0.0)
                })
            {
                return denial(id, request_id, invalid());
            }
            let report = CredentialReport {
                request_id,
                lease_id,
                outcome,
                stream_started,
                retry_after_ms,
                scope,
                usage,
                usage_windows,
            };
            let expected_request = report.request_id.clone();
            let expected_lease = report.lease_id.clone();
            match host.credential_report(report).await {
                Ok(ack) if ack.request_id == expected_request && ack.lease_id == expected_lease => {
                    DaemonMessage::CredentialReported {
                        id,
                        request_id: ack.request_id,
                        lease_id: ack.lease_id,
                        generation: ack.generation,
                        action: ack.action,
                        retry_at_ms: ack.retry_at_ms,
                        reason: ack.reason,
                    }
                }
                Ok(_) => denial(id, expected_request, invalid()),
                Err(error) => denial(id, expected_request, error),
            }
        }
        _ => unreachable!("credential transport dispatch is restricted to credential frames"),
    }
}
