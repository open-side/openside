import os
from pathlib import Path

import pytest


@pytest.fixture(autouse=True)
def hermetic_environment(monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
    for name in (
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_RUNTIME_DIR",
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
    ):
        path = tmp_path / name.lower()
        path.mkdir()
        monkeypatch.setenv(name, str(path))
    for name in list(os.environ):
        if name.startswith("OPENSIDE_") or name in {
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "OPENROUTER_API_KEY",
            "GOOGLE_API_KEY",
            "GEMINI_API_KEY",
            "GITHUB_COPILOT_API_KEY",
            "GITHUB_COPILOT_API_TOKEN",
            "COPILOT_GITHUB_TOKEN",
        }:
            monkeypatch.delenv(name, raising=False)
