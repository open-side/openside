import json
from pathlib import Path

from openside_agent.providers import doctor_report, subscriptions
from openside_agent.providers.accounts import accounts_report


async def test_account_diagnostics_pass_through_sanitized_daemon_metadata():
    metadata = {
        "sources": [
            {
                "name": "vault",
                "kind": "vault",
                "available": True,
                "reason": "Account available",
                "accounts": [
                    {
                        "id": "opaque-key-1",
                        "label": "Work",
                        "provider": "openai",
                        "models": ["openai:gpt-5"],
                        "available": True,
                        "enabled": True,
                        "ready": True,
                        "usage_windows": [
                            {
                                "name": "weekly",
                                "used_percent": None,
                                "reset_at_ms": None,
                            }
                        ],
                        "blocked_until_ms": None,
                    }
                ],
            }
        ],
        "cached_for_ms": 5000,
        "truncated": False,
    }
    config = {
        "_diagnostics": {"accounts": metadata},
        "_vault_keys": {"openai": "synthetic-ignored-secret-canary"},
    }
    result = await accounts_report(config, {"default": "openai:gpt-5"})
    assert result == metadata
    assert "synthetic-ignored-secret-canary" not in json.dumps(result)
    report = doctor_report(config, {"default": "openai:gpt-5"})
    assert report["models"]["providers_ok"]
    assert "synthetic-ignored-secret-canary" not in json.dumps(report)


async def test_standalone_diagnostics_do_not_read_tokens_or_detect_login(monkeypatch):
    seen = []

    def forbidden_open(*args, **kwargs):
        seen.append(args)
        raise AssertionError("Sidecar diagnostics must not inspect credential files")

    monkeypatch.setattr(Path, "open", forbidden_open)
    monkeypatch.setattr(subscriptions.shutil, "which", lambda _: "/fake/installed-cli")
    monkeypatch.setenv("OPENSIDE_BROKER_TOKEN", "synthetic-broker-canary")
    monkeypatch.setenv("OPENAI_API_KEY", "synthetic-key-canary")
    config = {
        "omp_broker": {
            "url": "http://not-a-real-broker.invalid",
            "token_file": "/fake/token",
        },
        "command": {"openai": ["not-a-real-command"]},
    }
    result = await accounts_report(config, {"default": "openai:gpt-5"})
    assert not seen
    assert all(
        not source["available"] and not source["accounts"]
        for source in result["sources"]
    )
    assert all("unknown" in source["reason"] for source in result["sources"])
    report = doctor_report(config, {"default": "openai:gpt-5"})
    assert not report["models"]["providers_ok"]
    assert "synthetic-broker-canary" not in json.dumps(result) + json.dumps(report)
    assert "synthetic-key-canary" not in json.dumps(result) + json.dumps(report)
