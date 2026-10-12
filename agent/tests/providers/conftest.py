import json
import threading
import time
from collections.abc import Iterator
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

import pytest
from pydantic_ai.models.test import TestModel

BROKER_CANARY = "synthetic-broker-canary-7ad391"
ACCESS_CANARY = "synthetic-access-canary-62f3ab"


def model_id(provider: str) -> str:
    name = TestModel().model_name
    if provider == "openrouter":
        name = f"openai/{name}"
    return f"{provider}:{name}"


def credential(
    provider: str = "openai-codex", cid: int = 1, *, seconds: float = 3600
) -> dict[str, Any]:
    return {
        "id": cid,
        "provider": provider,
        "identityKey": f"synthetic-identity-{cid}",
        "rotatesInMs": None,
        "credential": {
            "type": "oauth",
            "access": f"{ACCESS_CANARY}-{cid}",
            "refresh": "BROKER-HELD-SENTINEL",
            "expires": (time.time() + seconds) * 1000,
            "accountId": f"synthetic-account-{cid}",
        },
    }


@dataclass
class FakeBroker:
    entries: list[dict[str, Any]] = field(default_factory=lambda: [credential()])
    reports: list[dict[str, Any]] = field(default_factory=list)
    events: list[dict[str, Any]] = field(default_factory=list)
    blocks: list[tuple[int, dict[str, Any]]] = field(default_factory=list)
    requests: list[tuple[str, str]] = field(default_factory=list)
    status: int = 200
    url: str = ""

    def snapshot(self) -> dict[str, Any]:
        return {
            "generation": len(self.requests),
            "generatedAt": time.time() * 1000,
            "serverNowMs": time.time() * 1000,
            "refresher": {
                "enabled": True,
                "intervalMs": 60000,
                "skewMs": 300000,
                "nextSweepInMs": 0,
            },
            "credentials": self.entries,
        }

    def config(self) -> dict[str, Any]:
        return {
            "omp_broker": {
                "url": self.url,
                "snapshot_ttl_seconds": 0,
                "timeout_seconds": 1,
            }
        }


@pytest.fixture
def fake_broker(monkeypatch: pytest.MonkeyPatch) -> Iterator[FakeBroker]:
    state = FakeBroker()

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, format: str, *args: Any) -> None:
            pass

        def respond(self, value: Any, status: int = 200) -> None:
            body = json.dumps(value).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self) -> None:
            state.requests.append(("GET", self.path))
            if self.headers.get("Authorization") != f"Bearer {BROKER_CANARY}":
                self.respond({"error": BROKER_CANARY}, 401)
            elif state.status != 200:
                self.respond({"error": ACCESS_CANARY}, state.status)
            elif self.path == "/v1/snapshot":
                self.respond(state.snapshot())
            elif self.path == "/v1/usage":
                self.respond(
                    {"generatedAt": time.time() * 1000, "reports": state.reports}
                )
            elif self.path == "/v1/snapshot/stream":
                events = [{"kind": "snapshot", **state.snapshot()}, *state.events]
                body = ": keepalive\n\n" + "".join(
                    f"event: {event['kind']}\ndata: {json.dumps(event)}\n\n"
                    for event in events
                )
                encoded = body.encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Content-Length", str(len(encoded)))
                self.end_headers()
                self.wfile.write(encoded)
            else:
                self.respond({"error": "missing"}, 404)

        def do_POST(self) -> None:
            state.requests.append(("POST", self.path))
            value = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            if self.path.startswith("/v1/credential/") and self.path.endswith("/block"):
                cid = int(self.path.split("/")[3])
                state.blocks.append((cid, value))
                for entry in state.entries:
                    if entry["id"] == cid:
                        entry.setdefault("blocks", []).append(value)
                self.respond({"ok": True})
            else:
                self.respond({"error": "unexpected write"}, 400)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    state.url = f"http://127.0.0.1:{server.server_port}"
    thread = threading.Thread(
        target=lambda: server.serve_forever(poll_interval=0.01), daemon=True
    )
    thread.start()
    monkeypatch.setenv("OPENSIDE_BROKER_TOKEN", BROKER_CANARY)
    try:
        yield state
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
