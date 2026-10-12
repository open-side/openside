import os
from pathlib import Path

import pytest


@pytest.fixture(autouse=True)
def hermetic_environment(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, request: pytest.FixtureRequest
):
    # Live smoke must be explicit; otherwise no test can discover the user's home.
    if request.node.get_closest_marker("live") and os.getenv("OPENSIDE_LIVE_MODEL"):
        return
    if (
        request.node.get_closest_marker("live_broker")
        and os.getenv("OPENSIDE_LIVE_BROKER_URL")
        and os.getenv("OPENSIDE_LIVE_BROKER_TOKEN_ENV")
    ):
        return
    for name in (
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_RUNTIME_DIR",
        "CODEX_HOME",
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
