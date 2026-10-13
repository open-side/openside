"""Standalone diagnostics detect CLI installation, never credential-file contents."""

import shutil
from typing import Any


def statuses() -> list[dict[str, Any]]:
    return [
        {
            "id": source,
            "provider": provider,
            "installed": shutil.which(executable) is not None,
            "logged_in": False,
            "available": False,
            "reason": "Login readiness is owned by the daemon",
        }
        for source, executable, provider in (
            ("claude-code", "claude", "anthropic"),
            ("codex-cli", "codex", "openai-codex"),
        )
    ]
