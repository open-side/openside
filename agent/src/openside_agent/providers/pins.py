"""Token-free session affinity, suitable for daemon-owned snapshots."""

import json
import math
from dataclasses import dataclass, field
from typing import Any, cast


@dataclass(frozen=True)
class CredentialPin:
    credential_id: int
    last_request_at: float


@dataclass
class CredentialPins:
    entries: dict[str, dict[str, CredentialPin]] = field(
        default_factory=dict[str, dict[str, CredentialPin]]
    )

    def get(self, session_id: str, provider: str) -> CredentialPin | None:
        return self.entries.get(session_id, {}).get(provider)

    def set(
        self, session_id: str, provider: str, credential_id: int, timestamp: float
    ) -> None:
        self.entries.setdefault(session_id, {})[provider] = CredentialPin(
            credential_id, timestamp
        )

    def release(self, session_id: str, provider: str) -> None:
        self.entries.get(session_id, {}).pop(provider, None)

    def to_json(self) -> str:
        return json.dumps(
            {
                session: {
                    provider: {
                        "credential_id": pin.credential_id,
                        "last_request_at": pin.last_request_at,
                    }
                    for provider, pin in providers.items()
                }
                for session, providers in self.entries.items()
            },
            separators=(",", ":"),
            allow_nan=False,
        )

    @classmethod
    def from_json(cls, value: str) -> "CredentialPins":
        try:
            raw: Any = json.loads(value)
            if not isinstance(raw, dict):
                raise ValueError
            data = cast(dict[str, Any], raw)
            pins = cls()
            for session, raw_providers in data.items():
                if not isinstance(raw_providers, dict):
                    raise ValueError
                providers = cast(dict[str, Any], raw_providers)
                for provider, raw_pin in providers.items():
                    if not isinstance(raw_pin, dict):
                        raise ValueError
                    pin = cast(dict[str, Any], raw_pin)
                    if (
                        set(pin) != {"credential_id", "last_request_at"}
                        or type(pin["credential_id"]) is not int
                        or type(pin["last_request_at"]) not in (int, float)
                        or not math.isfinite(pin["last_request_at"])
                    ):
                        raise ValueError
                    pins.set(
                        session, provider, pin["credential_id"], pin["last_request_at"]
                    )
            return pins
        except (ValueError, TypeError, KeyError):
            raise ValueError(
                "Invalid credential pins; expected ids and timestamps only"
            ) from None
