"""Authentication errors contain only application-authored diagnostic reasons."""

from collections.abc import Sequence


class AuthUnavailable(Exception):
    def __init__(
        self,
        provider: str,
        reasons: Sequence[str] = (),
        *,
        reason: str | None = None,
        retry_at_ms: int | None = None,
    ) -> None:
        self.provider = provider
        self.reasons = tuple(reasons)
        self.reason = reason
        self.retry_at_ms = retry_at_ms
        detail = "; ".join(reasons) or "no usable credential source"
        super().__init__(
            f"{provider}: {detail}. "
            "Check the configured account pool in Models & accounts."
        )
