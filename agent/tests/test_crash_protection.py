import asyncio
import json
import os
import subprocess
import sys

import pytest


@pytest.mark.skipif(
    os.name != "posix", reason="sidecar transport requires Unix sockets"
)
def test_session_child_disables_core_dumps_and_dumpability_without_touching_parent():
    import resource

    parent_limits = resource.getrlimit(resource.RLIMIT_CORE)
    parent_dumpable = None
    if sys.platform == "linux":
        import ctypes

        parent_dumpable = ctypes.CDLL(None).prctl(3, 0, 0, 0, 0)
    process = subprocess.run(
        [
            sys.executable,
            "-c",
            "import ctypes, json, resource, sys; "
            "from openside_agent.__main__ import _protect_sidecar_process; "
            "_protect_sidecar_process(); "
            "print(json.dumps({'limits': resource.getrlimit(resource.RLIMIT_CORE), "
            "'dumpable': ctypes.CDLL(None).prctl(3, 0, 0, 0, 0) "
            "if sys.platform == 'linux' else None}))",
        ],
        capture_output=True,
        check=False,
        timeout=15,
    )
    assert process.returncode == 0, process.stderr
    report = json.loads(process.stdout)
    assert report["limits"] == [0, 0]
    assert resource.getrlimit(resource.RLIMIT_CORE) == parent_limits
    if sys.platform == "linux":
        assert report["dumpable"] == 0
        assert ctypes.CDLL(None).prctl(3, 0, 0, 0, 0) == parent_dumpable


async def test_session_fails_closed_before_opening_the_credential_socket(monkeypatch):
    from openside_agent import __main__

    connected = False

    def unavailable():
        raise OSError("Crash protection unavailable")

    async def connect(*args, **kwargs):
        nonlocal connected
        connected = True
        raise AssertionError("Must not connect without crash protection")

    monkeypatch.setattr(__main__, "_protect_sidecar_process", unavailable)
    monkeypatch.setattr(asyncio, "open_unix_connection", connect, raising=False)
    with pytest.raises(OSError, match="Crash protection unavailable"):
        await __main__.sidecar()
    assert not connected
