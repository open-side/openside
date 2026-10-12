"""Claude Code OAuth shaping; never applied to ordinary Anthropic API keys.

The default, identity, and required-version pattern follow upstream
packages/ai/src/providers/claude-code-fingerprint.ts at commit
 eec114a9226da698ebc015fd8996a8ca3bc4625c. The three headers were verified
by docs/08-pydantic-ai-pivot.md §B; live compatibility remains unverified.
"""

import json
import os
import re
from collections.abc import Mapping
from typing import Any, cast

import httpx2

from .config import section

DEFAULT_CLAUDE_CODE_VERSION = "2.1.280"
CLAUDE_CODE_IDENTITY = "You are Claude Code, Anthropic's official CLI for Claude."
SEMVER = re.compile(r"^\d+\.\d+\.\d+$")
REQUIRED_VERSION = re.compile(r"version (\d+\.\d+\.\d+) or newer is required", re.I)


class ClaudeCodeShaping:
    def __init__(self, credentials: Mapping[str, Any]) -> None:
        configured = os.getenv("OPENSIDE_CLAUDE_CODE_VERSION") or section(
            credentials, "anthropic"
        ).get("claude_code_version", DEFAULT_CLAUDE_CODE_VERSION)
        self.version = (
            configured
            if isinstance(configured, str) and SEMVER.fullmatch(configured)
            else DEFAULT_CLAUDE_CODE_VERSION
        )

    def headers(self) -> dict[str, str]:
        return {
            "anthropic-beta": "claude-code-20250219,oauth-2025-04-20",
            "user-agent": f"claude-cli/{self.version} (external, cli)",
            "x-app": "cli",
        }

    def apply(self, request: httpx2.Request) -> httpx2.Request:
        payload: Any = json.loads(request.content)
        system = payload.get("system", [])
        if isinstance(system, str):
            system = [{"type": "text", "text": system}]
        if not isinstance(system, list):
            raise ValueError("Invalid Anthropic system content")
        blocks = cast(list[Any], system)
        first_text = cast(dict[str, Any], blocks[0]).get("text") if blocks else None
        if first_text != CLAUDE_CODE_IDENTITY:
            payload["system"] = [
                {"type": "text", "text": CLAUDE_CODE_IDENTITY},
                *blocks,
            ]
        headers = dict(request.headers)
        headers.pop("content-length", None)
        headers.update(self.headers())
        return httpx2.Request(
            request.method,
            request.url,
            headers=headers,
            content=json.dumps(payload).encode(),
            extensions=request.extensions,
        )

    def adopt_required_version(self, response: httpx2.Response) -> bool:
        text = response.text
        if "claude_code_version_too_old" not in text:
            return False
        match = REQUIRED_VERSION.search(text)
        if match is None:
            return False
        required = match[1]
        if tuple(map(int, required.split("."))) <= tuple(
            map(int, self.version.split("."))
        ):
            return False
        self.version = required
        return True
