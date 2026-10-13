"""Private daemon RPC: the connection, not a frame field, owns session authority."""

from collections.abc import Awaitable, Callable
from uuid import uuid4

from pydantic import SecretStr
from pydantic_ai.usage import RequestUsage

from openside_agent.protocol import (
    CredentialError,
    CredentialReport,
    CredentialReported,
    CredentialRequest,
    CredentialResponse,
    CredentialUsage,
)

from .errors import AuthUnavailable
from .outcomes import ClassifiedOutcome
from .request_auth import CredentialAttempt
from .request_model import CredentialLease
from .secret import CredentialSecret

type CredentialReply = CredentialResponse | CredentialError | CredentialReported
type CredentialExchange = Callable[
    [CredentialRequest | CredentialReport, bool], Awaitable[CredentialReply | None]
]
SAFE_ERRORS = {
    "auth_unavailable": "No eligible account in the configured pool",
    "account_blocked": "Configured account pool is currently blocked",
    "account_not_in_pool": "Account is not in the effective configured pool",
    "provider_unavailable": "Configured credential provider is unavailable",
    "invalid_credential_request": "Daemon rejected the credential request",
}


class DaemonCredentialRpc:
    def __init__(self, exchange: CredentialExchange) -> None:
        self.exchange = exchange

    async def _exchange(
        self, message: CredentialRequest | CredentialReport, *, wait: bool = True
    ) -> CredentialReply | None:
        try:
            reply = await self.exchange(message, wait)
        except AuthUnavailable:
            raise
        except Exception:
            raise AuthUnavailable(
                "daemon",
                ["Private credential RPC unavailable"],
                reason="provider_unavailable",
            ) from None
        if reply is not None and (
            reply.id != message.id or reply.request_id != message.request_id
        ):
            if isinstance(reply, CredentialResponse):
                reply.auth.value = SecretStr("")
            raise AuthUnavailable(
                "daemon",
                ["Private credential response correlation mismatch"],
                reason="invalid_credential_request",
            ) from None
        if isinstance(message, CredentialReport) and isinstance(
            reply, CredentialResponse
        ):
            reply.auth.value = SecretStr("")
            reply.auth.upstream_account_id = None
            raise AuthUnavailable(
                "daemon",
                ["Invalid private credential acknowledgement"],
                reason="invalid_credential_request",
            ) from None
        if isinstance(reply, CredentialError):
            provider = (
                message.provider if isinstance(message, CredentialRequest) else "daemon"
            )
            raise AuthUnavailable(
                provider,
                [SAFE_ERRORS[reply.code]],
                reason=reply.code,
                retry_at_ms=reply.retry_at_ms,
            ) from None
        return reply

    async def acquire(
        self,
        provider: str,
        model: str,
        request_id: str,
        attempt: int,
        subagent: str | None,
    ) -> CredentialLease:
        reply = await self._exchange(
            CredentialRequest(
                id=str(uuid4()),
                request_id=request_id,
                provider=provider,
                model=model,
                attempt=attempt,
                subagent=subagent,
            )
        )
        if not isinstance(reply, CredentialResponse):
            raise AuthUnavailable(
                provider,
                ["Invalid private credential response"],
                reason="invalid_credential_request",
            )
        try:
            secret = CredentialSecret(reply.auth.value.get_secret_value())
        finally:
            reply.auth.value = SecretStr("")
        credential = CredentialAttempt(
            provider,
            reply.account.source,
            reply.auth.kind,
            secret,
            reply.auth.upstream_account_id,
        )
        reply.auth.upstream_account_id = None
        return CredentialLease(reply.lease_id, reply.account.account_id, credential)

    async def report(
        self,
        lease: CredentialLease,
        outcome: ClassifiedOutcome,
        request_id: str,
        *,
        stream_started: bool = False,
        usage: RequestUsage | None = None,
        wait: bool = True,
    ) -> bool:
        counts = (
            None
            if usage is None or not usage.has_values()
            else CredentialUsage(
                input_tokens=usage.input_tokens,
                output_tokens=usage.output_tokens,
                cache_read_tokens=usage.cache_read_tokens,
                cache_write_tokens=usage.cache_write_tokens,
            )
        )
        reply = await self._exchange(
            CredentialReport(
                id=str(uuid4()),
                request_id=request_id,
                lease_id=lease.lease_id,
                outcome=outcome.kind,
                stream_started=stream_started,
                retry_after_ms=outcome.retry_after_ms,
                scope=outcome.scope,
                usage=counts,
                usage_windows=[],
            ),
            wait=wait,
        )
        if not wait:
            return False
        if (
            not isinstance(reply, CredentialReported)
            or reply.lease_id != lease.lease_id
        ):
            raise AuthUnavailable(
                lease.credential.provider,
                ["Invalid private credential acknowledgement"],
                reason="invalid_credential_request",
            ) from None
        if reply.action == "stop" and outcome.kind in (
            "auth",
            "rate_limit",
            "quota",
            "policy",
        ):
            raise AuthUnavailable(
                lease.credential.provider,
                ["No eligible account in the configured pool"],
                reason="auth_unavailable",
                retry_at_ms=reply.retry_at_ms,
            ) from None
        return not stream_started and reply.action in ("refresh", "rotate")
