"""Authentication errors contain only application-authored diagnostic reasons."""

from collections.abc import Sequence


class AuthUnavailable(Exception):
    def __init__(self, provider: str, reasons: Sequence[str] = ()) -> None:
        self.provider = provider
        self.reasons = tuple(reasons)
        detail = "; ".join(reasons) or "no usable credential source"
        super().__init__(
            f"{provider}: {detail}. Configure credentials.order, provider API-key "
            "environment variables, credentials.command, or the optional broker."
        )
