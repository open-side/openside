"""Sidecar socket entrypoint and credential-value-free diagnostics."""

import argparse
import asyncio
import json
import os
import platform
import sys
from importlib.metadata import PackageNotFoundError, version
from typing import Any, cast

from openside_agent import __version__, providers


def package_versions() -> dict[str, str]:
    packages: dict[str, str] = {}
    for name in ("pydantic-ai-slim", "pydantic-ai-harness", "openside-agent"):
        try:
            packages[name] = version(name)
        except PackageNotFoundError:
            packages[name] = "unavailable"
    return packages


def doctor() -> dict[str, Any]:
    value: Any = json.loads(os.getenv("OPENSIDE_DOCTOR_CONFIG", "{}"))
    config = cast(dict[str, Any], value) if isinstance(value, dict) else {}
    credentials: Any = config.get("credentials", {})
    models: Any = config.get("models", {})
    report = providers.doctor_report(
        cast(dict[str, Any], credentials) if isinstance(credentials, dict) else {},
        cast(dict[str, Any], models) if isinstance(models, dict) else {},
    )
    return {
        "python": platform.python_version(),
        "packages": package_versions(),
        **report,
    }


async def accounts() -> dict[str, Any]:
    from openside_agent.providers.accounts import accounts_report

    value: Any = json.loads(os.getenv("OPENSIDE_DOCTOR_CONFIG", "{}"))
    config = cast(dict[str, Any], value) if isinstance(value, dict) else {}
    credentials = config.get("credentials", {})
    models = config.get("models", {})
    return await accounts_report(
        cast(dict[str, Any], credentials) if isinstance(credentials, dict) else {},
        cast(dict[str, Any], models) if isinstance(models, dict) else {},
    )


async def sidecar() -> None:
    # Diagnostics do not need the session runtime's startup work.
    from openside_agent.protocol import Hello, JsonLineConnection, Versions
    from openside_agent.runtime import Session

    socket_path = os.environ["OPENSIDE_SIDECAR_SOCKET"]
    nonce = os.environ["OPENSIDE_SIDECAR_NONCE"]
    reader, writer = await asyncio.open_unix_connection(socket_path)
    connection = JsonLineConnection(reader, writer)
    packages = package_versions()
    await connection.send(
        Hello(
            nonce=nonce,
            pid=os.getpid(),
            versions=Versions(
                python=platform.python_version(),
                pydantic_ai=packages["pydantic-ai-slim"],
                pydantic_ai_harness=packages["pydantic-ai-harness"],
                openside_agent=__version__,
            ),
        )
    )
    await Session(connection).serve()


def main() -> int:
    parser = argparse.ArgumentParser(prog="openside_agent")
    commands = parser.add_subparsers(dest="command")
    diagnostics = commands.add_parser("doctor")
    diagnostics.add_argument("--json", action="store_true")
    account_diagnostics = commands.add_parser("accounts")
    account_diagnostics.add_argument("--json", action="store_true")
    args = parser.parse_args()
    if args.command == "doctor":
        report: dict[str, Any]
        try:
            report = doctor()
        except Exception:
            report = {
                "python": platform.python_version(),
                "packages": package_versions(),
                "credentials": [],
                "models": {
                    "default": None,
                    "fallback": [],
                    "providers_ok": False,
                },
            }
        print(json.dumps(report, separators=(",", ":")))
        return 0
    if args.command == "accounts":
        try:
            report = asyncio.run(accounts())
        except Exception:
            print("Account diagnostics unavailable", file=sys.stderr)
            return 1
        print(json.dumps(report, separators=(",", ":"), allow_nan=False))
        return 0
    try:
        asyncio.run(sidecar())
    except Exception:
        # Exception strings can contain model input, tool arguments or credentials.
        print("Sidecar connection failed; check runtime configuration", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
