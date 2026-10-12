"""Outbound destination allow-list for the sidecar (D-5, G-4, PP-25).

The sidecar may reach loopback, the endpoints of the providers it actually built
(and the credential broker), and the user's ``network.allow`` entries, which the
daemon forwards as ``open.config.agent.network_allow``. Entry syntax and matching
mirror the Rust ``openside_policy::Egress``: ``host``, ``*.domain`` (subdomains
only), ``host:port`` or ``scheme://host[:port]``; an entry without a port matches
every port.

Enforcement sits in the HTTP transports of both httpx generations used here
(``httpx`` for the broker, ``httpx2`` inside the provider SDKs), so every request any
client sends, including each redirect hop, is checked before a connection opens.
Once :func:`install` runs, the check stays active for the life of the process.
"""

import ipaddress
import sys
import threading
from collections.abc import Iterable
from dataclasses import dataclass
from typing import Any

import httpx
import httpx2

DEFAULT_PORTS = {"http": 80, "https": 443, "ws": 80, "wss": 443}

# Token refresh endpoints the provider SDK calls besides its model base URL.
AUXILIARY_ENDPOINTS: dict[str, tuple[str, ...]] = {
    "openai-codex": ("https://auth.openai.com",),
}


class EgressRefused(Exception):
    """The destination is not loopback and matches no allow-list entry."""

    def __init__(self, destination: str) -> None:
        self.destination = destination
        super().__init__(
            f"outbound connection to {destination} is not in network.allow"
        )


@dataclass(frozen=True)
class Entry:
    scheme: str | None
    host: str
    port: int | None


def parse_entry(raw: str) -> Entry:
    text = raw.strip().lower()
    invalid = ValueError(f"invalid network.allow entry {raw!r}")
    scheme: str | None = None
    if "://" in text:
        scheme, text = text.split("://", 1)
    text = text.removesuffix("/")
    if any(mark in text for mark in "/?#@"):
        raise invalid
    host, port = _split_host_port(text, invalid)
    body = host.removeprefix("*.")
    if not body or "*" in body:
        raise invalid
    return Entry(scheme, host, port)


def _split_host_port(text: str, invalid: ValueError) -> tuple[str, int | None]:
    def number(value: str) -> int:
        if not value.isdigit() or int(value) > 65535:
            raise invalid
        return int(value)

    if text.startswith("["):
        host, separator, after = text[1:].partition("]")
        if not separator:
            raise invalid
        if after.startswith(":"):
            return host, number(after[1:])
        if after:
            raise invalid
        return host, None
    host, separator, port = text.rpartition(":")
    if not separator:
        return text, None
    if ":" in host:
        # A bare IPv6 literal has several colons and no port.
        return text, None
    return host, number(port)


def is_loopback(host: str) -> bool:
    if host == "localhost" or host.endswith(".localhost"):
        return True
    try:
        address = ipaddress.ip_address(host)
    except ValueError:
        return False
    mapped = getattr(address, "ipv4_mapped", None)
    return address.is_loopback or bool(mapped and mapped.is_loopback)


def _host_matches(pattern: str, host: str) -> bool:
    if pattern.startswith("*."):
        domain = pattern[2:]
        rest = host.removesuffix(domain)
        return rest != host and len(rest) > 1 and rest.endswith(".")
    return pattern == host


class Egress:
    def __init__(self, entries: Iterable[str] = ()) -> None:
        self._entries = [parse_entry(entry) for entry in entries]
        self._lock = threading.Lock()

    def allow(self, entry: str) -> None:
        parsed = parse_entry(entry)
        with self._lock:
            if parsed not in self._entries:
                self._entries.append(parsed)

    def check(self, scheme: str, host: str, port: int) -> None:
        scheme = scheme.lower()
        host = host.strip("[]").rstrip(".").lower()
        with self._lock:
            entries = list(self._entries)
        if is_loopback(host) or any(
            (entry.scheme is None or entry.scheme == scheme)
            and (entry.port is None or entry.port == port)
            and _host_matches(entry.host, host)
            for entry in entries
        ):
            return
        raise EgressRefused(f"{scheme}://{host}:{port}")

    def check_url(self, url: Any) -> None:
        """Check an ``httpx.URL``/``httpx2.URL``; the host is its IDNA (ASCII) form."""
        scheme = str(url.scheme)
        host = bytes(url.raw_host).decode("ascii")
        port = url.port or DEFAULT_PORTS.get(scheme)
        if not host or port is None:
            raise EgressRefused(str(url))
        self.check(scheme, host, port)


_active: Egress | None = None


def active() -> Egress | None:
    return _active


def install(network_allow: Iterable[str]) -> Egress:
    """Enforce ``network_allow`` (plus loopback) on every transport in this process."""
    global _active
    egress = Egress(network_allow)
    if _active is None:
        _patch()
    _active = egress
    return egress


def allow_endpoint(url: str | None) -> None:
    """Admit a configured endpoint (model base URL, broker) by its exact origin."""
    if _active is None or not url:
        return
    parsed = httpx2.URL(url)
    scheme = str(parsed.scheme)
    port = parsed.port or DEFAULT_PORTS.get(scheme)
    host = bytes(parsed.raw_host).decode("ascii")
    if not host or port is None:
        return
    host = f"[{host}]" if ":" in host else host
    _active.allow(f"{scheme}://{host}:{port}")


def _refuse_unless_allowed(url: Any) -> None:
    if _active is None:
        return
    try:
        _active.check_url(url)
    except EgressRefused as error:
        print(f"openside-agent: {error}", file=sys.stderr, flush=True)
        raise


def _patch() -> None:
    for module in (httpx, httpx2):
        sync_transport: Any = module.HTTPTransport
        async_transport: Any = module.AsyncHTTPTransport
        original_sync = sync_transport.handle_request
        original_async = async_transport.handle_async_request

        def handle_request(
            self: Any, request: Any, _original: Any = original_sync
        ) -> Any:
            _refuse_unless_allowed(request.url)
            return _original(self, request)

        async def handle_async_request(
            self: Any, request: Any, _original: Any = original_async
        ) -> Any:
            _refuse_unless_allowed(request.url)
            return await _original(self, request)

        sync_transport.handle_request = handle_request
        async_transport.handle_async_request = handle_async_request
