"""Sidecar outbound allow-list: every httpx/httpx2 transport refuses unlisted hosts."""

import threading
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, HTTPServer

import anthropic
import httpx
import httpx2
import pytest

from openside_agent import providers
from openside_agent.providers import egress


@pytest.fixture(autouse=True)
def reset_egress() -> Iterator[None]:
    yield
    egress._active = None  # pyright: ignore[reportPrivateUsage]


@pytest.fixture
def loopback() -> Iterator[str]:
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            if self.path == "/redirect":
                self.send_response(302)
                self.send_header("Location", "http://collector.invalid/steal")
                self.end_headers()
                return
            self.send_response(200)
            self.send_header("Content-Length", "2")
            self.end_headers()
            self.wfile.write(b"ok")

        def log_message(self, format: str, *args: object) -> None:
            pass

    server = HTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    yield f"http://127.0.0.1:{server.server_address[1]}"
    server.shutdown()
    server.server_close()


def test_entry_syntax_matches_the_rust_egress_list() -> None:
    allow = egress.Egress(
        [
            "api.example.com",
            "*.cdn.example",
            "proxy.example:8443",
            "https://only.example",
        ]
    )
    for scheme, host, port in [
        ("https", "API.example.com.", 443),
        ("http", "api.example.com", 8080),
        ("https", "a.b.cdn.example", 443),
        ("https", "proxy.example", 8443),
        ("https", "only.example", 443),
        ("http", "localhost", 9),
        ("http", "127.0.0.1", 1),
        ("http", "[::1]", 1),
        ("http", "::ffff:127.0.0.1", 1),
    ]:
        allow.check(scheme, host, port)
    for scheme, host, port in [
        ("https", "cdn.example", 443),
        ("https", "evilapi.example.com", 443),
        ("https", "proxy.example", 443),
        ("http", "only.example", 443),
    ]:
        with pytest.raises(egress.EgressRefused):
            allow.check(scheme, host, port)
    for invalid in [
        "",
        "*",
        "a.*.example",
        "https://x/path",
        "user@x",
        "x:99999",
        "*.:1",
    ]:
        with pytest.raises(ValueError):
            egress.parse_entry(invalid)
    # IDNA: a Unicode host is compared in its ASCII form, like the browser and Rust.
    egress.Egress(["xn--bcher-kva.example"]).check_url(
        httpx2.URL("https://bücher.example/")
    )


async def test_transports_refuse_unlisted_hosts_and_redirect_hops(
    loopback: str,
) -> None:
    egress.install(["allowed.invalid"])
    async with httpx2.AsyncClient(trust_env=False) as client:
        assert (await client.get(loopback + "/")).text == "ok"
        with pytest.raises(egress.EgressRefused, match="collector.invalid"):
            await client.get("http://collector.invalid/")
        with pytest.raises(egress.EgressRefused, match="collector.invalid"):
            await client.get(loopback + "/redirect", follow_redirects=True)
        # The allow-listed host gets past the guard; only name resolution fails then.
        with pytest.raises(httpx2.ConnectError):
            await client.get("http://allowed.invalid/")
    with httpx.Client(trust_env=False) as client:
        assert client.get(loopback + "/").text == "ok"
        with pytest.raises(egress.EgressRefused):
            client.get("https://collector.invalid/")
    async with httpx.AsyncClient(trust_env=False) as client:
        with pytest.raises(egress.EgressRefused):
            await client.get(loopback + "/redirect", follow_redirects=True)


async def test_provider_sdk_requests_are_refused_outside_the_list() -> None:
    egress.install([])
    client = anthropic.AsyncAnthropic(
        api_key="test-key", base_url="http://collector.invalid", max_retries=0
    )
    with pytest.raises(anthropic.APIConnectionError) as raised:
        await client.messages.create(
            model="test", max_tokens=1, messages=[{"role": "user", "content": "hi"}]
        )
    assert isinstance(raised.value.__cause__, egress.EgressRefused)
    await client.close()


async def test_built_provider_endpoint_is_admitted(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from .test_request_credentials import FakeCredentialDaemon

    allow = egress.install([])
    endpoint = httpx2.URL("https://provider.invalid/v1/chat/completions")
    with pytest.raises(egress.EgressRefused):
        allow.check_url(endpoint)
    monkeypatch.setenv("OPENAI_API_KEY", "test-key")
    monkeypatch.setenv("OPENAI_BASE_URL", "https://provider.invalid/v1")
    model = await providers.build_model(
        "openai:test-model",
        [],
        {"order": {"openai": ["env"]}},
        credential_rpc=FakeCredentialDaemon(),
    )
    allow.check_url(endpoint)
    with pytest.raises(egress.EgressRefused):
        allow.check_url(httpx2.URL("https://other.invalid/v1"))
    async with model:
        pass
