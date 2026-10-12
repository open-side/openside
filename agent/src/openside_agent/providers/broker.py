"""Trusted-client broker HTTP API; no credential persistence or OAuth refresh.

Route evidence (upstream commit eec114a9226da698ebc015fd8996a8ca3bc4625c):
https://github.com/can1357/oh-my-pi/blob/eec114a9226da698ebc015fd8996a8ca3bc4625c/docs/auth-broker-gateway.md
§ auth-broker / Endpoints confirms GET /v1/snapshot, GET /v1/snapshot/stream,
GET /v1/usage and POST /v1/credential/:id/block. packages/ai/src/auth-broker/
types.ts defines SnapshotResponse, SnapshotStreamEvent, UsageResponse and
CredentialBlockRequest {providerKey, blockScope, blockedUntilMs, updatedAtMs?}.
wire-schemas.ts validates those bodies. No block route is inferred.

The loopback URL and home-relative token path below are optional omp-user
candidates, not requirements. Tokens and response bodies never enter errors.
"""

import asyncio
import json
import os
import re
import time
from collections.abc import AsyncIterator, Mapping
from pathlib import Path
from typing import Any, cast
from urllib.parse import urlsplit

import httpx
from pydantic import BaseModel, Field, SecretStr, ValidationError, model_validator

from .config import number, section
from .errors import AuthUnavailable

OMP_DEFAULT_URL = "http://127.0.0.1:8765"
MAX_BROKER_BYTES = 4 * 1024 * 1024
MAX_BROKER_CREDENTIALS = 512


def response_json(response: httpx.Response) -> Any:
    response.raise_for_status()
    body = bytearray()
    for chunk in response.iter_bytes(chunk_size=8192):
        if len(body) + len(chunk) > MAX_BROKER_BYTES:
            raise ValueError("Broker response exceeds metadata limit")
        body.extend(chunk)
    return json.loads(body)


async def sse_payloads(response: httpx.Response) -> AsyncIterator[str]:
    pending = bytearray()
    lines: list[bytes] = []
    event_size = 0
    async for chunk in response.aiter_bytes(chunk_size=8192):
        pending.extend(chunk)
        while (newline := pending.find(b"\n")) >= 0:
            line = bytes(pending[:newline]).removesuffix(b"\r")
            del pending[: newline + 1]
            event_size += len(line) + 1
            if event_size > MAX_BROKER_BYTES:
                raise ValueError("Broker stream event exceeds metadata limit")
            if line.startswith(b"data:"):
                lines.append(line[5:].lstrip())
            elif not line:
                if lines:
                    yield b"\n".join(lines).decode("utf-8")
                lines.clear()
                event_size = 0
        if len(pending) + event_size > MAX_BROKER_BYTES:
            raise ValueError("Broker stream line exceeds metadata limit")


class OAuthCredential(BaseModel):
    type: str
    access: SecretStr = Field(repr=False)
    expires: float
    account_id: str | None = Field(default=None, alias="accountId", repr=False)
    email: str | None = Field(default=None, repr=False)
    org_id: str | None = Field(default=None, alias="orgId", repr=False)


class CredentialBlock(BaseModel):
    provider_key: str = Field(alias="providerKey")
    block_scope: str = Field(alias="blockScope")
    blocked_until_ms: float = Field(alias="blockedUntilMs")


class BrokerCredential(BaseModel):
    id: int
    provider: str
    credential: OAuthCredential = Field(repr=False)
    blocks: list[CredentialBlock] = Field(default_factory=list[CredentialBlock])


class Snapshot(BaseModel):
    generation: int
    server_now_ms: float = Field(alias="serverNowMs")
    credentials: list[BrokerCredential]

    @model_validator(mode="after")
    def bounded(self) -> "Snapshot":
        if len(self.credentials) > MAX_BROKER_CREDENTIALS:
            raise ValueError("Broker credential count exceeds metadata limit")
        size = 0
        for entry in self.credentials:
            credential = entry.credential
            values = [
                entry.provider,
                credential.type,
                credential.access.get_secret_value(),
                credential.account_id or "",
                credential.email or "",
                credential.org_id or "",
            ]
            size += 1024 + sum(len(value.encode("utf-8")) for value in values)
            for block in entry.blocks:
                size += (
                    128
                    + len(block.provider_key.encode("utf-8"))
                    + len(block.block_scope.encode("utf-8"))
                )
            if size > MAX_BROKER_BYTES:
                raise ValueError("Broker snapshot exceeds metadata limit")
        return self

    @classmethod
    def parse(cls, data: Any) -> "Snapshot":
        # API-key and refresh payloads are deliberately not retained by this source.
        if not isinstance(data, dict):
            raise ValueError("Invalid snapshot")
        payload = cast(dict[str, Any], data)
        if not isinstance(payload.get("credentials"), list):
            raise ValueError("Invalid snapshot")
        if len(payload["credentials"]) > MAX_BROKER_CREDENTIALS:
            raise ValueError("Broker credential count exceeds metadata limit")
        entries: list[dict[str, Any]] = []
        for raw in cast(list[Any], payload["credentials"]):
            if not isinstance(raw, dict):
                raise ValueError("Invalid snapshot")
            entry = cast(dict[str, Any], raw)
            credential = entry.get("credential")
            if not isinstance(credential, dict):
                raise ValueError("Invalid snapshot")
            if cast(dict[str, Any], credential).get("type") not in ("oauth", "api_key"):
                raise ValueError("Invalid snapshot credential kind")
            if cast(dict[str, Any], credential).get("type") == "oauth":
                entries.append(entry)
        return cls.model_validate({**payload, "credentials": entries})


def broker_settings(config: Mapping[str, Any]) -> tuple[str, SecretStr | None, str]:
    settings = section(config, "omp_broker")
    url = os.getenv("OPENSIDE_BROKER_URL") or settings.get("url") or OMP_DEFAULT_URL
    if not isinstance(url, str):
        return "", None, "invalid broker URL configuration"
    parsed = urlsplit(url)
    if parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.username:
        return "", None, "invalid broker URL configuration"
    token = os.getenv("OPENSIDE_BROKER_TOKEN", "").strip()
    token_env = settings.get("token_env")
    if token_env is not None:
        if (
            not isinstance(token_env, str)
            or len(token_env) > 128
            or re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", token_env) is None
        ):
            return (
                url.rstrip("/"),
                None,
                "invalid broker token environment configuration",
            )
        if not token:
            token = os.getenv(token_env, "").strip()
    if not token:
        configured = settings.get("token_file")
        path = (
            Path(configured).expanduser()
            if isinstance(configured, str)
            else Path.home() / ".omp" / "auth-broker.token"
        )
        try:
            token = path.read_text().strip()
        except (OSError, UnicodeError):
            return url.rstrip("/"), None, "broker token file unavailable"
    if not token:
        return url.rstrip("/"), None, "broker token unavailable"
    return url.rstrip("/"), SecretStr(token), "broker token configured"


class BrokerClient:
    def __init__(
        self,
        url: str,
        token: SecretStr | str,
        *,
        snapshot_ttl_seconds: float = 15,
        timeout_seconds: float = 10,
        http_client: httpx.AsyncClient | None = None,
    ) -> None:
        self._url = url.rstrip("/")
        self._token = token if isinstance(token, SecretStr) else SecretStr(token)
        self._http = http_client or httpx.AsyncClient(
            timeout=timeout_seconds, trust_env=False
        )
        self._owns_http = http_client is None
        self._ttl = snapshot_ttl_seconds
        self._snapshot: Snapshot | None = None
        self._snapshot_at = 0.0
        self._lock = asyncio.Lock()
        self._usage: list[dict[str, Any]] | None = None
        self._usage_at = 0.0
        self._usage_lock = asyncio.Lock()

    @classmethod
    def from_config(cls, config: Mapping[str, Any]) -> "BrokerClient | None":
        url, token, _ = broker_settings(config)
        if token is None:
            return None
        settings = section(config, "omp_broker")
        return cls(
            url,
            token,
            snapshot_ttl_seconds=number(settings, "snapshot_ttl_seconds", 15),
            timeout_seconds=number(settings, "timeout_seconds", 10),
        )

    def __repr__(self) -> str:
        return "BrokerClient(token=**********)"

    def headers(self) -> dict[str, str]:
        return {
            "Authorization": f"Bearer {self._token.get_secret_value()}",
            "OMP-Auth-Broker-Capabilities": "codex-meter-block-scopes",
        }

    async def _request(self, method: str, path: str, body: Any = None) -> Any:
        try:
            async with self._http.stream(
                method, self._url + path, headers=self.headers(), json=body
            ) as response:
                response.raise_for_status()
                body_bytes = bytearray()
                async for chunk in response.aiter_bytes(chunk_size=8192):
                    if len(body_bytes) + len(chunk) > MAX_BROKER_BYTES:
                        raise ValueError("Broker response exceeds metadata limit")
                    body_bytes.extend(chunk)
                return json.loads(body_bytes)
        except (httpx.HTTPError, ValueError, UnicodeError):
            raise AuthUnavailable("omp-broker", ["broker request failed"]) from None

    async def snapshot(self, *, force: bool = False) -> Snapshot:
        async with self._lock:
            if (
                not force
                and self._snapshot is not None
                and time.monotonic() - self._snapshot_at < self._ttl
            ):
                return self._snapshot
            try:
                self._snapshot = Snapshot.parse(
                    await self._request("GET", "/v1/snapshot")
                )
                self._snapshot_at = time.monotonic()
                return self._snapshot
            except (AuthUnavailable, ValidationError, ValueError):
                self._snapshot = None
                raise AuthUnavailable(
                    "omp-broker", ["invalid broker snapshot"]
                ) from None

    async def usage(self) -> list[dict[str, Any]]:
        async with self._usage_lock:
            if (
                self._usage is not None
                and time.monotonic() - self._usage_at < self._ttl
            ):
                return self._usage
            try:
                data = await self._request("GET", "/v1/usage")
            except AuthUnavailable:
                # Usage is advisory; snapshot availability remains authoritative.
                self._usage = []
            else:
                payload = cast(dict[str, Any], data) if isinstance(data, dict) else {}
                value = payload.get("reports", [])
                reports = cast(list[Any], value) if isinstance(value, list) else []
                self._usage = [
                    cast(dict[str, Any], report)
                    for report in reports
                    if isinstance(report, dict)
                ]
            self._usage_at = time.monotonic()
            return self._usage

    async def block(
        self,
        credential_id: int,
        provider: str,
        *,
        blocked_until_ms: float,
        block_scope: str = "",
    ) -> None:
        await self._request(
            "POST",
            f"/v1/credential/{credential_id}/block",
            {
                "providerKey": provider,
                "blockScope": block_scope,
                "blockedUntilMs": blocked_until_ms,
            },
        )
        self._snapshot_at = 0

    async def snapshot_stream(self) -> AsyncIterator[Snapshot]:
        try:
            async with self._http.stream(
                "GET",
                self._url + "/v1/snapshot/stream",
                headers={**self.headers(), "Accept": "text/event-stream"},
                timeout=None,
            ) as response:
                response.raise_for_status()
                if (
                    response.headers.get("content-type", "").split(";")[0]
                    != "text/event-stream"
                ):
                    raise ValueError
                initial = True
                async for data in sse_payloads(response):
                    raw: Any = json.loads(data)
                    if not isinstance(raw, dict):
                        raise ValueError
                    event = cast(dict[str, Any], raw)
                    async with self._lock:
                        kind = event.get("kind")
                        if initial and kind != "snapshot":
                            raise ValueError
                        if kind == "snapshot":
                            self._snapshot = Snapshot.parse(event)
                        elif self._snapshot is not None and kind in (
                            "entry",
                            "removed",
                        ):
                            entries = list(self._snapshot.credentials)
                            cid = (
                                event["entry"]["id"] if kind == "entry" else event["id"]
                            )
                            entries = [entry for entry in entries if entry.id != cid]
                            if (
                                kind == "entry"
                                and event["entry"]["credential"]["type"] == "oauth"
                            ):
                                entries.append(
                                    BrokerCredential.model_validate(event["entry"])
                                )
                            self._snapshot = Snapshot(
                                generation=event["generation"],
                                serverNowMs=event["serverNowMs"],
                                credentials=entries,
                            )
                        else:
                            raise ValueError
                        self._snapshot_at = time.monotonic()
                        initial = False
                        snapshot = self._snapshot
                    yield snapshot
                raise ValueError
        except (httpx.HTTPError, ValueError, KeyError, TypeError):
            self._snapshot_at = 0
            self._snapshot = None
            raise AuthUnavailable(
                "omp-broker", ["broker snapshot stream unavailable; use polling"]
            ) from None

    async def aclose(self) -> None:
        if self._owns_http:
            await self._http.aclose()
