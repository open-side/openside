import asyncio
import json
import os
import sys
import time
from pathlib import Path

import httpx
import pytest

from openside_agent.providers.broker import (
    MAX_BROKER_BYTES,
    MAX_BROKER_CREDENTIALS,
    BrokerClient,
    Snapshot,
)
from openside_agent.providers.credential_command import key_bytes
from openside_agent.providers.errors import AuthUnavailable
from openside_agent.providers.pins import CredentialPins
from openside_agent.providers.selector import AccountSelector
from openside_agent.providers.sources import OmpBrokerSource

from .conftest import ACCESS_CANARY, BROKER_CANARY, credential


async def test_key_command_stdout_is_bounded_and_errors_do_not_echo_values():
    start = time.monotonic()
    with pytest.raises(AuthUnavailable) as error:
        await key_bytes(
            [
                sys.executable,
                "-c",
                "import sys; sys.stdout.write('test-key-canary-' * 1000000)",
            ],
            "anthropic",
            5,
        )
    assert "test-key-canary" not in str(error.value)
    assert time.monotonic() - start < 3


@pytest.mark.skipif(os.name != "posix", reason="process-group regression requires Unix")
@pytest.mark.parametrize("cancel", [False, True])
async def test_key_command_kills_descendant_held_stdout_with_bounded_cleanup(
    tmp_path: Path, cancel: bool
):
    started = tmp_path / "started"
    finished = tmp_path / "survived"
    child = (
        "from pathlib import Path; import time; "
        f"Path({str(started)!r}).write_text('started'); time.sleep(1); "
        f"Path({str(finished)!r}).write_text('survived')"
    )
    parent = f"import subprocess,sys; subprocess.Popen([sys.executable,'-c',{child!r}])"
    task = asyncio.create_task(
        key_bytes([sys.executable, "-c", parent], "anthropic", 0.3 if not cancel else 5)
    )
    async with asyncio.timeout(2):
        while not started.exists():
            await asyncio.sleep(0.01)
    if cancel:
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(task, 2)
    else:
        with pytest.raises(AuthUnavailable):
            await asyncio.wait_for(task, 2)
    await asyncio.sleep(1.1)
    assert not finished.exists()


class BytesStream(httpx.AsyncByteStream):
    def __init__(self, chunks: list[bytes]):
        self.chunks = chunks
        self.closed = False

    async def __aiter__(self):
        for chunk in self.chunks:
            yield chunk

    async def aclose(self):
        self.closed = True


def snapshot_event(entries: list[dict]) -> bytes:
    return (
        "data:"
        + json.dumps(
            {
                "kind": "snapshot",
                "generation": 1,
                "serverNowMs": 0,
                "credentials": entries,
            }
        )
        + "\n\n"
    ).encode()


@pytest.mark.parametrize("kind", ["line", "event", "entries"])
async def test_broker_stream_limits_close_and_invalidate_polling_cache(kind: str):
    if kind == "line":
        chunks = [snapshot_event([]), b"data:" + b"x" * (MAX_BROKER_BYTES + 1)]
    elif kind == "event":
        chunks = [snapshot_event([]), (b"data:" + b"x" * 8192 + b"\n") * 513]
    else:
        chunks = [
            snapshot_event(
                [credential(cid=cid) for cid in range(MAX_BROKER_CREDENTIALS)]
            )
        ]
        chunks.append(
            (
                "data:"
                + json.dumps(
                    {
                        "kind": "entry",
                        "generation": 2,
                        "serverNowMs": 0,
                        "entry": credential(cid=MAX_BROKER_CREDENTIALS),
                    }
                )
                + "\n\n"
            ).encode()
        )
    stream = BytesStream(chunks)
    http = httpx.AsyncClient(
        transport=httpx.MockTransport(
            lambda _: httpx.Response(
                200, headers={"content-type": "text/event-stream"}, stream=stream
            )
        )
    )
    broker = BrokerClient("http://127.0.0.1:1", BROKER_CANARY, http_client=http)
    try:
        with pytest.raises(AuthUnavailable) as error:
            async for _ in broker.snapshot_stream():
                pass
        assert ACCESS_CANARY not in str(error.value)
        assert BROKER_CANARY not in str(error.value)
        assert broker._snapshot is None
        assert stream.closed
    finally:
        await http.aclose()


def test_doctor_sync_broker_response_is_stream_bounded(monkeypatch: pytest.MonkeyPatch):
    class Stream(httpx.SyncByteStream):
        def __iter__(self):
            for _ in range(513):
                yield b"x" * 8192

    original = httpx.Client
    monkeypatch.setattr(
        httpx,
        "Client",
        lambda **kwargs: original(
            transport=httpx.MockTransport(
                lambda _: httpx.Response(200, stream=Stream())
            ),
            **kwargs,
        ),
    )
    monkeypatch.setenv("OPENSIDE_BROKER_TOKEN", BROKER_CANARY)
    probe = OmpBrokerSource({"omp_broker": {"url": "http://127.0.0.1:1"}}).probe(
        "anthropic"
    )
    assert not probe.available
    assert BROKER_CANARY not in repr(probe)


def test_selector_prunes_expired_and_removed_local_blocks():
    selector = AccountSelector(
        BrokerClient("http://127.0.0.1:1", BROKER_CANARY),
        "anthropic",
        "session",
        CredentialPins(),
        clock=lambda: 10,
    )
    selector._blocked = {1: 10001, 2: 9999, 3: 20000}
    snapshot = Snapshot.parse(
        {
            "generation": 1,
            "serverNowMs": 0,
            "credentials": [
                credential(cid=1, provider="anthropic"),
                credential(cid=2, provider="anthropic"),
            ],
        }
    )
    assert [entry.id for entry in selector.candidates(snapshot)] == [2]
    assert selector._blocked == {1: 10001}
