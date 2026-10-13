import asyncio
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest
from runtime_helpers import FakeDaemon, install_script, text_response


async def test_doctor_subprocess_one_json_without_canary(monkeypatch):
    canary = "broker-token-canary-process-852124"
    monkeypatch.setenv("OPENSIDE_BROKER_TOKEN", canary)
    monkeypatch.setenv(
        "OPENSIDE_DOCTOR_CONFIG", json.dumps({"credentials": {}, "models": {}})
    )
    process = await asyncio.create_subprocess_exec(
        sys.executable,
        "-m",
        "openside_agent",
        "doctor",
        "--json",
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout, stderr = await asyncio.wait_for(process.communicate(), 15)
    assert process.returncode == 0
    assert len(stdout.splitlines()) == 1
    value = json.loads(stdout)
    assert {"python", "packages", "credentials", "models"} == set(value)
    assert value["models"]["default"] is None
    assert canary.encode() not in stdout + stderr


def test_diagnostics_do_not_initialize_the_session_runtime(monkeypatch):
    monkeypatch.setenv("OPENSIDE_DOCTOR_CONFIG", "{}")
    for command in ("accounts", "doctor"):
        process = subprocess.run(
            [
                sys.executable,
                "-c",
                "import runpy, sys; "
                "sys.argv = ['openside_agent', sys.argv[1], '--json']\n"
                "try:\n"
                " runpy.run_module('openside_agent', run_name='__main__')\n"
                "except SystemExit as result:\n"
                " assert result.code == 0\n"
                "assert 'openside_agent.runtime' not in sys.modules\n"
                "assert 'openside_agent.protocol' not in sys.modules\n",
                command,
            ],
            capture_output=True,
            check=False,
            timeout=15,
        )
        assert process.returncode == 0, process.stderr
        assert len(process.stdout.splitlines()) == 1
        json.loads(process.stdout)


@pytest.fixture
def socket_directory():
    with tempfile.TemporaryDirectory(prefix="osrpc-", dir="/tmp") as directory:
        yield Path(directory)


@pytest.mark.skipif(
    os.name != "posix", reason="sidecar transport requires Unix sockets"
)
async def test_real_sidecar_process_handshake_run_shutdown(
    tmp_path, monkeypatch, socket_directory
):
    install_script(tmp_path, monkeypatch, [text_response("real process answer")])
    canary = "vault-canary-real-sidecar-581239"
    monkeypatch.setenv("OPENSIDE_BROKER_TOKEN", canary)
    socket_path = socket_directory / "rpc.sock"
    connected = asyncio.get_running_loop().create_future()

    def accept(reader, writer):
        connected.set_result(FakeDaemon(reader, writer))

    server = await asyncio.start_unix_server(accept, path=str(socket_path))
    environment = {
        **os.environ,
        "OPENSIDE_SIDECAR_SOCKET": str(socket_path),
        "OPENSIDE_SIDECAR_NONCE": "test-nonce",
    }
    process = await asyncio.create_subprocess_exec(
        sys.executable,
        "-m",
        "openside_agent",
        env=environment,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    daemon = None
    try:
        daemon = await asyncio.wait_for(connected, 15)
        hello = await daemon.receive()
        assert hello["type"] == "hello" and hello["nonce"] == "test-nonce"
        assert hello["pid"] == process.pid and hello["protocol"] == 1
        await daemon.open()
        await daemon.user()
        final = await daemon.until("event", kind="assistant.final")
        assert final["event"]["text"] == "real process answer"
        await daemon.until("settled")
        await daemon.send("shutdown")
        await daemon.until("bye")
        stdout, stderr = await asyncio.wait_for(process.communicate(), 15)
        assert process.returncode == 0 and stdout == b""
        assert canary.encode() not in stdout + stderr
        assert canary not in json.dumps(daemon.outputs)
    finally:
        if process.returncode is None:
            process.kill()
            await process.wait()
        if daemon is not None:
            daemon.writer.close()
            await daemon.writer.wait_closed()
        server.close()
        await server.wait_closed()
